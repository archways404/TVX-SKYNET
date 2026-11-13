mod api;
mod collectors;
mod config;
mod domain;
mod polling;
mod storage;
mod telemetry;

use std::{env, sync::Arc};

use anyhow::Context;
use collectors::CollectorRegistry;
use config::AppConfig;
use polling::PollScheduler;
use storage::{InMemoryStore, PostgresStore, StatusStore};
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    telemetry::init();

    let config_path =
        env::var("EXTERNAL_STATUS_CONFIG").unwrap_or_else(|_| "config/sources.yaml".to_owned());
    let config = AppConfig::load(&config_path)
        .with_context(|| format!("failed to load configuration from {config_path}"))?;

    let store = create_store(&config).await?;
    let store_for_api = store.clone();
    let store_for_scheduler = store;

    let collectors = CollectorRegistry::new(&config.defaults)?;
    let shutdown = CancellationToken::new();

    let scheduler = PollScheduler::new(
        config.sources.clone(),
        config.defaults.clone(),
        collectors,
        store_for_scheduler,
        shutdown.clone(),
    );
    let scheduler_task = tokio::spawn(scheduler.run());

    let app = api::router(store_for_api);
    let listener = tokio::net::TcpListener::bind(config.listen_addr)
        .await
        .with_context(|| format!("failed to bind API listener to {}", config.listen_addr))?;

    info!(address = %config.listen_addr, "external status combinator started");

    let server_result = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(shutdown.clone()))
        .await;

    shutdown.cancel();

    if let Err(error) = scheduler_task.await {
        error!(%error, "poll scheduler task terminated unexpectedly");
    }

    server_result.context("API server failed")
}

async fn create_store(config: &AppConfig) -> anyhow::Result<Arc<dyn StatusStore>> {
    let backend = env::var("STORAGE_BACKEND").unwrap_or_else(|_| "memory".to_owned());
    match backend.as_str() {
        "memory" => {
            info!(backend = "memory", "status store initialized");
            Ok(Arc::new(InMemoryStore::new(
                &config.sources,
                &config.defaults,
            )))
        }
        "postgres" => {
            let database_url = env::var("DATABASE_URL")
                .context("DATABASE_URL is required when STORAGE_BACKEND=postgres")?;
            let store = PostgresStore::connect(&database_url, &config.sources, &config.defaults)
                .await
                .context("failed to initialize PostgreSQL status store")?;
            info!(backend = "postgres", "status store initialized");
            Ok(Arc::new(store))
        }
        other => {
            anyhow::bail!("unsupported STORAGE_BACKEND {other:?}; expected memory or postgres")
        }
    }
}

async fn shutdown_signal(shutdown: CancellationToken) {
    match tokio::signal::ctrl_c().await {
        Ok(()) => info!("shutdown signal received"),
        Err(error) => error!(%error, "failed to listen for shutdown signal"),
    }
    shutdown.cancel();
}
