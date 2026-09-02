//! Standalone mock-psp binary. The behaviour lives in the library (`lib.rs`);
//! this just reads config from the environment and serves.

use std::time::Duration;

use mock_psp::{router, MockConfig};
use tokio::net::TcpListener;
use tracing_subscriber::{fmt, EnvFilter};

#[tokio::main]
async fn main() {
    init_tracing();

    let bind_addr = env_or("MOCK_PSP_BIND_ADDR", "0.0.0.0:9090");
    let config = MockConfig {
        fast_delay: Duration::from_millis(env_ms("MOCK_PSP_DELAY_MS", 100)),
        timeout_delay: Duration::from_millis(env_ms("MOCK_PSP_TIMEOUT_MS", 30_000)),
    };

    let listener = TcpListener::bind(&bind_addr).await.expect("bind");
    tracing::info!(addr = %bind_addr, "mock-psp listening");
    axum::serve(listener, router(config)).await.expect("serve");
}

fn init_tracing() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,mock_psp=debug"));
    fmt()
        .json()
        .with_env_filter(filter)
        .with_target(true)
        .init();
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}

fn env_ms(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}
