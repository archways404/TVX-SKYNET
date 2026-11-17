use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceStatus {
    Operational,
    Maintenance,
    Degraded,
    PartialOutage,
    MajorOutage,
    #[default]
    Unknown,
}

impl ServiceStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Operational => "operational",
            Self::Maintenance => "maintenance",
            Self::Degraded => "degraded",
            Self::PartialOutage => "partial_outage",
            Self::MajorOutage => "major_outage",
            Self::Unknown => "unknown",
        }
    }

    pub fn severity(self) -> u8 {
        match self {
            Self::Operational => 0,
            Self::Maintenance => 1,
            Self::Unknown => 2,
            Self::Degraded => 3,
            Self::PartialOutage => 4,
            Self::MajorOutage => 5,
        }
    }

    pub fn worst(statuses: impl IntoIterator<Item = Self>) -> Self {
        statuses
            .into_iter()
            .max_by_key(|status| status.severity())
            .unwrap_or(Self::Unknown)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentLifecycle {
    Investigating,
    Identified,
    Monitoring,
    Resolved,
    Scheduled,
    InProgress,
    Completed,
    Unknown,
}

impl IncidentLifecycle {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Investigating => "investigating",
            Self::Identified => "identified",
            Self::Monitoring => "monitoring",
            Self::Resolved => "resolved",
            Self::Scheduled => "scheduled",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Unknown => "unknown",
        }
    }

    pub fn is_active(&self) -> bool {
        !matches!(self, Self::Resolved | Self::Completed)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentObservation {
    pub external_id: String,
    pub name: String,
    pub status: ServiceStatus,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncidentUpdate {
    pub external_id: String,
    pub lifecycle: IncidentLifecycle,
    pub message: String,
    pub published_at: DateTime<Utc>,
}

/// A marker represents the centre of a published outage area, never a customer address.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkLocation {
    pub name: String,
    pub latitude: f64,
    pub longitude: f64,
    pub precision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkIncidentDetails {
    pub location: Option<String>,
    pub services: Vec<String>,
    pub locations: Vec<NetworkLocation>,
    pub estimated_resolution_at: Option<DateTime<Utc>>,
    pub affected_customers: Option<u64>,
}

impl NetworkIncidentDetails {
    fn is_empty(&self) -> bool {
        self.location.is_none()
            && self.services.is_empty()
            && self.locations.is_empty()
            && self.estimated_resolution_at.is_none()
            && self.affected_customers.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Incident {
    pub provider_id: String,
    pub external_id: String,
    pub title: String,
    pub lifecycle: IncidentLifecycle,
    pub impact: ServiceStatus,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub source_url: Option<String>,
    pub affected_component_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "NetworkIncidentDetails::is_empty")]
    pub network_details: NetworkIncidentDetails,
    pub updates: Vec<IncidentUpdate>,
}

impl Incident {
    pub fn is_active(&self) -> bool {
        self.resolved_at.is_none() && self.lifecycle.is_active()
    }
}

#[derive(Debug, Clone)]
pub struct ObservationBatch {
    pub provider_id: String,
    pub provider_name: String,
    pub category: String,
    pub status: ServiceStatus,
    pub observed_at: DateTime<Utc>,
    pub components: Vec<ComponentObservation>,
    pub incidents: Vec<Incident>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderView {
    pub metadata: crate::config::SourceMetadata,
    pub id: String,
    pub name: String,
    pub category: String,
    pub status: ServiceStatus,
    pub stale: bool,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub components: Vec<ComponentObservation>,
    pub active_incidents: Vec<Incident>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceHealth {
    pub id: String,
    pub name: String,
    pub healthy: bool,
    pub stale: bool,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub consecutive_failures: u32,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusSummary {
    pub overall_status: ServiceStatus,
    pub generated_at: DateTime<Utc>,
    pub active_incident_count: usize,
    pub stale_source_count: usize,
    pub providers: Vec<ProviderView>,
}

#[cfg(test)]
mod tests {
    use super::ServiceStatus;

    #[test]
    fn worst_status_uses_operational_severity_order() {
        assert_eq!(
            ServiceStatus::worst([
                ServiceStatus::Operational,
                ServiceStatus::Unknown,
                ServiceStatus::Degraded,
            ]),
            ServiceStatus::Degraded
        );
    }
}
