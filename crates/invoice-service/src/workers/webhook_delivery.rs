//! Webhook delivery worker.
//!
//! Claim / lease, never a lock during the HTTP POST:
//!
//! 1. one tx: `SELECT ... FOR UPDATE SKIP LOCKED` due rows, mark them `inflight`
//!    with a lease, commit (lock released).
//! 2. no tx: sign and POST each one.
//! 3. one tx: record the outcome — `delivered`, `pending` with backoff, or
//!    `exhausted`.
//!
//! A crashed worker's `inflight` rows are reclaimed once `lease_until` passes.
//! Delivery is at-least-once by design; receivers dedupe on `Dodo-Event-Id`.

use std::fmt::Write as _;
use std::time::Duration;

use hmac::{Hmac, Mac};
use sha2::Sha256;
use sqlx::PgPool;
use time::OffsetDateTime;
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::state::AppState;

type HmacSha256 = Hmac<Sha256>;

const POST_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_ATTEMPTS: i32 = 6;
const BATCH: i64 = 50;

pub fn spawn(state: AppState) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(state.config.webhook_worker_interval);
        loop {
            ticker.tick().await;
            if let Err(e) = deliver_batch(&state).await {
                tracing::error!(error = %e, "webhook delivery batch failed");
            }
        }
    })
}

#[derive(sqlx::FromRow)]
struct Claimed {
    id: Uuid,
    event_id: Uuid,
    attempts: i32,
    #[sqlx(rename = "payload")]
    body: String,
    url: String,
    secret: String,
}

async fn deliver_batch(state: &AppState) -> Result<(), sqlx::Error> {
    let lease_secs = i64::try_from(state.config.webhook_lease.as_secs()).unwrap_or(30);

    // 1. claim + lease
    let mut tx = state.pool.begin().await?;
    let claimed: Vec<Claimed> = sqlx::query_as(
        "SELECT d.id, d.event_id, d.attempts, e.payload::text AS payload, ep.url, ep.secret \
         FROM webhook_deliveries d \
         JOIN webhook_events e   ON e.id = d.event_id \
         JOIN webhook_endpoints ep ON ep.id = d.endpoint_id \
         WHERE d.status IN ('pending', 'inflight') \
           AND d.next_attempt_at <= now() \
           AND (d.lease_until IS NULL OR d.lease_until < now()) \
           AND ep.active \
         ORDER BY d.next_attempt_at \
         FOR UPDATE SKIP LOCKED \
         LIMIT $1",
    )
    .bind(BATCH)
    .fetch_all(&mut *tx)
    .await?;

    for c in &claimed {
        sqlx::query(
            "UPDATE webhook_deliveries \
             SET status = 'inflight', lease_until = now() + make_interval(secs => $1) \
             WHERE id = $2",
        )
        .bind(lease_secs)
        .bind(c.id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    if claimed.is_empty() {
        return Ok(());
    }
    tracing::debug!(count = claimed.len(), "delivering webhooks");

    // 2. POST each, no tx open
    for c in claimed {
        let ts = OffsetDateTime::now_utc().unix_timestamp();
        let signature = sign(&c.secret, ts, &c.body);

        let result = state
            .http
            .post(&c.url)
            .timeout(POST_TIMEOUT)
            .header("content-type", "application/json")
            .header("Dodo-Signature", format!("t={ts},v1={signature}"))
            .header("Dodo-Event-Id", c.event_id.to_string())
            .body(c.body.clone())
            .send()
            .await;

        // 3. record the outcome
        if let Err(e) = record(&state.pool, &c, classify(result)).await {
            tracing::error!(error = %e, delivery = %c.id, "recording webhook outcome failed");
        }
    }

    Ok(())
}

enum Outcome {
    Delivered,
    /// Worth retrying: timeout, connection error, 5xx, 408, 429.
    Retry(String),
    /// A permanent 4xx — stop trying.
    Permanent(String),
}

fn classify(result: Result<reqwest::Response, reqwest::Error>) -> Outcome {
    let resp = match result {
        Ok(r) => r,
        Err(e) => return Outcome::Retry(e.to_string()),
    };

    let status = resp.status();
    if status.is_success() {
        Outcome::Delivered
    } else if status.is_server_error()
        || status == reqwest::StatusCode::REQUEST_TIMEOUT
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        Outcome::Retry(format!("http {status}"))
    } else {
        Outcome::Permanent(format!("http {status}"))
    }
}

async fn record(pool: &PgPool, c: &Claimed, outcome: Outcome) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;

    match outcome {
        Outcome::Delivered => {
            sqlx::query(
                "UPDATE webhook_deliveries \
                 SET status = 'delivered', delivered_at = now(), lease_until = NULL WHERE id = $1",
            )
            .bind(c.id)
            .execute(&mut *tx)
            .await?;
        }
        Outcome::Permanent(err) => {
            sqlx::query(
                "UPDATE webhook_deliveries \
                 SET status = 'exhausted', last_error = $1, lease_until = NULL WHERE id = $2",
            )
            .bind(err)
            .bind(c.id)
            .execute(&mut *tx)
            .await?;
        }
        Outcome::Retry(err) => {
            let attempts = c.attempts + 1;
            match backoff(attempts) {
                None => {
                    sqlx::query(
                        "UPDATE webhook_deliveries \
                         SET status = 'exhausted', attempts = $1, last_error = $2, lease_until = NULL \
                         WHERE id = $3",
                    )
                    .bind(attempts)
                    .bind(err)
                    .bind(c.id)
                    .execute(&mut *tx)
                    .await?;
                }
                Some(delay) => {
                    let secs = i64::try_from(delay.as_secs()).unwrap_or(i64::MAX);
                    sqlx::query(
                        "UPDATE webhook_deliveries \
                         SET status = 'pending', attempts = $1, last_error = $2, lease_until = NULL, \
                             next_attempt_at = now() + make_interval(secs => $3) \
                         WHERE id = $4",
                    )
                    .bind(attempts)
                    .bind(err)
                    .bind(secs)
                    .bind(c.id)
                    .execute(&mut *tx)
                    .await?;
                }
            }
        }
    }

    tx.commit().await
}

