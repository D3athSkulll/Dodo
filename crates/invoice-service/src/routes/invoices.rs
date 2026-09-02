//! Invoices: create (server computes the total), get one with line items, list
//! by state, void, mark uncollectible. All scoped to the authenticated business.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{PgConnection, QueryBuilder};
use time::{macros::format_description, Date, OffsetDateTime};
use uuid::Uuid;

use crate::{
    auth::Business,
    domain::invoice_state::{transition_invoice, InvoiceState},
    domain::outbox,
    error::{ApiError, FieldError},
    money::Cents,
    pagination::{clamp_limit, Cursor, Page},
    state::AppState,
};

const MAX_LINE_ITEMS: usize = 500;
const DATE_FMT: &[time::format_description::BorrowedFormatItem<'_>] =
    format_description!("[year]-[month]-[day]");

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/invoices", post(create).get(list))
        .route("/invoices/{id}", get(get_one))
        .route("/invoices/{id}/void", post(void))
        .route(
            "/invoices/{id}/mark-uncollectible",
            post(mark_uncollectible),
        )
}

// ---- request bodies ----------------------------------------------------------

// `deny_unknown_fields`: a client-supplied `total` (or anything else) is a hard
// error, not silently ignored — the server owns the total.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewInvoice {
    pub customer_id: Uuid,
    pub due_date: String,
    pub line_items: Vec<NewLineItem>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewLineItem {
    pub description: String,
    pub quantity: i32,
    pub unit_amount_cents: i64,
}

// ---- responses -------------------------------------------------------------

