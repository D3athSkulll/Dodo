//! Mock payment processor, as a library so the integration tests can run it
//! in-process. See `main.rs` for the standalone binary.
//!
//! One route, `POST /charge`. The outcome is decided entirely by the card token:
//!
//! | token                    | behaviour                                            |
//! |--------------------------|-----------------------------------------------------|
//! | `tok_success`            | short delay, then `{status:"succeeded", psp_ref}`    |
//! | `tok_insufficient_funds` | short delay, then `{status:"failed", code}`          |
//! | `tok_card_declined`      | short delay, then `{status:"failed", code}`          |
//! | `tok_timeout`            | long delay, then succeeds — the caller must not hang |
//! | `tok_network_error`      | always HTTP 500, immediately                         |
//! | anything else            | HTTP 422 `{code:"unknown_token"}`                    |
//!
//! Idempotent on `idempotency_key`: a repeated key returns the stored outcome
//! without re-running the delay or re-deciding. A 500 / 422 is not an outcome, so
//! nothing is stored for those.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::time::sleep;
use uuid::Uuid;

#[derive(Clone)]
pub struct MockConfig {
    pub fast_delay: Duration,
    pub timeout_delay: Duration,
}

impl Default for MockConfig {
    fn default() -> Self {
        Self {
            fast_delay: Duration::from_millis(100),
            timeout_delay: Duration::from_secs(30),
        }
    }
}

pub fn router(config: MockConfig) -> Router {
    let state = AppState {
        charges: Arc::new(Mutex::new(HashMap::new())),
        fast_delay: config.fast_delay,
        timeout_delay: config.timeout_delay,
    };
    Router::new()
        .route("/charge", post(charge))
        .route("/_debug/charges", get(debug_charges))
        .with_state(state)
}

#[derive(Clone)]
struct AppState {
    charges: Arc<Mutex<HashMap<String, StoredCharge>>>,
    fast_delay: Duration,
    timeout_delay: Duration,
}

#[derive(Clone)]
struct StoredCharge {
    idempotency_key: String,
    card_token: String,
    status: &'static str,
    psp_ref: Option<String>,
    code: Option<String>,
}

#[derive(Deserialize)]
struct ChargeRequest {
    card_token: String,
    #[allow(dead_code)] // accepted, but the mock's behaviour ignores it
    amount_cents: i64,
    idempotency_key: String,
}

async fn charge(State(state): State<AppState>, Json(req): Json<ChargeRequest>) -> Response {
    // Idempotent replay — same key, same answer, no delay.
    if let Some(existing) = state
        .charges
        .lock()
        .unwrap()
        .get(&req.idempotency_key)
        .cloned()
    {
        return (StatusCode::OK, Json(charge_body(&existing))).into_response();
    }

    let (delay, decided) = match req.card_token.as_str() {
        "tok_success" => (state.fast_delay, Ok(succeeded())),
        "tok_insufficient_funds" => (state.fast_delay, Ok(failed("insufficient_funds"))),
        "tok_card_declined" => (state.fast_delay, Ok(failed("card_declined"))),
        "tok_timeout" => (state.timeout_delay, Ok(succeeded())),
        "tok_network_error" => (
            Duration::ZERO,
            Err((StatusCode::INTERNAL_SERVER_ERROR, "network_error")),
        ),
        _ => (
            Duration::ZERO,
            Err((StatusCode::UNPROCESSABLE_ENTITY, "unknown_token")),
        ),
    };

    let (status, psp_ref, code) = match decided {
        Ok(outcome) => outcome,
        // A 500 / 422 is not a completed charge — store nothing so a retry gets a
        // fresh decision.
        Err((code, message)) => return (code, Json(json!({ "code": message }))).into_response(),
    };

    // Run the delay + store in a detached task: a real processor finishes a
    // charge even if the caller hangs up, and `tok_timeout` depends on that.
    let charges = state.charges.clone();
    let key = req.idempotency_key.clone();
    let card_token = req.card_token.clone();
    let stored = tokio::spawn(async move {
        sleep(delay).await;
        charges
            .lock()
            .unwrap()
            .entry(key.clone())
            .or_insert(StoredCharge {
                idempotency_key: key,
                card_token,
                status,
                psp_ref,
                code,
            })
            .clone()
    })
    .await
    .expect("charge task should not panic");

    (StatusCode::OK, Json(charge_body(&stored))).into_response()
}

async fn debug_charges(State(state): State<AppState>) -> Json<Vec<serde_json::Value>> {
    let charges = state.charges.lock().unwrap();
    Json(
        charges
            .values()
            .map(|c| {
                json!({
                    "idempotency_key": c.idempotency_key,
                    "card_token": c.card_token,
                    "psp_ref": c.psp_ref,
                    "status": c.status,
                })
            })
            .collect(),
    )
}

type Decided = (&'static str, Option<String>, Option<String>);

fn succeeded() -> Decided {
    ("succeeded", Some(Uuid::now_v7().to_string()), None)
}

fn failed(code: &str) -> Decided {
    ("failed", None, Some(code.to_owned()))
}

#[derive(Serialize)]
struct ChargeBody {
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    psp_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<String>,
}

fn charge_body(c: &StoredCharge) -> ChargeBody {
    ChargeBody {
        status: c.status,
        psp_ref: c.psp_ref.clone(),
        code: c.code.clone(),
    }
}
