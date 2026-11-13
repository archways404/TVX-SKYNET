use std::sync::Arc;

use chrono::Utc;
use tokio::{sync::Semaphore, task::JoinSet, time};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::{
    collectors::CollectorRegistry,
    config::{PollDefaults, SourceConfig},
    storage::SharedStore,
};

pub struct PollScheduler {
    sources: Vec<SourceConfig>,
    defaults: PollDefaults,
    collectors: CollectorRegistry,
    store: SharedStore,
    shutdown: CancellationToken,
}

impl PollScheduler {
    pub fn new(
        sources: Vec<SourceConfig>,
        defaults: PollDefaults,
        collectors: CollectorRegistry,
        store: SharedStore,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            sources,
            defaults,
            collectors,
            store,
            shutdown,
        }
    }

    pub async fn run(self) {
        let semaphore = Arc::new(Semaphore::new(self.defaults.max_concurrency));
        let mut tasks = JoinSet::new();

        for source in self.sources.into_iter().filter(|source| source.enabled) {
            tasks.spawn(run_source(
                source,
                self.defaults.clone(),
                self.collectors.clone(),
                self.store.clone(),
                semaphore.clone(),
                self.shutdown.clone(),
            ));
        }

        info!(source_count = tasks.len(), "poll scheduler started");
        self.shutdown.cancelled().await;

        while let Some(result) = tasks.join_next().await {
            if let Err(error) = result {
                error!(%error, "source poll task terminated unexpectedly");
            }
        }
        info!("poll scheduler stopped");
    }
}

async fn run_source(
    source: SourceConfig,
    defaults: PollDefaults,
    collectors: CollectorRegistry,
    store: SharedStore,
    semaphore: Arc<Semaphore>,
    shutdown: CancellationToken,
) {
    let mut interval = time::interval(source.poll_interval(&defaults));
    interval.set_missed_tick_behavior(time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = interval.tick() => {
                let permit = tokio::select! {
                    _ = shutdown.cancelled() => break,
                    permit = semaphore.clone().acquire_owned() => match permit {
                        Ok(permit) => permit,
                        Err(_) => break,
                    },
                };

                let started_at = Utc::now();
                debug!(source = %source.id, "polling source");
                let result = time::timeout(
                    source.request_timeout(&defaults),
                    collectors.collect(&source),
                )
                .await;
                drop(permit);

                match result {
                    Ok(Ok(batch)) => {
                        if let Err(error) = store.apply(batch).await {
                            error!(source = %source.id, %error, "failed to store observation batch");
                        } else {
                            debug!(source = %source.id, "source poll completed");
                        }
                    }
                    Ok(Err(error)) => {
                        warn!(source = %source.id, %error, "source poll failed");
                        if let Err(store_error) = store
                            .mark_failed(&source.id, started_at, error.to_string())
                            .await
                        {
                            error!(source = %source.id, %store_error, "failed to record poll failure");
                        }
                    }
                    Err(_) => {
                        let message = format!(
                            "request exceeded timeout of {:?}",
                            source.request_timeout(&defaults)
                        );
                        warn!(source = %source.id, error = %message, "source poll timed out");
                        if let Err(store_error) = store
                            .mark_failed(&source.id, started_at, message)
                            .await
                        {
                            error!(source = %source.id, %store_error, "failed to record poll timeout");
                        }
                    }
                }
            }
        }
    }
}
