//! The same public ArcGIS layers consumed by kartat.dna.fi/Hairiokartta.
use super::*;

const LAYERS: &[(u8, &str)] = &[
    (9, "Voice"),
    (10, "SMS"),
    (11, "ADSL broadband"),
    (13, "Mobile voice and data"),
];

pub(super) async fn collect(
    collector: &CustomCollector,
    source: &SourceConfig,
    base: &Url,
) -> Result<ObservationBatch, CollectorError> {
    let mut batch = empty(source);
    for &(layer, service) in LAYERS {
        let mut url = base
            .join(&format!("{layer}/query"))
            .map_err(|_| invalid("invalid DNA map URL"))?;
        url.query_pairs_mut().extend_pairs([
            ("where", "1=1"),
            ("outFields", "*"),
            ("returnGeometry", "true"),
            ("outSR", "4326"),
            ("f", "json"),
        ]);
        let body = collector.body(collector.client.get(url)).await?;
        append(
            source,
            &serde_json::from_slice(&body)?,
            layer,
            service,
            &mut batch,
        )?;
    }
    batch.components.push(ComponentObservation {
        external_id: "public-map".into(),
        name: "Published DNA outage-map scope".into(),
        status: ServiceStatus::Operational,
        updated_at: batch.observed_at,
    });
    finish(&mut batch);
    Ok(batch)
}

fn append(
    source: &SourceConfig,
    payload: &Value,
    layer: u8,
    service: &str,
    batch: &mut ObservationBatch,
) -> Result<(), CollectorError> {
    if payload.get("error").is_some()
        || payload
            .get("exceededTransferLimit")
            .and_then(Value::as_bool)
            == Some(true)
    {
        return Err(invalid("DNA map query failed or was truncated"));
    }
    if payload
        .pointer("/spatialReference/wkid")
        .and_then(Value::as_u64)
        != Some(4326)
    {
        return Err(invalid("DNA map did not return WGS84"));
    }
    for feature in array(payload, "features")? {
        let attrs = feature
            .get("attributes")
            .ok_or_else(|| invalid("DNA attributes missing"))?;
        let id = attrs
            .get("RIVI_ID")
            .and_then(Value::as_u64)
            .ok_or_else(|| invalid("DNA feature ID missing"))?;
        let started = attrs
            .get("VIKA_ALKOI")
            .and_then(Value::as_i64)
            .and_then(DateTime::from_timestamp_millis)
            .ok_or_else(|| invalid("DNA start time missing"))?;
        if started > batch.observed_at {
            continue;
        }
        // VIKA_LOPPUI is the published fault end, not an estimated repair time.
        if attrs
            .get("VIKA_LOPPUI")
            .and_then(Value::as_i64)
            .and_then(DateTime::from_timestamp_millis)
            .is_some_and(|ended| ended <= batch.observed_at)
        {
            continue;
        }
        let rings = feature
            .pointer("/geometry/rings")
            .ok_or_else(|| invalid("DNA polygon missing"))?;
        let mut bounds = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        let mut polygons = vec![];
        let mut holes = vec![];
        for ring in rings
            .as_array()
            .ok_or_else(|| invalid("DNA polygon rings missing"))?
        {
            let points = ring.as_array().ok_or_else(|| invalid("DNA ring invalid"))?;
            if points.len() < 4 || points.first() != points.last() {
                return Err(invalid("DNA polygon ring is not closed"));
            }
            for point in points {
                let lon = point
                    .get(0)
                    .and_then(Value::as_f64)
                    .filter(|v| v.is_finite() && v.abs() <= 180.0)
                    .ok_or_else(|| invalid("DNA longitude invalid"))?;
                let lat = point
                    .get(1)
                    .and_then(Value::as_f64)
                    .filter(|v| v.is_finite() && v.abs() <= 90.0)
                    .ok_or_else(|| invalid("DNA latitude invalid"))?;
                bounds[0] = bounds[0].min(lon);
                bounds[1] = bounds[1].min(lat);
                bounds[2] = bounds[2].max(lon);
                bounds[3] = bounds[3].max(lat);
            }
            // ArcGIS ring winding distinguishes outer boundaries from holes.
            let signed_area: f64 = points
                .windows(2)
                .map(|p| {
                    p[0][0].as_f64().unwrap() * p[1][1].as_f64().unwrap()
                        - p[1][0].as_f64().unwrap() * p[0][1].as_f64().unwrap()
                })
                .sum();
            if signed_area < 0.0 {
                polygons.push(vec![ring.clone()]);
            } else {
                holes.push(ring.clone());
            }
        }
        for hole in holes {
            let polygon = polygons
                .iter_mut()
                .find(|polygon| contains(&polygon[0], &hole[0]))
                .ok_or_else(|| invalid("DNA polygon hole without containing boundary"))?;
            polygon.push(hole);
        }
        if polygons.is_empty() {
            return Err(invalid("DNA polygon is empty"));
        }
        let locality = attrs
            .get("KUNTA")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("Finland · provider map area");
        let postal = attrs
            .get("POSTINUMERO")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let location = format!("{locality} {postal}").trim().to_owned();
        let external_id = format!("{layer}:{id}");
        let mut entry = incident(
            source,
            &external_id,
            &format!("{service} — {location}"),
            started,
            batch.observed_at,
            ServiceStatus::PartialOutage,
        );
        entry.source_url = Some(source.metadata.status_url.clone());
        entry.network_details = NetworkIncidentDetails {
            location: Some(location.clone()),
            services: vec![service.into()],
            locations: vec![NetworkLocation {
                name: location,
                latitude: (bounds[1] + bounds[3]) / 2.0,
                longitude: (bounds[0] + bounds[2]) / 2.0,
                precision: "provider_polygon".into(),
                geometry: Some(serde_json::json!({"type":"MultiPolygon", "coordinates":polygons})),
            }],
            ..Default::default()
        };
        if let Some(message) = attrs
            .get("INFO_TEKSTI_EN")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .or_else(|| attrs.get("INFO_TEKSTI_FI").and_then(Value::as_str))
        {
            let message = scraper::Html::parse_fragment(message)
                .root_element()
                .text()
                .collect::<Vec<_>>()
                .join(" ");
            entry.updates.push(IncidentUpdate {
                external_id: format!("{external_id}:notice"),
                lifecycle: entry.lifecycle.clone(),
                message,
                published_at: started,
            });
        }
        batch.incidents.push(entry);
    }
    Ok(())
}

