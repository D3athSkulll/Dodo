//! Paying an invoice.
//!
//! Three phases, and **no database transaction ever wraps the PSP HTTP call**:
//!
//! 1. **claim** — one short tx: lock the invoice row, insert a `pending`
//!    `payment_attempts` row. Two unique constraints decide the outcome.
//! 2. **call the PSP** — no tx open.
//! 3. **settle** — one short tx: record the result, move the invoice if it
//!    succeeded, write the webhook event.
//!
//! A crash between phases leaves a `pending` attempt that the reconciliation
//! sweeper ([`crate::sweeper`]) finishes — re-submitting the same idempotent
//! charge, so the customer is charged at most once.

use axum::{
    extract::{Path, State},
    http::{header::RETRY_AFTER, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    app::AppState,
    auth::Business,
    error::{ApiError, FieldError},
    invoice_state::{transition_invoice, InvoiceState},
    outbox,
    psp::{self, ChargeOutcome},
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/invoices/{id}/pay", post(pay))
        .route("/invoices/{id}/payments", get(list_for_invoice))
        .route("/payments/{id}", get(get_one))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PayRequest {
    card_token: String,
}

// ---- POST /v1/invoices/:id/pay --------------------------------------------

async fn pay(
    State(state): State<AppState>,
    business: Business,
    Path(invoice_id): Path<Uuid>,
    headers: HeaderMap,
    Json(req): Json<PayRequest>,
) -> Result<Response, ApiError> {
    let idem_key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            ApiError::Validation(vec![FieldError {
                field: "Idempotency-Key".to_owned(),
                message: "header is required".to_owned(),
            }])
        })?
        .to_owned();

    let fingerprint = fingerprint(invoice_id, &req.card_token);

    // Phase 1
    let attempt = match claim(
        &state.pool,
        business.0,
        invoice_id,
        &idem_key,
        &req.card_token,
        &fingerprint,
    )
    .await?
    {
        Claim::Proceed(a) => a,
        Claim::Replay(response) => return Ok(response),
    };

    // Phase 2 — no tx open
    let outcome = psp::charge(
        &state.http,
        &state.config.psp_base_url,
        state.config.psp_timeout,
        &req.card_token,
        attempt.amount_cents,
        &idem_key,
    )
    .await;

    // Phase 3
    settle(&state.pool, business.0, invoice_id, attempt.id, outcome).await
}

struct ClaimedAttempt {
    id: Uuid,
    amount_cents: i64,
}

enum Claim {
    /// A fresh `pending` row — go call the PSP.
    Proceed(ClaimedAttempt),
    /// Nothing to do: a terminal replay, an in-flight `202`, etc.
    Replay(Response),
}

/// Phase 1. One short transaction; no external I/O.
async fn claim(
    pool: &PgPool,
    business_id: Uuid,
    invoice_id: Uuid,
    idem_key: &str,
    card_token: &str,
    fingerprint: &[u8],
) -> Result<Claim, ApiError> {
    let mut tx = pool.begin().await?;

    let invoice: Option<(String, i64)> = sqlx::query_as(
        "SELECT state, total_cents FROM invoices \
         WHERE id = $1 AND business_id = $2 FOR UPDATE",
    )
    .bind(invoice_id)
    .bind(business_id)
    .fetch_optional(&mut *tx)
    .await?;

    let (state, total_cents) = match invoice {
        None => {
            tx.rollback().await?;
            return Err(ApiError::NotFound);
        }
        Some(row) => row,
    };

    if state != "open" {
        tx.rollback().await?;
        // If this exact key already paid the invoice, replay that result.
        if state == "paid" {
            if let Some(existing) = load_by_key(pool, business_id, idem_key).await? {
                if existing.fingerprint == fingerprint && existing.status == "succeeded" {
                    return Ok(Claim::Replay(terminal_response(&existing)));
                }
            }
        }
        return Err(ApiError::InvoiceNotOpen { state });
    }

    let attempt_id = Uuid::now_v7();
    let insert = sqlx::query(
        "INSERT INTO payment_attempts \
           (id, invoice_id, business_id, idempotency_key, card_token, request_fingerprint, \
            status, amount_cents) \
         VALUES ($1, $2, $3, $4, $5, $6, 'pending', $7)",
    )
    .bind(attempt_id)
    .bind(invoice_id)
    .bind(business_id)
    .bind(idem_key)
    .bind(card_token)
    .bind(fingerprint)
    .bind(total_cents)
    .execute(&mut *tx)
    .await;

    match insert {
        Ok(_) => {
            tx.commit().await?;
            Ok(Claim::Proceed(ClaimedAttempt {
                id: attempt_id,
                amount_cents: total_cents,
            }))
        }
        Err(e) => {
            tx.rollback().await?;
            resolve_conflict(pool, business_id, idem_key, fingerprint, &e).await
        }
    }
}

