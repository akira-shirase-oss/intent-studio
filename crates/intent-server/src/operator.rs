//! The operator's password session and the read-only inbox (D-015, ROADMAP I0).
//!
//! Sign-in verifies an argon2id hash from the server's configuration (off the async workers,
//! at most two at a time, five attempts a minute per address). The session token is 32 random
//! bytes in an `HttpOnly`, `SameSite=Strict` cookie; only its SHA-256 digest is stored, and the
//! token is never logged. Every inbox read is cross-tenant, so it writes an audit row.

use std::time::Duration;

use argon2::{Argon2, PasswordHash, PasswordVerifier as _};
use axum::{
    Json,
    extract::{ConnectInfo, Query, State, rejection::QueryRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use intent_db::{AuditEntry, InboxLead, InboxQuery};
use intent_domain::{
    ids::{LeadId, SiteId},
    ratelimit::{AddressKey, RetryAfter},
};
use intent_domain::{
    limits::{
        SESSION_CHECK_FAILURES_PER_ADDRESS_PER_MINUTE, SIGN_IN_GLOBAL_PER_MINUTE,
        SIGN_IN_KNOWN_PER_ADDRESS_PER_DAY, SIGN_IN_PER_ADDRESS_PER_MINUTE,
    },
    ratelimit::Quota,
};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest as _, Sha256};
use time::format_description::well_known::Rfc3339;

use crate::{
    app::{App, Peer},
    error::ApiError,
    limiter::Limiter,
};

/// The session cookie's name when it is `Secure`: the `__Host-` prefix makes browsers refuse
/// it unless it is Secure, host-only and for `/`, so no sibling subdomain can set or shadow it.
pub const COOKIE_SECURE: &str = "__Host-intent_operator";

/// The cookie's name on plain-HTTP development servers (`INTENT_COOKIE_SECURE=false`), where a
/// `__Host-` cookie would be refused by the browser.
pub const COOKIE_PLAIN: &str = "intent_operator";

fn cookie_name(app: &App) -> &'static str {
    if app.config.cookie_secure {
        COOKIE_SECURE
    } else {
        COOKIE_PLAIN
    }
}

/// How long a successful sign-in makes its address "known" (D-052).
const KNOWN_ADDRESS_WINDOW_MS: u64 = 30 * 24 * 3_600_000;

/// Operator sign-in admission, with time passed in so its rules are tested exactly (N3).
///
/// Per address first (5 a minute), and that check alone: a refused request never reaches the
/// global budget. The global budget (20 a minute) is spent only right before a password is
/// verified, and a right password gives its token back, so it counts failed verifications.
pub struct SignInGate {
    per_address: Limiter<AddressKey>,
    known: Limiter<AddressKey>,
    global: Limiter<u8>,
    /// Addresses that signed in within `KNOWN_ADDRESS_WINDOW` — in memory, so the check can
    /// never 5xx behind a database and a flood can never reach a pool to starve it (N10).
    /// A restart loses the set: the operator then shares the global budget like anyone —
    /// bounded, not locked out.
    known_addresses: std::sync::Mutex<hashlink::LruCache<AddressKey, u64>>,
}

impl SignInGate {
    /// The D-041 quotas.
    pub fn new() -> Self {
        let quota = |per_minute| {
            Quota::per_minute(per_minute)
                .unwrap_or_else(|| unreachable!("invariant: the sign-in quotas are non-zero"))
        };
        Self {
            per_address: Limiter::new(quota(SIGN_IN_PER_ADDRESS_PER_MINUTE), 10_000),
            known: Limiter::new(
                Quota::per_day(SIGN_IN_KNOWN_PER_ADDRESS_PER_DAY).unwrap_or_else(|| {
                    unreachable!("invariant: the known-address quota is non-zero")
                }),
                10_000,
            ),
            global: Limiter::new(quota(SIGN_IN_GLOBAL_PER_MINUTE), 16),
            known_addresses: std::sync::Mutex::new(hashlink::LruCache::new(65_536)),
        }
    }

    /// The per-address check.
    pub fn admit(&self, address: AddressKey, now_ms: u64) -> Result<(), RetryAfter> {
        self.per_address.check(address, now_ms)
    }

    /// Takes a global token right before a verification.
    pub fn reserve(&self, now_ms: u64) -> Result<(), RetryAfter> {
        self.global.check(0, now_ms)
    }

    /// Takes a known-address day token right before a verification.
    pub fn reserve_known(&self, address: AddressKey, now_ms: u64) -> Result<(), RetryAfter> {
        self.known.check(address, now_ms)
    }

