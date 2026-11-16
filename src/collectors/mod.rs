mod custom;
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
    #[error("invalid provider payload: {0}")]
    Invalid(String),
    #[error("manual status file: {0}")]
    Io(#[from] std::io::Error),
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
    custom: Arc<custom::CustomCollector>,
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
            custom: Arc::new(custom::CustomCollector::new(
                client.clone(),
                defaults.max_response_bytes,
            )),
            statuspage: Arc::new(StatuspageCollector::new(
                client,
                defaults.max_response_bytes,
            )),
        })
    }

    pub async fn collect(&self, source: &SourceConfig) -> Result<ObservationBatch, CollectorError> {
        match &source.collector {
            CollectorConfig::Statuspage { .. } => self.statuspage.collect(source).await,
            _ => self.custom.collect(source).await,
        }
    }
}
