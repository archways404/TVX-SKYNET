use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::{
    collectors::{Collector, CollectorError},
    config::{CollectorConfig, SourceConfig},
    domain::{
        ComponentObservation, Incident, IncidentLifecycle, IncidentUpdate, ObservationBatch,
        ServiceStatus,
    },
};

pub struct StatuspageCollector {
    client: reqwest::Client,
    max_response_bytes: usize,
}

impl StatuspageCollector {
    pub fn new(client: reqwest::Client, max_response_bytes: usize) -> Self {
        Self {
            client,
            max_response_bytes,
        }
    }

    fn parse(
        &self,
        source: &SourceConfig,
        body: &[u8],
    ) -> Result<ObservationBatch, CollectorError> {
        let payload: SummaryPayload = serde_json::from_slice(body)?;
        let observed_at = Utc::now();
        let included = |name: &str| {
            source.include_components.is_empty()
                || source
                    .include_components
                    .iter()
                    .any(|included| included.eq_ignore_ascii_case(name))
        };

        let components = payload
            .components
            .into_iter()
            .filter(|component| !component.group)
            .filter(|component| included(&component.name))
            .map(|component| ComponentObservation {
                external_id: component.id,
                name: component.name,
                status: map_component_status(&component.status),
                updated_at: component.updated_at,
            })
            .collect::<Vec<_>>();

        let included_component_ids = components
            .iter()
            .map(|component| component.external_id.as_str())
            .collect::<std::collections::HashSet<_>>();

        let provider_status = if components.is_empty() {
            map_indicator(&payload.status.indicator)
        } else {
            ServiceStatus::worst(components.iter().map(|component| component.status))
        };

        let incidents = payload
            .incidents
            .into_iter()
            .filter_map(|incident| {
                let affected_component_ids = incident
                    .components
                    .iter()
                    .map(|component| component.id.clone())
                    .collect::<Vec<_>>();

                if !source.include_components.is_empty()
                    && !affected_component_ids.is_empty()
                    && !affected_component_ids
                        .iter()
                        .any(|id| included_component_ids.contains(id.as_str()))
                {
                    return None;
                }

                let lifecycle = map_incident_lifecycle(&incident.status);
                let mut updates = incident
                    .incident_updates
                    .into_iter()
                    .map(|update| IncidentUpdate {
                        external_id: update.id,
                        lifecycle: map_incident_lifecycle(&update.status),
                        message: update.body,
                        published_at: update.display_at.unwrap_or(update.created_at),
                    })
                    .collect::<Vec<_>>();
                updates.sort_by_key(|update| update.published_at);

                Some(Incident {
                    provider_id: source.id.clone(),
                    external_id: incident.id,
                    title: incident.name,
                    lifecycle,
                    impact: map_impact(&incident.impact),
                    started_at: incident.created_at,
                    updated_at: incident.updated_at,
                    resolved_at: incident.resolved_at,
                    source_url: incident.shortlink,
                    affected_component_ids,
                    updates,
                })
            })
            .collect();

        Ok(ObservationBatch {
            provider_id: source.id.clone(),
            provider_name: source.name.clone(),
            category: source.category.clone(),
            status: provider_status,
            observed_at,
            components,
            incidents,
        })
    }
}

#[async_trait]
impl Collector for StatuspageCollector {
    async fn collect(&self, source: &SourceConfig) -> Result<ObservationBatch, CollectorError> {
        let CollectorConfig::Statuspage { base_url } = &source.collector else {
            return Err(CollectorError::Unsupported(source.id.clone()));
        };
        let endpoint = base_url
            .join("api/v2/summary.json")
            .map_err(|error| CollectorError::Unsupported(error.to_string()))?;

        let response = self.client.get(endpoint).send().await?;
        if !response.status().is_success() {
            return Err(CollectorError::HttpStatus(response.status()));
        }

        if let Some(length) = response.content_length()
            && length > self.max_response_bytes as u64
        {
            return Err(CollectorError::ResponseTooLarge {
                actual: length as usize,
                limit: self.max_response_bytes,
            });
        }

        let body = response.bytes().await?;
        if body.len() > self.max_response_bytes {
            return Err(CollectorError::ResponseTooLarge {
                actual: body.len(),
                limit: self.max_response_bytes,
            });
        }

        self.parse(source, &body)
    }
}

#[derive(Debug, Deserialize)]
struct SummaryPayload {
    status: StatuspageStatus,
    #[serde(default)]
    components: Vec<StatuspageComponent>,
    #[serde(default)]
    incidents: Vec<StatuspageIncident>,
}

#[derive(Debug, Deserialize)]
struct StatuspageStatus {
    indicator: String,
}

#[derive(Debug, Deserialize)]
struct StatuspageComponent {
    id: String,
    name: String,
    status: String,
    updated_at: DateTime<Utc>,
    #[serde(default)]
    group: bool,
}

#[derive(Debug, Deserialize)]
struct IncidentComponent {
    id: String,
}

#[derive(Debug, Deserialize)]
struct StatuspageIncident {
    id: String,
    name: String,
    status: String,
    impact: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    resolved_at: Option<DateTime<Utc>>,
    shortlink: Option<String>,
    #[serde(default)]
    components: Vec<IncidentComponent>,
    #[serde(default)]
    incident_updates: Vec<StatuspageIncidentUpdate>,
}

