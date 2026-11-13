use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool, postgres::PgPoolOptions};

use super::{StatusStore, StoreError};
use crate::{
    config::{PollDefaults, SourceConfig},
    domain::{
        ComponentObservation, Incident, IncidentLifecycle, IncidentUpdate, ObservationBatch,
        ProviderView, ServiceStatus, SourceHealth, StatusSummary,
    },
};

pub struct PostgresStore {
    pool: PgPool,
}

impl PostgresStore {
    pub async fn connect(
        database_url: &str,
        sources: &[SourceConfig],
        defaults: &PollDefaults,
    ) -> Result<Self, StoreError> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect(database_url)
            .await?;

        sqlx::migrate!("./migrations").run(&pool).await?;

        let store = Self { pool };
        store.initialize_sources(sources, defaults).await?;
        Ok(store)
    }

    async fn initialize_sources(
        &self,
        sources: &[SourceConfig],
        defaults: &PollDefaults,
    ) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin().await?;

        sqlx::query("UPDATE external_status.sources SET enabled = FALSE, updated_at = now()")
            .execute(&mut *transaction)
            .await?;

        for source in sources.iter().filter(|source| source.enabled) {
            let stale_after_seconds = duration_seconds(source.stale_after(defaults), &source.id)?;
            sqlx::query(
                r#"
                INSERT INTO external_status.sources (
                    id,
                    name,
                    category,
                    enabled,
                    stale_after_seconds
                )
                VALUES ($1, $2, $3, TRUE, $4)
                ON CONFLICT (id) DO UPDATE SET
                    name = EXCLUDED.name,
                    category = EXCLUDED.category,
                    enabled = TRUE,
                    stale_after_seconds = EXCLUDED.stale_after_seconds,
                    updated_at = now()
                "#,
            )
            .bind(&source.id)
            .bind(&source.name)
            .bind(&source.category)
            .bind(stale_after_seconds)
            .execute(&mut *transaction)
            .await?;
        }

        transaction.commit().await?;
        Ok(())
    }

    async fn source_rows(&self) -> Result<Vec<SourceRow>, StoreError> {
        Ok(sqlx::query_as::<_, SourceRow>(SOURCE_SELECT_ALL)
            .fetch_all(&self.pool)
            .await?)
    }

    async fn source_row(&self, id: &str) -> Result<Option<SourceRow>, StoreError> {
        Ok(sqlx::query_as::<_, SourceRow>(SOURCE_SELECT_ONE)
            .bind(id)
            .fetch_optional(&self.pool)
            .await?)
    }

    async fn provider_view(&self, source: SourceRow) -> Result<ProviderView, StoreError> {
        let components = sqlx::query_as::<_, ComponentRow>(
            r#"
            SELECT external_id, name, current_status, provider_updated_at
            FROM external_status.components
            WHERE provider_id = $1 AND active
            ORDER BY name
            "#,
        )
        .bind(&source.id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(ComponentObservation::try_from)
        .collect::<Result<Vec<_>, _>>()?;

        let incident_rows = sqlx::query_as::<_, IncidentRow>(
            r#"
            SELECT
                provider_id,
                external_id,
                title,
                lifecycle,
                impact,
                started_at,
                provider_updated_at,
                resolved_at,
                source_url,
                affected_component_ids
            FROM external_status.incidents
            WHERE provider_id = $1
              AND resolved_at IS NULL
              AND lifecycle NOT IN ('resolved', 'completed')
            ORDER BY provider_updated_at DESC
            "#,
        )
        .bind(&source.id)
        .fetch_all(&self.pool)
        .await?;

        let active_incidents = self.incidents_with_updates(incident_rows).await?;
        let stale = source.stale(Utc::now());
        let status = if stale {
            ServiceStatus::Unknown
        } else {
            parse_status("sources.current_status", &source.current_status)?
        };

        Ok(ProviderView {
            id: source.id,
            name: source.name,
            category: source.category,
            status,
            stale,
            last_checked_at: source.last_checked_at,
            last_success_at: source.last_success_at,
            components,
            active_incidents,
        })
    }

    async fn incidents_with_updates(
        &self,
        rows: Vec<IncidentRow>,
    ) -> Result<Vec<Incident>, StoreError> {
        let mut incidents = Vec::with_capacity(rows.len());
        for row in rows {
            let updates = sqlx::query_as::<_, IncidentUpdateRow>(
                r#"
                SELECT external_id, lifecycle, message, published_at
                FROM external_status.incident_updates
                WHERE provider_id = $1 AND incident_external_id = $2
                ORDER BY published_at
                "#,
            )
            .bind(&row.provider_id)
            .bind(&row.external_id)
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(IncidentUpdate::try_from)
            .collect::<Result<Vec<_>, _>>()?;

            incidents.push(row.into_incident(updates)?);
        }
        Ok(incidents)
    }
}