    /// The budget one verification spends: the known-address day when `known` and it has
    /// room, else the shared minute — a spent day budget never holds the operator at 429
    /// behind one refill tick, it just costs the same as an address nobody knows.
    /// `Some(address)` means the known budget was spent (give it back on success).
    pub fn reserve_for(
        &self,
        known: bool,
        address: AddressKey,
        now_ms: u64,
    ) -> Result<Option<AddressKey>, RetryAfter> {
        if known && self.known.check(address, now_ms).is_ok() {
            return Ok(Some(address));
        }
        self.global.check(0, now_ms).map(|()| None)
    }

    /// Gives the token back after a right password.
    pub fn give_back(&self) {
        self.global.refund(0);
    }

    /// Gives a known-address day token back after a right password.
    pub fn give_back_known(&self, address: AddressKey) {
        self.known.refund(address);
    }

    /// A sign-in succeeded from here: remember it for the day-budget window.
    pub fn mark_address(&self, address: AddressKey, now_ms: u64) {
        self.known_addresses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(address, now_ms + KNOWN_ADDRESS_WINDOW_MS);
    }

    /// Whether this address produced a session inside the window — memory, never I/O.
    pub fn is_known_address(&self, address: &AddressKey, now_ms: u64) -> bool {
        self.known_addresses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(address)
            .is_some_and(|&until| until > now_ms)
    }
}

impl Default for SignInGate {
    fn default() -> Self {
        Self::new()
    }
}

/// The per-address charge on a session check, and the three memories of digest state.
/// A check is a database write an anonymous client can ask for on every request, so:
/// `verified` answers for a digest that proved live within its trust window (never past
/// the session's own expiry or half its idle), `known` remembers digests that once proved
/// live so their re-checks skip the charge (a flood of strangers on the operator's NAT
/// cannot hold a known session at 429 at each renewal), and `revoked` tombstones a signed
/// out digest so a check that raced the delete cannot re-warm it. One mutex guards all
/// three: a mark that consulted `revoked` under one lock and wrote `verified` under
/// another could re-warm a digest a sign-out just killed.
struct DigestState {
    /// digest → the instant it stops being trusted without a fresh check.
    verified: hashlink::LruCache<[u8; 32], u64>,
    /// digests that once proved live — re-check these without spending the address budget.
    known: hashlink::LruCache<[u8; 32], ()>,
    /// revoked digest → tombstone end; a mark inside the tombstone is dropped.
    revoked: hashlink::LruCache<[u8; 32], u64>,
}

pub struct SessionGate {
    per_address: Limiter<AddressKey>,
    state: std::sync::Mutex<DigestState>,
}

/// The most a check is trusted: the shorter of this cap, the session's own expiry, and half
/// its idle timeout (a check must refresh at least that often, or the idle deadline is a lie).
const VERIFIED_SESSION_MS: u64 = 60_000;

fn lru<K: Ord + std::hash::Hash, V>() -> hashlink::LruCache<K, V> {
    hashlink::LruCache::new(10_000)
}

