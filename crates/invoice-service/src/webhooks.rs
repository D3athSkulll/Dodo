//! Webhook endpoint registration and the reconciliation read endpoints.
//!
//! Delivery itself is decoupled: domain state changes only *insert* rows into
//! the outbox (see [`crate::outbox`]); [`crate::webhook_worker`] does the HTTP.

use std::net::{IpAddr, SocketAddr};

use axum::{
    extract::{Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    app::AppState,
    auth::Business,
    error::{ApiError, FieldError},
    pagination::{clamp_limit, Cursor, Page},
    secret,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/webhook_endpoints", post(register))
        .route("/webhook_events", get(list_events))
        .route("/webhook_deliveries", get(list_deliveries))
}

// ---- POST /v1/webhook_endpoints -----------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewEndpoint {
    url: String,
}

async fn register(
    State(state): State<AppState>,
    business: Business,
    Json(input): Json<NewEndpoint>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let url = validate_url(&input.url, state.config.webhook_allow_private_targets).await?;
    let secret = secret::hex(32);
    let id = Uuid::now_v7();

    sqlx::query(
        "INSERT INTO webhook_endpoints (id, business_id, url, secret) VALUES ($1, $2, $3, $4)",
    )
    .bind(id)
    .bind(business.0)
    .bind(url.as_str())
    .bind(&secret)
    .execute(&state.pool)
    .await?;

    // The secret is returned exactly once.
    Ok((
        StatusCode::CREATED,
        Json(json!({ "id": id, "url": url.as_str(), "secret": secret })),
    ))
}

/// Best-effort SSRF guard: parse the URL, require http(s), resolve the host, and
/// reject if any resolved address is loopback / private / link-local / the cloud
/// metadata IP. Full protection also needs resolve-then-pin and a no-redirects
/// policy at connect time — documented in DESIGN.md, not built here.
async fn validate_url(raw: &str, allow_private: bool) -> Result<reqwest::Url, ApiError> {
    let url = reqwest::Url::parse(raw).map_err(|_| bad("url", "is not a valid URL"))?;

    if !matches!(url.scheme(), "http" | "https") {
        return Err(bad("url", "must be http or https"));
    }
    let host = url.host_str().ok_or_else(|| bad("url", "has no host"))?;
    let port = url.port_or_known_default().unwrap_or(443);

    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| bad("url", "host does not resolve"))?
        .collect();
    if addrs.is_empty() {
        return Err(bad("url", "host does not resolve"));
    }
    if !allow_private && addrs.iter().any(|a| is_blocked(a.ip())) {
        return Err(bad("url", "resolves to a disallowed address"));
    }
    Ok(url)
}

fn is_blocked(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local() // covers 169.254.0.0/16, incl. 169.254.169.254
                || v4.is_broadcast()
                || v4.is_unspecified()
        }
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified(),
    }
}

// ---- GET /v1/webhook_events (the durable log to replay from) -----------

#[derive(Serialize, sqlx::FromRow)]
struct EventView {
    id: Uuid,
    event_type: String,
    resource_id: Uuid,
    payload: Value,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

#[derive(Deserialize)]
struct EventQuery {
    limit: Option<i64>,
    cursor: Option<String>,
}

async fn list_events(
    State(state): State<AppState>,
    business: Business,
    Query(q): Query<EventQuery>,
) -> Result<Json<Page<EventView>>, ApiError> {
    let limit = clamp_limit(q.limit);
    let cursor = q.cursor.as_deref().map(Cursor::decode).transpose()?;
    let fetch_limit = i64::try_from(limit + 1).unwrap_or(i64::MAX);

    let mut rows: Vec<EventView> = match cursor {
        Some(c) => {
            sqlx::query_as(
                "SELECT id, event_type, resource_id, payload, created_at FROM webhook_events \
                 WHERE business_id = $1 AND (created_at, id) < ($2, $3) \
                 ORDER BY created_at DESC, id DESC LIMIT $4",
            )
            .bind(business.0)
            .bind(c.created_at)
            .bind(c.id)
            .bind(fetch_limit)
            .fetch_all(&state.pool)
            .await?
        }
        None => {
            sqlx::query_as(
                "SELECT id, event_type, resource_id, payload, created_at FROM webhook_events \
                 WHERE business_id = $1 ORDER BY created_at DESC, id DESC LIMIT $2",
            )
            .bind(business.0)
            .bind(fetch_limit)
            .fetch_all(&state.pool)
            .await?
        }
    };

    let next_cursor = if rows.len() > limit {
        rows.truncate(limit);
        rows.last().map(|r| {
            Cursor {
                created_at: r.created_at,
                id: r.id,
            }
            .encode()
        })
    } else {
        None
    };

    Ok(Json(Page {
        data: rows,
        next_cursor,
    }))
}

// ---- GET /v1/webhook_deliveries?status=exhausted ----------------------

#[derive(Serialize, sqlx::FromRow)]
struct DeliveryView {
    id: Uuid,
    event_id: Uuid,
    endpoint_id: Uuid,
    status: String,
    attempts: i32,
    #[serde(with = "time::serde::rfc3339")]
    next_attempt_at: OffsetDateTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
}

#[derive(Deserialize)]
struct DeliveryQuery {
    status: Option<String>,
}

async fn list_deliveries(
    State(state): State<AppState>,
    business: Business,
    Query(q): Query<DeliveryQuery>,
) -> Result<Json<Value>, ApiError> {
    let status = match q.status.as_deref() {
        None => None,
        Some(s @ ("pending" | "inflight" | "delivered" | "exhausted")) => Some(s.to_owned()),
        Some(_) => return Err(bad("status", "unknown delivery status")),
    };

    // webhook_deliveries has no business_id; scope through the event.
    let rows: Vec<DeliveryView> = sqlx::query_as(
        "SELECT d.id, d.event_id, d.endpoint_id, d.status, d.attempts, d.next_attempt_at, d.last_error \
         FROM webhook_deliveries d JOIN webhook_events e ON e.id = d.event_id \
         WHERE e.business_id = $1 AND ($2::text IS NULL OR d.status = $2) \
         ORDER BY d.next_attempt_at DESC LIMIT 200",
    )
    .bind(business.0)
    .bind(status)
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(json!({ "data": rows })))
}

// ---- helpers --------------------------------------------------------

fn bad(field: &str, message: &str) -> ApiError {
    ApiError::Validation(vec![FieldError {
        field: field.to_owned(),
        message: message.to_owned(),
    }])
}

#[cfg(test)]
mod tests {
    use super::is_blocked;

    #[test]
    fn blocks_internal_addresses() {
        for ip in [
            "127.0.0.1",
            "10.0.0.5",
            "192.168.1.1",
            "172.16.0.1",
            "169.254.169.254",
            "0.0.0.0",
            "::1",
        ] {
            assert!(is_blocked(ip.parse().unwrap()), "should block {ip}");
        }
    }

    #[test]
    fn allows_public_addresses() {
        for ip in ["1.1.1.1", "93.184.216.34", "2606:4700:4700::1111"] {
            assert!(!is_blocked(ip.parse().unwrap()), "should allow {ip}");
        }
    }
}
