mod carriers;
mod public;
mod telenor;
mod telia;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use url::Url;

use crate::{
    collectors::{Collector, CollectorError},
    config::{CollectorConfig, SourceConfig},
    domain::{
        ComponentObservation, Incident, IncidentLifecycle, IncidentUpdate, ObservationBatch,
        ServiceStatus,
    },
};

pub struct CustomCollector {
    client: reqwest::Client,
    limit: usize,
}

impl CustomCollector {
    pub fn new(client: reqwest::Client, limit: usize) -> Self {
        Self { client, limit }
    }

    async fn body(&self, request: reqwest::RequestBuilder) -> Result<Vec<u8>, CollectorError> {
        let mut response = request.send().await?;
        if !response.status().is_success() {
            return Err(CollectorError::HttpStatus(response.status()));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len() + chunk.len() > self.limit {
                return Err(CollectorError::ResponseTooLarge {
                    actual: bytes.len() + chunk.len(),
                    limit: self.limit,
                });
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    async fn graph(&self, source: &SourceConfig) -> Result<ObservationBatch, CollectorError> {
        let env = |key: &str| {
            std::env::var(key)
                .ok()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    CollectorError::Invalid(format!("{key} is required for tenant service health"))
                })
        };
        let tenant = env("MICROSOFT_TENANT_ID")?;
        // Keep the credential destination fixed, and never log provider response bodies or secrets.
        if !tenant
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
            || tenant.is_empty()
        {
            return Err(CollectorError::Invalid(
                "invalid MICROSOFT_TENANT_ID".into(),
            ));
        }
        let token_body = self
            .body(
                self.client
                    .post(format!(
                        "https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token"
                    ))
                    .form(&[
                        ("client_id", env("MICROSOFT_CLIENT_ID")?),
                        ("client_secret", env("MICROSOFT_CLIENT_SECRET")?),
                        ("scope", "https://graph.microsoft.com/.default".into()),
                        ("grant_type", "client_credentials".into()),
                    ]),
            )
            .await?;
        let token: Value = serde_json::from_slice(&token_body)?;
        let token = text(&token, "access_token")?;
        let mut endpoint = Url::parse("https://graph.microsoft.com/v1.0/admin/serviceAnnouncement/issues?$filter=isResolved%20eq%20false").unwrap();
        let mut batch = empty(source);
        for _ in 0..100 {
            let body = self
                .body(self.client.get(endpoint.clone()).bearer_auth(token))
                .await?;
            let payload: Value = serde_json::from_slice(&body)?;
            append_graph_issues(source, &payload, &mut batch)?;
            match payload.get("@odata.nextLink").and_then(Value::as_str) {
                None => {
                    finish(&mut batch);
                    return Ok(batch);
                }
                Some(next) => {
                    endpoint =
                        Url::parse(next).map_err(|_| invalid("invalid Graph pagination URL"))?;
                    if endpoint.scheme() != "https"
                        || endpoint.host_str() != Some("graph.microsoft.com")
                        || endpoint.port_or_known_default() != Some(443)
                    {
                        return Err(invalid("untrusted Graph pagination URL"));
                    }
                }
            }
        }
        Err(invalid("Graph pagination exceeded limit"))
    }
}

fn append_graph_issues(
    source: &SourceConfig,
    payload: &Value,
    batch: &mut ObservationBatch,
) -> Result<(), CollectorError> {
    for item in array(payload, "value")? {
        if !included(source, &[text(item, "service")?]) {
            continue;
        }
        let status = match text(item, "status")? {
            "serviceOperational"
            | "serviceRestored"
            | "falsePositive"
            | "postIncidentReviewPublished" => continue,
            "serviceDegradation" | "restoringService" | "extendedRecovery" | "verifyingService" => {
                ServiceStatus::Degraded
            }
            "serviceInterruption" => ServiceStatus::MajorOutage,
            _ => ServiceStatus::Unknown,
        };
        let resolved = item
            .get("isResolved")
            .and_then(Value::as_bool)
            .ok_or_else(|| invalid("Graph isResolved missing"))?;
        if resolved {
            continue;
        }
        let mut incident = incident(
            source,
            text(item, "id")?,
            text(item, "title")?,
            date(item, "startDateTime")?,
            date(item, "lastModifiedDateTime")?,
            status,
        );
        incident.source_url = Some("https://admin.microsoft.com/#/servicehealth".into());
        batch.incidents.push(incident);
    }
    Ok(())
}

#[async_trait]
impl Collector for CustomCollector {
    async fn collect(&self, source: &SourceConfig) -> Result<ObservationBatch, CollectorError> {
        let url = match &source.collector {
            CollectorConfig::MicrosoftGraph => return self.graph(source).await,
            CollectorConfig::Telia { url } => return telia::collect(self, source, url).await,
            CollectorConfig::Telenor { url } => return telenor::collect(self, source, url).await,
            CollectorConfig::Manual { path } => {
                let file = std::fs::File::open(path)?;
                use std::io::Read;
                let mut body = Vec::new();
                file.take(self.limit as u64 + 1).read_to_end(&mut body)?;
                if body.len() > self.limit {
                    return Err(invalid("manual status file too large"));
                }
                return parse_manual(source, &body);
            }
            CollectorConfig::Google { url }
            | CollectorConfig::Apple { url }
            | CollectorConfig::Salesforce { url }
            | CollectorConfig::Slack { url }
            | CollectorConfig::Meta { url }
            | CollectorConfig::Feed { url }
            | CollectorConfig::MicrosoftPublic { url }
            | CollectorConfig::Freshstatus { url }
            | CollectorConfig::Instatus { url }
            | CollectorConfig::Adyen { url }
            | CollectorConfig::Zendesk { url }
            | CollectorConfig::Pipedrive { url }
            | CollectorConfig::PublicNotice { url }
            | CollectorConfig::Tdc { url }
            | CollectorConfig::Norlys { url }
            | CollectorConfig::StatusIo { url } => url,
            _ => return Err(CollectorError::Unsupported(source.id.clone())),
        };
        let mut endpoint = url.clone();
        if matches!(source.collector, CollectorConfig::Zendesk { .. }) {
            endpoint
                .query_pairs_mut()
                .append_pair("as_of_date", &Utc::now().format("%Y-%m-%d").to_string());
        }
        let body = self.body(self.client.get(endpoint)).await?;
        parse(source, &body)
    }
}

fn invalid(message: &str) -> CollectorError {
    CollectorError::Invalid(message.into())
}
fn text<'a>(item: &'a Value, key: &str) -> Result<&'a str, CollectorError> {
    item.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(&format!("missing string {key}")))
}
fn array<'a>(item: &'a Value, key: &str) -> Result<&'a Vec<Value>, CollectorError> {
    item.get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| invalid(&format!("missing array {key}")))
}
fn date(item: &Value, key: &str) -> Result<DateTime<Utc>, CollectorError> {
    DateTime::parse_from_rfc3339(text(item, key)?)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|_| invalid(&format!("invalid timestamp {key}")))
}
fn included(source: &SourceConfig, names: &[&str]) -> bool {
    source.include_components.is_empty()
        || names.iter().any(|name| {
            source
                .include_components
                .iter()
                .any(|s| s.eq_ignore_ascii_case(name))
        })
}
fn empty(source: &SourceConfig) -> ObservationBatch {
    ObservationBatch {
        provider_id: source.id.clone(),
        provider_name: source.name.clone(),
        category: source.category.clone(),
        status: ServiceStatus::Operational,
        observed_at: Utc::now(),
        components: vec![],
        incidents: vec![],
    }
}
fn incident(
    source: &SourceConfig,
    id: &str,
    title: &str,
    started: DateTime<Utc>,
    updated: DateTime<Utc>,
    impact: ServiceStatus,
) -> Incident {
    Incident {
        provider_id: source.id.clone(),
        external_id: id.into(),
        title: title.into(),
        lifecycle: IncidentLifecycle::Investigating,
        impact,
        started_at: started,
        updated_at: updated,
        resolved_at: None,
        source_url: Some(source.metadata.status_url.clone()),
        affected_component_ids: vec![],
        updates: vec![],
    }
}
fn finish(batch: &mut ObservationBatch) {
    batch.status = ServiceStatus::worst(
        std::iter::once(ServiceStatus::Operational)
            .chain(batch.components.iter().map(|c| c.status))
            .chain(
                batch
                    .incidents
                    .iter()
                    .filter(|i| i.is_active())
                    .map(|i| i.impact),
            ),
    );
}

