use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

pub fn init() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("external_status_combinator=info,tower_http=info"));

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().compact())
        .init();
}
