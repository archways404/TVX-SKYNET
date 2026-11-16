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
    let endpoint = |path: &str| -> Result<Url, CollectorError> {
        let mut endpoint = url
            .join(path)
            .map_err(|_| invalid("invalid Telenor endpoint"))?;
        endpoint.set_query(Some(&format!("rt={token}")));
        endpoint
            .query_pairs_mut()
            .append_pair("services", "FIXED_20,FIXED_22,FIXED_23")
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
    let mut batch = empty(source);
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
        append_cases(source, &body, name, &mut batch)?;
    }
    if counties.is_empty() {
        batch.components.push(ComponentObservation {
            external_id: "fixed".into(),
            name: "Sweden / Fixed network".into(),
            status: ServiceStatus::Operational,
            updated_at: batch.observed_at,
        });
    }
    finish(&mut batch);
    Ok(batch)
}

fn append_cases(
    source: &SourceConfig,
    body: &[u8],
    county: &str,
    batch: &mut ObservationBatch,
) -> Result<(), CollectorError> {
    let payload: Value = serde_json::from_slice(body)?;
    let items = payload
        .as_array()
        .ok_or_else(|| invalid("Telenor case list missing"))?;
    let component_id = format!("fixed/{county}");
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
        let title = text(item, "Text")?;
        if let Some(entry) = batch.incidents.iter_mut().find(|i| i.external_id == id) {
            if !entry.affected_component_ids.contains(&component_id) {
                entry.affected_component_ids.push(component_id.clone());
            }
        } else {
            let mut entry = incident(
                source,
                id,
                &format!("{id}: {title} — fixed-network fault"),
                started,
                started,
                ServiceStatus::PartialOutage,
            );
            entry.affected_component_ids.push(component_id.clone());
            let estimate = text(item, "CloseTime")?;
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
    batch.components.push(ComponentObservation {
        external_id: component_id,
        name: format!("{county} / Fixed network"),
        status: ServiceStatus::PartialOutage,
        updated_at: batch.observed_at,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn telenor_keeps_cases_and_rejects_invalid_endpoint_responses() {
        let config = crate::config::AppConfig::load("config/sources.yaml").unwrap();
        let source = config.sources.iter().find(|s| s.id == "telenor").unwrap();
        let mut batch = empty(source);
        let body = br#"[{"ExternalId":"36359253","IsHistoric":false,"StartTime":"2026-10-28T06:14:11","CloseTime":"2026-11-16T12:00:00","Text":"Investigation ongoing"}]"#;
        append_cases(source, body, "Stockholm", &mut batch).unwrap();
        append_cases(source, body, "Uppsala", &mut batch).unwrap();
        assert_eq!(batch.incidents.len(), 1);
        assert_eq!(batch.incidents[0].affected_component_ids.len(), 2);
        assert_eq!(batch.incidents[0].impact, ServiceStatus::PartialOutage);
        assert!(append_cases(source, b"null", "Stockholm", &mut batch).is_err());
    }
}