pub(super) fn parse(
    source: &SourceConfig,
    body: &[u8],
) -> Result<ObservationBatch, CollectorError> {
    if matches!(
        source.collector,
        CollectorConfig::Tdc { .. }
            | CollectorConfig::Norlys { .. }
            | CollectorConfig::StatusIo { .. }
    ) {
        return carriers::parse(source, body);
    }
    if matches!(
        source.collector,
        CollectorConfig::MicrosoftPublic { .. }
            | CollectorConfig::Freshstatus { .. }
            | CollectorConfig::Instatus { .. }
            | CollectorConfig::Adyen { .. }
            | CollectorConfig::Zendesk { .. }
            | CollectorConfig::Pipedrive { .. }
            | CollectorConfig::PublicNotice { .. }
    ) {
        return public::parse(source, body);
    }
    if matches!(source.collector, CollectorConfig::Feed { .. }) {
        return parse_rss(source, body);
    }
    let mut batch = empty(source);
    let json_body;
    let body = if matches!(source.collector, CollectorConfig::Apple { .. }) {
        let s = std::str::from_utf8(body)
            .map_err(|_| invalid("invalid Apple encoding"))?
            .trim();
        json_body = s
            .strip_prefix("jsonCallback(")
            .and_then(|s| s.strip_suffix(");"))
            .unwrap_or(s)
            .as_bytes()
            .to_vec();
        &json_body
    } else {
        body
    };
    let payload: Value = serde_json::from_slice(body)?;
    match &source.collector {
        CollectorConfig::Google { url } => {
            let items = payload
                .as_array()
                .ok_or_else(|| invalid("expected Google incident array"))?;
            for item in items {
                let mut products = array(item, "affected_products")?
                    .iter()
                    .map(|p| text(p, "title"))
                    .collect::<Result<Vec<_>, _>>()?;
                if let Some(name) = item.get("service_name").and_then(Value::as_str) {
                    products.push(name);
                }
                if !included(source, &products) {
                    continue;
                }
                // Require explicit resolution information. A schema change must not turn an outage green.
                let end = item
                    .get("end")
                    .ok_or_else(|| invalid("Google incident end missing"))?;
                if !end.is_null() {
                    date(item, "end")?;
                    continue;
                }
                let impact = match text(item, "status_impact")? {
                    "SERVICE_OUTAGE" => ServiceStatus::MajorOutage,
                    "SERVICE_DISRUPTION" => ServiceStatus::Degraded,
                    "SERVICE_INFORMATION" => ServiceStatus::Unknown,
                    _ => ServiceStatus::Unknown,
                };
                let mut entry = incident(
                    source,
                    text(item, "id")?,
                    text(item, "external_desc")?,
                    date(item, "begin")?,
                    date(item, "modified")?,
                    impact,
                );
                entry.source_url = Some(
                    url.join(text(item, "uri")?)
                        .map_err(|_| invalid("invalid Google incident URL"))?
                        .into(),
                );
                for (index, update) in array(item, "updates")?.iter().enumerate() {
                    entry.updates.push(IncidentUpdate {
                        external_id: format!("{}-{index}", entry.external_id),
                        lifecycle: IncidentLifecycle::Investigating,
                        message: text(update, "text")?.into(),
                        published_at: date(update, "when")?,
                    });
                }
                batch.incidents.push(entry);
            }
        }
        CollectorConfig::Apple { .. } => {
            for service in array(&payload, "services")? {
                let name = text(service, "serviceName")?;
                if !included(source, &[name]) {
                    continue;
                }
                let mut status = ServiceStatus::Operational;
                for (index, event) in array(service, "events")?.iter().enumerate() {
                    let event_status = text(event, "eventStatus")?;
                    if event_status == "resolved" {
                        continue;
                    }
                    let impact = if event_status == "ongoing" {
                        match text(event, "statusType")? {
                            "Outage" => ServiceStatus::MajorOutage,
                            "Maintenance" => ServiceStatus::Maintenance,
                            "Issue" => ServiceStatus::Degraded,
                            _ => ServiceStatus::Unknown,
                        }
                    } else {
                        ServiceStatus::Unknown
                    };
                    status = ServiceStatus::worst([status, impact]);
                    let start = event
                        .get("epochStartDate")
                        .and_then(Value::as_i64)
                        .and_then(DateTime::from_timestamp_millis)
                        .ok_or_else(|| invalid("invalid Apple epochStartDate"))?;
                    batch.incidents.push(incident(
                        source,
                        &format!("{name}-{}-{index}", start.timestamp_millis()),
                        event.get("message").and_then(Value::as_str).unwrap_or(name),
                        start,
                        batch.observed_at,
                        impact,
                    ));
                }
                batch.components.push(ComponentObservation {
                    external_id: name.into(),
                    name: name.into(),
                    status,
                    updated_at: batch.observed_at,
                });
            }
            if batch.components.is_empty() {
                return Err(invalid("no selected Apple services found"));
            }
        }
        CollectorConfig::Salesforce { .. } => {
            for item in payload
                .as_array()
                .ok_or_else(|| invalid("expected Salesforce instance array"))?
            {
                let name = text(item, "key")?;
                if !included(source, &[name])
                    || item.get("isActive").and_then(Value::as_bool) != Some(true)
                    || text(item, "environment")? != "production"
                {
                    continue;
                }
                let status = match text(item, "status")? {
                    "OK" => ServiceStatus::Operational,
                    "MAINTENANCE_CORE" | "MAINTENANCE_NONCORE" => ServiceStatus::Maintenance,
                    "MINOR" | "MINOR_CORE" | "MINOR_NONCORE" => ServiceStatus::Degraded,
                    "MAJOR" | "MAJOR_CORE" | "MAJOR_NONCORE" => ServiceStatus::MajorOutage,
                    _ => ServiceStatus::Unknown,
                };
                batch.components.push(ComponentObservation {
                    external_id: name.into(),
                    name: name.into(),
                    status,
                    updated_at: date(item, "updatedAt")?,
                });
            }
            if batch.components.is_empty() {
                return Err(invalid("no selected Salesforce instances found"));
            }
        }
        CollectorConfig::Meta { .. } => {
            let org = payload
                .as_array()
                .ok_or_else(|| invalid("expected Meta organisation array"))?
                .iter()
                .find(|o| o.get("id").and_then(Value::as_str) == Some("whatsapp-business-api"))
                .ok_or_else(|| invalid("WhatsApp organisation missing"))?;
            for service in array(org, "services")? {
                let name = text(service, "name")?;
                if !included(source, &[name]) {
                    continue;
                }
                let status = match text(service, "status")? {
                    "No known issues" | "Resolved" => ServiceStatus::Operational,
                    "Major disruptions" | "High disruptions" => ServiceStatus::MajorOutage,
                    "Medium disruptions" => ServiceStatus::PartialOutage,
                    "Some disruptions" | "Low disruptions" | "Recovering from disruptions" => {
                        ServiceStatus::Degraded
                    }
                    _ => ServiceStatus::Unknown,
                };
                batch.components.push(ComponentObservation {
                    external_id: service
                        .get("id")
                        .ok_or_else(|| invalid("Meta service ID missing"))?
                        .to_string(),
                    name: name.into(),
                    status,
                    updated_at: batch.observed_at,
                });
            }
            if batch.components.is_empty() {
                return Err(invalid("no selected WhatsApp services found"));
            }
        }
        CollectorConfig::Slack { .. } => {
            let status = match text(&payload, "status")? {
                "ok" => ServiceStatus::Operational,
                "active" => ServiceStatus::Degraded,
                _ => ServiceStatus::Unknown,
            };
            batch.components.push(ComponentObservation {
                external_id: "slack".into(),
                name: "Slack".into(),
                status,
                updated_at: date(&payload, "date_updated")?,
            });
            for item in array(&payload, "active_incidents")? {
                let id = item
                    .get("id")
                    .ok_or_else(|| invalid("Slack incident ID missing"))?
                    .to_string();
                let impact = if text(item, "type")? == "outage" {
                    ServiceStatus::MajorOutage
                } else {
                    ServiceStatus::Degraded
                };
                let mut entry = incident(
                    source,
                    &id,
                    text(item, "title")?,
                    date(item, "date_created")?,
                    date(item, "date_updated")?,
                    impact,
                );
                entry.source_url = Some(text(item, "url")?.into());
                match text(item, "status")? {
                    "resolved" | "completed" | "cancelled" => {
                        entry.lifecycle = IncidentLifecycle::Resolved;
                        entry.resolved_at = Some(entry.updated_at);
                    }
                    "scheduled" => {
                        entry.lifecycle = IncidentLifecycle::Scheduled;
                        entry.impact = ServiceStatus::Maintenance;
                    }
                    _ => {}
                }
                batch.incidents.push(entry);
            }
        }
        _ => return Err(CollectorError::Unsupported(source.id.clone())),
    }
    finish(&mut batch);
    Ok(batch)
}

