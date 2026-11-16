use super::*;
use scraper::{Html, Selector};

pub(super) fn parse(
    source: &SourceConfig,
    body: &[u8],
) -> Result<ObservationBatch, CollectorError> {
    if matches!(
        source.collector,
        CollectorConfig::Pipedrive { .. } | CollectorConfig::PublicNotice { .. }
    ) {
        return parse_page(source, body);
    }
    let payload: Value = serde_json::from_slice(body)?;
    let mut batch = empty(source);
    match source.collector {
        CollectorConfig::MicrosoftPublic { .. } => {
            let status = match text(&payload, "Status")? {
                "Available" => ServiceStatus::Operational,
                "ServiceDegradation" | "Service degradation" => ServiceStatus::Degraded,
                "ServiceInterruption" | "Service interruption" => ServiceStatus::MajorOutage,
                _ => ServiceStatus::Unknown,
            };
            batch.components.push(ComponentObservation {
                external_id: "m365-public".into(),
                name: "Microsoft 365 public advisory".into(),
                status,
                updated_at: date(&payload, "LastUpdatedTime")?,
            });
            if status != ServiceStatus::Operational {
                let title = text(&payload, "Title")?;
                batch.incidents.push(incident(
                    source,
                    text(&payload, "Id")?,
                    if title.is_empty() {
                        "Microsoft 365 public service advisory"
                    } else {
                        title
                    },
                    date(&payload, "AuthoredDateTime")?,
                    date(&payload, "LastUpdatedTime")?,
                    status,
                ));
            }
        }
        CollectorConfig::Freshstatus { .. } => {
            fn visit(
                nodes: &[Value],
                prefix: &str,
                batch: &mut ObservationBatch,
            ) -> Result<(), CollectorError> {
                for node in nodes {
                    if matches!(node.pointer("/display_options/ignore_overall_status"), Some(Value::String(s)) if s == "true")
                        || node
                            .pointer("/display_options/ignore_overall_status")
                            .and_then(Value::as_bool)
                            == Some(true)
                    {
                        continue;
                    }
                    let name = text(node, "name")?;
                    let name = if prefix.is_empty() {
                        name.to_owned()
                    } else {
                        format!("{prefix} / {name}")
                    };
                    if let Some(children) = node
                        .get("components")
                        .and_then(Value::as_array)
                        .filter(|n| !n.is_empty())
                    {
                        visit(children, &name, batch)?;
                        continue;
                    }
                    let status = match text(node, "status")? {
                        "OP" => ServiceStatus::Operational,
                        "PD" | "DE" => ServiceStatus::Degraded,
                        "PO" => ServiceStatus::PartialOutage,
                        "MO" => ServiceStatus::MajorOutage,
                        "UM" => ServiceStatus::Maintenance,
                        _ => ServiceStatus::Unknown,
                    };
                    batch.components.push(ComponentObservation {
                        external_id: node
                            .get("id")
                            .ok_or_else(|| invalid("Freshstatus id missing"))?
                            .to_string(),
                        name,
                        status,
                        updated_at: batch.observed_at,
                    });
                }
                Ok(())
            }
            visit(array(&payload, "components")?, "", &mut batch)?;
            if batch.components.is_empty() {
                return Err(invalid("Freshstatus components missing"));
            }
        }
        CollectorConfig::Instatus { .. } => {
            let page = payload
                .get("page")
                .ok_or_else(|| invalid("Instatus page missing"))?;
            let status = match text(page, "status")? {
                "UP" => ServiceStatus::Operational,
                "HASISSUES" => ServiceStatus::Degraded,
                "UNDERMAINTENANCE" => ServiceStatus::Maintenance,
                _ => ServiceStatus::Unknown,
            };
            batch.components.push(ComponentObservation {
                external_id: "page".into(),
                name: source.name.clone(),
                status,
                updated_at: batch.observed_at,
            });
            if let Some(items) = payload.get("activeIncidents").and_then(Value::as_array) {
                for item in items {
                    let impact = match text(item, "impact")? {
                        "MAJOROUTAGE" => ServiceStatus::MajorOutage,
                        "PARTIALOUTAGE" => ServiceStatus::PartialOutage,
                        "DEGRADEDPERFORMANCE" => ServiceStatus::Degraded,
                        _ => ServiceStatus::Unknown,
                    };
                    let mut entry = incident(
                        source,
                        text(item, "id")?,
                        text(item, "name")?,
                        date(item, "started")?,
                        date(item, "updatedAt")?,
                        impact,
                    );
                    entry.source_url = Some(text(item, "url")?.into());
                    batch.incidents.push(entry);
                }
            }
        }
        CollectorConfig::Adyen { .. } => {
            let collection = payload
                .get("incidentMessageCollection")
                .ok_or_else(|| invalid("Adyen collection missing"))?;
            for item in array(collection, "items")? {
                let resolved = item
                    .get("resolved")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| invalid("Adyen resolved missing"))?;
                if resolved {
                    continue;
                }
                let impact = match text(item, "severity")? {
                    "RED" | "MAJOR" | "SEVERE" => ServiceStatus::MajorOutage,
                    "ORANGE" => ServiceStatus::PartialOutage,
                    "YELLOW" | "MINOR" => ServiceStatus::Degraded,
                    _ => ServiceStatus::Unknown,
                };
                let start = date(item, "date")?;
                let id = item
                    .pointer("/sys/id")
                    .and_then(Value::as_str)
                    .unwrap_or(text(item, "title")?);
                batch.incidents.push(incident(
                    source,
                    id,
                    text(item, "title")?,
                    start,
                    start,
                    impact,
                ));
            }
        }
        CollectorConfig::Zendesk { .. } => {
            for item in array(&payload, "data")? {
                let attrs = item
                    .get("attributes")
                    .ok_or_else(|| invalid("Zendesk attributes missing"))?;
                let status = text(attrs, "status")?;
                if matches!(status, "resolved" | "completed" | "cancelled") {
                    continue;
                }
                // Resolution must be explicit; missing fields must never imply healthy service.
                if !attrs
                    .get("resolvedAt")
                    .ok_or_else(|| invalid("Zendesk resolvedAt missing"))?
                    .is_null()
                {
                    continue;
                }
                if attrs.get("internalToolsOnly").and_then(Value::as_bool) == Some(true) {
                    continue;
                }
                let impact = match text(attrs, "impact")? {
                    "critical" => ServiceStatus::MajorOutage,
                    "major" => ServiceStatus::PartialOutage,
                    "minor" => ServiceStatus::Degraded,
                    _ => ServiceStatus::Unknown,
                };
                let start = date(attrs, "startedAt")?;
                let mut entry = incident(
                    source,
                    text(item, "id")?,
                    text(attrs, "name")?,
                    start,
                    start,
                    impact,
                );
                entry.source_url = Some(format!(
                    "https://status.zendesk.com/incidents/{}",
                    entry.external_id
                ));
                entry.lifecycle = match status {
                    "monitoring" => IncidentLifecycle::Monitoring,
                    "identified" => IncidentLifecycle::Identified,
                    _ => IncidentLifecycle::Investigating,
                };
                batch.incidents.push(entry);
            }
        }
        _ => return Err(invalid("unsupported public collector")),
    }
    finish(&mut batch);
    Ok(batch)
}

