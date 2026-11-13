use std::{collections::HashMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use thiserror::Error;
use tokio::sync::RwLock;

use crate::{
    config::{PollDefaults, SourceConfig},
    domain::{
        ComponentObservation, Incident, ObservationBatch, ProviderView, ServiceStatus,
        SourceHealth, StatusSummary,
    },
};

mod postgres;

pub use postgres::PostgresStore;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("unknown source: {0}")]
    UnknownSource(String),
    #[error("database operation failed: {0}")]
    Database(#[from] sqlx::Error),
    #[error("database migration failed: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("invalid value stored in {field}: {value}")]
    InvalidStoredValue { field: &'static str, value: String },
    #[error("configured duration for source {0} exceeds PostgreSQL limits")]
    InvalidDuration(String),
}

#[async_trait]
pub trait StatusStore: Send + Sync {
    async fn apply(&self, batch: ObservationBatch) -> Result<(), StoreError>;
    async fn mark_failed(
        &self,
        source_id: &str,
        checked_at: DateTime<Utc>,
        error: String,
    ) -> Result<(), StoreError>;
    async fn summary(&self) -> Result<StatusSummary, StoreError>;
    async fn providers(&self) -> Result<Vec<ProviderView>, StoreError>;
    async fn provider(&self, id: &str) -> Result<Option<ProviderView>, StoreError>;
    async fn active_incidents(&self) -> Result<Vec<Incident>, StoreError>;
    async fn source_health(&self) -> Result<Vec<SourceHealth>, StoreError>;
}

pub struct InMemoryStore {
    records: RwLock<HashMap<String, ProviderRecord>>,
}

impl InMemoryStore {
    pub fn new(sources: &[SourceConfig], defaults: &PollDefaults) -> Self {
        let records = sources
            .iter()
            .filter(|source| source.enabled)
            .map(|source| {
                (
                    source.id.clone(),
                    ProviderRecord {
                        id: source.id.clone(),
                        name: source.name.clone(),
                        category: source.category.clone(),
                        status: ServiceStatus::Unknown,
                        components: Vec::new(),
                        incidents: Vec::new(),
                        last_checked_at: None,
                        last_success_at: None,
                        consecutive_failures: 0,
                        last_error: None,
                        stale_after: source.stale_after(defaults),
                    },
                )
            })
            .collect();

        Self {
            records: RwLock::new(records),
        }
    }

    async fn views(&self) -> Vec<ProviderView> {
        let now = Utc::now();
        let records = self.records.read().await;
        let mut views = records
            .values()
            .map(|record| record.view(now))
            .collect::<Vec<_>>();
        views.sort_by(|left, right| left.name.cmp(&right.name));
        views
    }
}

#[derive(Debug, Clone)]
struct ProviderRecord {
    id: String,
    name: String,
    category: String,
    status: ServiceStatus,
    components: Vec<ComponentObservation>,
    incidents: Vec<Incident>,
    last_checked_at: Option<DateTime<Utc>>,
    last_success_at: Option<DateTime<Utc>>,
    consecutive_failures: u32,
    last_error: Option<String>,
    stale_after: Duration,
}

impl ProviderRecord {
    fn stale(&self, now: DateTime<Utc>) -> bool {
        self.last_success_at
            .and_then(|last_success| now.signed_duration_since(last_success).to_std().ok())
            .is_none_or(|age| age > self.stale_after)
    }

    fn view(&self, now: DateTime<Utc>) -> ProviderView {
        let stale = self.stale(now);
        ProviderView {
            id: self.id.clone(),
            name: self.name.clone(),
            category: self.category.clone(),
            status: if stale {
                ServiceStatus::Unknown
            } else {
                self.status
            },
            stale,
            last_checked_at: self.last_checked_at,
            last_success_at: self.last_success_at,
            components: self.components.clone(),
            active_incidents: self
                .incidents
                .iter()
                .filter(|incident| incident.is_active())
                .cloned()
                .collect(),
        }
    }

    fn health(&self, now: DateTime<Utc>) -> SourceHealth {
        let stale = self.stale(now);
        SourceHealth {
            id: self.id.clone(),
            name: self.name.clone(),
            healthy: !stale && self.last_error.is_none(),
            stale,
            last_checked_at: self.last_checked_at,
            last_success_at: self.last_success_at,
            consecutive_failures: self.consecutive_failures,
            last_error: self.last_error.clone(),
        }
    }
}

#[async_trait]
impl StatusStore for InMemoryStore {
    async fn apply(&self, batch: ObservationBatch) -> Result<(), StoreError> {
        let mut records = self.records.write().await;
        let record = records
            .get_mut(&batch.provider_id)
            .ok_or_else(|| StoreError::UnknownSource(batch.provider_id.clone()))?;

        record.name = batch.provider_name;
        record.category = batch.category;
        record.status = batch.status;
        record.components = batch.components;
        record.incidents = batch.incidents;
        record.last_checked_at = Some(batch.observed_at);
        record.last_success_at = Some(batch.observed_at);
        record.consecutive_failures = 0;
        record.last_error = None;
        Ok(())
    }

    async fn mark_failed(
        &self,
        source_id: &str,
        checked_at: DateTime<Utc>,
        error: String,
    ) -> Result<(), StoreError> {
        let mut records = self.records.write().await;
        let record = records
            .get_mut(source_id)
            .ok_or_else(|| StoreError::UnknownSource(source_id.to_owned()))?;
        record.last_checked_at = Some(checked_at);
        record.consecutive_failures = record.consecutive_failures.saturating_add(1);
        record.last_error = Some(error);
        Ok(())
    }

    async fn summary(&self) -> Result<StatusSummary, StoreError> {
        let providers = self.views().await;
        Ok(StatusSummary {
            overall_status: ServiceStatus::worst(providers.iter().map(|provider| provider.status)),
            generated_at: Utc::now(),
            active_incident_count: providers
                .iter()
                .map(|provider| provider.active_incidents.len())
                .sum(),
            stale_source_count: providers.iter().filter(|provider| provider.stale).count(),
            providers,
        })
    }

    async fn providers(&self) -> Result<Vec<ProviderView>, StoreError> {
        Ok(self.views().await)
    }

    async fn provider(&self, id: &str) -> Result<Option<ProviderView>, StoreError> {
        let now = Utc::now();
        Ok(self
            .records
            .read()
            .await
            .get(id)
            .map(|record| record.view(now)))
    }

    async fn active_incidents(&self) -> Result<Vec<Incident>, StoreError> {
        let mut incidents = self
            .records
            .read()
            .await
            .values()
            .flat_map(|record| record.incidents.iter())
            .filter(|incident| incident.is_active())
            .cloned()
            .collect::<Vec<_>>();
        incidents.sort_by_key(|incident| std::cmp::Reverse(incident.updated_at));
        Ok(incidents)
    }

    async fn source_health(&self) -> Result<Vec<SourceHealth>, StoreError> {
        let now = Utc::now();
        let mut health = self
            .records
            .read()
            .await
            .values()
            .map(|record| record.health(now))
            .collect::<Vec<_>>();
        health.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(health)
    }
}

pub type SharedStore = Arc<dyn StatusStore>;
