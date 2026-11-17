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
    for (technology, service) in [("BROADBAND", "Broadband"), ("IPTEL", "IP telephony")] {
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
        count += 1;
        if id.starts_with("TV-") || service == "TV" {
            continue;
        }
        let published_description = cell_text(cells[2]);
        let description = match published_description.as_str() {
            "Utrustningsfel" => "Equipment failure",
            "Kabelfel" => "Cable fault",
            "Strömavbrott" => "Power outage",
            "Okänd" => "Cause not published",
            other => other,
        };
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
        let mut areas = features
            .iter()
            .filter(|feature| {
                feature
                    .pointer("/properties/isRelevant")
                    .and_then(Value::as_bool)
                    != Some(false)
            })
            .filter_map(|feature| feature.pointer("/properties/name").and_then(Value::as_str))
            .collect::<Vec<_>>();
        areas.sort_unstable();
        areas.dedup();
        let location = if areas.is_empty() {
            county.to_owned()
        } else {
            format!("{county} · provider area {}", areas.join(", "))
        };
        let component_id = format!("{id}/{county}/{service}");
        if let Some(component) = batch
            .components
            .iter_mut()
            .find(|c| c.external_id == component_id)
        {
            component.status = ServiceStatus::worst([component.status, impact]);
        } else {
            batch.components.push(ComponentObservation {
                external_id: component_id.clone(),
                name: format!("{location} / {service}"),
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
            for point in map_locations(features, county)? {
                if !entry.network_details.locations.iter().any(|p| {
                    (p.latitude - point.latitude).abs() < 1e-7
                        && (p.longitude - point.longitude).abs() < 1e-7
                }) {
                    entry.network_details.locations.push(point);
                }
            }
        } else {
            let mut entry = incident(
                source,
                &id,
                &format!("{description} — {customers} affected customers"),
                start,
                start,
                impact,
            );
            entry.affected_component_ids.push(component_id);
            entry.source_url =
                map_url(features).or_else(|| Some(source.metadata.status_url.clone()));
            entry.network_details = NetworkIncidentDetails {
                location: Some(location.clone()),
                services: vec![service.into()],
                locations: map_locations(features, county)?,
                estimated_resolution_at: local_date(&estimate).ok(),
                affected_customers: Some(customers),
            };
            entry.lifecycle = if planned {
                IncidentLifecycle::InProgress
            } else {
                IncidentLifecycle::Investigating
            };
            entry.updates.push(IncidentUpdate { external_id: format!("{id}-details"), lifecycle: entry.lifecycle.clone(),
                message: format!("Telia fixed-network report ({affected_service}). Estimated repair: {estimate} (Europe/Stockholm). {customers} affected customers. Geographic association follows the provider's county map viewport."), published_at: start });
            batch.incidents.push(entry);
        }
    }
    if count == 0 {
        return Err(invalid(
            "Telia lists an affected county without case details",
        ));
    }
    Ok(())
}

// The public map accepts RT90 viewport bounds directly (the same coordinates as its polygons).
fn projected_bounds(features: &[Value]) -> Option<[f64; 4]> {
    fn points(value: &Value, bounds: &mut [f64; 4]) {
        if let Some(items) = value.as_array() {
            if items.len() >= 2 && items[0].is_number() && items[1].is_number() {
                if let (Some(x), Some(y)) = (items[0].as_f64(), items[1].as_f64()) {
                    bounds[0] = bounds[0].min(x);
                    bounds[1] = bounds[1].min(y);
                    bounds[2] = bounds[2].max(x);
                    bounds[3] = bounds[3].max(y);
                }
            } else {
                for item in items {
                    points(item, bounds);
                }
            }
        }
    }
    let mut bounds = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for feature in features {
        if feature
            .pointer("/properties/isRelevant")
            .and_then(Value::as_bool)
            == Some(false)
        {
            continue;
        }
        if let Some(coordinates) = feature.pointer("/geometry/coordinates") {
            points(coordinates, &mut bounds);
        }
    }
    if !bounds.iter().all(|n| n.is_finite()) {
        return None;
    }
    Some(bounds)
}

fn map_url(features: &[Value]) -> Option<String> {
    let bounds = projected_bounds(features)?;
    Some(format!(
        "https://glu2.han.telia.se/bios/glup/glup.html?xmin={}&ymin={}&xmax={}&ymax={}",
        bounds[0], bounds[1], bounds[2], bounds[3]
    ))
}

fn map_locations(features: &[Value], county: &str) -> Result<Vec<NetworkLocation>, CollectorError> {
    use proj4rs::proj::Proj;
    // Exact definition published by Telia at /bios/glup/proj4js/defs/RT90DP.js.
    let from = Proj::from_proj_string("+proj=tmerc +lat_0=0 +lon_0=15.8062845294444 +k=1.00000561024 +x_0=1500064.274 +y_0=-667.711 +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs")
        .map_err(|_| invalid("invalid Telia map projection"))?;
    let to = Proj::from_proj_string("+proj=longlat +ellps=WGS84 +datum=WGS84 +no_defs")
        .map_err(|_| invalid("invalid geographic projection"))?;
    let mut locations = Vec::new();
    for feature in features {
        let Some(bounds) = projected_bounds(std::slice::from_ref(feature)) else {
            continue;
        };
        let mut point = (
            (bounds[0] + bounds[2]) / 2.0,
            (bounds[1] + bounds[3]) / 2.0,
            0.0,
        );
        proj4rs::transform::transform(&from, &to, &mut point)
            .map_err(|_| invalid("Telia area projection failed"))?;
        let (longitude, latitude) = (point.0.to_degrees(), point.1.to_degrees());
        if !latitude.is_finite()
            || !longitude.is_finite()
            || !(-90.0..=90.0).contains(&latitude)
            || !(-180.0..=180.0).contains(&longitude)
        {
            return Err(invalid("Telia map point is outside geographic bounds"));
        }
        if !latitude.is_finite()
            || !longitude.is_finite()
            || !(-90.0..=90.0).contains(&latitude)
            || !(-180.0..=180.0).contains(&longitude)
        {
            return Err(invalid("invalid Telia geographic coordinates"));
        }
        let area = feature
            .pointer("/properties/name")
            .and_then(Value::as_str)
            .unwrap_or("unnamed provider area");
        let geometry = map_geometry(feature, &from, &to)?;
        locations.push(NetworkLocation {
            name: format!("{county} · {area}"),
            latitude,
            longitude,
            precision: if geometry.is_some() {
                "provider_polygon"
            } else {
                "area_center"
            }
            .into(),
            geometry,
        });
    }
    Ok(locations)
}

fn map_geometry(
    feature: &Value,
    from: &proj4rs::proj::Proj,
    to: &proj4rs::proj::Proj,
) -> Result<Option<Value>, CollectorError> {
    let Some(mut geometry) = feature.get("geometry").cloned() else {
        return Ok(None);
    };
    if !matches!(
        geometry.get("type").and_then(Value::as_str),
        Some("Polygon" | "MultiPolygon")
    ) {
        return Ok(None);
    }
    fn transform(
        value: &mut Value,
        from: &proj4rs::proj::Proj,
        to: &proj4rs::proj::Proj,
    ) -> Result<(), CollectorError> {
        let values = value
            .as_array_mut()
            .ok_or_else(|| invalid("invalid Telia polygon"))?;
        if values.first().is_some_and(Value::is_number) {
            if values.len() < 2 {
                return Err(invalid("invalid Telia polygon point"));
            }
            let mut point = (
                values[0]
                    .as_f64()
                    .ok_or_else(|| invalid("invalid easting"))?,
                values[1]
                    .as_f64()
                    .ok_or_else(|| invalid("invalid northing"))?,
                0.0,
            );
            proj4rs::transform::transform(from, to, &mut point)
                .map_err(|_| invalid("Telia polygon projection failed"))?;
            let (lon, lat) = (point.0.to_degrees(), point.1.to_degrees());
            if !lon.is_finite() || !lat.is_finite() || lon.abs() > 180.0 || lat.abs() > 90.0 {
                return Err(invalid("invalid Telia polygon bounds"));
            }
            *value = serde_json::json!([lon, lat]);
        } else {
            for value in values {
                transform(value, from, to)?;
            }
        }
        Ok(())
    }
    transform(&mut geometry["coordinates"], from, to)?;
    Ok(Some(geometry))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn telia_excludes_tv_cases_and_links_the_relevant_map_geometry() {
        let config = crate::config::AppConfig::load("config/sources.yaml").unwrap();
        let source = config.sources.iter().find(|s| s.id == "telia").unwrap();
        let mut batch = empty(source);
        let html = br#"<div id="disturbanceListTable"><table><tbody><tr><td></td><td>TV-1</td><td>TV failure</td><td></td><td></td><td>TV</td><td></td></tr></tbody></table></div>"#;
        append_cases(source, html, "Skane", "Broadband", &mut batch).unwrap();
        assert!(batch.incidents.is_empty());
        assert!(batch.components.is_empty());
        let features = serde_json::json!([{"properties":{"isRelevant":true},"geometry":{"coordinates":[[[1346000,6187000],[1347000,6188000]]]}},{"properties":{"isRelevant":false},"geometry":{"coordinates":[[0,0]]}}]);
        let link = map_url(features.as_array().unwrap()).unwrap();
        let points = map_locations(features.as_array().unwrap(), "Skane").unwrap();
        assert_eq!(points.len(), 1);
        // Independently checked with proj4js and Telia's published RT90DP definition.
        assert!((points[0].latitude - 55.79213075586905).abs() < 1e-7);
        assert!((points[0].longitude - 13.357897210046875).abs() < 1e-7);
        assert_eq!(points[0].precision, "area_center");
        assert!(link.contains("xmin=1346000&ymin=6187000&xmax=1347000&ymax=6188000"));
    }

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
