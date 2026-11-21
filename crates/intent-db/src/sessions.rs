//! Operator password sessions (D-015). Only the token's SHA-256 digest is stored; the token
//! itself exists in the cookie and nowhere else.

use std::time::Duration;

use crate::{Db, DbError, classify};

impl Db {
    /// Stores a new session, recording which credential issued it (F1): a rotation
    /// must end sessions issued under the rotated-out credential.
    pub async fn create_session(
        &self,
        digest: &[u8; 32],
        ttl: Duration,
        credential_fp: &str,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO operator_sessions (token_digest, expires_at, credential_fp)
             VALUES ($1, now() + $2 * interval '1 second', $3)",
        )
        .bind(digest.as_slice())
        .bind(f64::from(u32::try_from(ttl.as_secs()).unwrap_or(u32::MAX)))
        .bind(credential_fp)
        .execute(&self.pool)
        .await
        .map(|_| ())
        .map_err(classify)
    }

    /// Seconds of lifetime left for the session with this digest if it exists, has not
    /// expired, was used within `idle` AND was issued under the current credential —
    /// a rotated credential ends its sessions at the first check (F1). A live session
    /// is touched in the same statement, so use keeps it alive. The remaining time is
    /// computed at the database so the caller never compares a monotonic clock against
    /// `expires_at`'s epoch — the bound it puts on trusting this check must come in
    /// the same units.
    pub async fn session_lifetime(
        &self,
        digest: &[u8; 32],
        idle: Duration,
        credential_fp: &str,
    ) -> Result<Option<f64>, DbError> {
        // extract(epoch …) is `numeric` on Postgres ≥14 — sqlx maps f64 to FLOAT8 only,
        // so without the cast every row fails to decode (N15).
        sqlx::query_scalar(
            "UPDATE operator_sessions SET last_seen_at = now()
             WHERE token_digest = $1 AND expires_at > now()
               AND last_seen_at > now() - $2 * interval '1 second'
               AND credential_fp = $3
             RETURNING extract(epoch from expires_at - now())::float8",
        )
        .bind(digest.as_slice())
        .bind(f64::from(u32::try_from(idle.as_secs()).unwrap_or(u32::MAX)))
        .bind(credential_fp)
        .fetch_optional(&self.pool)
        .await
        .map_err(classify)
    }

    /// The newest addresses that produced a session within `within` UNDER THE CURRENT
    /// CREDENTIAL, each with its latest sign-in time — the boot seed for the
    /// known-address memory (N10). The time travels with the row so a restart marks
    /// `at + window`, never a fresh window, and the LIMIT bounds the read (D). Only
    /// rows carrying `credential_fp` count: a sign-in under a rotated-out credential
    /// must not keep its address's day budget (the rotation trade-off).
    pub async fn recent_signer_addresses(
        &self,
        within: Duration,
        credential_fp: &str,
    ) -> Result<Vec<(String, time::OffsetDateTime)>, DbError> {
        sqlx::query_as::<_, (String, time::OffsetDateTime)>(
            "SELECT address, max(at) AS last_at FROM operator_audit
             WHERE action = 'operator.sign_in' AND address IS NOT NULL
               AND detail->>'credential_fp' = $2
               AND at > now() - $1 * interval '1 second'
             GROUP BY address ORDER BY last_at DESC LIMIT 65536",
        )
        .bind(f64::from(
            u32::try_from(within.as_secs()).unwrap_or(u32::MAX),
        ))
        .bind(credential_fp)
        .fetch_all(&self.pool)
        .await
        .map_err(classify)
    }

    /// Ends a session (and reaps any expired ones in passing); whether *the token's* row
    /// went — reaped rows must not count, or sign-out claims a dead session it never had.
    pub async fn delete_session(&self, digest: &[u8; 32]) -> Result<bool, DbError> {
        let mut tx = self.pool.begin().await.map_err(classify)?;
        let deleted = sqlx::query("DELETE FROM operator_sessions WHERE token_digest = $1")
            .bind(digest.as_slice())
            .execute(&mut *tx)
            .await
            .map_err(classify)?
            .rows_affected()
            > 0;
        sqlx::query("DELETE FROM operator_sessions WHERE expires_at <= now()")
            .execute(&mut *tx)
            .await
            .map_err(classify)?;
        tx.commit().await.map_err(classify)?;
        Ok(deleted)
    }

    /// Ends every session bound to a different credential fingerprint (N1).
    /// Runs at boot before readiness: `session_lifetime` only REFUSES a
    /// rotated-out fingerprint, it does not remove the row, so booting the old
    /// hash again would revive a thief's cookie inside the idle window. The
    /// caller skips this when no fingerprint exists — a null match would wipe
    /// rows it cannot judge.
    pub async fn drop_sessions_outside_credential(&self, fp: &str) -> Result<u64, DbError> {
        let rows =
            sqlx::query("DELETE FROM operator_sessions WHERE credential_fp IS DISTINCT FROM $1")
                .bind(fp)
                .execute(&self.pool)
                .await
                .map_err(classify)?
                .rows_affected();
        Ok(rows)
    }
}
