//! Liveness and readiness probes.

use axum::{extract::State, http::StatusCode};

use crate::app::AppState;

/// Liveness: the process is up. No dependency checks on purpose — a slow
/// database must not get a healthy process restarted.
pub async fn healthz() -> StatusCode {
    StatusCode::OK
}

/// Readiness: can we actually serve traffic? Fails while the database is
/// unreachable so the load balancer stops sending requests.
pub async fn readyz(State(state): State<AppState>) -> StatusCode {
    match sqlx::query("SELECT 1").execute(&state.pool).await {
        Ok(_) => StatusCode::OK,
        Err(e) => {
            tracing::warn!(error = %e, "readiness check failed");
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}
