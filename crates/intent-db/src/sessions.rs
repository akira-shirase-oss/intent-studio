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

    /// Whether a session with this digest exists, has not expired and was used within `idle`;
    /// a live session is touched in the same statement, so use keeps it alive.
    pub async fn session_is_valid(
        &self,
        digest: &[u8; 32],
        idle: Duration,
    ) -> Result<bool, DbError> {
        let touched = sqlx::query(
            "UPDATE operator_sessions SET last_seen_at = now()
             WHERE token_digest = $1 AND expires_at > now()
               AND last_seen_at > now() - $2 * interval '1 second'",
        )
        .bind(digest.as_slice())
        .bind(f64::from(u32::try_from(idle.as_secs()).unwrap_or(u32::MAX)))
        .execute(&self.pool)
        .await
        .map_err(classify)?;
        Ok(touched.rows_affected() == 1)
    }

    /// Whether the operator signed in successfully from `address` within `within`.
    pub async fn signed_in_from_recently(
        &self,
        address: &str,
        within: Duration,
    ) -> Result<bool, DbError> {
        sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM operator_audit
                            WHERE action = 'operator.sign_in' AND address = $1
                              AND at > now() - $2 * interval '1 second')",
        )
        .bind(address)
        .bind(f64::from(
            u32::try_from(within.as_secs()).unwrap_or(u32::MAX),
        ))
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
