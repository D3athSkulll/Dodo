//! Required test: a retry with the same idempotency key replays the same
//! response and makes no second PSP call.

mod common;

#[sqlx::test]
async fn same_key_and_body_replays_without_a_second_charge(pool: sqlx::PgPool) {
    let app = common::spawn(pool).await;
    let invoice = app.create_invoice(2500).await;

    let first = app.pay(invoice, "tok_success", "key-1").await;
    assert_eq!(first.status(), 200);
    let first_body: serde_json::Value = first.json().await.unwrap();

    let second = app.pay(invoice, "tok_success", "key-1").await;
    assert_eq!(second.status(), 200);
    let second_body: serde_json::Value = second.json().await.unwrap();

    assert_eq!(first_body, second_body, "replay must be byte-identical");
    assert_eq!(app.charge_count().await, 1, "no second PSP call");
    assert_eq!(app.invoice_state(invoice).await, "paid");
    assert_eq!(app.succeeded_attempts(invoice).await, 1);
}