/// Delay before attempt N+1, given N attempts have now been made.
/// `1m, 5m, 30m, 2h, 6h`, then exhausted (6 attempts, ~8h46m total budget).
/// Jitter would be a production improvement.
fn backoff(attempts: i32) -> Option<Duration> {
    let minutes = match attempts {
        1 => 1,
        2 => 5,
        3 => 30,
        4 => 120,
        5 => 360,
        _ => return None, // >= MAX_ATTEMPTS
    };
    debug_assert!(attempts < MAX_ATTEMPTS);
    Some(Duration::from_secs(minutes * 60))
}

/// `hex(hmac_sha256(secret, "<ts>.<body>"))`.
fn sign(secret: &str, ts: i64, body: &str) -> String {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts a key of any length");
    mac.update(format!("{ts}.{body}").as_bytes());

    let bytes = mac.finalize().into_bytes();
    let mut hex = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(hex, "{b:02x}");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::{backoff, sign, MAX_ATTEMPTS};

    #[test]
    fn backoff_schedule_then_exhaust() {
        let mins = |n| backoff(n).map(|d| d.as_secs() / 60);
        assert_eq!(mins(1), Some(1));
        assert_eq!(mins(2), Some(5));
        assert_eq!(mins(3), Some(30));
        assert_eq!(mins(4), Some(120));
        assert_eq!(mins(5), Some(360));
        assert_eq!(backoff(MAX_ATTEMPTS), None);
    }

    #[test]
    fn signature_is_stable_and_key_sensitive() {
        assert_eq!(sign("s", 100, "body"), sign("s", 100, "body"));
        assert_ne!(sign("s", 100, "body"), sign("s", 101, "body"));
        assert_ne!(sign("s", 100, "body"), sign("t", 100, "body"));
    }
}
