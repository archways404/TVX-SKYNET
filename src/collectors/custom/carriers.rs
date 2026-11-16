//! Official public incident data, scoped to the services the provider exposes.
use super::*;
use scraper::{ElementRef, Html, Selector};

pub(super) fn parse(
    source: &SourceConfig,
    body: &[u8],
) -> Result<ObservationBatch, CollectorError> {
    if matches!(source.collector, CollectorConfig::Norlys { .. }) {
        return norlys(source, body);
    }
    let payload: Value = serde_json::from_slice(body)?;
    let mut batch = empty(source);
    match source.collector {
        CollectorConfig::Tdc { .. } => tdc(source, &payload, &mut batch)?,
        CollectorConfig::StatusIo { .. } => status_io(source, &payload, &mut batch)?,
        _ => return Err(invalid("unsupported public carrier format")),
    }
    finish(&mut batch);
    Ok(batch)
}

fn tdc(
    source: &SourceConfig,
    payload: &Value,
    batch: &mut ObservationBatch,
) -> Result<(), CollectorError> {
    let items = payload
        .as_array()
        .ok_or_else(|| invalid("TDC incident array missing"))?;
    for item in items {
        let service = text(item, "function")?.trim();
        if !included(source, &[service]) {
            continue;
        }
        if !item
            .get("realResolved")
            .ok_or_else(|| invalid("TDC resolution field missing"))?
            .is_null()
        {
            continue;
        }
        let planned = item
            .get("plannedStart")
            .ok_or_else(|| invalid("TDC planning field missing"))?;
        let start = if planned.is_null() {
            date(item, "occured")?
        } else {
            date(item, "plannedStart")?
        };
        if start > batch.observed_at {
            continue;
        }
        // Planned windows are active only while their explicitly published window is open.
        if !planned.is_null() && date(item, "plannedEnd")? <= batch.observed_at {
            continue;
        }
        let impact = if !planned.is_null() {
            ServiceStatus::Maintenance
        } else {
            match text(item, "level")? {
                "Low" => ServiceStatus::Degraded,
                "Medium" => ServiceStatus::PartialOutage,
                "High" | "Critical" => ServiceStatus::MajorOutage,
                _ => ServiceStatus::Unknown,
            }
        };
        let id = text(item, "fasId")?;
        let component_id = format!("case:{id}");
        let updated = date(item, "updated")?;
        let mut entry = incident(
            source,
            id,
            &format!(
                "{} — {}",
                text(item, "reason")?.trim(),
                text(item, "consequence")?
                    .split("\n\n")
                    .next()
                    .unwrap_or_default()
                    .trim()
            ),
            start,
            updated,
            impact,
        );
        entry.affected_component_ids.push(component_id.clone());
        entry.network_details = NetworkIncidentDetails {
            location: Some(format!(
                "{}, {}",
                text(item, "title")?.trim(),
                text(item, "geography")?
            )),
            services: vec![service.into()],
            estimated_resolution_at: date(item, "estimateResolved").ok(),
            ..Default::default()
        };
        if !planned.is_null() {
            entry.lifecycle = IncidentLifecycle::InProgress;
        }
        entry.updates.push(IncidentUpdate {
            external_id: format!("{id}-{updated}"),
            lifecycle: entry.lifecycle.clone(),
            message: format!("{}\n{}", text(item, "reason")?, text(item, "consequence")?),
            published_at: updated,
        });
        batch.incidents.push(entry);
        if let Some(component) = batch
            .components
            .iter_mut()
            .find(|c| c.external_id == component_id)
        {
            component.status = ServiceStatus::worst([component.status, impact]);
            component.updated_at = component.updated_at.max(updated);
        } else {
            batch.components.push(ComponentObservation {
                external_id: component_id,
                name: format!(
                    "{}, {} / {service}",
                    text(item, "title")?.trim(),
                    text(item, "geography")?
                ),
                status: impact,
                updated_at: updated,
            });
        }
    }
    if batch.components.is_empty() {
        batch.components.push(ComponentObservation {
            external_id: "public-network".into(),
            name: "Published network incident scope".into(),
            status: ServiceStatus::Operational,
            updated_at: batch.observed_at,
        });
    }
    Ok(())
}

