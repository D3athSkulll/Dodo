//! Invoice state machine.
//!
//! ```text
//!            ┌────────┐
//!            │  open  │  ← created here, the only entry point
//!            └───┬────┘
//!     payment ok │ void │ mark-uncollectible
//!         ┌──────┼──────┐
//!         ▼      ▼      ▼
//!      ┌──────┐┌────┐┌───────────────┐
//!      │ paid ││void││ uncollectible │   all terminal
//!      └──────┘└────┘└───────────────┘
//! ```
//!
//! No transition is reversible. Invalid transitions are rejected by a
//! conditional `UPDATE` (see [`transition_invoice`]) — never a trigger, never a
//! read-then-write.

use serde::Serialize;
use uuid::Uuid;

use crate::error::ApiError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InvoiceState {
    Open,
    Paid,
    Void,
    Uncollectible,
}

impl InvoiceState {
    pub fn as_str(self) -> &'static str {
        match self {
            InvoiceState::Open => "open",
            InvoiceState::Paid => "paid",
            InvoiceState::Void => "void",
            InvoiceState::Uncollectible => "uncollectible",
        }
    }

    /// `open` is the only state anything can leave.
    pub fn is_terminal(self) -> bool {
        !matches!(self, InvoiceState::Open)
    }

    /// The whole machine, in one place. A no-op (`open -> open`) is not a
    /// transition. This mirrors the conditional `UPDATE` in [`transition_invoice`]
    /// and exists so the machine can be tested without a database.
    pub fn can_transition_to(self, target: InvoiceState) -> bool {
        use InvoiceState::{Open, Paid, Uncollectible, Void};
        matches!(
            (self, target),
            (Open, Paid) | (Open, Void) | (Open, Uncollectible)
        )
    }
}

/// Move an invoice from one of `from` to `to`, atomically.
///
/// The `WHERE state = ANY(from)` clause is the enforcement: exactly one caller
/// can win a given transition, and a late or illegal one simply updates zero
/// rows. On zero rows we re-read to tell "no such invoice" apart from "wrong
/// state".
pub async fn transition_invoice(
    conn: &mut sqlx::PgConnection,
    id: Uuid,
    business_id: Uuid,
    from: &[InvoiceState],
    to: InvoiceState,
) -> Result<(), ApiError> {
    let from: Vec<String> = from.iter().map(|s| s.as_str().to_owned()).collect();

    let updated = sqlx::query(
        "UPDATE invoices SET state = $1, updated_at = now() \
         WHERE id = $2 AND business_id = $3 AND state = ANY($4)",
    )
    .bind(to.as_str())
    .bind(id)
    .bind(business_id)
    .bind(&from)
    .execute(&mut *conn)
    .await?;

    if updated.rows_affected() == 1 {
        return Ok(());
    }

    let current: Option<String> =
        sqlx::query_scalar("SELECT state FROM invoices WHERE id = $1 AND business_id = $2")
            .bind(id)
            .bind(business_id)
            .fetch_optional(&mut *conn)
            .await?;

    match current {
        None => Err(ApiError::NotFound),
        Some(state) => Err(ApiError::InvalidStateTransition {
            from: state,
            to: to.as_str().to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::InvoiceState::{self, Open, Paid, Uncollectible, Void};

    const ALL: [InvoiceState; 4] = [Open, Paid, Void, Uncollectible];

    #[test]
    fn transition_table_matches_the_spec() {
        // The authoritative table: only these three are allowed.
        let allowed = |from, to| {
            matches!(
                (from, to),
                (Open, Paid) | (Open, Void) | (Open, Uncollectible)
            )
        };

        for &from in &ALL {
            for &to in &ALL {
                assert_eq!(
                    from.can_transition_to(to),
                    allowed(from, to),
                    "{from:?} -> {to:?}"
                );
            }
        }
    }

    #[test]
    fn only_open_is_non_terminal() {
        assert!(!Open.is_terminal());
        assert!(Paid.is_terminal());
        assert!(Void.is_terminal());
        assert!(Uncollectible.is_terminal());
    }
}