impl SessionGate {
    /// The D-019 bound, at the sign-in table's size.
    pub fn new() -> Self {
        Self {
            per_address: Limiter::new(
                Quota::per_minute(SESSION_CHECK_FAILURES_PER_ADDRESS_PER_MINUTE).unwrap_or_else(
                    || unreachable!("invariant: the session-check quota is non-zero"),
                ),
                10_000,
            ),
            state: std::sync::Mutex::new(DigestState {
                verified: lru(),
                known: lru(),
                revoked: lru(),
            }),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, DigestState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Whether this digest is trusted live right now — consulted before anything else.
    pub fn was_verified(&self, digest: &[u8; 32], now_ms: u64) -> bool {
        self.state()
            .verified
            .get(digest)
            .is_some_and(|&until| until > now_ms)
    }

    /// Whether sign-out revoked this digest inside its tombstone — beats a live row, a
    /// warm memory and every charge, because the row may not be confirmed gone.
    pub fn is_revoked(&self, digest: &[u8; 32], now_ms: u64) -> bool {
        self.state()
            .revoked
            .get(digest)
            .is_some_and(|&until| until > now_ms)
    }

    /// Whether this digest once proved live — its re-check skips the per-address charge.
    /// A trusted-live digest counts too.
    pub fn is_known(&self, digest: &[u8; 32], now_ms: u64) -> bool {
        let mut state = self.state();
        state
            .verified
            .get(digest)
            .is_some_and(|&until| until > now_ms)
            || state.known.get(digest).is_some()
    }

    /// A live check (or a sign-in): trust the digest until `until_ms` and remember it as
    /// known — unless it was revoked so recently the mark could be a sign-out race. The
    /// check and both writes hold the one lock, so a `forget` cannot slip between them.
    pub fn mark_verified(&self, digest: [u8; 32], until_ms: u64, now_ms: u64) {
        let mut state = self.state();
        if state
            .revoked
            .get(&digest)
            .is_some_and(|&until| until > now_ms)
        {
            return;
        }
        state.verified.insert(digest, until_ms);
        state.known.insert(digest, ());
    }

    /// A session is dead the moment sign-out is asked: drop it from both memories and
    /// tombstone it against a racing re-check until `until_ms` — at least as long as the
    /// row itself could stay live, so an unconfirmed delete can never re-warm (N7). Callers
    /// tombstone only a digest that was known or whose row was actually deleted — a forged
    /// cookie may not fill the bound.
    pub fn forget(&self, digest: &[u8; 32], _now_ms: u64, until_ms: u64) {
        let mut state = self.state();
        state.verified.remove(digest);
        state.known.remove(digest);
        state.revoked.insert(*digest, until_ms);
    }

    /// The database says this digest is dead: drop the memory so its next sighting pays
    /// the charge again — a dead session re-checking free forever is a gift to a flood.
    pub fn unknow(&self, digest: &[u8; 32]) {
        let mut state = self.state();
        state.verified.remove(digest);
        state.known.remove(digest);
    }

    /// Takes the charge a check costs this address.
    pub fn charge(&self, address: AddressKey, now_ms: u64) -> Result<(), RetryAfter> {
        self.per_address.check(address, now_ms)
    }

    /// A live session (or a session that really was deleted) proved the request: refund.
    pub fn refund(&self, address: AddressKey) {
        self.per_address.refund(address);
    }
}

impl Default for SessionGate {
    fn default() -> Self {
        Self::new()
    }
}

/// Who asked, for the audit: the client address and the request id.
pub(crate) struct Caller {
    pub(crate) address: String,
    pub(crate) request_id: Option<String>,
}

impl Caller {
    pub(crate) fn of(app: &App, peer: Peer, headers: &HeaderMap) -> Self {
        let address = crate::intake::client_address_of(
            headers,
            peer.ip(),
            &app.config.trusted_proxies,
            crate::mutated!(app.config, crate::config::Mutation::TrustAnyForwarded),
        );
        let request_id = headers
            .get(&crate::app::REQUEST_ID)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        Self {
            address: address.to_string(),
            request_id,
        }
    }

    /// An audit entry for an operator action, to commit with the action itself.
    pub(crate) fn entry<'a>(&'a self, action: &'a str, detail: &'a str) -> AuditEntry<'a> {
        AuditEntry {
            action,
            actor: "operator",
            address: Some(&self.address),
            request_id: self.request_id.as_deref(),
            detail,
        }
    }