#[derive(Deserialize)]
struct ManualStatus {
    status: ServiceStatus,
    checked_at: DateTime<Utc>,
    note: String,
}
fn parse_manual(source: &SourceConfig, body: &[u8]) -> Result<ObservationBatch, CollectorError> {
    let records: std::collections::HashMap<String, ManualStatus> = serde_json::from_slice(body)?;
    let record = records
        .get(&source.id)
        .ok_or_else(|| invalid("awaiting manual status report"))?;
    if record.checked_at > Utc::now() {
        return Err(invalid("manual report timestamp is in the future"));
    }
    if record.note.trim().is_empty() {
        return Err(invalid("manual report requires a note"));
    }
    let mut batch = empty(source);
    // Age the report itself, not the time at which the file was read.
    batch.observed_at = record.checked_at;
    batch.status = record.status;
    if !matches!(
        record.status,
        ServiceStatus::Operational | ServiceStatus::Unknown
    ) {
        batch.incidents.push(incident(
            source,
            "manual",
            &record.note,
            record.checked_at,
            record.checked_at,
            record.status,
        ));
    }
    Ok(batch)
}

fn parse_rss(source: &SourceConfig, body: &[u8]) -> Result<ObservationBatch, CollectorError> {
    let xml = std::str::from_utf8(body).map_err(|_| invalid("invalid RSS encoding"))?;
    let doc = roxmltree::Document::parse(xml).map_err(|_| invalid("invalid RSS document"))?;
    if !doc.root_element().has_tag_name("rss") {
        return Err(invalid("expected RSS feed"));
    }
    let channel = doc
        .root_element()
        .children()
        .find(|n| n.has_tag_name("channel"))
        .ok_or_else(|| invalid("RSS channel missing"))?;
    let mut batch = empty(source);
    for item in channel.children().filter(|n| n.has_tag_name("item")) {
        let field = |key| {
            item.children()
                .find(|n| n.has_tag_name(key))
                .and_then(|n| n.text())
        };
        let title = field("title").ok_or_else(|| invalid("RSS item title missing"))?;
        // RSS has no standard lifecycle. Preserve advisories as unknown until their provider explicitly marks them resolved.
        let lower = title.to_ascii_lowercase();
        if lower.contains("[resolved]")
            || lower.starts_with("resolved:")
            || lower.starts_with("resolved -")
            || lower.starts_with("korjattu:")
        {
            continue;
        }
        let start = field("pubDate")
            .and_then(|s| DateTime::parse_from_rfc2822(s).ok())
            .map(|d| d.with_timezone(&Utc))
            .ok_or_else(|| invalid("RSS item pubDate invalid"))?;
        let id = field("guid")
            .or_else(|| field("link"))
            .ok_or_else(|| invalid("RSS item ID missing"))?;
        let mut entry = incident(source, id, title, start, start, ServiceStatus::Unknown);
        entry.source_url = field("link").map(str::to_owned);
        entry.lifecycle = IncidentLifecycle::Unknown;
        batch.incidents.push(entry);
    }
    finish(&mut batch);
    Ok(batch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppConfig, PollDefaults};
    use crate::storage::{InMemoryStore, StatusStore};
    fn source(kind: &str) -> SourceConfig {
        serde_yaml::from_str(&format!("id: example\nname: Example\ncategory: test\ncollector:\n  type: {kind}\n  url: https://example.com/incidents.json\n  path: /tmp/status.json\n")).unwrap()
    }
    #[test]
    fn google_filters_products_and_removes_resolved_incidents() {
        let mut source = source("google");
        source.include_components = vec!["Cloud Messaging".into()];
        let item = serde_json::json!({"id":"1","begin":"2026-11-01T10:00:00Z","modified":"2026-11-01T11:00:00Z","end":null,
            "external_desc":"Push unavailable","status_impact":"SERVICE_OUTAGE","affected_products":[{"title":"Cloud Messaging"}],
            "uri":"incidents/1","updates":[{"when":"2026-11-01T10:00:00Z","text":"Investigating"}]});
        let batch = parse(&source, &serde_json::to_vec(&vec![&item]).unwrap()).unwrap();
        assert_eq!(batch.status, ServiceStatus::MajorOutage);
        assert_eq!(batch.incidents.len(), 1);
        assert_eq!(
            batch.incidents[0].source_url.as_deref(),
            Some("https://example.com/incidents/1")
        );
        let mut resolved = item.clone();
        resolved["end"] = "2026-11-01T12:00:00Z".into();
        assert_eq!(
            parse(&source, &serde_json::to_vec(&vec![resolved]).unwrap())
                .unwrap()
                .status,
            ServiceStatus::Operational
        );
        let mut unrelated = item.clone();
        unrelated["affected_products"][0]["title"] = "Hosting".into();
        assert!(
            parse(&source, &serde_json::to_vec(&vec![unrelated]).unwrap())
                .unwrap()
                .incidents
                .is_empty()
        );
        let mut malformed = item.clone();
        malformed.as_object_mut().unwrap().remove("end");
        assert!(parse(&source, &serde_json::to_vec(&vec![malformed]).unwrap()).is_err());
    }
    #[test]
    fn apple_reports_only_ongoing_apns_events() {
        let mut source = source("apple");
        source.include_components = vec!["APNS".into()];
        let body=br#"jsonCallback({"services":[{"serviceName":"APNS","events":[{"eventStatus":"ongoing","statusType":"Outage","epochStartDate":1794470400000,"message":"Push unavailable"},{"eventStatus":"resolved","statusType":"Issue"}]},{"serviceName":"Xcode","events":[]}]});"#;
        let batch = parse(&source, body).unwrap();
        assert_eq!(batch.status, ServiceStatus::MajorOutage);
        assert_eq!(batch.incidents.len(), 1);
        assert_eq!(batch.components.len(), 1);
        assert!(parse(&source, br#"{"services":[]}"#).is_err());
    }
    #[test]
    fn malformed_payloads_do_not_become_operational() {
        for kind in ["google", "apple", "salesforce", "slack", "meta", "feed"] {
            assert!(parse(&source(kind), b"{}").is_err(), "{kind}");
            assert!(
                parse(&source(kind), b"<html>Unavailable</html>").is_err(),
                "{kind}"
            );
        }
    }
    #[test]
    fn salesforce_uses_active_production_instances_and_unknown_statuses() {
        let mut source = source("salesforce");
        source.include_components = vec!["EU1".into()];
        let body=br#"[{"key":"EU1","isActive":true,"environment":"production","status":"MAJOR_CORE","updatedAt":"2026-11-01T10:00:00Z"},{"key":"NA1","isActive":true,"environment":"production","status":"OK","updatedAt":"2026-11-01T10:00:00Z"}]"#;
        let batch = parse(&source, body).unwrap();
        assert_eq!(batch.components.len(), 1);
        assert_eq!(batch.status, ServiceStatus::MajorOutage);
        source.include_components = vec!["missing".into()];
        assert!(parse(&source, body).is_err());
    }
    #[test]
    fn meta_is_scoped_to_whatsapp_and_preserves_unknown_states() {
        let body=br#"[{"id":"whatsapp-business-api","services":[{"id":1,"name":"Cloud API","status":"Low disruptions"}]}]"#;
        assert_eq!(
            parse(&source("meta"), body).unwrap().status,
            ServiceStatus::Degraded
        );
        assert!(parse(&source("meta"), b"[]").is_err());
    }
    #[test]
    fn slack_retains_active_outage_details() {
        let body=br#"{"status":"active","date_updated":"2026-11-01T10:00:00Z","active_incidents":[{"id":1,"title":"Messaging unavailable","type":"outage","status":"active","url":"https://slack-status.com/i/1","date_created":"2026-11-01T09:00:00Z","date_updated":"2026-11-01T10:00:00Z"}]}"#;
        let batch = parse(&source("slack"), body).unwrap();
        assert_eq!(batch.status, ServiceStatus::MajorOutage);
        assert_eq!(batch.incidents.len(), 1);
    }
    #[test]
    fn rss_does_not_infer_resolution_from_historical_advisories() {
        let source = source("feed");
        let batch=parse(&source,br#"<rss><channel><item><title>Service disruption</title><guid>1</guid><pubDate>Mon, 16 Nov 2026 08:20:00 +0000</pubDate></item></channel></rss>"#).unwrap();
        assert_eq!(batch.status, ServiceStatus::Unknown);
        assert_eq!(batch.incidents.len(), 1);
        assert_eq!(
            parse(&source, b"<rss><channel/></rss>").unwrap().status,
            ServiceStatus::Operational
        );
    }
    #[tokio::test]
    async fn rereading_manual_file_does_not_refresh_expired_reports() {
        let source = source("manual");
        let defaults = PollDefaults::default();
        let body=serde_json::to_vec(&serde_json::json!({"example":{"status":"operational","checked_at":Utc::now()-chrono::Duration::days(2),"note":"Vendor confirmed"}})).unwrap();
        let batch = parse_manual(&source, &body).unwrap();
        let store = InMemoryStore::new(std::slice::from_ref(&source), &defaults);
        store.apply(batch).await.unwrap();
        let provider = store.provider("example").await.unwrap().unwrap();
        assert!(provider.stale);
        assert_eq!(provider.status, ServiceStatus::Unknown);
        assert!(parse_manual(&source, b"{}").is_err());
    }
    #[test]
    fn graph_issues_preserve_unknown_and_remove_resolved() {
        let source = source("microsoft_graph");
        let issue = serde_json::json!({"id":"TM1","title":"Teams unavailable","service":"Microsoft Teams","status":"serviceInterruption",
            "isResolved":false,"startDateTime":"2026-11-01T10:00:00Z","lastModifiedDateTime":"2026-11-01T11:00:00Z"});
        let mut batch = empty(&source);
        append_graph_issues(
            &source,
            &serde_json::json!({"value":[issue.clone()]}),
            &mut batch,
        )
        .unwrap();
        finish(&mut batch);
        assert_eq!(batch.status, ServiceStatus::MajorOutage);
        let mut resolved = issue.clone();
        resolved["isResolved"] = true.into();
        let mut batch = empty(&source);
        append_graph_issues(
            &source,
            &serde_json::json!({"value":[resolved]}),
            &mut batch,
        )
        .unwrap();
        assert!(batch.incidents.is_empty());
        let mut unknown = issue.clone();
        unknown["status"] = "unknownFutureValue".into();
        append_graph_issues(&source, &serde_json::json!({"value":[unknown]}), &mut batch).unwrap();
        finish(&mut batch);
        assert_eq!(batch.status, ServiceStatus::Unknown);
        assert!(append_graph_issues(&source, &serde_json::json!({}), &mut batch).is_err());
    }

    #[test]
    fn shipped_configuration_is_valid() {
        let config = AppConfig::load("config/sources.yaml").unwrap();
        assert_eq!(config.sources.len(), 41);
        let compose = std::fs::read_to_string("deploy/portainer/combinator.yaml").unwrap();
        let _: serde_yaml::Value = serde_yaml::from_str(&compose).unwrap();
    }
    #[tokio::test]
    #[ignore = "explicit live provider verification; no database or credentials required"]
    async fn live_public_sources() {
        let config = AppConfig::load("config/sources.yaml").unwrap();
        let registry = crate::collectors::CollectorRegistry::new(&config.defaults).unwrap();
        let mut failures = vec![];
        let only = std::env::var("LIVE_SOURCE").ok();
        for source in config.sources.iter().filter(|s| {
            only.as_ref().is_none_or(|id| id == &s.id)
                && !matches!(
                    s.collector,
                    CollectorConfig::Manual { .. } | CollectorConfig::MicrosoftGraph
                )
        }) {
            match tokio::time::timeout(std::time::Duration::from_secs(30), registry.collect(source))
                .await
            {
                Ok(Ok(batch)) => println!(
                    "{}: {:?}, {} components, {} incidents",
                    source.id,
                    batch.status,
                    batch.components.len(),
                    batch.incidents.len()
                ),
                result => failures.push(format!("{}: {result:?}", source.id)),
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}
