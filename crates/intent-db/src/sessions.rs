//! Operator password sessions (D-015). Only the token's SHA-256 digest is stored; the token
//! itself exists in the cookie and nowhere else.

use std::time::Duration;

use crate::{Db, DbError, classify};

impl Db {
    /// Stores a new session.
    pub async fn create_session(&self, digest: &[u8; 32], ttl: Duration) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO operator_sessions (token_digest, expires_at)
             VALUES ($1, now() + $2 * interval '1 second')",
        )
        .bind(digest.as_slice())
        .bind(f64::from(u32::try_from(ttl.as_secs()).unwrap_or(u32::MAX)))
        .execute(&self.pool)
        .await
        .map(|_| ())
        .map_err(classify)
    }

    /// Whether a session with this digest exists and has not expired.
    pub async fn session_is_valid(&self, digest: &[u8; 32]) -> Result<bool, DbError> {
        sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM operator_sessions
                            WHERE token_digest = $1 AND expires_at > now())",
        )
        .bind(digest.as_slice())
        .fetch_one(&self.pool)
        .await
        .map_err(classify)
    }

    /// Ends a session.
    pub async fn delete_session(&self, digest: &[u8; 32]) -> Result<(), DbError> {
        sqlx::query("DELETE FROM operator_sessions WHERE token_digest = $1 OR expires_at <= now()")
            .bind(digest.as_slice())
            .execute(&self.pool)
            .await
            .map(|_| ())
            .map_err(classify)
    }
}
