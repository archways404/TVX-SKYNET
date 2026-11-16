//! Telia's public fixed-network map uses HTML fragments, not an RSS feed.
use super::*;
use chrono::{NaiveDateTime, TimeZone};
use scraper::{ElementRef, Html, Selector};

fn select(css: &str) -> Selector {
    Selector::parse(css).expect("static Telia selector")
}
fn cell_text(node: ElementRef<'_>) -> String {
    node.children()
        .filter_map(|child| match child.value() {
            scraper::Node::Text(text) => Some(text.to_string()),
            _ => None,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn document(body: &[u8]) -> Result<Html, CollectorError> {
    Ok(Html::parse_document(
        std::str::from_utf8(body).map_err(|_| invalid("invalid Telia encoding"))?,
    ))
}
pub(super) fn local_date(value: &str) -> Result<DateTime<Utc>, CollectorError> {
    let date = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M"))
        .map_err(|_| invalid("invalid Telia local timestamp"))?;
    chrono_tz::Europe::Stockholm
        .from_local_datetime(&date)
        .single()
        .map(|date| date.with_timezone(&Utc))
        .ok_or_else(|| invalid("ambiguous Telia local timestamp"))
}

pub(super) async fn collect(
    collector: &CustomCollector,
    source: &SourceConfig,
    url: &Url,
) -> Result<ObservationBatch, CollectorError> {
    let mut batch = empty(source);
    // These are the service filters offered by Telia's public fixed-network map.
    // Fetch each affected county so map viewport/zoom never limits incident coverage.
    for (technology, service) in [
        ("BROADBAND", "Broadband"),
        ("IPTEL", "IP telephony"),
        ("IPTV", "TV"),
    ] {
        let mut endpoint = url.clone();
        endpoint.set_query(Some("affectedCounties"));
        endpoint
            .query_pairs_mut()
            .append_pair("typeTech", technology)
            .append_pair("type", "ALL VALID");
        let body = collector.body(collector.client.get(endpoint)).await?;
        let counties = counties(&body)?;
        if counties.is_empty() {
            batch.components.push(ComponentObservation {
                external_id: technology.into(),
                name: format!("Sweden / {service}"),
                status: ServiceStatus::Operational,
                updated_at: batch.observed_at,
            });
        }
        if counties.len() > 25 {
            return Err(invalid("unexpected Telia county count"));
        }
        for county in counties {
            let mut endpoint = url.clone();
            endpoint.set_query(Some("disturbances"));
            endpoint
                .query_pairs_mut()
                .append_pair("typeTech", technology)
                .append_pair("type", "ALL VALID")
                .append_pair("squareSize", "1000");
            for key in ["xmin", "ymin", "xmax", "ymax"] {
                let coordinate = county
                    .get(key)
                    .and_then(Value::as_f64)
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| invalid("Telia county bounds missing"))?;
                endpoint
                    .query_pairs_mut()
                    .append_pair(key, &coordinate.to_string());
            }
            let body = collector.body(collector.client.get(endpoint)).await?;
            append_cases(source, &body, text(&county, "name")?, service, &mut batch)?;
        }
    }
    finish(&mut batch);
    Ok(batch)
}

fn counties(body: &[u8]) -> Result<Vec<Value>, CollectorError> {
    let doc = document(body)?;
    let table = doc
        .select(&select("#affectedCountiesListTable table"))
        .next()
        .ok_or_else(|| invalid("Telia county table missing"))?;
    table
        .select(&select("tbody tr .countiesTitle input"))
        .map(|node| {
            serde_json::from_str(
                node.value()
                    .attr("value")
                    .ok_or_else(|| invalid("Telia county metadata missing"))?,
            )
            .map_err(CollectorError::from)
        })
        .collect()
}

fn append_cases(
    source: &SourceConfig,
    body: &[u8],
    county: &str,
    service: &str,
    batch: &mut ObservationBatch,
) -> Result<(), CollectorError> {
    let doc = document(body)?;
    let table = doc
        .select(&select("#disturbanceListTable table"))
        .next()
        .ok_or_else(|| invalid("Telia case table missing"))?;
    let mut count = 0;
    for row in table.select(&select("tbody tr")) {
        let cells: Vec<_> = row.select(&select("td")).collect();
        if cells.len() != 7 {
            return Err(invalid("Telia case columns changed"));
        }
        let id = cell_text(cells[1]);
        if !["BB-", "BT-", "TV-"]
            .iter()
            .any(|prefix| id.starts_with(prefix))
        {
            return Err(invalid("Telia case identifier missing"));
        }
        let description = cell_text(cells[2]);
        let start = local_date(&cell_text(cells[3]))?;
        let estimate = cell_text(cells[4]);
        let affected_service = cell_text(cells[5]);
        let customers: u64 = cell_text(cells[6])
            .parse()
            .map_err(|_| invalid("Telia customer count missing"))?;
        let geometry: Value = serde_json::from_str(
            cells[0]
                .select(&select("input[name=disturbancePolygons]"))
                .next()
                .and_then(|node| node.value().attr("value"))
                .ok_or_else(|| invalid("Telia case properties missing"))?,
        )?;
        let features = array(&geometry, "features")?;
        if features.is_empty() {
            return Err(invalid("Telia case properties empty"));
        }
        let mut planned = true;
        for feature in features {
            planned &= feature
                .pointer("/properties/isPlanned")
                .and_then(Value::as_bool)
                .ok_or_else(|| invalid("Telia planned flag missing"))?;
        }
        let impact = if planned {
            ServiceStatus::Maintenance
        } else {
            ServiceStatus::PartialOutage
        };
        let component_id = format!("{county}/{service}");
        if let Some(component) = batch
            .components
            .iter_mut()
            .find(|c| c.external_id == component_id)
        {
            component.status = ServiceStatus::worst([component.status, impact]);
        } else {
            batch.components.push(ComponentObservation {
                external_id: component_id.clone(),
                name: format!("{county} / {service}"),
                status: impact,
                updated_at: start,
            });
        }
        if let Some(entry) = batch
            .incidents
            .iter_mut()
            .find(|entry| entry.external_id == id)
        {
            if !entry.affected_component_ids.contains(&component_id) {
                entry.affected_component_ids.push(component_id);
            }
        } else {
            let mut entry = incident(
                source,
                &id,
                &format!(
                    "{id}: {description} — {affected_service} ({customers} affected customers)"
                ),
                start,
                start,
                impact,
            );
            entry.affected_component_ids.push(component_id);
            entry.lifecycle = if planned {
                IncidentLifecycle::InProgress
            } else {
                IncidentLifecycle::Investigating
            };
            entry.updates.push(IncidentUpdate { external_id: format!("{id}-details"), lifecycle: entry.lifecycle.clone(),
                message: format!("Telia fixed-network report. Estimated repair: {estimate} (Europe/Stockholm). {customers} affected customers. Geographic association follows the provider's county map viewport."), published_at: start });
            batch.incidents.push(entry);
        }
        count += 1;
    }
    if count == 0 {
        return Err(invalid(
            "Telia lists an affected county without case details",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn telia_reads_cases_and_preserves_scope_without_duplicate_incidents() {
        let config = crate::config::AppConfig::load("config/sources.yaml").unwrap();
        let source = config.sources.iter().find(|s| s.id == "telia").unwrap();
        let mut batch = empty(source);
        let html = br#"<div id="disturbanceListTable"><table><tbody><tr><td><input name="disturbancePolygons" value='{"features":[{"properties":{"isPlanned":false}}]}'></td><td><b>Case</b> BB-347231</td><td><b>Description</b> Utrustningsfel</td><td>2026-11-16 08:55</td><td>2026-11-16 15:55</td><td>Fast telekommunikation</td><td>6</td></tr></tbody></table></div>"#;
        append_cases(source, html, "Skane", "Broadband", &mut batch).unwrap();
        append_cases(source, html, "Skane", "IP telephony", &mut batch).unwrap();
        finish(&mut batch);
        assert_eq!(batch.status, ServiceStatus::PartialOutage);
        assert_eq!(batch.incidents.len(), 1);
        assert_eq!(batch.incidents[0].affected_component_ids.len(), 2);
        assert!(batch.incidents[0].title.contains("6 affected customers"));
        assert_eq!(
            batch.incidents[0].started_at.to_rfc3339(),
            "2026-11-16T07:55:00+00:00"
        );
        assert_eq!(
            local_date("2026-07-16 08:55").unwrap().to_rfc3339(),
            "2026-07-16T06:55:00+00:00"
        );
        assert!(
            append_cases(
                source,
                b"<title>Error</title>",
                "Skane",
                "Broadband",
                &mut batch
            )
            .is_err()
        );
        assert!(counties(b"<title>Error</title>").is_err());
    }
}
