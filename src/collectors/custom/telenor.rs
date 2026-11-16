//! Public anonymous portal: use its request token and AJAX headers, no customer login.
use super::*;
use scraper::{Html, Selector};

pub(super) async fn collect(
    collector: &CustomCollector,
    source: &SourceConfig,
    url: &Url,
) -> Result<ObservationBatch, CollectorError> {
    let body = collector.body(collector.client.get(url.clone())).await?;
    let token = {
        let html =
            std::str::from_utf8(&body).map_err(|_| invalid("invalid Telenor portal encoding"))?;
        let doc = Html::parse_document(html);
        doc.select(&Selector::parse("input#csrft").unwrap())
            .next()
            .and_then(|node| node.value().attr("value"))
            .filter(|s| !s.is_empty())
            .ok_or_else(|| invalid("Telenor public request token missing"))?
            .to_owned()
    };
    let mut batch = empty(source);
    // The public portal labels FIXED_20 as broadband, FIXED_22 as telephony,
    // and FIXED_23 as TV. Request the first two separately to preserve scope.
    for (technology, service) in [("FIXED_20", "Broadband"), ("FIXED_22", "Telephony")] {
        let endpoint = |path: &str| -> Result<Url, CollectorError> {
            let mut endpoint = url
                .join(path)
                .map_err(|_| invalid("invalid Telenor endpoint"))?;
            endpoint.set_query(Some(&format!("rt={token}")));
            endpoint
                .query_pairs_mut()
                .append_pair("services", technology)
                .append_pair("faultStatuses", "ACTIVE");
            Ok(endpoint)
        };
        let request = |endpoint: Url| {
            collector
                .client
                .get(endpoint)
                .header("X-Requested-With", "XMLHttpRequest")
                .header("Referer", url.as_str())
        };
        let body = collector
            .body(request(endpoint("Fault/AdminAreaFixedList")?))
            .await?;
        let counties: Value = serde_json::from_slice(&body)?;
        let counties = counties
            .as_array()
            .ok_or_else(|| invalid("Telenor county list missing"))?;
        if counties.len() > 25 {
            return Err(invalid("unexpected Telenor county count"));
        }
        for county in counties {
            let name = text(county, "Name")?;
            let mut url = endpoint("Fault/AreaFixedTicketList")?;
            for (param, pointer) in [
                ("llx", "/BBox/LL/Easting"),
                ("lly", "/BBox/LL/Northing"),
                ("urx", "/BBox/UR/Easting"),
                ("ury", "/BBox/UR/Northing"),
            ] {
                let value = county
                    .pointer(pointer)
                    .and_then(Value::as_f64)
                    .filter(|n| n.is_finite())
                    .ok_or_else(|| invalid("Telenor county bounds missing"))?;
                url.query_pairs_mut().append_pair(param, &value.to_string());
            }
            let body = collector.body(request(url)).await?;
            append_cases(source, &body, name, service, &mut batch)?;
        }
        if counties.is_empty() {
            batch.components.push(ComponentObservation {
                external_id: technology.into(),
                name: format!("Sweden / {service}"),
                status: ServiceStatus::Operational,
                updated_at: batch.observed_at,
            });
        }
    }
    finish(&mut batch);
    Ok(batch)
}