#[async_trait]
impl StatusStore for PostgresStore {
    async fn apply(&self, batch: ObservationBatch) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin().await?;

        let source_update = sqlx::query(
            r#"
            UPDATE external_status.sources
            SET
                name = $2,
                category = $3,
                current_status = $4,
                last_checked_at = $5,
                last_success_at = $5,
                consecutive_failures = 0,
                last_error = NULL,
                updated_at = now()
            WHERE id = $1 AND enabled
            "#,
        )
        .bind(&batch.provider_id)
        .bind(&batch.provider_name)
        .bind(&batch.category)
        .bind(batch.status.as_str())
        .bind(batch.observed_at)
        .execute(&mut *transaction)
        .await?;

        if source_update.rows_affected() == 0 {
            return Err(StoreError::UnknownSource(batch.provider_id));
        }

        sqlx::query("UPDATE external_status.components SET active = FALSE WHERE provider_id = $1")
            .bind(&batch.provider_id)
            .execute(&mut *transaction)
            .await?;

        for component in &batch.components {
            sqlx::query(
                r#"
                INSERT INTO external_status.components (
                    provider_id,
                    external_id,
                    name,
                    current_status,
                    provider_updated_at,
                    active,
                    last_seen_at
                )
                VALUES ($1, $2, $3, $4, $5, TRUE, $6)
                ON CONFLICT (provider_id, external_id) DO UPDATE SET
                    name = EXCLUDED.name,
                    current_status = EXCLUDED.current_status,
                    provider_updated_at = EXCLUDED.provider_updated_at,
                    active = TRUE,
                    last_seen_at = EXCLUDED.last_seen_at
                "#,
            )
            .bind(&batch.provider_id)
            .bind(&component.external_id)
            .bind(&component.name)
            .bind(component.status.as_str())
            .bind(component.updated_at)
            .bind(batch.observed_at)
            .execute(&mut *transaction)
            .await?;

            sqlx::query(
                r#"
                INSERT INTO external_status.component_observations (
                    provider_id,
                    component_external_id,
                    observed_at,
                    status
                )
                VALUES ($1, $2, $3, $4)
                "#,
            )
            .bind(&batch.provider_id)
            .bind(&component.external_id)
            .bind(batch.observed_at)
            .bind(component.status.as_str())
            .execute(&mut *transaction)
            .await?;
        }

        let current_incident_ids = batch
            .incidents
            .iter()
            .map(|incident| incident.external_id.clone())
            .collect::<Vec<_>>();

        sqlx::query(
            r#"
            UPDATE external_status.incidents
            SET
                lifecycle = 'resolved',
                resolved_at = COALESCE(resolved_at, $3),
                provider_updated_at = GREATEST(provider_updated_at, $3),
                last_seen_at = $3
            WHERE provider_id = $1
              AND resolved_at IS NULL
              AND NOT (external_id = ANY($2))
            "#,
        )
        .bind(&batch.provider_id)
        .bind(&current_incident_ids)
        .bind(batch.observed_at)
        .execute(&mut *transaction)
        .await?;