fn code(value: &Value) -> Result<ServiceStatus, CollectorError> {
    Ok(
        match value
            .as_u64()
            .ok_or_else(|| invalid("Status.io status code missing"))?
        {
            100 => ServiceStatus::Operational,
            200 => ServiceStatus::Maintenance,
            300 => ServiceStatus::Degraded,
            400 => ServiceStatus::PartialOutage,
            500 => ServiceStatus::MajorOutage,
            _ => ServiceStatus::Unknown,
        },
    )
}
fn status_io(
    source: &SourceConfig,
    payload: &Value,
    batch: &mut ObservationBatch,
) -> Result<(), CollectorError> {
    let result = payload
        .get("result")
        .ok_or_else(|| invalid("Status.io result missing"))?;
    let mut selected_groups = Vec::new();
    for group in array(result, "status")? {
        if !included(source, &[text(group, "name")?]) {
            continue;
        }
        selected_groups.push(text(group, "id")?.to_owned());
        for component in array(group, "containers")? {
            batch.components.push(ComponentObservation {
                external_id: text(component, "id")?.into(),
                name: format!("{} / {}", text(group, "name")?, text(component, "name")?),
                status: code(
                    component
                        .get("status_code")
                        .ok_or_else(|| invalid("Status.io component status missing"))?,
                )?,
                updated_at: date(component, "updated")?,
            });
        }
    }
    if batch.components.is_empty() {
        return Err(invalid("Status.io selected region/components missing"));
    }
    let maintenance = result
        .get("maintenance")
        .ok_or_else(|| invalid("Status.io maintenance missing"))?;
    for (items, is_maintenance) in [
        (array(result, "incidents")?, false),
        (array(maintenance, "active")?, true),
    ] {
        for item in items {
            let groups = array(item, "components_affected")?;
            if !groups.iter().any(|g| {
                g.get("_id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| selected_groups.iter().any(|selected| selected == id))
            }) {
                continue;
            }
            let messages = array(item, "messages")?;
            let latest = messages
                .iter()
                .max_by_key(|m| m.get("datetime").and_then(Value::as_str))
                .ok_or_else(|| invalid("Status.io incident messages missing"))?;
            let state = latest
                .get("state")
                .and_then(Value::as_u64)
                .ok_or_else(|| invalid("Status.io incident lifecycle missing"))?;
            if (!is_maintenance && state == 400) || (is_maintenance && state == 300) {
                continue;
            }
            let mut entry = incident(
                source,
                text(item, "_id")?,
                text(item, "name")?,
                date(item, "datetime_open")?,
                date(latest, "datetime")?,
                if is_maintenance {
                    ServiceStatus::Maintenance
                } else {
                    code(
                        latest
                            .get("status")
                            .ok_or_else(|| invalid("Status.io impact missing"))?,
                    )?
                },
            );
            entry.lifecycle = if is_maintenance {
                IncidentLifecycle::InProgress
            } else {
                match state {
                    100 => IncidentLifecycle::Investigating,
                    200 => IncidentLifecycle::Identified,
                    300 => IncidentLifecycle::Monitoring,
                    _ => IncidentLifecycle::Unknown,
                }
            };
            entry.affected_component_ids = array(item, "containers_affected")?
                .iter()
                .filter_map(|c| c.get("_id").and_then(Value::as_str))
                .filter(|id| batch.components.iter().any(|c| c.external_id == *id))
                .map(str::to_owned)
                .collect();
            entry.source_url = Some(source.metadata.status_url.clone());
            for message in messages {
                let at = date(message, "datetime")?;
                entry.updates.push(IncidentUpdate {
                    external_id: at.to_rfc3339(),
                    lifecycle: entry.lifecycle.clone(),
                    message: text(message, "details")?.into(),
                    published_at: at,
                });
            }
            batch.incidents.push(entry);
        }
    }
    Ok(())
}

fn sel(css: &str) -> Selector {
    Selector::parse(css).expect("static Norlys selector")
}
fn content(node: ElementRef<'_>) -> String {
    node.text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn danish_date(text: &str, label: &str) -> Result<DateTime<Utc>, CollectorError> {
    let rest = text
        .split_once(label)
        .ok_or_else(|| invalid("Norlys date label missing"))?
        .1
        .trim();
    let raw = rest
        .get(..16)
        .ok_or_else(|| invalid("Norlys timestamp missing"))?;
    let date = chrono::NaiveDateTime::parse_from_str(raw, "%d-%m-%Y %H:%M")
        .map_err(|_| invalid("invalid Norlys timestamp"))?;
    use chrono::TimeZone;
    chrono_tz::Europe::Copenhagen
        .from_local_datetime(&date)
        .single()
        .map(|date| date.with_timezone(&Utc))
        .ok_or_else(|| invalid("ambiguous Norlys timestamp"))
}
fn norlys(source: &SourceConfig, body: &[u8]) -> Result<ObservationBatch, CollectorError> {
    let html = std::str::from_utf8(body).map_err(|_| invalid("invalid Norlys encoding"))?;
    let doc = Html::parse_document(html);
    let current = doc
        .select(&sel(".c-accordion__wrapper"))
        .find(|node| {
            node.select(&sel("h5"))
                .next()
                .is_some_and(|heading| content(heading) == "Aktuelle fejl:")
        })
        .ok_or_else(|| invalid("Norlys current incidents section missing"))?;
    let body = current
        .select(&sel("div[itemprop=articleBody]"))
        .next()
        .ok_or_else(|| invalid("Norlys service sections missing"))?;
    let mut batch = empty(source);
    let mut service = String::new();
    let mut recognized = false;
    for node in body.children().filter_map(ElementRef::wrap) {
        if node.value().name() != "h2" && !included(source, &[&service]) {
            continue;
        }
        match node.value().name() {
            "h2" => {
                if content(node).starts_with("Planlagte") {
                    break;
                }
                if !service.is_empty() && !recognized {
                    return Err(invalid(&format!(
                        "Norlys service health missing for {service}"
                    )));
                }
                service = content(node);
                recognized = !included(source, &[&service]);
            }
            "p" if content(node) == "Der er ingen kendte driftsforstyrrelser." => {
                batch.components.push(ComponentObservation {
                    external_id: service.clone(),
                    name: service.clone(),
                    status: ServiceStatus::Operational,
                    updated_at: batch.observed_at,
                });
                recognized = true;
            }
            "details" => {
                let title = node
                    .select(&sel("summary[itemprop=name]"))
                    .next()
                    .map(content)
                    .ok_or_else(|| invalid("Norlys incident title missing"))?;
                let text = content(node);
                let created = danish_date(&text, "Oprettet:")?;
                let start = danish_date(&text, "Fejl opstået:").unwrap_or(created);
                let estimate = danish_date(&text, "Fejl forventes løst:").ok();
                let id = title
                    .rsplit_once("(#")
                    .and_then(|(_, id)| id.strip_suffix(')'))
                    .ok_or_else(|| invalid("Norlys incident id missing"))?;
                // Old bulletins can remain in the current section long after their repair
                // estimate. Keep them as unconfirmed information, not fresh outage evidence.
                let expired = estimate.is_none_or(|estimate| {
                    estimate + chrono::Duration::hours(24) < batch.observed_at
                });
                let impact = if expired {
                    ServiceStatus::Unknown
                } else {
                    ServiceStatus::PartialOutage
                };
                let mut entry = incident(source, id, &title, start, created, impact);
                entry.affected_component_ids.push(service.clone());
                entry.network_details = NetworkIncidentDetails {
                    services: vec![service.clone()],
                    estimated_resolution_at: estimate,
                    ..Default::default()
                };
                if expired {
                    entry.lifecycle = IncidentLifecycle::Unknown;
                }
                entry.updates.push(IncidentUpdate { external_id: format!("{id}-notice"), lifecycle: entry.lifecycle.clone(), message: format!("{text}\nProvider repair estimate: {estimate:?}. {}", if expired { "Estimate exceeded by more than 24 hours; current impact is unconfirmed." } else { "Published as a current issue by Norlys." }), published_at: created });
                batch.incidents.push(entry);
                if let Some(component) = batch
                    .components
                    .iter_mut()
                    .find(|c| c.external_id == service)
                {
                    component.status = ServiceStatus::worst([component.status, impact]);
                } else {
                    batch.components.push(ComponentObservation {
                        external_id: service.clone(),
                        name: service.clone(),
                        status: impact,
                        updated_at: created,
                    });
                }
                recognized = true;
            }
            _ => {}
        }
    }
    if service.is_empty() || !recognized || batch.components.is_empty() {
        return Err(invalid(&format!(
            "Norlys service health missing for {service}"
        )));
    }
    finish(&mut batch);
    Ok(batch)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source(kind: &str) -> SourceConfig {
        serde_yaml::from_str(&format!("id: test\nname: Test\ncategory: carriers\ncollector:\n  type: {kind}\n  url: https://status.example.com/\n")).unwrap()
    }
    #[test]
    fn tdc_uses_resolution_and_planned_windows_instead_of_estimated_repairs() {
        let mut item = serde_json::json!({"function":"Mobil Telefoni","realResolved":null,"plannedStart":null,"occured":"2026-01-01T00:00:00Z","updated":"2026-01-02T00:00:00Z","level":"Medium","fasId":"case-1","title":"Local calls failing","geography":"Sjælland","reason":"Power","consequence":"Calls affected"});
        let batch = parse(
            &source("tdc"),
            &serde_json::to_vec(&vec![item.clone()]).unwrap(),
        )
        .unwrap();
        assert_eq!(batch.status, ServiceStatus::PartialOutage);
        assert_eq!(
            batch.incidents[0].affected_component_ids,
            vec!["case:case-1"]
        );
        item["realResolved"] = "2026-01-03T00:00:00Z".into();
        assert!(
            parse(
                &source("tdc"),
                &serde_json::to_vec(&vec![item.clone()]).unwrap()
            )
            .unwrap()
            .incidents
            .is_empty()
        );
        item["realResolved"] = Value::Null;
        item["plannedStart"] = "2099-01-01T00:00:00Z".into();
        assert!(
            parse(&source("tdc"), &serde_json::to_vec(&vec![item]).unwrap())
                .unwrap()
                .incidents
                .is_empty()
        );
        assert!(parse(&source("tdc"), b"null").is_err());
    }
    #[test]
    fn status_io_filters_regions_and_rejects_missing_components() {
        let mut config = source("status_io");
        config.include_components = vec!["Norway".into()];
        let mut value = serde_json::json!({"result":{"status":[{"id":"no","name":"Norway","containers":[{"id":"calls","name":"Mobile Telephony","status_code":100,"updated":"2026-01-01T00:00:00Z"}]},{"id":"se","name":"Sweden","containers":[{"id":"se-calls","name":"Calls","status_code":500,"updated":"2026-01-01T00:00:00Z"}]}],"incidents":[],"maintenance":{"active":[]}}});
        assert_eq!(
            parse(&config, &serde_json::to_vec(&value).unwrap())
                .unwrap()
                .status,
            ServiceStatus::Operational
        );
        let mut issue = serde_json::json!({"_id":"no-incident","name":"Norway calls failing","datetime_open":"2026-01-01T00:00:00Z","messages":[{"datetime":"2026-01-01T01:00:00Z","status":400,"state":300,"details":"Recovery being monitored"}],"components_affected":[{"_id":"no"}],"containers_affected":[{"_id":"calls"}]});
        let mut foreign = issue.clone();
        foreign["_id"] = "se-incident".into();
        foreign["components_affected"] = serde_json::json!([{"_id":"se"}]);
        value["result"]["incidents"] = serde_json::json!([issue.clone(), foreign]);
        let batch = parse(&config, &serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(batch.incidents.len(), 1);
        assert_eq!(batch.status, ServiceStatus::PartialOutage);
        assert_eq!(batch.incidents[0].lifecycle, IncidentLifecycle::Monitoring);
        assert_eq!(batch.incidents[0].affected_component_ids, vec!["calls"]);
        issue["messages"][0]["state"] = 400.into();
        value["result"]["incidents"] = serde_json::json!([issue]);
        assert!(
            parse(&config, &serde_json::to_vec(&value).unwrap())
                .unwrap()
                .incidents
                .is_empty()
        );
        assert!(parse(&config, br#"{"result":{"status":[]}}"#).is_err());
    }
    #[test]
    fn norlys_keeps_expired_bulletins_unknown_and_stops_before_planned_work() {
        let html = "<div class='c-accordion__wrapper'><h5>Aktuelle fejl:</h5><div itemprop='articleBody'><h2>DataNet</h2><details><summary itemprop='name'>Old local work (#100)</summary><p>Oprettet: 01-01-2000 12:00 Fejl opstået: 01-01-2000 13:00 Fejl forventes løst: 01-01-2000 14:00</p></details><h2>Mobile</h2><p>Der er ingen kendte driftsforstyrrelser.</p><h2>Planlagte ændringer:</h2><h2>DataNet</h2><details><summary>Future work with different markup</summary></details></div></div>";
        let batch = parse(&source("norlys"), html.as_bytes()).unwrap();
        assert_eq!(batch.incidents.len(), 1);
        assert_eq!(batch.incidents[0].impact, ServiceStatus::Unknown);
        assert_eq!(
            batch
                .components
                .iter()
                .find(|c| c.name == "Mobile")
                .unwrap()
                .status,
            ServiceStatus::Operational
        );
    }

    #[test]
    fn norlys_requires_explicit_current_service_health() {
        let html = br#"<div class="c-accordion__wrapper"><h5>Aktuelle fejl:</h5><div itemprop="articleBody"><h2>Mobile</h2><p>Der er ingen kendte driftsforstyrrelser.</p></div></div>"#;
        assert_eq!(
            parse(&source("norlys"), html).unwrap().status,
            ServiceStatus::Operational
        );
        assert!(parse(&source("norlys"), b"<title>Provider homepage</title>").is_err());
    }
}
