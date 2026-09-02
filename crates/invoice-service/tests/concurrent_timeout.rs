//! Not spec-required, but it proves mechanism #2: the invariant is "one pending
//! external payment per invoice", not "N PSP requests". 20 concurrent pays with
//! distinct keys and a slow PSP → one in-flight charge, 19 rejected.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{spawn_with, Timings};
use tokio::task::JoinSet;

#[sqlx::test]
async fn twenty_concurrent_timeouts_make_one_charge(pool: sqlx::PgPool) {
    let app = Arc::new(
        spawn_with(
            pool,
            Timings {
                // PSP records the charge at 400ms; the client gives up at 150ms.
                psp_timeout_delay: Duration::from_millis(400),
                client_psp_timeout: Duration::from_millis(150),
                ..Timings::default()
            },
        )
        .await,
    );
    let invoice = app.create_invoice(5_000).await;

    let mut set = JoinSet::new();
    for i in 0..20 {
        let app = Arc::clone(&app);
        set.spawn(async move {
            app.pay(invoice, "tok_timeout", &format!("key-{i}"))
                .await
                .status()
                .as_u16()
        });
    }
    let mut statuses = Vec::new();
    while let Some(res) = set.join_next().await {
        statuses.push(res.unwrap());
    }

    let accepted = statuses.iter().filter(|&&s| s == 202).count();
    let rejected = statuses.iter().filter(|&&s| s == 409).count();
    assert_eq!(
        accepted, 1,
        "one attempt goes in-flight; statuses {statuses:?}"
    );
    assert_eq!(rejected, 19, "the rest are payment_in_progress");

    let attempts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM payment_attempts WHERE invoice_id = $1")
            .bind(invoice)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(attempts, 1, "one attempt row, not twenty");
    assert_eq!(app.invoice_state(invoice).await, "open");

    // Once the PSP finishes its one charge, that's all there is.
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(app.charge_count().await, 1);
}
