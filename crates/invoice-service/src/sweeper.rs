//! Reconciliation sweeper.
//!
//! A `pending` payment attempt means the PSP call timed out, errored, or the
//! service crashed before Phase 3. This task periodically re-submits the same
//! idempotent charge and settles the result. If an attempt has been stuck past
//! `PAYMENT_PENDING_MAX_AGE_SECONDS` it is failed with `psp_unreachable` and the
//! invoice stays `open` so the business can retry with a new key.
//!
//! No external I/O happens inside the claiming transaction — rows are claimed
//! with `FOR UPDATE SKIP LOCKED`, the lock is released, then the charge is made.

use std::time::Duration;

use serde_json::json;
use sqlx::PgPool;
use time::OffsetDateTime;
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::{app::AppState, payments, psp};

/// Only attempts idle at least this long are swept, so a request still in its
/// own Phase 3 is never touched.
const MIN_IDLE: Duration = Duration::from_secs(3);

pub fn spawn(state: AppState) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(state.config.payment_sweep_interval);
        loop {
            ticker.tick().await;
            if let Err(e) = sweep_once(&state).await {
                tracing::error!(error = %e, "payment sweep failed");
            }
        }
    })
}

#[derive(sqlx::FromRow)]
struct Stale {
    id: Uuid,
    invoice_id: Uuid,
    business_id: Uuid,
    idempotency_key: String,
    card_token: String,
    amount_cents: i64,
    created_at: OffsetDateTime,
}

async fn sweep_once(state: &AppState) -> Result<(), sqlx::Error> {
    let idle_secs = i64::try_from(MIN_IDLE.as_secs()).unwrap_or(3);

    // Claim, bump updated_at so it is not re-claimed straight away, commit.
    let mut tx = state.pool.begin().await?;
    let claimed: Vec<Stale> = sqlx::query_as(
        "SELECT id, invoice_id, business_id, idempotency_key, card_token, amount_cents, created_at \
         FROM payment_attempts \
         WHERE status = 'pending' AND updated_at < now() - make_interval(secs => $1) \
         ORDER BY updated_at \
         FOR UPDATE SKIP LOCKED \
         LIMIT 20",
    )
    .bind(idle_secs)
    .fetch_all(&mut *tx)
    .await?;

    for a in &claimed {
        sqlx::query("UPDATE payment_attempts SET updated_at = now() WHERE id = $1")
            .bind(a.id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;

    if claimed.is_empty() {
        return Ok(());
    }
    tracing::debug!(count = claimed.len(), "sweeping pending payment attempts");

    let max_age = state.config.payment_pending_max_age;
    for a in claimed {
        let outcome = psp::charge(
            &state.http,
            &state.config.psp_base_url,
            state.config.psp_timeout,
            &a.card_token,
            a.amount_cents,
            &a.idempotency_key,
        )
        .await;

        let expired = age(a.created_at) > max_age;
        match outcome {
            psp::ChargeOutcome::Unavailable { detail } if expired => {
                if let Err(e) = give_up(&state.pool, &a, &detail).await {
                    tracing::error!(error = %e, attempt = %a.id, "failed to give up on stuck attempt");
                }
            }
            other => {
                if let Err(e) = payments::settle_from_sweeper(
                    &state.pool,
                    a.business_id,
                    a.invoice_id,
                    a.id,
                    other,
                )
                .await
                {
                    tracing::error!(error = %e, attempt = %a.id, "sweeper settle failed");
                }
            }
        }
    }

    Ok(())
}

/// Past the max age and still unreachable — fail the attempt, leave the invoice
/// `open`.
async fn give_up(pool: &PgPool, a: &Stale, detail: &str) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;

    let updated = sqlx::query(
        "UPDATE payment_attempts \
         SET status = 'failed', failure_code = 'psp_unreachable', last_error = $1, updated_at = now() \
         WHERE id = $2 AND status = 'pending'",
    )
    .bind(detail)
    .bind(a.id)
    .execute(&mut *tx)
    .await?;

    if updated.rows_affected() == 1 {
        crate::outbox::emit(
            &mut tx,
            a.business_id,
            "invoice.payment_failed",
            a.invoice_id,
            json!({
                "type": "invoice.payment_failed",
                "invoice": { "id": a.invoice_id, "state": "open" },
                "payment": { "id": a.id, "failure_code": "psp_unreachable" },
            }),
        )
        .await?;
    }

    tx.commit().await
}

fn age(created_at: OffsetDateTime) -> Duration {
    (OffsetDateTime::now_utc() - created_at)
        .try_into()
        .unwrap_or_default()
}
