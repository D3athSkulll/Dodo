//! Required test: N concurrent `POST /pay` for one invoice with distinct keys —
//! exactly one succeeds, no double charge, final state consistent.

mod common;

use std::sync::Arc;

use tokio::task::JoinSet;

#[sqlx::test]
async fn twenty_concurrent_pays_charge_exactly_once(pool: sqlx::PgPool) {
    let app = Arc::new(common::spawn(pool).await);
    let invoice = app.create_invoice(9_000).await;

    let mut set = JoinSet::new();
    for i in 0..20 {
        let app = Arc::clone(&app);
        set.spawn(async move {
            app.pay(invoice, "tok_success", &format!("key-{i}"))
                .await
                .status()
                .as_u16()
        });
    }

    let mut statuses = Vec::new();
    while let Some(res) = set.join_next().await {
        statuses.push(res.unwrap());
    }

    let ok = statuses.iter().filter(|&&s| s == 200).count();
    assert_eq!(ok, 1, "exactly one 200; statuses were {statuses:?}");
    assert!(
        statuses.iter().all(|&s| matches!(s, 200 | 202 | 409)),
        "every response is a success or a documented rejection: {statuses:?}"
    );

    // The real anti-double-charge proof:
    assert_eq!(app.succeeded_attempts(invoice).await, 1);
    assert_eq!(app.charge_count().await, 1, "exactly one charge at the PSP");
    assert_eq!(app.invoice_state(invoice).await, "paid");
}
