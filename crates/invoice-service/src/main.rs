//! Binary entrypoint. `invoice-service` serves; `invoice-service seed` creates
//! one business + API key and prints the key once.

use invoice_service::{app, auth, config::Config, sweeper, telemetry, webhook_worker};

#[tokio::main]
async fn main() {
    telemetry::init_tracing();

    let result = match std::env::args().nth(1).as_deref() {
        Some("seed") => seed().await,
        _ => serve().await,
    };

    if let Err(e) = result {
        tracing::error!(error = %e, "startup failed");
        std::process::exit(1);
    }
}

async fn serve() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env()?;
    let pool = app::connect_pool(&config).await?;

    // Single service, single writer, so migrations run on the way up.
    sqlx::migrate!().run(&pool).await?;

    let bind = config.bind_addr.clone();
    let state = app::build_state(pool, config);

    // Background workers. Both are idempotent and resume on the next start, so
    // they are simply aborted on shutdown.
    sweeper::spawn(state.clone());
    webhook_worker::spawn(state.clone());

    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!(%bind, "listening");

    axum::serve(listener, app::router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn seed() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env()?;
    let pool = app::connect_pool(&config).await?;
    sqlx::migrate!().run(&pool).await?;

    let (business_id, token) = auth::seed(&pool).await?;
    // Printed once, to stdout. This is the only time the full key exists.
    println!("business_id {business_id}");
    println!("api_key     {token}");

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
