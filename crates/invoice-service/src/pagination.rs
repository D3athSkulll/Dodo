//! Keyset pagination helpers, shared by the list endpoints.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::Serialize;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::{ApiError, FieldError};

/// The envelope every list endpoint returns.
#[derive(Serialize)]
pub struct Page<T> {
    pub data: Vec<T>,
    pub next_cursor: Option<String>,
}

/// Where the previous page ended: the `(created_at, id)` of its last row. Rows
/// are ordered by `(created_at DESC, id DESC)`, so the next page is everything
/// strictly less than this pair. `id` breaks ties when timestamps collide.
pub struct Cursor {
    pub created_at: OffsetDateTime,
    pub id: Uuid,
}

impl Cursor {
    /// Encode as an opaque token. Clients pass it back verbatim; they are not
    /// meant to parse it.
    pub fn encode(&self) -> String {
        let raw = format!("{}_{}", self.created_at.unix_timestamp_nanos(), self.id);
        URL_SAFE_NO_PAD.encode(raw)
    }

    pub fn decode(token: &str) -> Result<Self, ApiError> {
        let parsed = (|| {
            let bytes = URL_SAFE_NO_PAD.decode(token).ok()?;
            let text = String::from_utf8(bytes).ok()?;
            let (nanos, id) = text.split_once('_')?;
            Some(Cursor {
                created_at: OffsetDateTime::from_unix_timestamp_nanos(nanos.parse().ok()?).ok()?,
                id: id.parse().ok()?,
            })
        })();

        parsed.ok_or_else(|| {
            ApiError::Validation(vec![FieldError {
                field: "cursor".to_owned(),
                message: "malformed pagination cursor".to_owned(),
            }])
        })
    }
}

/// Clamp a client-supplied `limit` to `1..=100`; default to 20 when absent.
pub fn clamp_limit(limit: Option<i64>) -> usize {
    let n = limit.unwrap_or(20).clamp(1, 100);
    usize::try_from(n).unwrap_or(20)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trips() {
        let c = Cursor {
            created_at: OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap(),
            id: Uuid::now_v7(),
        };
        let back = Cursor::decode(&c.encode()).expect("decodes");
        assert_eq!(back.created_at, c.created_at);
        assert_eq!(back.id, c.id);
    }

    #[test]
    fn garbage_cursor_is_a_validation_error() {
        assert!(Cursor::decode("not-base64!!").is_err());
        assert!(Cursor::decode(&URL_SAFE_NO_PAD.encode("missing-separator")).is_err());
    }

    #[test]
    fn limit_is_clamped() {
        assert_eq!(clamp_limit(None), 20);
        assert_eq!(clamp_limit(Some(0)), 1);
        assert_eq!(clamp_limit(Some(5000)), 100);
        assert_eq!(clamp_limit(Some(50)), 50);
    }
}
