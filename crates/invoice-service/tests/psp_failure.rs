//! Required test: a slow or failing PSP never leaves the invoice in a bad state.

mod common;

use std::time::Duration;

use common::{spawn_with, Timings};

/// `tok_timeout`: the endpoint returns `202` quickly; the sweeper finishes it.
#[sqlx::test]
async fn timeout_is_settled_by_the_sweeper(pool: sqlx::PgPool) {
    let app = spawn_with(
        pool,
        Timings {
            // PSP "completes" at 400ms; the client gives up at 150ms.
            psp_timeout_delay: Duration::from_millis(400),
            client_psp_timeout: Duration::from_millis(150),
            sweep_interval: Duration::from_millis(200),
            ..Timings::default()
        },
    )
    .await;

    let invoice = app.create_invoice(7_777).await;

    let resp = app.pay(invoice, "tok_timeout", "k-timeout").await;
    assert_eq!(resp.status(), 202, "caller is told it is pending");
    assert_eq!(app.invoice_state(invoice).await, "open");
    assert_eq!(app.attempt_status(invoice).await, "pending");

    // Sweeper only touches attempts idle >= 3s, then re-submits the idempotent
    // charge and settles it.
    wait_until(Duration::from_secs(10), || async {
        app.invoice_state(invoice).await == "paid"
    })
    .await;

    assert_eq!(app.attempt_status(invoice).await, "succeeded");
    assert_eq!(app.charge_count().await, 1, "still exactly one charge");
}

/// `tok_network_error`: retried, then failed cleanly once past the max age —
/// never stuck `pending`, invoice stays `open`.
#[sqlx::test]
async fn network_error_gives_up_without_getting_stuck(pool: sqlx::PgPool) {
    let app = spawn_with(
        pool,
        Timings {
            sweep_interval: Duration::from_millis(200),
            pending_max_age: Duration::from_secs(1),
            ..Timings::default()
        },
    )
    .await;

    let invoice = app.create_invoice(4_444).await;

    let resp = app.pay(invoice, "tok_network_error", "k-neterr").await;
    assert_eq!(resp.status(), 202);
    assert_eq!(app.attempt_status(invoice).await, "pending");

    wait_until(Duration::from_secs(10), || async {
        app.attempt_status(invoice).await == "failed"
    })
    .await;

    assert_eq!(
        app.invoice_state(invoice).await,
        "open",
        "retryable, not stuck"
    );
    let code: Option<String> = sqlx::query_scalar(
        "SELECT failure_code FROM payment_attempts WHERE invoice_id = $1 LIMIT 1",
    )
    .bind(invoice)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(code.as_deref(), Some("psp_unreachable"));
}

async fn wait_until<F, Fut>(timeout: Duration, mut cond: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if cond().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    panic!("condition not met within {timeout:?}");
}
