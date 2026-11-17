//! The notification provider port (D-005, D-020, D-032).
//!
//! A provider takes one [`Notification`] and reports an
//! [`AttemptOutcome`](intent_domain::outbox::AttemptOutcome); it never sees the database and the
//! notifier never holds a transaction while it runs. Two adapters exist in I0:
//!
//! - [`WebhookProvider`]: JSON over HTTP to an operator-configured endpoint, with the outbox row
//!   id as the `Idempotency-Key`. The provider fixture (`intent-provider-fixture`) speaks it, so
//!   drills and load runs exercise a real socket that can refuse, hang or recover. Plain HTTP
//!   only until the first real e-mail adapter brings TLS (I3).
//! - [`Recorder`]: accepts and counts in memory, for development without a fixture.
//!
//! No real e-mail or SMS provider and no provider keys exist in I0.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use intent_domain::{ids::OutboxId, outbox::AttemptOutcome};
use serde::Serialize;

/// One notification to send. `Debug` prints sizes, never the recipient or the text.
#[derive(Clone, Serialize)]
pub struct Notification {
    /// The outbox row id, sent as the idempotency key.
    pub event_id: OutboxId,
    /// The operator-configured recipient.
    pub to: String,
    /// The subject (no submitted values, D-019).
    pub subject: String,
    /// The plain-text body.
    pub text: String,
}

impl std::fmt::Debug for Notification {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Notification")
            .field("event_id", &self.event_id)
            .field("text_chars", &self.text.chars().count())
            .finish_non_exhaustive()
    }
}

/// A provider configuration problem, found at startup.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// The endpoint is not a plain `http://` URL.
    #[error(
        "the webhook endpoint must be an http:// URL (TLS arrives with the first real provider adapter)"
    )]
    UnsupportedScheme,
    /// The endpoint does not parse.
    #[error("the webhook endpoint is not a valid URL")]
    InvalidUrl,
    /// The HTTP client could not be built.
    #[error("could not build the HTTP client")]
    Client(#[source] reqwest::Error),
}

/// The configured provider.
#[derive(Debug, Clone)]
pub enum Provider {
    /// JSON over HTTP.
    Webhook(WebhookProvider),
    /// In memory.
    Recorder(Recorder),
}

impl Provider {
    /// Attempts one delivery within `deadline`.
    pub async fn send(&self, notification: &Notification, deadline: Duration) -> AttemptOutcome {
        match self {
            Self::Webhook(provider) => provider.send(notification, deadline).await,
            Self::Recorder(recorder) => recorder.send(notification),
        }
    }

    /// A short name for logs and metrics.
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Webhook(_) => "webhook",
            Self::Recorder(_) => "recorder",
        }
    }
}

/// JSON over HTTP to one endpoint.
#[derive(Debug, Clone)]
pub struct WebhookProvider {
    client: reqwest::Client,
    url: reqwest::Url,
}

impl WebhookProvider {
    /// Builds the adapter: one pooled client, short connect timeout, no ambient proxy.
    pub fn new(url: &str) -> Result<Self, ProviderError> {
        let url = reqwest::Url::parse(url).map_err(|_| ProviderError::InvalidUrl)?;
        if url.scheme() != "http" {
            return Err(ProviderError::UnsupportedScheme);
        }
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .pool_idle_timeout(Duration::from_secs(60))
            .pool_max_idle_per_host(64)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(ProviderError::Client)?;
        Ok(Self { client, url })
    }

    /// One attempt. The response body is never read: provider text is not stored or logged.
    pub async fn send(&self, notification: &Notification, deadline: Duration) -> AttemptOutcome {
        let request = self
            .client
            .post(self.url.clone())
            .timeout(deadline)
            .header("idempotency-key", notification.event_id.to_string())
            .json(notification);
        match request.send().await {
            Ok(response) => classify_status(response.status().as_u16()),
            Err(error) if error.is_connect() => AttemptOutcome::Unavailable,
            // A timeout, or a failure after the request may have been sent: the provider may
            // have it. Uncertain, retried with the same idempotency key.
            Err(_) => AttemptOutcome::TimedOut,
        }
    }
}

/// How a provider's HTTP status maps to an outcome (D-046).
///
/// Only a refusal of this notification itself (400, 413, 415, 422) is permanent. Everything
/// else that is not a 2xx is retried on the schedule: the provider being busy or down (408,
/// 429, 5xx), and our own configuration being wrong (401, 403, 404, a 3xx we do not follow),
/// which the operator can fix while the row waits.
pub const fn classify_status(status: u16) -> AttemptOutcome {
    match status {
        200..=299 => AttemptOutcome::Accepted,
        400 | 413 | 415 | 422 => AttemptOutcome::Rejected,
        _ => AttemptOutcome::Unavailable,
    }
}

