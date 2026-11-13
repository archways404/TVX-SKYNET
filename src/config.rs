use std::{
    collections::HashSet,
    fs,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::Path,
    time::Duration,
};

use anyhow::{Context, bail};
use serde::Deserialize;
use url::Url;

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_listen_addr")]
    pub listen_addr: SocketAddr,
    #[serde(default)]
    pub defaults: PollDefaults,
    pub sources: Vec<SourceConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PollDefaults {
    #[serde(default = "default_poll_interval", with = "humantime_serde")]
    pub poll_interval: Duration,
    #[serde(default = "default_request_timeout", with = "humantime_serde")]
    pub request_timeout: Duration,
    #[serde(default = "default_stale_after", with = "humantime_serde")]
    pub stale_after: Duration,
    #[serde(default = "default_max_concurrency")]
    pub max_concurrency: usize,
    #[serde(default = "default_max_response_bytes")]
    pub max_response_bytes: usize,
}

impl Default for PollDefaults {
    fn default() -> Self {
        Self {
            poll_interval: default_poll_interval(),
            request_timeout: default_request_timeout(),
            stale_after: default_stale_after(),
            max_concurrency: default_max_concurrency(),
            max_response_bytes: default_max_response_bytes(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SourceConfig {
    pub id: String,
    pub name: String,
    pub category: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, with = "humantime_serde::option")]
    pub poll_interval: Option<Duration>,
    #[serde(default, with = "humantime_serde::option")]
    pub request_timeout: Option<Duration>,
    #[serde(default, with = "humantime_serde::option")]
    pub stale_after: Option<Duration>,
    #[serde(default)]
    pub include_components: Vec<String>,
    pub collector: CollectorConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CollectorConfig {
    Statuspage { base_url: Url },
    Feed { url: Url },
}

impl AppConfig {
    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let contents = fs::read_to_string(path)
            .with_context(|| format!("could not read {}", path.display()))?;
        let config: Self = serde_yaml::from_str(&contents)
            .with_context(|| format!("could not parse {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.sources.is_empty() {
            bail!("at least one source must be configured");
        }
        if self.defaults.max_concurrency == 0 {
            bail!("defaults.max_concurrency must be greater than zero");
        }

        let mut ids = HashSet::new();
        for source in &self.sources {
            if source.id.trim().is_empty() || source.name.trim().is_empty() {
                bail!("source id and name must not be empty");
            }
            if !ids.insert(source.id.as_str()) {
                bail!("duplicate source id: {}", source.id);
            }
        }
        Ok(())
    }
}

impl SourceConfig {
    pub fn poll_interval(&self, defaults: &PollDefaults) -> Duration {
        self.poll_interval.unwrap_or(defaults.poll_interval)
    }

    pub fn request_timeout(&self, defaults: &PollDefaults) -> Duration {
        self.request_timeout.unwrap_or(defaults.request_timeout)
    }

    pub fn stale_after(&self, defaults: &PollDefaults) -> Duration {
        self.stale_after.unwrap_or(defaults.stale_after)
    }
}

fn default_listen_addr() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8080)
}

fn default_poll_interval() -> Duration {
    Duration::from_secs(300)
}

fn default_request_timeout() -> Duration {
    Duration::from_secs(15)
}

fn default_stale_after() -> Duration {
    Duration::from_secs(900)
}

fn default_max_concurrency() -> usize {
    16
}

fn default_max_response_bytes() -> usize {
    2 * 1024 * 1024
}

fn default_true() -> bool {
    true
}
