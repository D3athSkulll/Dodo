//! Stand-in payment processor. invoice-service calls it over HTTP and treats it
//! as a real external dependency.

fn main() {
    init_tracing();

    tracing::info!(
        service = "mock-psp",
        version = env!("CARGO_PKG_VERSION"),
        "bootstrap complete"
    );
}

/// JSON logs to stdout, level from `RUST_LOG`. Duplicated from invoice-service
/// on purpose — no shared crate for one small function.
fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,mock_psp=debug"));

    fmt()
        .json()
        .with_env_filter(filter)
        .with_target(true)
        .init();
}