/// Accepts everything and counts it.
#[derive(Debug, Clone, Default)]
pub struct Recorder {
    accepted: Arc<AtomicU64>,
}

impl Recorder {
    fn send(&self, _notification: &Notification) -> AttemptOutcome {
        self.accepted.fetch_add(1, Ordering::Relaxed);
        AttemptOutcome::Accepted
    }

    /// Notifications accepted so far.
    pub fn accepted(&self) -> u64 {
        self.accepted.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU16;

    use axum::{Router, http::HeaderMap, routing::post};

    use super::*;

    fn notification() -> Notification {
        Notification {
            event_id: "0192f4a0-7c1e-7b3a-9d2e-1f0a2b3c4d5e".parse().unwrap(),
            to: "owner@example.invalid".into(),
            subject: "New quote request: Site".into(),
            text: "Name: Ada".into(),
        }
    }

    /// Test listeners stay inside the repository's port range (59490–59498 for this crate;
    /// 59499 is kept closed for the refused-connection test).
    async fn bind_in_range() -> tokio::net::TcpListener {
        for port in 59490..=59498 {
            if let Ok(listener) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
                return listener;
            }
        }
        panic!("no free port in 59490..=59498");
    }

    async fn serve(status: Arc<AtomicU16>, delay: Duration) -> String {
        let app = Router::new().route(
            "/notify",
            post(move |headers: HeaderMap| {
                let status = status.clone();
                async move {
                    assert_eq!(
                        headers.get("idempotency-key").unwrap(),
                        "0192f4a0-7c1e-7b3a-9d2e-1f0a2b3c4d5e"
                    );
                    tokio::time::sleep(delay).await;
                    axum::http::StatusCode::from_u16(status.load(Ordering::Relaxed)).unwrap()
                }
            }),
        );
        let listener = bind_in_range().await;
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{address}/notify")
    }

    #[test]
    fn statuses_map_to_outcomes() {
        assert_eq!(classify_status(202), AttemptOutcome::Accepted);
        // The provider did not take it, for now: retried.
        for status in [408, 429, 500, 503] {
            assert_eq!(
                classify_status(status),
                AttemptOutcome::Unavailable,
                "{status}"
            );
        }
        // Our configuration is wrong (credentials, permission, endpoint, a redirect we do not
        // follow): the operator can fix it, so the schedule keeps trying meanwhile.
        for status in [301, 302, 307, 401, 403, 404, 405] {
            assert_eq!(
                classify_status(status),
                AttemptOutcome::Unavailable,
                "{status}"
            );
        }
        // This notification itself is refused: retrying cannot help.
        for status in [400, 413, 415, 422] {
            assert_eq!(
                classify_status(status),
                AttemptOutcome::Rejected,
                "{status}"
            );
        }
    }

    #[test]
    fn only_plain_http_endpoints_are_accepted_in_i0() {
        assert!(matches!(
            WebhookProvider::new("https://provider.example/notify"),
            Err(ProviderError::UnsupportedScheme)
        ));
        assert!(matches!(
            WebhookProvider::new("not a url"),
            Err(ProviderError::InvalidUrl)
        ));
    }

    #[tokio::test]
    async fn a_live_endpoint_accepts_refuses_and_times_out() {
        let status = Arc::new(AtomicU16::new(202));
        let url = serve(status.clone(), Duration::ZERO).await;
        let provider = WebhookProvider::new(&url).unwrap();
        let deadline = Duration::from_secs(2);
        assert_eq!(
            provider.send(&notification(), deadline).await,
            AttemptOutcome::Accepted
        );
        status.store(503, Ordering::Relaxed);
        assert_eq!(
            provider.send(&notification(), deadline).await,
            AttemptOutcome::Unavailable
        );
        status.store(422, Ordering::Relaxed);
        assert_eq!(
            provider.send(&notification(), deadline).await,
            AttemptOutcome::Rejected
        );

        let slow = WebhookProvider::new(&serve(status, Duration::from_secs(5)).await).unwrap();
        assert_eq!(
            slow.send(&notification(), Duration::from_millis(100)).await,
            AttemptOutcome::TimedOut
        );
    }

    #[tokio::test]
    async fn a_closed_port_is_unavailable_not_uncertain() {
        // Nothing in this repository listens on 59499.
        let provider = WebhookProvider::new("http://127.0.0.1:59499/notify").unwrap();
        assert_eq!(
            provider.send(&notification(), Duration::from_secs(2)).await,
            AttemptOutcome::Unavailable
        );
    }

    #[test]
    fn debug_hides_the_recipient_and_the_text() {
        let printed = format!("{:?}", notification());
        assert!(!printed.contains("owner@") && !printed.contains("Ada"));
    }
}
