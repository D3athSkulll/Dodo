//! Binary entrypoint. Sets up logging and starts the service.

fn main() {
    init_tracing();

    tracing::info!(
        service = "invoice-service",
        version = env!("CARGO_PKG_VERSION"),
        "bootstrap complete"
    );
}

/// JSON logs to stdout, level from `RUST_LOG`. Shared by the binary and tests.
fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};

    // Fall back to a sane filter so a bare `cargo run` still prints something.
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,invoice_service=debug"));

    fmt()
        .json()
        .with_env_filter(filter)
        .with_target(true)
        .init();
}
