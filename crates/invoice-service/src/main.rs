//! Binary entrypoint: load config, connect to Postgres, run migrations, serve.

use invoice_service::{app, config::Config, telemetry};

#[tokio::main]
async fn main() {
    telemetry::init_tracing();

    if let Err(e) = run().await {
        tracing::error!(error = %e, "startup failed");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env()?;
    let pool = app::connect_pool(&config).await?;

    // Single service, single writer, so migrations run on the way up.
    sqlx::migrate!().run(&pool).await?;

    let bind = config.bind_addr.clone();
    let state = app::build_state(pool, config);

    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!(%bind, "listening");

    axum::serve(listener, app::router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;

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
