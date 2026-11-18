//! Public dependency checks with deliberately narrow, documented coverage.
use super::*;
use chrono::{Datelike, NaiveDate};

pub(super) async fn nager(
    collector: &CustomCollector,
    source: &SourceConfig,
    base: &Url,
) -> Result<ObservationBatch, CollectorError> {
    let mut batch = empty(source);
    let year = batch.observed_at.year();
    for country in ["FO", "AD", "GL", "IS"] {
        let url = base
            .join(&format!("{year}/{country}"))
            .map_err(|_| invalid("invalid holiday URL"))?;
        let body = collector.body(collector.client.get(url)).await?;
        validate_holidays(&serde_json::from_slice(&body)?, year, country)?;
        batch.components.push(ComponentObservation {
            external_id: country.into(),
            name: format!("{country} current-year public holidays"),
            status: ServiceStatus::Operational,
            updated_at: batch.observed_at,
        });
    }
    finish(&mut batch);
    Ok(batch)
}
fn validate_holidays(payload: &Value, year: i32, country: &str) -> Result<(), CollectorError> {
    let holidays = payload
        .as_array()
        .filter(|items| !items.is_empty())
        .ok_or_else(|| invalid("holiday calendar empty or invalid"))?;
    for item in holidays {
        let date = NaiveDate::parse_from_str(text(item, "date")?, "%Y-%m-%d")
            .map_err(|_| invalid("invalid holiday date"))?;
        if date.year() != year
            || text(item, "countryCode")? != country
            || text(item, "name")?.trim().is_empty()
        {
            return Err(invalid("holiday calendar country, year or name mismatch"));
        }
    }
    Ok(())
}

pub(super) async fn hund(
    collector: &CustomCollector,
    source: &SourceConfig,
    url: &Url,
) -> Result<ObservationBatch, CollectorError> {
    let body = collector.body(collector.client.get(url.clone())).await?;
    parse_hund(source, &String::from_utf8_lossy(&body))
}
fn parse_hund(source: &SourceConfig, html: &str) -> Result<ObservationBatch, CollectorError> {
    let doc = scraper::Html::parse_document(html);
    let selector = |s| scraper::Selector::parse(s).unwrap();
    let mut batch = empty(source);
    for item in doc.select(&selector(".item")) {
        let Some(link) = item
            .select(&selector(".info .name a[href^='/components/']"))
            .next()
        else {
            continue;
        };
        let name = link.text().collect::<String>();
        if !included(source, &[name.as_str()]) {
            continue;
        }
        let node = item
            .select(&selector(".status .state"))
            .next()
            .ok_or_else(|| invalid("Hund component status missing"))?;
        let status = if node
            .value()
            .has_class("operational", scraper::CaseSensitivity::CaseSensitive)
        {
            ServiceStatus::Operational
        } else if node
            .value()
            .has_class("degraded", scraper::CaseSensitivity::CaseSensitive)
        {
            ServiceStatus::Degraded
        } else if node
            .value()
            .has_class("outage", scraper::CaseSensitivity::CaseSensitive)
        {
            ServiceStatus::MajorOutage
        } else if node
            .value()
            .has_class("maintenance", scraper::CaseSensitivity::CaseSensitive)
        {
            ServiceStatus::Maintenance
        } else {
            ServiceStatus::Unknown
        };
        batch.components.push(ComponentObservation {
            external_id: link
                .value()
                .attr("href")
                .unwrap()
                .trim_start_matches("/components/")
                .into(),
            name,
            status,
            updated_at: batch.observed_at,
        });
    }
    if batch.components.is_empty()
        || source
            .include_components
            .iter()
            .any(|name| !batch.components.iter().any(|c| &c.name == name))
    {
        return Err(invalid(
            "Hund configured components missing; page may have changed",
        ));
    }
    finish(&mut batch);
    Ok(batch)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn holidays_require_nonempty_correct_country_and_year() {
        let good =
            serde_json::json!([{"date":"2026-01-01", "countryCode":"FO", "name":"New Year"}]);
        assert!(validate_holidays(&good, 2026, "FO").is_ok());
        assert!(validate_holidays(&good, 2027, "FO").is_err());
        assert!(validate_holidays(&good, 2026, "IS").is_err());
        assert!(validate_holidays(&serde_json::json!([]), 2026, "FO").is_err());
    }
    #[test]
    fn hund_scopes_components_and_rejects_changed_markup() {
        let config = crate::config::AppConfig::load("config/sources.yaml").unwrap();
        let mut source = config
            .sources
            .iter()
            .find(|s| s.id == "creditsafe")
            .unwrap()
            .clone();
        source.include_components = vec!["Connect API".into()];
        let html = "<div class='item'><div class='info'><span class='name'><a href='/components/api'>Connect API</a></span></div><div class='status'><span class='degraded state icon'></span></div></div>";
        assert_eq!(
            parse_hund(&source, html).unwrap().status,
            ServiceStatus::Degraded
        );
        assert!(parse_hund(&source, "<html>All good</html>").is_err());
        assert_eq!(
            parse_hund(&source, &html.replace("degraded", "unrecognised"))
                .unwrap()
                .status,
            ServiceStatus::Unknown
        );
    }
    #[tokio::test]
    async fn gaps_are_visible_but_never_reported_healthy() {
        let config = crate::config::AppConfig::load("config/sources.yaml").unwrap();
        let registry = crate::collectors::CollectorRegistry::new(&config.defaults).unwrap();
        for source in &config.sources {
            if matches!(source.collector, CollectorConfig::CoverageGap { .. }) {
                assert!(
                    registry
                        .collect(source)
                        .await
                        .unwrap_err()
                        .to_string()
                        .contains("Coverage gap:")
                );
            }
        }
    }
}
