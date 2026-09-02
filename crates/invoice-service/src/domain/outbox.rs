//! Transactional outbox for webhooks.
//!
//! Every domain state change writes one `webhook_events` row plus one
//! `webhook_deliveries` row per active endpoint, **in the same transaction** as
//! the change itself. If the transaction rolls back, so does the event — there
//! is no orphan notification. A separate worker (added later) does the actual
//! HTTP delivery, off the request path.

use serde_json::Value;
use uuid::Uuid;

/// Record `event_type` for `resource_id` and fan it out to the business's active
/// endpoints. Until any endpoint is registered this just writes the event row.
pub async fn emit(
    conn: &mut sqlx::PgConnection,
    business_id: Uuid,
    event_type: &str,
    resource_id: Uuid,
    payload: Value,
) -> Result<(), sqlx::Error> {
    let event_id = Uuid::now_v7();

    sqlx::query(
        "INSERT INTO webhook_events (id, business_id, event_type, resource_id, payload) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(event_id)
    .bind(business_id)
    .bind(event_type)
    .bind(resource_id)
    .bind(&payload)
    .execute(&mut *conn)
    .await?;

    let endpoints: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM webhook_endpoints WHERE business_id = $1 AND active")
            .bind(business_id)
            .fetch_all(&mut *conn)
            .await?;

    for endpoint_id in endpoints {
        sqlx::query(
            "INSERT INTO webhook_deliveries (id, event_id, endpoint_id, status) \
             VALUES ($1, $2, $3, 'pending')",
        )
        .bind(Uuid::now_v7())
        .bind(event_id)
        .bind(endpoint_id)
        .execute(&mut *conn)
        .await?;
    }

    Ok(())
}
