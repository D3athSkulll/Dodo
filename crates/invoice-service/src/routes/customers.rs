//! Customers: create, get one, list. All scoped to the authenticated business.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    auth::Business,
    error::{ApiError, FieldError},
    pagination::{clamp_limit, Cursor, Page},
    state::AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/customers", post(create).get(list))
        .route("/customers/{id}", get(get_one))
}

#[derive(Deserialize)]
pub struct NewCustomer {
    pub name: String,
    pub email: String,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct Customer {
    pub id: Uuid,
    pub name: String,
    pub email: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Deserialize)]
struct ListQuery {
    limit: Option<i64>,
    cursor: Option<String>,
}

async fn create(
    State(state): State<AppState>,
    business: Business,
    Json(input): Json<NewCustomer>,
) -> Result<(StatusCode, Json<Customer>), ApiError> {
    let name = input.name.trim();
    let email = input.email.trim();

    let mut errors = Vec::new();
    if name.is_empty() {
        errors.push(field("name", "must not be empty"));
    }
    if !looks_like_email(email) {
        errors.push(field("email", "must be a valid email address"));
    }
    if !errors.is_empty() {
        return Err(ApiError::Validation(errors));
    }

    let customer = sqlx::query_as::<_, Customer>(
        "INSERT INTO customers (id, business_id, name, email) VALUES ($1, $2, $3, $4) \
         RETURNING id, name, email, created_at",
    )
    .bind(Uuid::now_v7())
    .bind(business.0)
    .bind(name)
    .bind(email)
    .fetch_one(&state.pool)
    .await?;

    Ok((StatusCode::CREATED, Json(customer)))
}

async fn get_one(
    State(state): State<AppState>,
    business: Business,
    Path(id): Path<Uuid>,
) -> Result<Json<Customer>, ApiError> {
    let customer = sqlx::query_as::<_, Customer>(
        "SELECT id, name, email, created_at FROM customers WHERE id = $1 AND business_id = $2",
    )
    .bind(id)
    .bind(business.0)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(ApiError::NotFound)?;

    Ok(Json(customer))
}

async fn list(
    State(state): State<AppState>,
    business: Business,
    Query(q): Query<ListQuery>,
) -> Result<Json<Page<Customer>>, ApiError> {
    let limit = clamp_limit(q.limit);
    let cursor = q.cursor.as_deref().map(Cursor::decode).transpose()?;

    // Fetch one extra row to learn whether another page exists.
    let fetch_limit = i64::try_from(limit + 1).unwrap_or(i64::MAX);
    let mut rows = fetch_page(&state.pool, business.0, fetch_limit, cursor).await?;

    let next_cursor = if rows.len() > limit {
        rows.truncate(limit);
        rows.last().map(|c| {
            Cursor {
                created_at: c.created_at,
                id: c.id,
            }
            .encode()
        })
    } else {
        None
    };

    Ok(Json(Page {
        data: rows,
        next_cursor,
    }))
}

/// Newest first, keyset on `(created_at, id)`. Matches `customers_list_idx`.
async fn fetch_page(
    pool: &PgPool,
    business_id: Uuid,
    limit: i64,
    cursor: Option<Cursor>,
) -> Result<Vec<Customer>, sqlx::Error> {
    match cursor {
        Some(c) => {
            sqlx::query_as::<_, Customer>(
                "SELECT id, name, email, created_at FROM customers \
                 WHERE business_id = $1 AND (created_at, id) < ($2, $3) \
                 ORDER BY created_at DESC, id DESC LIMIT $4",
            )
            .bind(business_id)
            .bind(c.created_at)
            .bind(c.id)
            .bind(limit)
            .fetch_all(pool)
            .await
        }
        None => {
            sqlx::query_as::<_, Customer>(
                "SELECT id, name, email, created_at FROM customers \
                 WHERE business_id = $1 \
                 ORDER BY created_at DESC, id DESC LIMIT $2",
            )
            .bind(business_id)
            .bind(limit)
            .fetch_all(pool)
            .await
        }
    }
}

fn field(name: &str, message: &str) -> FieldError {
    FieldError {
        field: name.to_owned(),
        message: message.to_owned(),
    }
}

/// Deliberately loose: one `@`, a non-empty local part, and a dot in the domain.
/// Not RFC 5322 — just enough to catch obvious mistakes.
fn looks_like_email(value: &str) -> bool {
    match value.split_once('@') {
        Some((local, domain)) => {
            !local.is_empty()
                && domain.len() >= 3
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::looks_like_email;

    #[test]
    fn email_check_accepts_normal_addresses() {
        assert!(looks_like_email("a@b.co"));
        assert!(looks_like_email("first.last@sub.example.com"));
    }

    #[test]
    fn email_check_rejects_obvious_junk() {
        for bad in [
            "",
            "no-at",
            "@nolocal.com",
            "trailing@dot.",
            "a@b",
            "a@.com",
        ] {
            assert!(!looks_like_email(bad), "should reject {bad:?}");
        }
    }
}
