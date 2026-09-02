//! Shared application state and the database pool / HTTP client behind it.

use std::sync::Arc;
use std::time::Duration;

use sqlx::{postgres::PgPoolOptions, PgPool};

use crate::config::Config;

/// Everything a handler or worker needs. Cheap to clone: the pool and http
/// client are reference-counted internally, and the config sits behind an `Arc`.
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
