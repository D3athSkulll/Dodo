//! Process configuration, read once from the environment at startup.

use std::time::Duration;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("{0} is not set")]
    Missing(&'static str),
    #[error("{0} is invalid: {1}")]
    Invalid(&'static str, String),
}

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub psp_base_url: String,
    pub psp_timeout: Duration,
    pub bind_addr: String,
    pub webhook_worker_interval: Duration,
    pub webhook_lease: Duration,
    pub payment_sweep_interval: Duration,
    pub payment_pending_max_age: Duration,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            database_url: required("DATABASE_URL")?,
            psp_base_url: required("PSP_BASE_URL")?,
            psp_timeout: millis("PSP_TIMEOUT_MS")?,
            bind_addr: optional("BIND_ADDR", "0.0.0.0:8080"),
            webhook_worker_interval: millis("WEBHOOK_WORKER_INTERVAL_MS")?,
            webhook_lease: seconds("WEBHOOK_LEASE_SECONDS")?,
            payment_sweep_interval: millis("PAYMENT_SWEEP_INTERVAL_MS")?,
            payment_pending_max_age: seconds("PAYMENT_PENDING_MAX_AGE_SECONDS")?,
        })
    }
}

fn required(key: &'static str) -> Result<String, ConfigError> {
    std::env::var(key).map_err(|_| ConfigError::Missing(key))
}

fn optional(key: &'static str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}

fn millis(key: &'static str) -> Result<Duration, ConfigError> {
    Ok(Duration::from_millis(parse_u64(key)?))
}

fn seconds(key: &'static str) -> Result<Duration, ConfigError> {
    Ok(Duration::from_secs(parse_u64(key)?))
}

fn parse_u64(key: &'static str) -> Result<u64, ConfigError> {
    required(key)?
        .parse()
        .map_err(|e: std::num::ParseIntError| ConfigError::Invalid(key, e.to_string()))
}
