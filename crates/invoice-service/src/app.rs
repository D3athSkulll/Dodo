//! Wiring: shared state and the router.

use std::sync::Arc;
use std::time::Duration;

use axum::{middleware::from_fn_with_state, routing::get, Router};
use sqlx::{postgres::PgPoolOptions, PgPool};

use crate::{auth, config::Config, customers, health, invoices, payments, telemetry, webhooks};

/// Everything a handler needs. Cheap to clone: the pool and http client are
/// reference-counted internally, and the config sits behind an `Arc`.
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub http: reqwest::Client,
    pub config: Arc<Config>,
}

pub async fn connect_pool(config: &Config) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&config.database_url)
        .await
}

pub fn build_state(pool: PgPool, config: Config) -> AppState {
    let http = reqwest::Client::builder()
        .timeout(config.psp_timeout)
        .build()
        .expect("reqwest client with default TLS should build");

    AppState {
        pool,
        http,
        config: Arc::new(config),
    }
}

pub fn router(state: AppState) -> Router {
    // Everything under /v1 is behind API-key auth.
    let v1 = customers::routes()
        .merge(invoices::routes())
        .merge(payments::routes())
        .merge(webhooks::routes())
        .layer(from_fn_with_state(state.clone(), auth::require_api_key));

    Router::new()
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        .nest("/v1", v1)
        .layer(axum::middleware::from_fn(telemetry::request_id))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}