        for incident in &batch.incidents {
            sqlx::query(
                r#"
                INSERT INTO external_status.incidents (
                    provider_id,
                    external_id,
                    title,
                    lifecycle,
                    impact,
                    started_at,
                    provider_updated_at,
                    resolved_at,
                    source_url,
                    affected_component_ids,
                    last_seen_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                ON CONFLICT (provider_id, external_id) DO UPDATE SET
                    title = EXCLUDED.title,
                    lifecycle = EXCLUDED.lifecycle,
                    impact = EXCLUDED.impact,
                    started_at = EXCLUDED.started_at,
                    provider_updated_at = EXCLUDED.provider_updated_at,
                    resolved_at = EXCLUDED.resolved_at,
                    source_url = EXCLUDED.source_url,
                    affected_component_ids = EXCLUDED.affected_component_ids,
                    last_seen_at = EXCLUDED.last_seen_at
                "#,
            )
            .bind(&batch.provider_id)
            .bind(&incident.external_id)
            .bind(&incident.title)
            .bind(incident.lifecycle.as_str())
            .bind(incident.impact.as_str())
            .bind(incident.started_at)
            .bind(incident.updated_at)
            .bind(incident.resolved_at)
            .bind(&incident.source_url)
            .bind(&incident.affected_component_ids)
            .bind(batch.observed_at)
            .execute(&mut *transaction)
            .await?;

            for update in &incident.updates {
                sqlx::query(
                    r#"
                    INSERT INTO external_status.incident_updates (
                        provider_id,
                        incident_external_id,
                        external_id,
                        lifecycle,
                        message,
                        published_at
                    )
                    VALUES ($1, $2, $3, $4, $5, $6)
                    ON CONFLICT (provider_id, incident_external_id, external_id) DO UPDATE SET
                        lifecycle = EXCLUDED.lifecycle,
                        message = EXCLUDED.message,
                        published_at = EXCLUDED.published_at
                    "#,
                )
                .bind(&batch.provider_id)
                .bind(&incident.external_id)
                .bind(&update.external_id)
                .bind(update.lifecycle.as_str())
                .bind(&update.message)
                .bind(update.published_at)
                .execute(&mut *transaction)
                .await?;
            }
        }

        sqlx::query(
            r#"
            INSERT INTO external_status.provider_observations (provider_id, observed_at, status)
            VALUES ($1, $2, $3)
            "#,
        )
        .bind(&batch.provider_id)
        .bind(batch.observed_at)
        .bind(batch.status.as_str())
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO external_status.poll_attempts (source_id, checked_at, succeeded)
            VALUES ($1, $2, TRUE)
            "#,
        )
        .bind(&batch.provider_id)
        .bind(batch.observed_at)
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(())
    }

    async fn mark_failed(
        &self,
        source_id: &str,
        checked_at: DateTime<Utc>,
        error: String,
    ) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin().await?;
        let update = sqlx::query(
            r#"
            UPDATE external_status.sources
            SET
                last_checked_at = $2,
                consecutive_failures = consecutive_failures + 1,
                last_error = $3,
                updated_at = now()
            WHERE id = $1 AND enabled
            "#,
        )
        .bind(source_id)
        .bind(checked_at)
        .bind(&error)
        .execute(&mut *transaction)
        .await?;

        if update.rows_affected() == 0 {
            return Err(StoreError::UnknownSource(source_id.to_owned()));
        }

        sqlx::query(
            r#"
            INSERT INTO external_status.poll_attempts (source_id, checked_at, succeeded, error)
            VALUES ($1, $2, FALSE, $3)
            "#,
        )
        .bind(source_id)
        .bind(checked_at)
        .bind(error)
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(())
    }

    async fn summary(&self) -> Result<StatusSummary, StoreError> {
        let providers = self.providers().await?;
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
        let rows = self.source_rows().await?;
        let mut providers = Vec::with_capacity(rows.len());
        for row in rows {
            providers.push(self.provider_view(row).await?);
        }
        Ok(providers)
    }

    async fn provider(&self, id: &str) -> Result<Option<ProviderView>, StoreError> {
        let Some(row) = self.source_row(id).await? else {
            return Ok(None);
        };
        Ok(Some(self.provider_view(row).await?))
    }

    async fn active_incidents(&self) -> Result<Vec<Incident>, StoreError> {
        let rows = sqlx::query_as::<_, IncidentRow>(
            r#"
            SELECT
                incident.provider_id,
                incident.external_id,
                incident.title,
                incident.lifecycle,
                incident.impact,
                incident.started_at,
                incident.provider_updated_at,
                incident.resolved_at,
                incident.source_url,
                incident.affected_component_ids
            FROM external_status.incidents AS incident
            JOIN external_status.sources AS source ON source.id = incident.provider_id
            WHERE source.enabled
              AND incident.resolved_at IS NULL
              AND incident.lifecycle NOT IN ('resolved', 'completed')
            ORDER BY incident.provider_updated_at DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        self.incidents_with_updates(rows).await
    }

    async fn source_health(&self) -> Result<Vec<SourceHealth>, StoreError> {
        let now = Utc::now();
        let mut health = self
            .source_rows()
            .await?
            .into_iter()
            .map(|source| {
                let stale = source.stale(now);
                SourceHealth {
                    id: source.id,
                    name: source.name,
                    healthy: !stale && source.last_error.is_none(),
                    stale,
                    last_checked_at: source.last_checked_at,
                    last_success_at: source.last_success_at,
                    consecutive_failures: source.consecutive_failures.max(0) as u32,
                    last_error: source.last_error,
                }
            })
            .collect::<Vec<_>>();
        health.sort_by_key(|source| source.name.clone());
        Ok(health)
    }
}