/// The `INSERT` hit one of two unique constraints — work out which and what it
/// means for the caller.
async fn resolve_conflict(
    pool: &PgPool,
    business_id: Uuid,
    idem_key: &str,
    fingerprint: &[u8],
    err: &sqlx::Error,
) -> Result<Claim, ApiError> {
    let constraint = err.as_database_error().and_then(|d| d.constraint());

    match constraint {
        // A different key already has a pending charge for this invoice.
        Some("one_pending_payment_per_invoice") => Err(ApiError::PaymentInProgress),

        // This exact key was used before.
        Some(name) if name.contains("idempotency_key") => {
            let existing = load_by_key(pool, business_id, idem_key)
                .await?
                .ok_or(ApiError::Internal)?; // just violated its own unique constraint

            if existing.fingerprint != fingerprint {
                return Err(ApiError::IdempotencyKeyConflict);
            }
            Ok(Claim::Replay(match existing.status.as_str() {
                "pending" => in_flight_response(existing.id),
                _ => terminal_response(&existing),
            }))
        }

        _ => Err(err_to_api(err)),
    }
}

/// Phase 3. One short transaction.
async fn settle(
    pool: &PgPool,
    business_id: Uuid,
    invoice_id: Uuid,
    attempt_id: Uuid,
    outcome: ChargeOutcome,
) -> Result<Response, ApiError> {
    let mut tx = pool.begin().await?;

    match outcome {
        ChargeOutcome::Succeeded { psp_ref } => {
            let updated = sqlx::query(
                "UPDATE payment_attempts \
                 SET status = 'succeeded', psp_ref = $1, updated_at = now() \
                 WHERE id = $2 AND status = 'pending'",
            )
            .bind(&psp_ref)
            .bind(attempt_id)
            .execute(&mut *tx)
            .await?;

            if updated.rows_affected() == 0 {
                // A concurrent settle (handler vs. sweeper) already finished it.
                tx.rollback().await?;
                let a = load_by_id(pool, business_id, attempt_id)
                    .await?
                    .ok_or(ApiError::Internal)?;
                return Ok(terminal_response(&a));
            }

            // At most one `open -> paid`: a late winner just no-ops here.
            match transition_invoice(
                &mut tx,
                invoice_id,
                business_id,
                &[InvoiceState::Open],
                InvoiceState::Paid,
            )
            .await
            {
                Ok(()) | Err(ApiError::InvalidStateTransition { .. }) => {}
                Err(e) => return Err(e),
            }

            outbox::emit(
                &mut tx,
                business_id,
                "invoice.paid",
                invoice_id,
                json!({
                    "type": "invoice.paid",
                    "invoice": { "id": invoice_id, "state": "paid" },
                    "payment": { "id": attempt_id, "psp_ref": psp_ref },
                }),
            )
            .await?;
            tx.commit().await?;

            Ok((
                StatusCode::OK,
                Json(json!({
                    "attempt": { "id": attempt_id, "status": "succeeded", "psp_ref": psp_ref },
                    "invoice": { "id": invoice_id, "state": "paid" },
                })),
            )
                .into_response())
        }

        ChargeOutcome::Failed { code } => {
            sqlx::query(
                "UPDATE payment_attempts \
                 SET status = 'failed', failure_code = $1, updated_at = now() \
                 WHERE id = $2 AND status = 'pending'",
            )
            .bind(&code)
            .bind(attempt_id)
            .execute(&mut *tx)
            .await?;

            // Invoice stays 'open' — the business can retry with a new key.
            outbox::emit(
                &mut tx,
                business_id,
                "invoice.payment_failed",
                invoice_id,
                json!({
                    "type": "invoice.payment_failed",
                    "invoice": { "id": invoice_id, "state": "open" },
                    "payment": { "id": attempt_id, "failure_code": code },
                }),
            )
            .await?;
            tx.commit().await?;

            Ok((
                StatusCode::PAYMENT_REQUIRED,
                Json(json!({
                    "attempt": { "id": attempt_id, "status": "failed", "failure_code": code },
                })),
            )
                .into_response())
        }

        ChargeOutcome::Unavailable { detail } => {
            // Attempt stays 'pending'; the sweeper picks it up.
            sqlx::query(
                "UPDATE payment_attempts SET last_error = $1, updated_at = now() \
                 WHERE id = $2 AND status = 'pending'",
            )
            .bind(&detail)
            .bind(attempt_id)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;

            Ok((
                StatusCode::ACCEPTED,
                [(RETRY_AFTER, "5")],
                Json(json!({ "attempt_id": attempt_id, "status": "pending" })),
            )
                .into_response())
        }
    }
}