#[derive(Serialize)]
struct InvoiceSummary {
    id: Uuid,
    customer_id: Uuid,
    state: String,
    total_cents: i64,
    currency: String,
    due_date: String,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

#[derive(Serialize)]
struct InvoiceView {
    #[serde(flatten)]
    invoice: InvoiceSummary,
    line_items: Vec<LineItemView>,
}

#[derive(Serialize, sqlx::FromRow)]
struct LineItemView {
    description: String,
    quantity: i32,
    unit_amount_cents: i64,
    amount_cents: i64,
}

#[derive(sqlx::FromRow)]
struct InvoiceRow {
    id: Uuid,
    customer_id: Uuid,
    state: String,
    total_cents: i64,
    currency: String,
    due_date: Date,
    created_at: OffsetDateTime,
}

impl From<InvoiceRow> for InvoiceSummary {
    fn from(r: InvoiceRow) -> Self {
        InvoiceSummary {
            id: r.id,
            customer_id: r.customer_id,
            state: r.state,
            total_cents: r.total_cents,
            currency: r.currency,
            due_date: r.due_date.format(DATE_FMT).unwrap_or_default(),
            created_at: r.created_at,
        }
    }
}

// ---- create ---------------------------------------------------------------

async fn create(
    State(state): State<AppState>,
    business: Business,
    Json(input): Json<NewInvoice>,
) -> Result<(StatusCode, Json<InvoiceView>), ApiError> {
    let due_date = Date::parse(&input.due_date, DATE_FMT)
        .map_err(|_| field_error("due_date", "expected YYYY-MM-DD"))?;

    if input.line_items.is_empty() {
        return Err(field_error("line_items", "must have at least one line"));
    }
    if input.line_items.len() > MAX_LINE_ITEMS {
        return Err(field_error("line_items", "too many lines (max 500)"));
    }

    // Validate each line and compute its amount with checked arithmetic.
    let mut errors = Vec::new();
    let mut amounts = Vec::with_capacity(input.line_items.len());
    for (i, line) in input.line_items.iter().enumerate() {
        if line.description.trim().is_empty() {
            errors.push(field(
                format!("line_items[{i}].description"),
                "must not be empty",
            ));
        }
        if line.quantity < 1 {
            errors.push(field(
                format!("line_items[{i}].quantity"),
                "must be at least 1",
            ));
        }
        if line.unit_amount_cents < 0 {
            errors.push(field(
                format!("line_items[{i}].unit_amount_cents"),
                "must not be negative",
            ));
        }
        if errors.is_empty() {
            // quantity >= 1 here, so the cast is safe.
            let qty = u32::try_from(line.quantity).unwrap_or(u32::MAX);
            match Cents::new(line.unit_amount_cents).checked_mul_qty(qty) {
                Some(a) => amounts.push(a),
                None => errors.push(field(
                    format!("line_items[{i}]"),
                    "amount does not fit in i64",
                )),
            }
        }
    }
    if !errors.is_empty() {
        return Err(ApiError::Validation(errors));
    }

    let total = Cents::try_sum(amounts.iter().copied())
        .ok_or_else(|| field_error("line_items", "total does not fit in i64"))?;

    // Clean 422 for a bad customer_id, rather than a raw FK error from the insert.
    let known_customer: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM customers WHERE id = $1 AND business_id = $2")
            .bind(input.customer_id)
            .bind(business.0)
            .fetch_optional(&state.pool)
            .await?;
    if known_customer.is_none() {
        return Err(field_error("customer_id", "unknown customer"));
    }

    let invoice_id = Uuid::now_v7();

    let mut tx = state.pool.begin().await?;

    sqlx::query(
        "INSERT INTO invoices (id, business_id, customer_id, state, total_cents, currency, due_date) \
         VALUES ($1, $2, $3, 'open', $4, 'USD', $5)",
    )
    .bind(invoice_id)
    .bind(business.0)
    .bind(input.customer_id)
    .bind(total.into_inner())
    .bind(due_date)
    .execute(&mut *tx)
    .await?;

    for (line, amount) in input.line_items.iter().zip(&amounts) {
        sqlx::query(
            "INSERT INTO invoice_line_items \
               (id, invoice_id, description, quantity, unit_amount_cents, amount_cents) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(Uuid::now_v7())
        .bind(invoice_id)
        .bind(line.description.trim())
        .bind(line.quantity)
        .bind(line.unit_amount_cents)
        .bind(amount.into_inner())
        .execute(&mut *tx)
        .await?;
    }

    // Same transaction as the insert — no orphan event if this rolls back.
    outbox::emit(
        &mut tx,
        business.0,
        "invoice.created",
        invoice_id,
        json!({
            "type": "invoice.created",
            "invoice": {
                "id": invoice_id,
                "customer_id": input.customer_id,
                "state": "open",
                "total_cents": total.into_inner(),
                "currency": "USD",
                "due_date": input.due_date,
            }
        }),
    )
    .await?;

    tx.commit().await?;

    let view = load_view(&mut *state.pool.acquire().await?, invoice_id, business.0)
        .await?
        .ok_or(ApiError::Internal)?;
    Ok((StatusCode::CREATED, Json(view)))
}

// ---- read ---------------------------------------------------------------

async fn get_one(
    State(state): State<AppState>,
    business: Business,
    Path(id): Path<Uuid>,
) -> Result<Json<InvoiceView>, ApiError> {
    let mut conn = state.pool.acquire().await?;
    let view = load_view(&mut conn, id, business.0)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(view))
}

#[derive(Deserialize)]
struct ListQuery {
    state: Option<String>,
    limit: Option<i64>,
    cursor: Option<String>,
}

async fn list(
    State(state): State<AppState>,
    business: Business,
    Query(q): Query<ListQuery>,
) -> Result<Json<Page<InvoiceSummary>>, ApiError> {
    let state_filter = match q.state.as_deref() {
        None => None,
        Some(s) => Some(
            parse_state(s)
                .ok_or_else(|| field_error("state", "unknown invoice state"))?
                .as_str(),
        ),
    };
    let limit = clamp_limit(q.limit);
    let cursor = q.cursor.as_deref().map(Cursor::decode).transpose()?;
    let fetch_limit = i64::try_from(limit + 1).unwrap_or(i64::MAX);

    // Optional filters → build the statement instead of writing four variants.
    let mut qb = QueryBuilder::new(
        "SELECT id, customer_id, state, total_cents, currency, due_date, created_at \
         FROM invoices WHERE business_id = ",
    );
    qb.push_bind(business.0);
    if let Some(s) = state_filter {
        qb.push(" AND state = ").push_bind(s);
    }
    if let Some(c) = &cursor {
        qb.push(" AND (created_at, id) < (")
            .push_bind(c.created_at)
            .push(", ")
            .push_bind(c.id)
            .push(")");
    }
    qb.push(" ORDER BY created_at DESC, id DESC LIMIT ")
        .push_bind(fetch_limit);

    let mut rows: Vec<InvoiceRow> = qb.build_query_as().fetch_all(&state.pool).await?;

    let next_cursor = if rows.len() > limit {
        rows.truncate(limit);
        rows.last().map(|r| {
            Cursor {
                created_at: r.created_at,
                id: r.id,
            }
            .encode()
        })
    } else {
        None
    };

    Ok(Json(Page {
        data: rows.into_iter().map(InvoiceSummary::from).collect(),
        next_cursor,
    }))
}

// ---- transitions ---------------------------------------------------------

async fn void(
    State(state): State<AppState>,
    business: Business,
    Path(id): Path<Uuid>,
) -> Result<Json<InvoiceView>, ApiError> {
    change_state(&state, business.0, id, InvoiceState::Void).await
}

async fn mark_uncollectible(
    State(state): State<AppState>,
    business: Business,
    Path(id): Path<Uuid>,
) -> Result<Json<InvoiceView>, ApiError> {
    change_state(&state, business.0, id, InvoiceState::Uncollectible).await
}

async fn change_state(
    state: &AppState,
    business_id: Uuid,
    id: Uuid,
    to: InvoiceState,
) -> Result<Json<InvoiceView>, ApiError> {
    let mut conn = state.pool.acquire().await?;
    transition_invoice(&mut conn, id, business_id, &[InvoiceState::Open], to).await?;
    let view = load_view(&mut conn, id, business_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(view))
}

// ---- helpers -----------------------------------------------------------

async fn load_view(
    conn: &mut PgConnection,
    id: Uuid,
    business_id: Uuid,
) -> Result<Option<InvoiceView>, sqlx::Error> {
    let row: Option<InvoiceRow> = sqlx::query_as(
        "SELECT id, customer_id, state, total_cents, currency, due_date, created_at \
         FROM invoices WHERE id = $1 AND business_id = $2",
    )
    .bind(id)
    .bind(business_id)
    .fetch_optional(&mut *conn)
    .await?;

    let Some(row) = row else {
        return Ok(None);
    };

    let line_items: Vec<LineItemView> = sqlx::query_as(
        "SELECT description, quantity, unit_amount_cents, amount_cents \
         FROM invoice_line_items WHERE invoice_id = $1 ORDER BY id",
    )
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;

    Ok(Some(InvoiceView {
        invoice: row.into(),
        line_items,
    }))
}

fn parse_state(s: &str) -> Option<InvoiceState> {
    match s {
        "open" => Some(InvoiceState::Open),
        "paid" => Some(InvoiceState::Paid),
        "void" => Some(InvoiceState::Void),
        "uncollectible" => Some(InvoiceState::Uncollectible),
        _ => None,
    }
}

fn field(name: impl Into<String>, message: &str) -> FieldError {
    FieldError {
        field: name.into(),
        message: message.to_owned(),
    }
}

fn field_error(name: &str, message: &str) -> ApiError {
    ApiError::Validation(vec![field(name, message)])
}
