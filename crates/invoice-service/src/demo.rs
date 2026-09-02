//! `invoice-service demo` — populate the database with a spread of sample data so
//! every read endpoint returns something interesting with no setup.
//!
//! Creates one business + API key, three customers, one invoice in each state
//! (`open` / `paid` / `void` / `uncollectible`), matching line items and payment
//! attempts, a webhook endpoint, and the webhook events + deliveries those imply.
//! Additive: run it again for another independent business.

use serde_json::json;
use sqlx::PgConnection;
use time::{Date, Month};
use uuid::Uuid;

use crate::{auth, domain::outbox};

pub struct DemoData {
    pub api_key: String,
    pub business_id: Uuid,
    pub open_invoice_id: Uuid,
    pub paid_invoice_id: Uuid,
    pub customer_id: Uuid,
}

pub async fn run(pool: &sqlx::PgPool) -> Result<DemoData, sqlx::Error> {
    let (business_id, api_key) = auth::seed(pool).await?;

    let mut tx = pool.begin().await?;

    let acme = customer(&mut tx, business_id, "Acme Corp", "ap@acme.example").await?;
    let globex = customer(&mut tx, business_id, "Globex", "billing@globex.example").await?;
    let initech = customer(&mut tx, business_id, "Initech", "finance@initech.example").await?;

    let due = Date::from_calendar_date(2026, Month::June, 1).unwrap();

    // open + unpaid — the one to try `POST /pay` against.
    let open_invoice_id = invoice(
        &mut tx,
        business_id,
        acme,
        "open",
        due,
        &[("Widget", 2, 1500), ("Bolt", 3, 99)],
    )
    .await?;

    // open, with a prior declined attempt on record.
    let retry_invoice_id = invoice(
        &mut tx,
        business_id,
        acme,
        "open",
        due,
        &[("Support plan", 1, 4900)],
    )
    .await?;
    payment_attempt(
        &mut tx,
        retry_invoice_id,
        business_id,
        "demo-declined",
        "tok_card_declined",
        "failed",
        None,
        Some("card_declined"),
        4900,
    )
    .await?;

    // paid, with the succeeded attempt that paid it.
    let paid_invoice_id = invoice(
        &mut tx,
        business_id,
        globex,
        "paid",
        due,
        &[("Seat licence", 10, 1200)],
    )
    .await?;
    payment_attempt(
        &mut tx,
        paid_invoice_id,
        business_id,
        "demo-paid",
        "tok_success",
        "succeeded",
        Some("psp_demo_ref_0001"),
        None,
        12_000,
    )
    .await?;

    let void_invoice_id = invoice(
        &mut tx,
        business_id,
        globex,
        "void",
        due,
        &[("Cancelled order", 1, 2500)],
    )
    .await?;
    let uncollectible_invoice_id = invoice(
        &mut tx,
        business_id,
        initech,
        "uncollectible",
        due,
        &[("Consulting", 8, 15_000)],
    )
    .await?;

    // A webhook endpoint, then the events these invoices would have produced.
    webhook_endpoint(&mut tx, business_id, "https://example.com/hooks/dodo").await?;

    for (id, state) in [
        (open_invoice_id, "open"),
        (retry_invoice_id, "open"),
        (paid_invoice_id, "paid"),
        (void_invoice_id, "void"),
        (uncollectible_invoice_id, "uncollectible"),
    ] {
        outbox::emit(
            &mut tx,
            business_id,
            "invoice.created",
            id,
            json!({ "type": "invoice.created", "invoice": { "id": id, "state": state } }),
        )
        .await?;
    }
    outbox::emit(
        &mut tx,
        business_id,
        "invoice.paid",
        paid_invoice_id,
        json!({ "type": "invoice.paid", "invoice": { "id": paid_invoice_id, "state": "paid" } }),
    )
    .await?;
    outbox::emit(
        &mut tx,
        business_id,
        "invoice.payment_failed",
        retry_invoice_id,
        json!({
            "type": "invoice.payment_failed",
            "invoice": { "id": retry_invoice_id, "state": "open" },
            "payment": { "failure_code": "card_declined" }
        }),
    )
    .await?;

    // Give the delivery list some variety: the oldest delivered, one exhausted,
    // the rest left pending.
    sqlx::query(
        "UPDATE webhook_deliveries SET status = 'delivered', delivered_at = now() \
         WHERE id IN (SELECT id FROM webhook_deliveries ORDER BY created_at LIMIT 1)",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE webhook_deliveries SET status = 'exhausted', attempts = 6, \
             last_error = 'http 500' \
         WHERE id IN (SELECT id FROM webhook_deliveries WHERE status = 'pending' \
                      ORDER BY created_at DESC LIMIT 1)",
    )
    .execute(&mut *tx)
    .await?;

    // Deactivate the endpoint so the delivery worker leaves this seeded state
    // alone — the point is data for the read routes, not a live send to
    // example.com. Flip `active` back on to watch delivery happen.
    sqlx::query("UPDATE webhook_endpoints SET active = false WHERE business_id = $1")
        .bind(business_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    Ok(DemoData {
        api_key,
        business_id,
        open_invoice_id,
        paid_invoice_id,
        customer_id: acme,
    })
}

async fn customer(
    tx: &mut PgConnection,
    business_id: Uuid,
    name: &str,
    email: &str,
) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO customers (id, business_id, name, email) VALUES ($1, $2, $3, $4)")
        .bind(id)
        .bind(business_id)
        .bind(name)
        .bind(email)
        .execute(&mut *tx)
        .await?;
    Ok(id)
}

