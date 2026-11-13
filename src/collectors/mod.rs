mod statuspage;

use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;

use crate::{
    config::{CollectorConfig, PollDefaults, SourceConfig},
    domain::ObservationBatch,
};

pub use statuspage::StatuspageCollector;

#[derive(Debug, Error)]
pub enum CollectorError {
    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("provider returned HTTP {0}")]
    HttpStatus(reqwest::StatusCode),
    #[error("response body was {actual} bytes, above the configured {limit} byte limit")]
    ResponseTooLarge { actual: usize, limit: usize },
    #[error("could not parse provider response: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("collector is not implemented yet: {0}")]
    Unsupported(String),
}

#[async_trait]
pub trait Collector: Send + Sync {
    async fn collect(&self, source: &SourceConfig) -> Result<ObservationBatch, CollectorError>;
}

#[derive(Clone)]
pub struct CollectorRegistry {
    statuspage: Arc<StatuspageCollector>,
}

impl CollectorRegistry {
    pub fn new(defaults: &PollDefaults) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(concat!(
                "TVX-SKYNET/external-status-combinator/",
                env!("CARGO_PKG_VERSION")
            ))
            .redirect(reqwest::redirect::Policy::limited(5))
            .pool_max_idle_per_host(4)
            .build()?;

        Ok(Self {
            statuspage: Arc::new(StatuspageCollector::new(
                client,
                defaults.max_response_bytes,
            )),
        })
    }

    pub async fn collect(&self, source: &SourceConfig) -> Result<ObservationBatch, CollectorError> {
        match &source.collector {
            CollectorConfig::Statuspage { .. } => self.statuspage.collect(source).await,
            CollectorConfig::Feed { url } => Err(CollectorError::Unsupported(format!(
                "feed source {} ({url})",
                source.id
            ))),
        }
    }
}
