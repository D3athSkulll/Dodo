//! API key authentication.
//!
//! Token format: `dodo_<key_id>_<secret>`. `key_id` is stored in plaintext and
//! uniquely indexed, so a lookup is one indexed row with no prefix-scan
//! ambiguity. Only `sha256(secret)` is stored. The secret is 256 bits of CSPRNG
//! output, so SHA-256 is enough — a slow password KDF would add latency to every
//! request and only defends against low-entropy guessing, which cannot happen
//! with a key this random.

use axum::{
    extract::{FromRequestParts, Request, State},
    http::{header::AUTHORIZATION, request::Parts},
    middleware::Next,
    response::Response,
};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::{app::AppState, error::ApiError};

const TOKEN_PREFIX: &str = "dodo";

/// A freshly generated key. The full token is shown to the caller once; only
/// `key_id` and `secret_hash` are persisted.
pub struct GeneratedKey {
    pub token: String,
    pub key_id: String,
    pub secret_hash: [u8; 32],
}

impl GeneratedKey {
    pub fn new() -> Self {
        let key_id = crate::secret::hex(12);
        let secret = crate::secret::hex(32);
        let secret_hash = sha256(secret.as_bytes());
        let token = format!("{TOKEN_PREFIX}_{key_id}_{secret}");
        Self {
            token,
            key_id,
            secret_hash,
        }
    }
}

impl Default for GeneratedKey {
    fn default() -> Self {
        Self::new()
    }
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(&Sha256::digest(bytes));
    out
}

/// Split `dodo_<key_id>_<secret>`. `None` for anything that is not exactly the
/// prefix plus two non-empty `_`-separated segments. `key_id` and `secret` are
/// hex, so they never contain `_` themselves.
fn parse_token(token: &str) -> Option<(&str, &str)> {
    let mut parts = token.split('_');
    let prefix = parts.next()?;
    let key_id = parts.next()?;
    let secret = parts.next()?;
    if prefix != TOKEN_PREFIX || parts.next().is_some() || key_id.is_empty() || secret.is_empty() {
        return None;
    }
    Some((key_id, secret))
}

/// The business id proven by a valid API key. Put in request extensions by
/// [`require_api_key`], read back by the [`Business`] extractor.
#[derive(Debug, Clone, Copy)]
pub struct BusinessId(pub Uuid);

#[derive(sqlx::FromRow)]
struct KeyLookup {
    business_id: Uuid,
    secret_hash: Vec<u8>,
    revoked: bool,
}

/// Middleware: authenticate the `Authorization: Bearer` token, or 401.
pub async fn require_api_key(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let token = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(ApiError::Unauthorized)?;

    let (key_id, secret) = parse_token(token).ok_or(ApiError::Unauthorized)?;

    // key_id is unique, so this is a single-row lookup.
    let key = sqlx::query_as::<_, KeyLookup>(
        "SELECT business_id, secret_hash, (revoked_at IS NOT NULL) AS revoked \
         FROM api_keys WHERE key_id = $1",
    )
    .bind(key_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(ApiError::Unauthorized)?;

    if key.revoked {
        return Err(ApiError::Unauthorized);
    }

    // Compare hashes, not secrets, and in constant time. A timing leak would only
    // expose bits of sha256(guess), but the check costs nothing.
    let presented = sha256(secret.as_bytes());
    if !bool::from(presented.as_slice().ct_eq(&key.secret_hash)) {
        return Err(ApiError::Unauthorized);
    }

    req.extensions_mut().insert(BusinessId(key.business_id));
    Ok(next.run(req).await)
}

/// Extractor for handlers behind [`require_api_key`]: the caller's business id.
pub struct Business(pub Uuid);

impl<S: Send + Sync> FromRequestParts<S> for Business {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<BusinessId>()
            .map(|b| Business(b.0))
            .ok_or(ApiError::Unauthorized)
    }
}

/// Create one business plus one API key. Returns the token so the caller can
/// print it exactly once.
pub async fn seed(pool: &PgPool) -> Result<(Uuid, String), sqlx::Error> {
    let business_id = Uuid::now_v7();
    let key = GeneratedKey::new();

    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO businesses (id, name) VALUES ($1, $2)")
        .bind(business_id)
        .bind("Seed Business")
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO api_keys (id, business_id, key_id, secret_hash, name) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(business_id)
    .bind(&key.key_id)
    .bind(&key.secret_hash[..])
    .bind("seed")
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok((business_id, key.token))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_token_round_trips() {
        let k = GeneratedKey::new();
        let (key_id, secret) = parse_token(&k.token).expect("token parses");
        assert_eq!(key_id, k.key_id);
        assert_eq!(sha256(secret.as_bytes()), k.secret_hash);
    }

    #[test]
    fn malformed_tokens_are_rejected() {
        for bad in [
            "nope",
            "dodo_only",
            "dodo__secret",
            "wrong_a_b",
            "dodo_a_b_c",
            "",
        ] {
            assert!(parse_token(bad).is_none(), "should reject {bad:?}");
        }
    }

    #[test]
    fn hashing_is_stable_and_sensitive() {
        assert_eq!(sha256(b"abc"), sha256(b"abc"));
        assert_ne!(sha256(b"abc"), sha256(b"abd"));
    }
}