/// Insert an invoice plus its line items, with the totals computed the same way
/// the API computes them (integer cents, `unit * qty`).
async fn invoice(
    tx: &mut PgConnection,
    business_id: Uuid,
    customer_id: Uuid,
    state: &str,
    due_date: Date,
    lines: &[(&str, i32, i64)],
) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    let total: i64 = lines
        .iter()
        .map(|(_, qty, unit)| unit * i64::from(*qty))
        .sum();

    sqlx::query(
        "INSERT INTO invoices (id, business_id, customer_id, state, total_cents, currency, due_date) \
         VALUES ($1, $2, $3, $4, $5, 'USD', $6)",
    )
    .bind(id)
    .bind(business_id)
    .bind(customer_id)
    .bind(state)
    .bind(total)
    .bind(due_date)
    .execute(&mut *tx)
    .await?;

    for (description, quantity, unit_amount_cents) in lines {
        sqlx::query(
            "INSERT INTO invoice_line_items \
               (id, invoice_id, description, quantity, unit_amount_cents, amount_cents) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(Uuid::now_v7())
        .bind(id)
        .bind(description)
        .bind(quantity)
        .bind(unit_amount_cents)
        .bind(unit_amount_cents * i64::from(*quantity))
        .execute(&mut *tx)
        .await?;
    }

    Ok(id)
}

#[allow(clippy::too_many_arguments)]
async fn payment_attempt(
    tx: &mut PgConnection,
    invoice_id: Uuid,
    business_id: Uuid,
    idempotency_key: &str,
    card_token: &str,
    status: &str,
    psp_ref: Option<&str>,
    failure_code: Option<&str>,
    amount_cents: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO payment_attempts \
           (id, invoice_id, business_id, idempotency_key, card_token, request_fingerprint, \
            status, psp_ref, failure_code, amount_cents) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(Uuid::now_v7())
    .bind(invoice_id)
    .bind(business_id)
    .bind(idempotency_key)
    .bind(card_token)
    .bind(&b"demo-fingerprint"[..])
    .bind(status)
    .bind(psp_ref)
    .bind(failure_code)
    .bind(amount_cents)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

async fn webhook_endpoint(
    tx: &mut PgConnection,
    business_id: Uuid,
    url: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO webhook_endpoints (id, business_id, url, secret) VALUES ($1, $2, $3, $4)",
    )
    .bind(Uuid::now_v7())
    .bind(business_id)
    .bind(url)
    .bind(crate::secret::hex(32))
    .execute(&mut *tx)
    .await?;
    Ok(())
}
