use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Serialize;
use tower_http::{compression::CompressionLayer, trace::TraceLayer};

use crate::{
    domain::{Incident, ProviderView, SourceHealth, StatusSummary},
    storage::{SharedStore, StatusStore, StoreError},
};

#[derive(Clone)]
struct ApiState {
    store: SharedStore,
}

pub fn router(store: Arc<dyn StatusStore>) -> Router {
    let state = ApiState { store };
    Router::new()
        .route("/", get(root))
        .route("/api/v1/status", get(status))
        .route("/api/v1/providers", get(providers))
        .route("/api/v1/providers/{id}", get(provider))
        .route("/api/v1/incidents", get(incidents))
        .route("/api/v1/source-health", get(source_health))
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn root() -> Json<ServiceInfo> {
    Json(ServiceInfo {
        name: "external-status-combinator",
        version: env!("CARGO_PKG_VERSION"),
        status_endpoint: "/api/v1/status",
    })
}

async fn status(State(state): State<ApiState>) -> Result<Json<StatusSummary>, ApiError> {
    Ok(Json(state.store.summary().await?))
}

async fn providers(State(state): State<ApiState>) -> Result<Json<Vec<ProviderView>>, ApiError> {
    Ok(Json(state.store.providers().await?))
}

async fn provider(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<ProviderView>, ApiError> {
    state
        .store
        .provider(&id)
        .await?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

async fn incidents(State(state): State<ApiState>) -> Result<Json<Vec<Incident>>, ApiError> {
    Ok(Json(state.store.active_incidents().await?))
}

async fn source_health(State(state): State<ApiState>) -> Result<Json<Vec<SourceHealth>>, ApiError> {
    Ok(Json(state.store.source_health().await?))
}

async fn live() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn ready(State(state): State<ApiState>) -> Result<Json<HealthResponse>, ApiError> {
    state.store.source_health().await?;
    Ok(Json(HealthResponse { status: "ready" }))
}

#[derive(Serialize)]
struct ServiceInfo {
    name: &'static str,
    version: &'static str,
    status_endpoint: &'static str,
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

enum ApiError {
    NotFound,
    Store(StoreError),
}

impl From<StoreError> for ApiError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::NotFound => (StatusCode::NOT_FOUND, "resource not found".to_owned()),
            Self::Store(error) => {
                tracing::error!(%error, "status store error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal storage error".to_owned(),
                )
            }
        };
        (status, Json(ErrorResponse { error: message })).into_response()
    }
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}