#[derive(Debug, Deserialize)]
struct StatuspageIncidentUpdate {
    id: String,
    status: String,
    body: String,
    created_at: DateTime<Utc>,
    display_at: Option<DateTime<Utc>>,
}

fn map_indicator(value: &str) -> ServiceStatus {
    match value {
        "none" => ServiceStatus::Operational,
        "minor" => ServiceStatus::Degraded,
        "major" => ServiceStatus::PartialOutage,
        "critical" => ServiceStatus::MajorOutage,
        _ => ServiceStatus::Unknown,
    }
}

fn map_component_status(value: &str) -> ServiceStatus {
    match value {
        "operational" => ServiceStatus::Operational,
        "under_maintenance" => ServiceStatus::Maintenance,
        "degraded_performance" => ServiceStatus::Degraded,
        "partial_outage" => ServiceStatus::PartialOutage,
        "major_outage" => ServiceStatus::MajorOutage,
        _ => ServiceStatus::Unknown,
    }
}

fn map_impact(value: &str) -> ServiceStatus {
    match value {
        "none" => ServiceStatus::Operational,
        "minor" => ServiceStatus::Degraded,
        "major" => ServiceStatus::PartialOutage,
        "critical" => ServiceStatus::MajorOutage,
        _ => ServiceStatus::Unknown,
    }
}

fn map_incident_lifecycle(value: &str) -> IncidentLifecycle {
    match value {
        "investigating" => IncidentLifecycle::Investigating,
        "identified" => IncidentLifecycle::Identified,
        "monitoring" => IncidentLifecycle::Monitoring,
        "resolved" | "postmortem" => IncidentLifecycle::Resolved,
        "scheduled" => IncidentLifecycle::Scheduled,
        "in_progress" | "verifying" => IncidentLifecycle::InProgress,
        "completed" => IncidentLifecycle::Completed,
        _ => IncidentLifecycle::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use url::Url;

    use super::StatuspageCollector;
    use crate::{
        config::{CollectorConfig, SourceConfig},
        domain::{IncidentLifecycle, ServiceStatus},
    };

    #[test]
    fn parses_and_filters_statuspage_summary() {
        let collector = StatuspageCollector::new(reqwest::Client::new(), 1024 * 1024);
        let source = SourceConfig {
            id: "claude".into(),
            name: "Claude".into(),
            category: "developer-tools".into(),
            enabled: true,
            poll_interval: Some(Duration::from_secs(60)),
            request_timeout: None,
            stale_after: None,
            include_components: vec!["Claude Code".into()],
            collector: CollectorConfig::Statuspage {
                base_url: Url::parse("https://status.claude.com/").unwrap(),
            },
        };
        let payload = br#"{
          "status":{"indicator":"minor"},
          "components":[
            {"id":"code","name":"Claude Code","status":"degraded_performance","updated_at":"2026-11-13T10:00:00Z","group":false},
            {"id":"api","name":"Claude API","status":"operational","updated_at":"2026-11-13T10:00:00Z","group":false}
          ],
          "incidents":[{
            "id":"incident-1","name":"Elevated errors","status":"investigating","impact":"minor",
            "created_at":"2026-11-13T09:55:00Z","updated_at":"2026-11-13T10:00:00Z",
            "resolved_at":null,"shortlink":"https://stspg.io/example",
            "components":[{"id":"code"}],
            "incident_updates":[{"id":"update-1","status":"investigating","body":"We are investigating.","created_at":"2026-11-13T09:55:00Z","display_at":null}]
          }]
        }"#;

        let batch = collector.parse(&source, payload).unwrap();
        assert_eq!(batch.status, ServiceStatus::Degraded);
        assert_eq!(batch.components.len(), 1);
        assert_eq!(batch.components[0].name, "Claude Code");
        assert_eq!(batch.incidents.len(), 1);
        assert_eq!(
            batch.incidents[0].lifecycle,
            IncidentLifecycle::Investigating
        );
    }

    #[test]
    fn selected_components_determine_provider_status() {
        let collector = StatuspageCollector::new(reqwest::Client::new(), 1024 * 1024);
        let source = SourceConfig {
            id: "github".into(),
            name: "GitHub".into(),
            category: "developer-tools".into(),
            enabled: true,
            poll_interval: None,
            request_timeout: None,
            stale_after: None,
            include_components: vec!["Actions".into()],
            collector: CollectorConfig::Statuspage {
                base_url: Url::parse("https://www.githubstatus.com/").unwrap(),
            },
        };
        let payload = br#"{
          "status":{"indicator":"major"},
          "components":[
            {"id":"actions","name":"Actions","status":"operational","updated_at":"2026-11-13T10:00:00Z","group":false},
            {"id":"pages","name":"Pages","status":"major_outage","updated_at":"2026-11-13T10:00:00Z","group":false}
          ],
          "incidents":[]
        }"#;

        let batch = collector.parse(&source, payload).unwrap();
        assert_eq!(batch.status, ServiceStatus::Operational);
        assert_eq!(batch.components.len(), 1);
    }
}
