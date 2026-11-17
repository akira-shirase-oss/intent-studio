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
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest as _, Sha256};
use time::format_description::well_known::Rfc3339;

use crate::{
    app::{App, Peer},
    error::ApiError,
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

/// Who asked, for the audit: the client address and the request id.
struct Caller {
    address: String,
    request_id: Option<String>,
}

impl Caller {
    fn of(app: &App, peer: Peer, headers: &HeaderMap) -> Self {
        let address =
            crate::intake::client_address_of(headers, peer.ip(), &app.config.trusted_proxies);
        let request_id = headers
            .get(&crate::app::REQUEST_ID)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        Self {
            address: address.to_string(),
            request_id,
        }
    }

    async fn audit(
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
}

/// `POST /v1/operator/session` with `{"password": "..."}`.
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
    let address =
        crate::intake::client_address_of(&headers, peer.ip(), &app.config.trusted_proxies);
    // Per address first, then everywhere together: guesses spread over many addresses are
    // bounded too (D-041). The cost: during such an attack the operator may wait a minute.
    let now = app.now_ms();
    for check in [
        app.sign_in_limiter.check(AddressKey::of(address), now),
        app.sign_in_global.check(0, now),
    ] {
        if let Err(RetryAfter(secs)) = check {
            return Err(ApiError::RateLimited {
                retry_after_secs: secs,
            });
        }
    }
    // JSON only: a cross-site form cannot send it without a preflight.
    let is_json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    if !is_json {
        return Err(ApiError::UnsupportedMediaType);
    }
    let body = crate::intake::read_body(body).await?;
    let SignIn { password } = serde_json::from_slice(&body).map_err(|_| ApiError::MalformedBody)?;
    if password.is_empty() || password.len() > 1024 {
        return Err(ApiError::Unauthorized);
    }
    let permit = tokio::time::timeout(
        Duration::from_secs(2),
        app.hash_gate.clone().acquire_owned(),
    )
    .await
    .map_err(|_| ApiError::Overloaded)?
    .map_err(|_| ApiError::Overloaded)?;
    let verified = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        PasswordHash::new(hash.expose()).is_ok_and(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
    })
    .await
    .map_err(|_| ApiError::Internal(anyhow::anyhow!("password check task failed")))?;
    if !verified {
        tracing::info!("operator sign-in refused");
        caller
            .audit(&app, "operator.sign_in_failed", "unknown", "{}")
            .await?;
        return Err(ApiError::Unauthorized);
    }

    let token = new_token();
    app.db
        .create_session(&digest(&token), app.config.session_ttl)
        .await?;
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
pub async fn session(State(app): State<App>, headers: HeaderMap) -> Result<Response, ApiError> {
    require_operator(&app, &headers).await?;
    Ok(no_store(Json(json!({ "signed_in": true })).into_response()))
}

/// `DELETE /v1/operator/session`.
pub async fn sign_out(State(app): State<App>, headers: HeaderMap) -> Result<Response, ApiError> {
    let name = cookie_name(&app);
    if let Some(token) = session_token(&headers, name) {
        app.db.delete_session(&digest(token)).await?;
    }
    let secure = if app.config.cookie_secure {
        "; Secure"
    } else {
        ""
    };
    let clear = format!("{name}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0{secure}");
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&clear)
            .map_err(|_| ApiError::Internal(anyhow::anyhow!("cookie is not a header value")))?,
    );
    Ok(response)
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
    require_operator(&app, &headers).await?;
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

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// 401 unless the request carries a live operator session (not expired, not idle too long).
async fn require_operator(app: &App, headers: &HeaderMap) -> Result<(), ApiError> {
    let Some(token) = session_token(headers, cookie_name(app)) else {
        return Err(ApiError::Unauthorized);
    };
    if app
        .db
        .session_is_valid(&digest(token), app.config.session_idle)
        .await?
    {
        Ok(())
    } else {
        Err(ApiError::Unauthorized)
    }
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
