//! Logging setup and the per-request id middleware.

use axum::{extract::Request, http::HeaderValue, middleware::Next, response::Response};
use tracing::Instrument;
use tracing_subscriber::{fmt, EnvFilter};
use uuid::Uuid;

/// JSON logs to stdout, level from `RUST_LOG`. Shared by the binary and tests.
pub fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,invoice_service=debug"));

    fmt()
        .json()
        .with_env_filter(filter)
        .with_target(true)
        .init();
}

/// Give every request an id: reuse an incoming `x-request-id` if the caller sent
/// one, otherwise mint a UUID v7. It goes on the tracing span for this request
/// and is echoed back on the response header.
pub async fn request_id(req: Request, next: Next) -> Response {
    let id = req
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .unwrap_or_else(|| Uuid::now_v7().to_string());

    let span = tracing::info_span!("request", request_id = %id);
    async move {
        let mut res = next.run(req).await;
        if let Ok(value) = HeaderValue::from_str(&id) {
            res.headers_mut().insert("x-request-id", value);
        }
        res
    }
    .instrument(span)
    .await
}