    pub(crate) async fn audit(
        &self,
        app: &App,
        action: &str,
        actor: &str,
        detail: &str,
    ) -> Result<(), ApiError> {
        app.db
            .audit(&AuditEntry {
                action,
                actor,
                address: Some(&self.address),
                request_id: self.request_id.as_deref(),
                detail,
            })
            .await
            .map_err(ApiError::from)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SignIn {
    password: String,
    /// A client-named token is never honored outside the `client-session-token` proof seam:
    /// the field parses so the seam can prove the guard, and production ignores it.
    session: Option<String>,
}

/// `POST /v1/operator/session` with `{"password": "..."}`.
/// The bounded argon2 check: a semaphore caps concurrent verifications, the work runs off
/// the async threads, and the answer is only `bool` — the hash never leaves the helper.
async fn verify_password(
    app: &App,
    hash: crate::config::Secret,
    password: String,
) -> Result<bool, ApiError> {
    let permit = tokio::time::timeout(
        Duration::from_secs(2),
        app.hash_gate.clone().acquire_owned(),
    )
    .await
    .map_err(|_| ApiError::Overloaded)?
    .map_err(|_| ApiError::Overloaded)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        PasswordHash::new(hash.expose()).is_ok_and(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
    })
    .await
    .map_err(|_| ApiError::Internal(anyhow::anyhow!("password check task failed")))
}

pub async fn sign_in(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<Peer>,
    headers: HeaderMap,
    // Read only after the rate limit (see `intake::read_body`).
    body: axum::body::Body,
) -> Result<Response, ApiError> {
    let Some(hash) = app.config.operator_password_hash.clone() else {
        return Err(ApiError::SignInDisabled);
    };
    let caller = Caller::of(&app, peer, &headers);
    let address = crate::intake::client_address_of(
        &headers,
        peer.ip(),
        &app.config.trusted_proxies,
        crate::mutated!(app.config, crate::config::Mutation::TrustAnyForwarded),
    );
    // JSON only: a cross-site form cannot send it without a preflight. Checked before the
    // per-address charge so requests that cannot be sign-in attempts cannot churn the
    // bounded table either.
    let is_json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    if !is_json && !crate::mutated!(app.config, crate::config::Mutation::NoContentType) {
        return Err(ApiError::UnsupportedMediaType);
    }
    // Per address first, alone: a request refused here never touches a verification budget.
    if let Err(RetryAfter(secs)) = app.sign_in.admit(AddressKey::of(address), app.now_ms()) {
        return Err(ApiError::RateLimited {
            retry_after_secs: secs,
        });
    }
    let body = crate::intake::read_body(body).await?;
    let SignIn { password, session } =
        serde_json::from_slice(&body).map_err(|_| ApiError::MalformedBody)?;
    if password.is_empty() || password.len() > 1024 {
        return Err(ApiError::Unauthorized);
    }
    // Only a request that reaches verification spends a budget, and a right password gives
    // the token back, so budgets count failed verifications only. An address the operator
    // signed in from in the last 30 days spends its own day-long budget rather than the
    // shared minute one: a flood from elsewhere cannot lock the operator out of their usual
    // place (D-052), and it cannot guess past its own bound either.
    // The known-address set lives in memory: it cannot fail behind a database, and no
    // unauthenticated path reaches a pool to starve it (N10). A restart forgets it — then
    // the operator takes the global budget like anyone, bounded but never refused at 5xx.
    let known = app
        .sign_in
        .is_known_address(&AddressKey::of(address), app.now_ms());
    let reserved_known = app
        .sign_in
        .reserve_for(known, AddressKey::of(address), app.now_ms())
        .map_err(|RetryAfter(secs)| ApiError::RateLimited {
            retry_after_secs: secs,
        })?;
    let verified = verify_password(&app, hash, password).await?;
    if !verified {
        tracing::info!("operator sign-in refused");
        caller
            .audit(&app, "operator.sign_in_failed", "unknown", "{}")
            .await?;
        return Err(ApiError::Unauthorized);
    }

    if let Some(key) = reserved_known {
        app.sign_in.give_back_known(key);
    } else {
        app.sign_in.give_back();
    }
    // The proof's seam: honor a client-named session token so the attack that forbids it
    // has something to prove — the field is parsed and ignored everywhere else (M3).
    let token = {
        #[cfg(feature = "mutation-seams")]
        if crate::mutated!(app.config, crate::config::Mutation::ClientSessionToken) {
            session.filter(|t| !t.is_empty()).unwrap_or_else(new_token)
        } else {
            new_token()
        }
        #[cfg(not(feature = "mutation-seams"))]
        let _ = session;
        #[cfg(not(feature = "mutation-seams"))]
        new_token()
    };
    app.db
        .create_session(&digest(&token), app.config.session_ttl)
        .await?;
    // A fresh session is warm: the first requests after sign-in answer from memory.
    let now = app.now_ms();
    app.session_gate.mark_verified(
        digest(&token),
        trust_until(
            now,
            u64::try_from(app.config.session_ttl.as_millis()).unwrap_or(u64::MAX),
            u64::try_from(app.config.session_idle.as_millis()).unwrap_or(u64::MAX),
        ),
        now,
    );
    app.sign_in.mark_address(AddressKey::of(address), now);
    caller
        .audit(&app, "operator.sign_in", "operator", "{}")
        .await?;
    tracing::info!("operator signed in");
    let secure = if app.config.cookie_secure {
        "; Secure"
    } else {
        ""
    };
    let cookie = format!(
        "{}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}{secure}",
        cookie_name(&app),
        app.config.session_ttl.as_secs()
    );
    let mut response = (StatusCode::OK, Json(json!({ "signed_in": true }))).into_response();
    let cookie = HeaderValue::from_str(&cookie)
        .map_err(|_| ApiError::Internal(anyhow::anyhow!("session cookie is not a header value")))?;
    response.headers_mut().insert(header::SET_COOKIE, cookie);
    Ok(response)
}

/// `GET /v1/operator/session`: 200 when signed in, else 401.
pub async fn session(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<Peer>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    require_operator(&app, peer, &headers).await?;
    Ok(no_store(Json(json!({ "signed_in": true })).into_response()))
}

/// `DELETE /v1/operator/session`. Revocation is a fact, not a wish: 204 only once the row
/// is confirmed gone (or was never there for one of ours); anything unconfirmed answers
/// 503 `{"signed_out":false}` — and keeps the cookie, because the session is still live and
/// the client must be able to retry rather than be told it is signed out. A digest that
/// never proved itself gets 401 like every other operator route; tombstones — at least as
/// long as the row itself could live — stay for digests that were ours or really died.
pub async fn sign_out(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<Peer>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let name = cookie_name(&app);
    let secure = if app.config.cookie_secure {
        "; Secure"
    } else {
        ""
    };
    let clear = format!("{name}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0{secure}");
    let answer = |status: StatusCode, body: Option<serde_json::Value>, clear_cookie: bool| {
        let mut response = match body {
            Some(body) => (status, Json(body)).into_response(),
            None => status.into_response(),
        };
        if clear_cookie && let Ok(value) = HeaderValue::from_str(&clear) {
            response.headers_mut().insert(header::SET_COOKIE, value);
        }
        response
    };
    let Some(token) = session_token(&headers, name) else {
        return Err(ApiError::Unauthorized);
    };
    let token_digest = digest(token);
    let now = app.now_ms();
    // Ours — known from a live check, or tombstoned by a sign-out already — ends on the
    // connection only sign-outs and their retries reach: no unauthenticated path — cookie
    // flood or sign-in flood — can queue ahead of it (N7 residual).
    // Anyone else pays the check's charge first: an unsigned delete is a database write.
    let ours = app.session_gate.is_known(&token_digest, now)
        || app.session_gate.is_revoked(&token_digest, now);
    // The proof's seam: claim the delete without running it — then evict the memory, so the
    // replay that must refuse it is answered by the database, not by a warm cache (M2).
    let faked = ours && crate::mutated!(app.config, crate::config::Mutation::NoSessionDelete);
    let deleted = if ours {
        if faked {
            Some(true)
        } else {
            app.signout_db.delete_session(&token_digest).await.ok()
        }
    } else {
        let address = AddressKey::of(crate::intake::client_address_of(
            &headers,
            peer.ip(),
            &app.config.trusted_proxies,
            crate::mutated!(app.config, crate::config::Mutation::TrustAnyForwarded),
        ));
        if app.session_gate.charge(address, now).is_err() {
            None // the check could not even be afforded — nothing is confirmed
        } else {
            match app.sessions.delete_session(&token_digest).await {
                Ok(true) => {
                    app.session_gate.refund(address);
                    Some(true)
                }
                Ok(false) => Some(false),
                Err(_) => None,
            }
        }
    };
    // The tombstone outlives the session itself: until the row could no longer be live the
    // digest must never be re-warmed — a delete that was never confirmed is covered by it
    // while the background retry finishes the job (N7 residual).
    let until =
        now.saturating_add(u64::try_from(app.config.session_ttl.as_millis()).unwrap_or(u64::MAX));
    match (deleted, ours) {
        // Confirmed gone — or, for a digest that was ours, confirmed absent: either way it
        // is dead, to the table and to memory.
        (Some(true), _) | (Some(false), true) => {
            if faked {
                app.session_gate.unknow(&token_digest);
            } else {
                app.session_gate.forget(&token_digest, now, until);
            }
            Ok(answer(StatusCode::NO_CONTENT, None, true))
        }
        // A forged or long-dead digest: nothing was ours to end.
        (Some(false), false) => Ok(answer(StatusCode::UNAUTHORIZED, None, true)),
        // Refused or failed before the row was confirmed gone — say so, keep the cookie,
        // tombstone it for its whole TTL and let the tracked retry finish the delete.
        _ => {
            if ours {
                app.session_gate.forget(&token_digest, now, until);
                retry_delete(&app, token_digest);
            }
            Ok(answer(
                StatusCode::SERVICE_UNAVAILABLE,
                Some(json!({ "signed_out": false })),
                false,
            ))
        }
    }
}

/// A delete the pool refused once keeps retrying on the sign-out connection until the row
/// is confirmed gone or the session's own expiry makes the retry pointless — the tombstone
/// covers it either way, and the `JoinSet` keeps the task tracked rather than leaked.
fn retry_delete(app: &App, token_digest: [u8; 32]) {
    let sessions = app.signout_db.clone();
    let deadline = std::time::Instant::now() + app.config.session_ttl;
    app.retries
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .spawn(async move {
            let mut backoff = Duration::from_millis(250);
            loop {
                match sessions.delete_session(&token_digest).await {
                    Ok(_) => return,
                    Err(error) if std::time::Instant::now() < deadline => {
                        tracing::warn!(%error, "session delete retry failed");
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(Duration::from_secs(30));
                    }
                    Err(error) => {
                        tracing::error!(%error, "session delete never confirmed before expiry");
                        return;
                    }
                }
            }
        });
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboxParams {
    site: Option<String>,
    before: Option<String>,
    limit: Option<u16>,
}

/// `GET /v1/operator/leads?site=&before=&limit=`: newest first, at most 200 per page.
pub async fn inbox(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<Peer>,
    headers: HeaderMap,
    params: Result<Query<InboxParams>, QueryRejection>,
) -> Result<Response, ApiError> {
    require_operator(&app, peer, &headers).await?;
    let Query(params) = params.map_err(|_| ApiError::MalformedBody)?;
    let site = params
        .site
        .map(|s| s.parse::<SiteId>())
        .transpose()
        .map_err(|_| ApiError::MalformedBody)?;
    let before = params
        .before
        .map(|s| s.parse::<LeadId>())
        .transpose()
        .map_err(|_| ApiError::MalformedBody)?;
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    let leads = app
        .db
        .inbox_page(InboxQuery {
            site,
            before,
            limit,
        })
        .await?;
    let detail = json!({ "site": site, "rows": leads.len() }).to_string();
    Caller::of(&app, peer, &headers)
        .audit(&app, "operator.inbox_read", "operator", &detail)
        .await?;
    let next_before = (leads.len() == usize::from(limit))
        .then(|| leads.last().map(|l| l.id))
        .flatten();
    let body = json!({
        "leads": leads.iter().map(render).collect::<Vec<_>>(),
        "next_before": next_before,
    });
    Ok(no_store(Json(body).into_response()))
}

fn render(lead: &InboxLead) -> serde_json::Value {
    let ts = |t: time::OffsetDateTime| t.format(&Rfc3339).ok();
    json!({
        "id": lead.id,
        "tenant_id": lead.tenant_id,
        "site_id": lead.site_id,
        "site_name": lead.site_name,
        "received_at": ts(lead.received_at),
        "retain_until": ts(lead.retain_until),
        "name": lead.name,
        "email": lead.email,
        "phone": lead.phone,
        "service": lead.service,
        "area": lead.area,
        "message": lead.message,
        "is_test": lead.is_test,
        "consent_notice": lead.consent_notice,
        "state": lead.state.as_str(),
        "notification": {
            "attempts": lead.attempts,
            "uncertain_attempts": lead.uncertain_attempts,
            "last_error": lead.last_error,
            "delivered_at": lead.delivered_at.and_then(ts),
            "next_attempt_at": lead.next_attempt_at.and_then(ts),
        },
    })
}

pub(crate) fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// 401 unless the request carries a live operator session (not expired, not idle too long).
/// The check is a database write; per address it is charged on failure and refunded on a live
/// session, and it runs on its own small pool so a flood of bad cookies cannot starve intake's.
pub(crate) async fn require_operator(
    app: &App,
    peer: Peer,
    headers: &HeaderMap,
) -> Result<(), ApiError> {
    if crate::mutated!(app.config, crate::config::Mutation::NoSessionCheck) {
        return Ok(());
    }
    let Some(token) = session_token(headers, cookie_name(app)) else {
        return Err(ApiError::Unauthorized);
    };
    let token_digest = digest(token);
    let now = app.now_ms();
    // A digest a sign-out killed is refused before anything else: its row may still exist
    // (a delete that errored or a check that raced), so the tombstone must answer for it.
    if app.session_gate.is_revoked(&token_digest, now) {
        return Err(ApiError::Unauthorized);
    }
    // A digest trusted from a live check moments ago needs no database hit and no charge.
    if app.session_gate.was_verified(&token_digest, now) {
        return Ok(());
    }
    // One that once proved live re-checks without a charge: strangers on the operator's NAT
    // cannot hold a known session at 429 at every renewal. An unknown digest pays the charge
    // first, and keeps paying it on failure — a database error spends it too, since the only
    // attacker-caused errors are this pool's own timeouts under flood.
    let known = app.session_gate.is_known(&token_digest, now);
    let address = AddressKey::of(crate::intake::client_address_of(
        headers,
        peer.ip(),
        &app.config.trusted_proxies,
        crate::mutated!(app.config, crate::config::Mutation::TrustAnyForwarded),
    ));
    if !known && let Err(RetryAfter(secs)) = app.session_gate.charge(address, now) {
        return Err(ApiError::RateLimited {
            retry_after_secs: secs,
        });
    }
    match app
        .sessions
        .session_lifetime(&token_digest, app.config.session_idle)
        .await
    {
        Ok(Some(remaining_secs)) => {
            if !known {
                app.session_gate.refund(address);
            }
            let remaining_ms = u64::try_from(
                Duration::try_from_secs_f64(remaining_secs.max(0.0))
                    .unwrap_or_default()
                    .as_millis(),
            )
            .unwrap_or(u64::MAX);
            let idle_ms = u64::try_from(app.config.session_idle.as_millis()).unwrap_or(u64::MAX);
            app.session_gate.mark_verified(
                token_digest,
                trust_until(now, remaining_ms, idle_ms),
                now,
            );
            Ok(())
        }
        // The table says dead: drop the memory so the next sighting of this digest pays
        // the charge like any stranger — a dead session re-checking free is a flood's gift.
        Ok(None) => {
            app.session_gate.unknow(&token_digest);
            Err(ApiError::Unauthorized)
        }
        Err(e) => Err(e.into()),
    }
}

/// How long a live check may be trusted without asking again: never past the 60 s cap, the
/// session's own remaining lifetime, or half its idle timeout (a check must refresh at
/// least that often, or the idle deadline is a lie).
fn trust_until(now_ms: u64, remaining_ms: u64, idle_ms: u64) -> u64 {
    now_ms.saturating_add(VERIFIED_SESSION_MS.min(remaining_ms).min(idle_ms / 2))
}

/// The session token from the `Cookie` header, if it is shaped like one (64 hex digits).
fn session_token<'h>(headers: &'h HeaderMap, cookie: &str) -> Option<&'h str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == cookie)
        .map(|(_, value)| value)
        .filter(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// 32 bytes from the operating system's CSPRNG (two v4 UUIDs: 244 random bits), hex-encoded.
fn new_token() -> String {
    let mut bytes = [0_u8; 32];
    bytes[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    bytes[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    hex::encode(bytes)
}

fn digest(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u32) -> AddressKey {
        AddressKey::V4(n)
    }

    #[test]
    fn refusals_per_address_never_spend_the_global_budget() {
        let gate = SignInGate::new();
        let admitted = (0..25).filter(|_| gate.admit(key(1), 0).is_ok()).count();
        assert_eq!(admitted, 5);
        // All 20 global tokens are still there.
        assert_eq!((0..25).filter(|_| gate.reserve(0).is_ok()).count(), 20);
    }

    #[test]
    fn a_right_password_gives_its_global_token_back() {
        let gate = SignInGate::new();
        for _ in 0..20 {
            gate.reserve(0).unwrap();
        }
        assert!(gate.reserve(0).is_err());
        gate.give_back();
        assert!(gate.reserve(0).is_ok());
        assert!(gate.reserve(0).is_err());
    }

    #[test]
    fn the_global_budget_refills_one_verification_every_three_seconds() {
        let gate = SignInGate::new();
        for _ in 0..20 {
            gate.reserve(0).unwrap();
        }
        assert!(gate.reserve(2_999).is_err());
        assert!(gate.reserve(3_000).is_ok());
        assert!(gate.reserve(3_000).is_err());
    }

    #[test]
    fn a_known_address_has_a_day_budget_of_its_own() {
        let gate = SignInGate::new();
        for _ in 0..100 {
            gate.reserve_known(key(1), 0).unwrap();
        }
        // The 101st failed verification in a day is refused, and the shared global
        // budget is untouched by all of them.
        assert!(gate.reserve_known(key(1), 0).is_err());
        assert_eq!((0..25).filter(|_| gate.reserve(0).is_ok()).count(), 20);
        // One token back every 864 s (a refused look spends the accrual up to its instant,
        // so the last unit lands a few milliseconds late); a right password gives it back.
        assert!(gate.reserve_known(key(1), 863_999).is_err());
        assert!(gate.reserve_known(key(1), 864_015).is_ok());
        gate.give_back_known(key(1));
        assert!(gate.reserve_known(key(1), 864_015).is_ok());
        assert!(gate.reserve_known(key(1), 864_015).is_err());
        // Another known address gets its own day.
        assert_eq!(
            (0..100)
                .filter(|_| gate.reserve_known(key(2), 0).is_ok())
                .count(),
            100
        );
    }

    #[test]
    fn a_session_check_is_charged_per_address_and_refunded_on_success() {
        let gate = SessionGate::new();
        // Twenty failed checks spend one address's minute; the twenty-first is refused
        // without reaching the database.
        for _ in 0..20 {
            gate.charge(key(1), 0).unwrap();
        }
        assert!(gate.charge(key(1), 0).is_err());
        // A live session (or a delete that found a row) gives the charge back.
        gate.refund(key(1));
        assert!(gate.charge(key(1), 0).is_ok());
        // Another address's checks are untouched.
        assert_eq!(
            (0..20).filter(|_| gate.charge(key(2), 0).is_ok()).count(),
            20
        );
    }

    #[test]
    fn a_verified_digest_skips_the_gate_until_its_ttl_or_a_forget() {
        let gate = SessionGate::new();
        let digest = [7u8; 32];
        // The address's whole minute is spent — the verified digest still answers.
        for _ in 0..20 {
            gate.charge(key(1), 0).unwrap();
        }
        gate.mark_verified(digest, 60_000, 0);
        assert!(gate.was_verified(&digest, 0));
        assert!(gate.was_verified(&digest, 59_999));
        assert!(!gate.was_verified(&digest, 60_000));
        // The trust lapses but the digest stays known: it re-checks without a charge.
        assert!(gate.is_known(&digest, 70_000));
        assert!(!gate.is_known(&[9u8; 32], 0));
        // Sign-out tombstones it: a mark inside the tombstone (a racing check) is dropped.
        gate.forget(&digest, 70_000, 130_000);
        assert!(!gate.is_known(&digest, 70_000));
        gate.mark_verified(digest, 200_000, 70_001);
        assert!(!gate.was_verified(&digest, 70_001));
        // Past the tombstone a fresh live check may mark it again.
        gate.mark_verified(digest, 200_000, 131_000);
        assert!(gate.was_verified(&digest, 131_000));
        // Forgetting leaves the per-address budget where the failures left it.
        assert!(gate.charge(key(1), 0).is_err());
    }

    #[test]
    fn a_spent_known_budget_falls_back_to_the_global_one() {
        let gate = SignInGate::new();
        for _ in 0..100 {
            gate.reserve_known(key(1), 0).unwrap();
        }
        // The day is spent — a known address still gets the shared minute, not a 429.
        assert_eq!(gate.reserve_for(true, key(1), 0).unwrap(), None);
        // While the day has room the known budget answers.
        assert_eq!(gate.reserve_for(true, key(2), 0).unwrap(), Some(key(2)));
        // Unknown addresses only ever see the global budget.
        assert!(gate.reserve_for(false, key(1), 0).is_ok());
    }

    #[test]
    fn the_session_charge_refills() {
        let gate = SessionGate::new();
        for _ in 0..20 {
            gate.charge(key(1), 0).unwrap();
        }
        assert!(gate.charge(key(1), 2_999).is_err());
        assert!(gate.charge(key(1), 3_000).is_ok());
    }

    #[test]
    fn trust_ends_at_the_soonest_of_the_cache_absolute_and_idle_bounds() {
        // 5 s of session left beats the 60 s cache ceiling outright.
        assert_eq!(trust_until(1_000, 5_000, 3_600_000), 6_000);
        // A session with days left still trusts only 60 s.
        assert_eq!(trust_until(1_000, 86_400_000, 3_600_000), 61_000);
        // Half the idle window can be the tightest of the three.
        assert_eq!(trust_until(1_000, 86_400_000, 40_000), 21_000);
        // Nothing left: no trust at all.
        assert_eq!(trust_until(1_000, 0, 3_600_000), 1_000);
    }

    #[test]
    fn a_dead_digest_is_forgotten_not_kept_known() {
        let gate = SessionGate::new();
        let digest = [8u8; 32];
        gate.mark_verified(digest, 60_000, 0);
        assert!(gate.is_known(&digest, 70_000));
        // The table said dead: the memory lets go — the next sighting pays the charge.
        gate.unknow(&digest);
        assert!(!gate.is_known(&digest, 70_000));
        assert!(!gate.was_verified(&digest, 0));
        // And unknowing never tombstones: a re-created session may warm again.
        gate.mark_verified(digest, 200_000, 70_001);
        assert!(gate.was_verified(&digest, 70_001));
    }

    #[test]
    fn tokens_are_64_hex_digits_and_differ() {
        let a = new_token();
        let b = new_token();
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn only_a_well_formed_session_cookie_is_read() {
        let token = new_token();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("other=1; {COOKIE_SECURE}={token}")).unwrap(),
        );
        assert_eq!(session_token(&headers, COOKIE_SECURE), Some(token.as_str()));
        assert_eq!(
            session_token(&headers, COOKIE_PLAIN),
            None,
            "names are not interchangeable"
        );
        let mut bad = HeaderMap::new();
        bad.insert(
            header::COOKIE,
            HeaderValue::from_static("intent_operator=short"),
        );
        assert_eq!(session_token(&bad, COOKIE_PLAIN), None);
    }
}