// Associate holes by containment: ArcGIS does not require outer/hole ring ordering.
fn contains(ring: &Value, point: &Value) -> bool {
    let (x, y) = (point[0].as_f64().unwrap(), point[1].as_f64().unwrap());
    let mut inside = false;
    for edge in ring.as_array().unwrap().windows(2) {
        let (x1, y1, x2, y2) = (
            edge[0][0].as_f64().unwrap(),
            edge[0][1].as_f64().unwrap(),
            edge[1][0].as_f64().unwrap(),
            edge[1][1].as_f64().unwrap(),
        );
        if (y1 > y) != (y2 > y) && x < (x2 - x1) * (y - y1) / (y2 - y1) + x1 {
            inside = !inside;
        }
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn official_map_preserves_geometry_service_and_municipality() {
        let config = crate::config::AppConfig::load("config/sources.yaml").unwrap();
        let source = config.sources.iter().find(|s| s.id == "dna-fi").unwrap();
        let mut batch = empty(source);
        let mut payload = serde_json::json!({"spatialReference":{"wkid":4326},"features":[{"attributes":{"RIVI_ID":7,"KUNTA":"Espoo","POSTINUMERO":"02150","VIKA_ALKOI":1735689600000_i64,"INFO_TEKSTI_EN":"<p>Voice affected</p>"},"geometry":{"rings":[[[24.8,60.1],[24.8,60.2],[24.9,60.2],[24.9,60.1],[24.8,60.1]]]}}]});
        append(source, &payload, 9, "Voice", &mut batch).unwrap();
        let incident = &batch.incidents[0];
        assert_eq!(
            incident.network_details.location.as_deref(),
            Some("Espoo 02150")
        );
        assert_eq!(incident.network_details.services, ["Voice"]);
        assert_eq!(
            incident.network_details.locations[0]
                .geometry
                .as_ref()
                .unwrap()["type"],
            "MultiPolygon"
        );
        assert_eq!(incident.updates[0].message, "Voice affected");
        let outer = payload["features"][0]["geometry"]["rings"][0].clone();
        let hole = serde_json::json!([
            [24.82, 60.12],
            [24.88, 60.12],
            [24.88, 60.18],
            [24.82, 60.18],
            [24.82, 60.12]
        ]);
        payload["features"][0]["geometry"]["rings"] = serde_json::json!([hole, outer]);
        let mut with_hole = empty(source);
        append(source, &payload, 9, "Voice", &mut with_hole).unwrap();
        assert_eq!(
            with_hole.incidents[0].network_details.locations[0]
                .geometry
                .as_ref()
                .unwrap()["coordinates"][0]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        payload["exceededTransferLimit"] = true.into();
        assert!(append(source, &payload, 9, "Voice", &mut empty(source)).is_err());
        assert!(
            append(
                source,
                &serde_json::json!({"error":{"code":500}}),
                9,
                "Voice",
                &mut empty(source)
            )
            .is_err()
        );
    }
    #[test]
    fn ended_outages_do_not_remain_on_the_map() {
        let config = crate::config::AppConfig::load("config/sources.yaml").unwrap();
        let source = config.sources.iter().find(|s| s.id == "dna-fi").unwrap();
        let payload = serde_json::json!({"spatialReference":{"wkid":4326},"features":[{"attributes":{"RIVI_ID":7,"KUNTA":"Espoo","VIKA_ALKOI":1735689600000_i64,"VIKA_LOPPUI":1735776000000_i64},"geometry":{"rings":[[[24.8,60.1],[24.8,60.2],[24.9,60.2],[24.9,60.1],[24.8,60.1]]]}}]});
        let mut batch = empty(source);
        append(source, &payload, 9, "Voice", &mut batch).unwrap();
        assert!(
            batch.incidents.is_empty(),
            "ended faults must not generate active map points"
        );
    }
}