fn selector(css: &str) -> Selector {
    Selector::parse(css).expect("static selector")
}
fn parse_page(source: &SourceConfig, body: &[u8]) -> Result<ObservationBatch, CollectorError> {
    let html = std::str::from_utf8(body).map_err(|_| invalid("invalid public page encoding"))?;
    let doc = Html::parse_document(html);
    let mut batch = empty(source);
    if matches!(source.collector, CollectorConfig::PublicNotice { .. }) {
        // A reachable corporate/support page does not establish network health.
        // Preserve automatic collection/freshness while explicitly reporting its coverage limit.
        if doc.select(&selector("title")).next().is_none() {
            return Err(invalid("public provider page has no title"));
        }
        batch.status = ServiceStatus::Unknown;
        return Ok(batch);
    }
    for node in doc.select(&selector(
        "#page-components li.list-component:not(.has-nested-components)",
    )) {
        let name = node
            .select(&selector(".list-component-name"))
            .next()
            .ok_or_else(|| invalid("Pipedrive component name missing"))?
            .text()
            .collect::<String>();
        let state = node
            .select(&selector(".list-component-state"))
            .next()
            .ok_or_else(|| invalid("Pipedrive component state missing"))?;
        let class = state.value().attr("class").unwrap_or("");
        let status = if class.contains("state-operational") {
            ServiceStatus::Operational
        } else if class.contains("state-degraded") {
            ServiceStatus::Degraded
        } else if class.contains("state-unavailable") {
            ServiceStatus::MajorOutage
        } else if class.contains("state-maintenance") {
            ServiceStatus::Maintenance
        } else {
            ServiceStatus::Unknown
        };
        batch.components.push(ComponentObservation {
            external_id: node
                .value()
                .attr("id")
                .ok_or_else(|| invalid("Pipedrive component id missing"))?
                .into(),
            name: name.trim().into(),
            status,
            updated_at: batch.observed_at,
        });
    }
    if batch.components.is_empty() {
        return Err(invalid("Pipedrive component markup missing"));
    }
    for node in doc.select(&selector("#current-status .notice-subject")) {
        let title = node.text().collect::<String>();
        batch.incidents.push(incident(
            source,
            &title,
            &title,
            batch.observed_at,
            batch.observed_at,
            ServiceStatus::Degraded,
        ));
    }
    finish(&mut batch);
    Ok(batch)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source(kind: &str) -> SourceConfig {
        serde_yaml::from_str(&format!("id: test\nname: Test\ncategory: crm\ncollector:\n  type: {kind}\n  url: https://example.com/\n")).unwrap()
    }
    #[test]
    fn microsoft_public_health_needs_no_tenant() {
        let batch=parse(&source("microsoft_public"),br#"{"Id":"1","Status":"Available","Title":"","AuthoredDateTime":"2026-11-16T09:00:00Z","LastUpdatedTime":"2026-11-16T10:00:00Z"}"#).unwrap();
        assert_eq!(batch.status, ServiceStatus::Operational);
        assert!(parse(&source("microsoft_public"), br#"{"Status":"Available"}"#).is_err());
    }
    #[test]
    fn freshdesk_keeps_region_and_normalizes_component_outages() {
        let batch=parse(&source("freshstatus"),br#"{"components":[{"id":1,"name":"EU","components":[{"id":2,"name":"Agent Portal","status":"PD"}]}]}"#).unwrap();
        assert_eq!(batch.status, ServiceStatus::Degraded);
        assert_eq!(batch.components[0].name, "EU / Agent Portal");
        assert!(parse(&source("freshstatus"), br#"{"components":[]}"#).is_err());
    }
    #[test]
    fn freshdesk_excludes_components_vendor_marks_ignored() {
        let batch=parse(&source("freshstatus"),br#"{"components":[{"id":1,"name":"EU","status":"OP"},{"id":2,"name":"Unused region","status":"MO","display_options":{"ignore_overall_status":"true"}}]}"#).unwrap();
        assert_eq!(batch.status, ServiceStatus::Operational);
        assert_eq!(batch.components.len(), 1);
    }
    #[test]
    fn spirius_retains_outage_title_and_url() {
        let batch=parse(&source("instatus"),br#"{"page":{"status":"HASISSUES"},"activeIncidents":[{"id":"sms","name":"SMS delivery failing","impact":"MAJOROUTAGE","started":"2026-11-16T09:00:00Z","updatedAt":"2026-11-16T10:00:00Z","url":"https://spirius.instatus.com/incident/sms"}]}"#).unwrap();
        assert_eq!(batch.status, ServiceStatus::MajorOutage);
        assert_eq!(batch.incidents[0].title, "SMS delivery failing");
    }
    #[test]
    fn adyen_requires_valid_collection_and_explicit_resolution() {
        assert!(parse(&source("adyen"), b"{}").is_err());
        let batch=parse(&source("adyen"),br#"{"incidentMessageCollection":{"items":[{"resolved":false,"title":"Checkout failures","severity":"RED","date":"2026-11-16T09:00:00Z"}]}}"#).unwrap();
        assert_eq!(batch.status, ServiceStatus::MajorOutage);
        assert_eq!(batch.incidents.len(), 1);
    }
    #[test]
    fn zendesk_excludes_resolved_and_internal_incidents() {
        let batch=parse(&source("zendesk"),br#"{"data":[{"id":"1","attributes":{"status":"resolved"}},{"id":"2","attributes":{"status":"investigating","resolvedAt":null,"internalToolsOnly":false,"impact":"major","name":"API unavailable","startedAt":"2026-11-16T09:00:00Z"}}]}"#).unwrap();
        assert_eq!(batch.incidents.len(), 1);
        assert_eq!(batch.status, ServiceStatus::PartialOutage);
    }
    #[test]
    fn public_corporate_page_never_implies_healthy_network() {
        let batch = parse(
            &source("public_notice"),
            b"<html><title>Carrier support</title><body>Support</body></html>",
        )
        .unwrap();
        assert_eq!(batch.status, ServiceStatus::Unknown);
        assert!(batch.incidents.is_empty());
    }
    #[test]
    fn pipedrive_reads_active_notices_even_when_components_are_green() {
        let batch=parse(&source("pipedrive"),br#"<section id="page-components"><li id="api" class="list-component"><strong class="list-component-name">API</strong><div class="list-component-state list-component-state-operational"></div></li></section><section id="current-status"><h3 class="notice-subject">Email sync interrupted</h3></section>"#).unwrap();
        assert_eq!(batch.status, ServiceStatus::Degraded);
        assert_eq!(batch.incidents[0].title, "Email sync interrupted");
        assert!(parse(&source("pipedrive"), b"<title>Changed page</title>").is_err());
    }
    #[test]
    fn shipped_sources_have_no_manual_or_tenant_requirement() {
        let config = crate::config::AppConfig::load("config/sources.yaml").unwrap();
        for source in config.sources.iter().filter(|s| s.enabled) {
            assert!(
                !matches!(
                    source.collector,
                    CollectorConfig::Manual { .. }
                        | CollectorConfig::MicrosoftGraph
                        | CollectorConfig::PublicNotice { .. }
                ),
                "{}",
                source.id
            );
        }
    }
}
