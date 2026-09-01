//! One error type for the whole API, one JSON shape on the wire:
//! `{"error": {"code", "message", "details"?}}`.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use serde_json::json;

#[derive(Debug, Serialize)]
pub struct FieldError {
    pub field: String,
    pub message: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("unauthorized")]
    Unauthorized,
    #[error("not found")]
    NotFound,
    #[error("validation failed")]
    Validation(Vec<FieldError>),
    #[error("invalid state transition from {from} to {to}")]
    InvalidStateTransition { from: String, to: String },
    #[error("idempotency key already used with a different request")]
    IdempotencyKeyConflict,
    #[error("a payment is already in progress for this invoice")]
    PaymentInProgress,
    #[error("invoice is not open (state: {state})")]
    InvoiceNotOpen { state: String },
    #[error("payment processor unavailable")]
    PspUnavailable,
    #[error("internal error")]
    Internal,
}

impl ApiError {
    fn status_and_code(&self) -> (StatusCode, &'static str) {
        use ApiError::*;
        match self {
            Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            NotFound => (StatusCode::NOT_FOUND, "not_found"),
            // 422, not 400: the request parsed fine, it is semantically rejected.
            Validation(_) => (StatusCode::UNPROCESSABLE_ENTITY, "validation_error"),
            InvalidStateTransition { .. } => (StatusCode::CONFLICT, "invalid_state_transition"),
            IdempotencyKeyConflict => (StatusCode::CONFLICT, "idempotency_key_conflict"),
            PaymentInProgress => (StatusCode::CONFLICT, "payment_in_progress"),
            InvoiceNotOpen { .. } => (StatusCode::CONFLICT, "invoice_not_open"),
            PspUnavailable => (StatusCode::BAD_GATEWAY, "psp_unavailable"),
            Internal => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = self.status_and_code();

        let (message, details) = match &self {
            // The client gets a generic message; the real cause is only logged.
            ApiError::Internal => {
                tracing::error!(error = %self, "internal error");
                ("internal error".to_owned(), None)
            }
            ApiError::Validation(fields) => (
                "one or more fields are invalid".to_owned(),
                Some(json!(fields)),
            ),
            other => (other.to_string(), None),
        };

        let mut body = json!({ "code": code, "message": message });
        if let Some(details) = details {
            body["details"] = details;
        }
        (status, Json(json!({ "error": body }))).into_response()
    }
}

/// A database error reaching a handler is a bug or an outage — never something
/// the client can act on, so it collapses to an opaque 500.
impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        tracing::error!(error = %e, "database error");
        ApiError::Internal
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    #[test]
    fn statuses_match_the_spec() {
        assert_eq!(
            ApiError::Unauthorized.status_and_code().0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            ApiError::NotFound.status_and_code().0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            ApiError::Validation(vec![]).status_and_code().0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            ApiError::PaymentInProgress.status_and_code().0,
            StatusCode::CONFLICT
        );
        assert_eq!(
            ApiError::PspUnavailable.status_and_code().0,
            StatusCode::BAD_GATEWAY
        );
    }
}