const SOURCE_SELECT_ALL: &str = r#"
    SELECT
        id,
        name,
        category,
        stale_after_seconds,
        current_status,
        last_checked_at,
        last_success_at,
        consecutive_failures,
        last_error
    FROM external_status.sources
    WHERE enabled
    ORDER BY name
"#;

const SOURCE_SELECT_ONE: &str = r#"
    SELECT
        id,
        name,
        category,
        stale_after_seconds,
        current_status,
        last_checked_at,
        last_success_at,
        consecutive_failures,
        last_error
    FROM external_status.sources
    WHERE enabled AND id = $1
"#;

#[derive(FromRow)]
struct SourceRow {
    id: String,
    name: String,
    category: String,
    stale_after_seconds: i64,
    current_status: String,
    last_checked_at: Option<DateTime<Utc>>,
    last_success_at: Option<DateTime<Utc>>,
    consecutive_failures: i32,
    last_error: Option<String>,
}

impl SourceRow {
    fn stale(&self, now: DateTime<Utc>) -> bool {
        self.last_success_at
            .and_then(|last_success| now.signed_duration_since(last_success).to_std().ok())
            .is_none_or(|age| age.as_secs() > self.stale_after_seconds.max(0) as u64)
    }
}

#[derive(FromRow)]
struct ComponentRow {
    external_id: String,
    name: String,
    current_status: String,
    provider_updated_at: DateTime<Utc>,
}

impl TryFrom<ComponentRow> for ComponentObservation {
    type Error = StoreError;

    fn try_from(row: ComponentRow) -> Result<Self, Self::Error> {
        Ok(Self {
            external_id: row.external_id,
            name: row.name,
            status: parse_status("components.current_status", &row.current_status)?,
            updated_at: row.provider_updated_at,
        })
    }
}

#[derive(FromRow)]
struct IncidentRow {
    provider_id: String,
    external_id: String,
    title: String,
    lifecycle: String,
    impact: String,
    started_at: DateTime<Utc>,
    provider_updated_at: DateTime<Utc>,
    resolved_at: Option<DateTime<Utc>>,
    source_url: Option<String>,
    affected_component_ids: Vec<String>,
}

