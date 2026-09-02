//! HTTP layer: one module per resource, and the router that ties them together.

use axum::{middleware::from_fn_with_state, routing::get, Router};

use crate::{auth, state::AppState, telemetry};

pub mod customers;
pub mod health;
pub mod invoices;
pub mod payments;
pub mod webhooks;

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