/// Used by the sweeper: same as [`settle`] but the caller doesn't want a
/// `Response`.
pub async fn settle_from_sweeper(
    pool: &PgPool,
    business_id: Uuid,
    invoice_id: Uuid,
    attempt_id: Uuid,
    outcome: ChargeOutcome,
) -> Result<(), ApiError> {
    settle(pool, business_id, invoice_id, attempt_id, outcome)
        .await
        .map(|_| ())
}

// ---- read model ---------------------------------------------------------

#[derive(Serialize, sqlx::FromRow)]
struct PaymentView {
    id: Uuid,
    invoice_id: Uuid,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    psp_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_code: Option<String>,
    amount_cents: i64,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

const PAYMENT_COLUMNS: &str =
    "id, invoice_id, status, psp_ref, failure_code, amount_cents, created_at, updated_at";

async fn get_one(
    State(state): State<AppState>,
    business: Business,
    Path(id): Path<Uuid>,
) -> Result<Json<PaymentView>, ApiError> {
    let view: Option<PaymentView> = sqlx::query_as(&format!(
        "SELECT {PAYMENT_COLUMNS} FROM payment_attempts WHERE id = $1 AND business_id = $2"
    ))
    .bind(id)
    .bind(business.0)
    .fetch_optional(&state.pool)
    .await?;

    view.map(Json).ok_or(ApiError::NotFound)
}

async fn list_for_invoice(
    State(state): State<AppState>,
    business: Business,
    Path(invoice_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let rows: Vec<PaymentView> = sqlx::query_as(&format!(
        "SELECT {PAYMENT_COLUMNS} FROM payment_attempts \
         WHERE invoice_id = $1 AND business_id = $2 ORDER BY created_at"
    ))
    .bind(invoice_id)
    .bind(business.0)
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(json!({ "data": rows })))
}

// ---- helpers ----------------------------------------------------------

/// Only the payment-relevant fields — no headers, no request id.
fn fingerprint(invoice_id: Uuid, card_token: &str) -> Vec<u8> {
    let mut h = Sha256::new();
    h.update(invoice_id.as_bytes());
    h.update(b"|");
    h.update(card_token.as_bytes());
    h.finalize().to_vec()
}

#[derive(sqlx::FromRow)]
struct ExistingAttempt {
    id: Uuid,
    status: String,
    psp_ref: Option<String>,
    failure_code: Option<String>,
    #[sqlx(rename = "request_fingerprint")]
    fingerprint: Vec<u8>,
    invoice_id: Uuid,
}

async fn load_by_key(
    pool: &PgPool,
    business_id: Uuid,
    idem_key: &str,
) -> Result<Option<ExistingAttempt>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, status, psp_ref, failure_code, request_fingerprint, invoice_id \
         FROM payment_attempts WHERE business_id = $1 AND idempotency_key = $2",
    )
    .bind(business_id)
    .bind(idem_key)
    .fetch_optional(pool)
    .await
}

async fn load_by_id(
    pool: &PgPool,
    business_id: Uuid,
    id: Uuid,
) -> Result<Option<ExistingAttempt>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, status, psp_ref, failure_code, request_fingerprint, invoice_id \
         FROM payment_attempts WHERE business_id = $1 AND id = $2",
    )
    .bind(business_id)
    .bind(id)
    .fetch_optional(pool)
    .await
}

fn terminal_response(a: &ExistingAttempt) -> Response {
    match a.status.as_str() {
        "succeeded" => (
            StatusCode::OK,
            Json(json!({
                "attempt": { "id": a.id, "status": "succeeded", "psp_ref": a.psp_ref },
                "invoice": { "id": a.invoice_id, "state": "paid" },
            })),
        )
            .into_response(),
        _ => (
            StatusCode::PAYMENT_REQUIRED,
            Json(json!({
                "attempt": { "id": a.id, "status": "failed", "failure_code": a.failure_code },
            })),
        )
            .into_response(),
    }
}

fn in_flight_response(attempt_id: Uuid) -> Response {
    (
        StatusCode::ACCEPTED,
        [(RETRY_AFTER, "5")],
        Json(json!({ "attempt_id": attempt_id, "status": "pending" })),
    )
        .into_response()
}

fn err_to_api(e: &sqlx::Error) -> ApiError {
    tracing::error!(error = %e, "payment claim database error");
    ApiError::Internal
}