impl IncidentRow {
    fn into_incident(self, updates: Vec<IncidentUpdate>) -> Result<Incident, StoreError> {
        Ok(Incident {
            provider_id: self.provider_id,
            external_id: self.external_id,
            title: self.title,
            lifecycle: parse_lifecycle("incidents.lifecycle", &self.lifecycle)?,
            impact: parse_status("incidents.impact", &self.impact)?,
            started_at: self.started_at,
            updated_at: self.provider_updated_at,
            resolved_at: self.resolved_at,
            source_url: self.source_url,
            affected_component_ids: self.affected_component_ids,
            updates,
        })
    }
}

#[derive(FromRow)]
struct IncidentUpdateRow {
    external_id: String,
    lifecycle: String,
    message: String,
    published_at: DateTime<Utc>,
}

impl TryFrom<IncidentUpdateRow> for IncidentUpdate {
    type Error = StoreError;

    fn try_from(row: IncidentUpdateRow) -> Result<Self, Self::Error> {
        Ok(Self {
            external_id: row.external_id,
            lifecycle: parse_lifecycle("incident_updates.lifecycle", &row.lifecycle)?,
            message: row.message,
            published_at: row.published_at,
        })
    }
}

fn parse_status(field: &'static str, value: &str) -> Result<ServiceStatus, StoreError> {
    match value {
        "operational" => Ok(ServiceStatus::Operational),
        "maintenance" => Ok(ServiceStatus::Maintenance),
        "degraded" => Ok(ServiceStatus::Degraded),
        "partial_outage" => Ok(ServiceStatus::PartialOutage),
        "major_outage" => Ok(ServiceStatus::MajorOutage),
        "unknown" => Ok(ServiceStatus::Unknown),
        value => Err(StoreError::InvalidStoredValue {
            field,
            value: value.to_owned(),
        }),
    }
}

fn parse_lifecycle(field: &'static str, value: &str) -> Result<IncidentLifecycle, StoreError> {
    match value {
        "investigating" => Ok(IncidentLifecycle::Investigating),
        "identified" => Ok(IncidentLifecycle::Identified),
        "monitoring" => Ok(IncidentLifecycle::Monitoring),
        "resolved" => Ok(IncidentLifecycle::Resolved),
        "scheduled" => Ok(IncidentLifecycle::Scheduled),
        "in_progress" => Ok(IncidentLifecycle::InProgress),
        "completed" => Ok(IncidentLifecycle::Completed),
        "unknown" => Ok(IncidentLifecycle::Unknown),
        value => Err(StoreError::InvalidStoredValue {
            field,
            value: value.to_owned(),
        }),
    }
}

fn duration_seconds(duration: Duration, source_id: &str) -> Result<i64, StoreError> {
    i64::try_from(duration.as_secs()).map_err(|_| StoreError::InvalidDuration(source_id.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::{parse_lifecycle, parse_status};
    use crate::domain::{IncidentLifecycle, ServiceStatus};

    #[test]
    fn stored_status_values_round_trip() {
        for status in [
            ServiceStatus::Operational,
            ServiceStatus::Maintenance,
            ServiceStatus::Degraded,
            ServiceStatus::PartialOutage,
            ServiceStatus::MajorOutage,
            ServiceStatus::Unknown,
        ] {
            assert_eq!(parse_status("test", status.as_str()).unwrap(), status);
        }
    }

    #[test]
    fn stored_lifecycle_values_round_trip() {
        for lifecycle in [
            IncidentLifecycle::Investigating,
            IncidentLifecycle::Identified,
            IncidentLifecycle::Monitoring,
            IncidentLifecycle::Resolved,
            IncidentLifecycle::Scheduled,
            IncidentLifecycle::InProgress,
            IncidentLifecycle::Completed,
            IncidentLifecycle::Unknown,
        ] {
            assert_eq!(
                parse_lifecycle("test", lifecycle.as_str()).unwrap(),
                lifecycle
            );
        }
    }
}