fn append_cases(
    source: &SourceConfig,
    body: &[u8],
    county: &str,
    service: &str,
    batch: &mut ObservationBatch,
) -> Result<(), CollectorError> {
    let payload: Value = serde_json::from_slice(body)?;
    let items = payload
        .as_array()
        .ok_or_else(|| invalid("Telenor case list missing"))?;
    let mut active = 0;
    for item in items {
        let historic = item
            .get("IsHistoric")
            .and_then(Value::as_bool)
            .ok_or_else(|| invalid("Telenor case lifecycle missing"))?;
        if historic {
            continue;
        }
        let id = text(item, "ExternalId")?;
        let started = telia::local_date(&text(item, "StartTime")?.replace('T', " "))?;
        let published = text(item, "Text")?;
        let title = if published.trim() == "Felsökning pågår" {
            "Network disruption — investigation in progress; cause not published"
        } else if published.trim().is_empty() {
            "Network disruption — cause not published"
        } else {
            published.trim()
        };
        let point = map_point(item);
        let location = point
            .map(|(lat, lon)| format!("{county} · approx. {lat:.4}, {lon:.4}"))
            .unwrap_or_else(|| format!("{county} · exact location not published"));
        let component_id = format!("{id}/{county}/{service}");
        batch.components.push(ComponentObservation {
            external_id: component_id.clone(),
            name: format!("{location} / {service}"),
            status: ServiceStatus::PartialOutage,
            updated_at: started,
        });
        if let Some(entry) = batch.incidents.iter_mut().find(|i| i.external_id == id) {
            if !entry.affected_component_ids.contains(&component_id) {
                entry.affected_component_ids.push(component_id.clone());
            }
            if !entry.network_details.services.iter().any(|s| s == service) {
                entry.network_details.services.push(service.into());
            }
        } else {
            let mut entry = incident(
                source,
                id,
                title,
                started,
                started,
                ServiceStatus::PartialOutage,
            );
            entry.affected_component_ids.push(component_id.clone());
            if let Some((lat, lon)) = point {
                entry.source_url = Some(format!(
                    "https://www.openstreetmap.org/?mlat={lat:.6}&mlon={lon:.6}#map=12/{lat:.6}/{lon:.6}"
                ));
            }
            let estimate = text(item, "CloseTime")?;
            entry.network_details = NetworkIncidentDetails {
                location: Some(location.clone()),
                services: vec![service.into()],
                locations: point
                    .map(|(latitude, longitude)| NetworkLocation {
                        name: county.into(),
                        latitude,
                        longitude,
                        precision: "area_center".into(),
                    })
                    .into_iter()
                    .collect(),
                estimated_resolution_at: telia::local_date(&estimate.replace('T', " ")).ok(),
                affected_customers: None,
            };
            entry.updates.push(IncidentUpdate { external_id: format!("{id}-details"), lifecycle: entry.lifecycle.clone(), message: format!("Telenor current fixed-network fault. Estimated repair: {estimate} (Europe/Stockholm). County association follows the public map viewport."), published_at: started });
            batch.incidents.push(entry);
        }
        active += 1;
    }
    if active == 0 {
        return Err(invalid(
            "Telenor lists affected county without current cases",
        ));
    }
    Ok(())
}

// The map publishes bounding boxes, not customer addresses. Link to their centre
// as an approximate affected area without inferring a town from a county viewport.
fn map_point(item: &Value) -> Option<(f64, f64)> {
    let read = |path| {
        item.pointer(path)
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite())
    };
    let south = read("/BBox/LL/Northing")?;
    let north = read("/BBox/UR/Northing")?;
    let west = read("/BBox/LL/Easting")?;
    let east = read("/BBox/UR/Easting")?;
    if south > north
        || west > east
        || south < -90.0
        || north > 90.0
        || west < -180.0
        || east > 180.0
    {
        return None;
    }
    Some(((south + north) / 2.0, (west + east) / 2.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn telenor_presents_the_provider_area_without_inventing_a_cause() {
        let config = crate::config::AppConfig::load("config/sources.yaml").unwrap();
        let source = config.sources.iter().find(|s| s.id == "telenor").unwrap();
        let mut batch = empty(source);
        let body = serde_json::json!([{"ExternalId":"1","IsHistoric":false,"StartTime":"2026-11-16T08:11:33","CloseTime":"2026-11-16T13:00:00","Text":"Felsökning pågår","BBox":{"LL":{"Easting":15.2,"Northing":59.2},"UR":{"Easting":15.3,"Northing":59.3}}}]);
        append_cases(
            source,
            &serde_json::to_vec(&body).unwrap(),
            "Orebro",
            "Telephony",
            &mut batch,
        )
        .unwrap();
        assert!(batch.incidents[0].title.contains("cause not published"));
        assert!(
            batch.components[0]
                .name
                .contains("approx. 59.2500, 15.2500 / Telephony")
        );
        assert!(
            batch.incidents[0]
                .source_url
                .as_ref()
                .unwrap()
                .contains("mlat=59.250000&mlon=15.250000")
        );
    }

    #[test]
    fn telenor_keeps_cases_and_rejects_invalid_endpoint_responses() {
        let config = crate::config::AppConfig::load("config/sources.yaml").unwrap();
        let source = config.sources.iter().find(|s| s.id == "telenor").unwrap();
        let mut batch = empty(source);
        let body = br#"[{"ExternalId":"36359253","IsHistoric":false,"StartTime":"2026-10-28T06:14:11","CloseTime":"2026-11-16T12:00:00","Text":"Investigation ongoing"}]"#;
        append_cases(source, body, "Stockholm", "Broadband", &mut batch).unwrap();
        append_cases(source, body, "Uppsala", "Broadband", &mut batch).unwrap();
        assert_eq!(batch.incidents.len(), 1);
        assert_eq!(batch.incidents[0].affected_component_ids.len(), 2);
        assert_eq!(batch.incidents[0].impact, ServiceStatus::PartialOutage);
        assert!(append_cases(source, b"null", "Stockholm", "Broadband", &mut batch).is_err());
    }
}
