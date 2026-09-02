//! Binary entrypoint.
//!
//! * `invoice-service`        — serve.
//! * `invoice-service seed`   — create one business + API key, print it.
//! * `invoice-service demo`   — seed a spread of sample data for exploring the API.

use invoice_service::{
    auth,
    config::Config,
    demo, routes, state, telemetry,
    workers::{payment_sweeper, webhook_delivery},
};

#[tokio::main]
async fn main() {
    telemetry::init_tracing();

    let result = match std::env::args().nth(1).as_deref() {
        Some("seed") => seed().await,
        Some("demo") => run_demo().await,
        _ => serve().await,
    };

    if let Err(e) = result {
        tracing::error!(error = %e, "startup failed");
        std::process::exit(1);
    }
}

async fn serve() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env()?;
    let pool = state::connect_pool(&config).await?;

    // Single service, single writer, so migrations run on the way up.
    sqlx::migrate!().run(&pool).await?;

    let bind = config.bind_addr.clone();
    let app_state = state::build_state(pool, config);

    // Background workers. Both are idempotent and resume on the next start, so
    // they are simply aborted on shutdown.
    payment_sweeper::spawn(app_state.clone());
    webhook_delivery::spawn(app_state.clone());

    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!(%bind, "listening");

    axum::serve(listener, routes::router(app_state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn seed() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env()?;
    let pool = state::connect_pool(&config).await?;
    sqlx::migrate!().run(&pool).await?;

    let (business_id, token) = auth::seed(&pool).await?;
    // Printed once, to stdout. This is the only time the full key exists.
    println!("business_id {business_id}");
    println!("api_key     {token}");

    Ok(())
}

async fn run_demo() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env()?;
    let pool = state::connect_pool(&config).await?;
    sqlx::migrate!().run(&pool).await?;

    let d = demo::run(&pool).await?;
    // Copy-paste straight into a shell.
    println!("export API_KEY={}", d.api_key);
    println!("export BUSINESS_ID={}", d.business_id);
    println!("export CUSTOMER_ID={}", d.customer_id);
    println!("export OPEN_INVOICE_ID={}", d.open_invoice_id);
    println!("export PAID_INVOICE_ID={}", d.paid_invoice_id);

    Ok(())
}

/// Resolves when the process is asked to stop, so in-flight requests (and later
/// the background workers) get to finish their current work.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.ok();
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }

    tracing::info!("shutdown signal received");
}
