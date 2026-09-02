//! Client for the mock PSP. One call — charge — with a hard timeout and the
//! idempotency key forwarded so a retry can never double-charge.

use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct ChargeRequest<'a> {
    card_token: &'a str,
    amount_cents: i64,
    idempotency_key: &'a str,
}

#[derive(Deserialize)]
struct ChargeResponse {
    status: String,
    psp_ref: Option<String>,
    code: Option<String>,
}

/// What Phase 3 (settle) needs to know about the charge.
pub enum ChargeOutcome {
    Succeeded {
        psp_ref: String,
    },
    /// The PSP definitively declined this attempt. Not retryable.
    Failed {
        code: String,
    },
    /// Timeout, connection error, 5xx, or an unusable 2xx body — the attempt
    /// stays `pending` and the sweeper retries later.
    Unavailable {
        detail: String,
    },
}

pub async fn charge(
    http: &reqwest::Client,
    base_url: &str,
    timeout: Duration,
    card_token: &str,
    amount_cents: i64,
    idempotency_key: &str,
) -> ChargeOutcome {
    let url = format!("{}/charge", base_url.trim_end_matches('/'));
    let body = ChargeRequest {
        card_token,
        amount_cents,
        idempotency_key,
    };

    let resp = match http.post(&url).timeout(timeout).json(&body).send().await {
        Ok(r) => r,
        Err(e) => {
            return ChargeOutcome::Unavailable {
                detail: e.to_string(),
            }
        }
    };

    let status = resp.status();
    if status.is_server_error() {
        return ChargeOutcome::Unavailable {
            detail: format!("psp returned {status}"),
        };
    }
    if !status.is_success() {
        // e.g. 422 unknown_token: a real, non-retryable rejection of this attempt.
        let code = resp
            .json::<ChargeResponse>()
            .await
            .ok()
            .and_then(|b| b.code)
            .unwrap_or_else(|| "psp_rejected".to_owned());
        return ChargeOutcome::Failed { code };
    }

    match resp.json::<ChargeResponse>().await {
        Ok(b) if b.status == "succeeded" => match b.psp_ref {
            Some(psp_ref) => ChargeOutcome::Succeeded { psp_ref },
            None => ChargeOutcome::Unavailable {
                detail: "psp succeeded without a psp_ref".to_owned(),
            },
        },
        Ok(b) if b.status == "failed" => ChargeOutcome::Failed {
            code: b.code.unwrap_or_else(|| "declined".to_owned()),
        },
        Ok(b) => ChargeOutcome::Unavailable {
            detail: format!("psp returned unknown status {:?}", b.status),
        },
        Err(e) => ChargeOutcome::Unavailable {
            detail: e.to_string(),
        },
    }
}
