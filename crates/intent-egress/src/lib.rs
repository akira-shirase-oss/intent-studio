//! The egress guard (D-012, D-060): the one way Intent Studio fetches a URL a client named.
//!
//! A reference page is a server-side request forgery path into cloud metadata and private
//! services, so every fetch goes through [`Guard::fetch`], which:
//!
//! 1. accepts only `http` and `https` on their standard ports, with no credentials;
//! 2. resolves the host once through its [`Resolve`] port and refuses the whole answer if any
//!    address fails the public-unicast [`policy`] — before any connection is opened;
//! 3. connects through its [`Connect`] port to an address from that same answer (pinned: no
//!    second lookup, so no rebinding between check and connect);
//! 4. speaks HTTP/1.1 itself (`Accept-Encoding: identity`, so no decompression bomb), over
//!    rustls with the Mozilla roots for `https`;
//! 5. follows at most three redirects, each checked from step 1 again, and never from `https`
//!    down to `http`;
//! 6. bounds the whole fetch by one deadline and the body by a size cap.
//!
//! There is no option that disables the policy. Tests replace the resolver and the connector
//! (D-060) and keep the policy.

pub mod policy;

use std::{
    future::Future,
    io,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use http_body_util::{BodyExt as _, Empty};
use hyper::{
    Request, StatusCode,
    body::Bytes,
    header::{self, HeaderValue},
};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite};
use url::Url;

pub use policy::Forbidden;

/// The default deadline for a whole fetch, redirects included (D-012: 10 s).
pub const DEFAULT_DEADLINE: Duration = Duration::from_secs(10);
/// Most redirects followed (D-012: 3).
pub const MAX_REDIRECTS: u8 = 3;
/// Largest page (D-012: 5 MiB).
pub const MAX_PAGE_BYTES: usize = 5 * 1024 * 1024;
/// Largest asset (D-012: 20 MiB).
pub const MAX_ASSET_BYTES: usize = 20 * 1024 * 1024;
/// Most addresses tried from one answer.
const MAX_CONNECT_ATTEMPTS: usize = 2;

/// What is being fetched, which sets the size cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// A reference page (5 MiB).
    Page,
    /// An asset a reference names (20 MiB).
    Asset,
}

impl Purpose {
    const fn max_bytes(self) -> usize {
        match self {
            Self::Page => MAX_PAGE_BYTES,
            Self::Asset => MAX_ASSET_BYTES,
        }
    }
}

/// Why a fetch did not return a body. Stable codes; no variant carries the address or any
/// response content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// Not a URL.
    #[error("invalid_url")]
    InvalidUrl,
    /// Not `http` or `https`.
    #[error("scheme_not_allowed")]
    SchemeNotAllowed,
    /// A user name or password in the URL.
    #[error("credentials_in_url")]
    CredentialsInUrl,
    /// A port other than the scheme's standard one.
    #[error("port_not_allowed")]
    PortNotAllowed,
    /// The address, or one of the addresses the name resolves to, fails the policy.
    #[error("address_forbidden")]
    AddressForbidden(Forbidden),
    /// The name did not resolve.
    #[error("dns_failed")]
    DnsFailed,
    /// No address could be connected to.
    #[error("connect_failed")]
    ConnectFailed,
    /// The TLS handshake failed (an invalid or untrusted certificate included).
    #[error("tls_failed")]
    TlsFailed,
    /// More than [`MAX_REDIRECTS`] redirects.
    #[error("too_many_redirects")]
    TooManyRedirects,
    /// A redirect from `https` to `http`.
    #[error("redirect_downgrade")]
    RedirectDowngrade,
    /// A redirect without a usable `Location`.
    #[error("redirect_invalid")]
    RedirectInvalid,
    /// A status other than 200 or a followed redirect.
    #[error("http_status")]
    HttpStatus(u16),
    /// Larger than the purpose's cap.
    #[error("too_large")]
    TooLarge,
    /// The deadline passed.
    #[error("timeout")]
    Timeout,
    /// The server did not speak HTTP/1.1 correctly.
    #[error("protocol_error")]
    ProtocolError,
}

impl Refusal {
    /// The wire code.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidUrl => "invalid_url",
            Self::SchemeNotAllowed => "scheme_not_allowed",
            Self::CredentialsInUrl => "credentials_in_url",
            Self::PortNotAllowed => "port_not_allowed",
            Self::AddressForbidden(_) => "address_forbidden",
            Self::DnsFailed => "dns_failed",
            Self::ConnectFailed => "connect_failed",
            Self::TlsFailed => "tls_failed",
            Self::TooManyRedirects => "too_many_redirects",
            Self::RedirectDowngrade => "redirect_downgrade",
            Self::RedirectInvalid => "redirect_invalid",
            Self::HttpStatus(_) => "http_status",
            Self::TooLarge => "too_large",
            Self::Timeout => "timeout",
            Self::ProtocolError => "protocol_error",
        }
    }
}

/// Name resolution, as a port: the system resolver in production, a fixture in tests.
pub trait Resolve: Send + Sync {
    /// Every address `host` resolves to.
    fn resolve(&self, host: &str) -> impl Future<Output = io::Result<Vec<IpAddr>>> + Send;
}

/// Opening a connection, as a port: TCP in production, an in-memory pipe in tests. It is only
/// ever given an address the policy accepted.
pub trait Connect: Send + Sync {
    /// The connection.
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;
    /// Connects to `address`.
    fn connect(&self, address: SocketAddr)
    -> impl Future<Output = io::Result<Self::Stream>> + Send;
}

/// The operating system's resolver (`getaddrinfo`, off the async workers).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemResolver;

impl Resolve for SystemResolver {
    async fn resolve(&self, host: &str) -> io::Result<Vec<IpAddr>> {
        Ok(tokio::net::lookup_host((host, 0))
            .await?
            .map(|address| address.ip())
            .collect())
    }
}

/// Plain TCP.
#[derive(Debug, Clone, Copy, Default)]
pub struct TcpConnector;

impl Connect for TcpConnector {
    type Stream = tokio::net::TcpStream;

    async fn connect(&self, address: SocketAddr) -> io::Result<Self::Stream> {
        let stream = tokio::net::TcpStream::connect(address).await?;
        stream.set_nodelay(true)?;
        Ok(stream)
    }
}

/// A fetched body and where it came from.
#[derive(Clone)]
pub struct Fetched {
    /// The URL that answered 200, after redirects.
    pub final_url: Url,
    /// The `Content-Type` it declared, if it was text.
    pub content_type: Option<String>,
    /// The body.
    pub body: Vec<u8>,
    /// Each redirect followed, in order.
    pub redirects: Vec<Url>,
    /// The address the final response came from.
    pub address: SocketAddr,
}

impl std::fmt::Debug for Fetched {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fetched")
            .field("bytes", &self.body.len())
            .field("redirects", &self.redirects.len())
            .finish_non_exhaustive()
    }
}

/// The guard.
pub struct Guard<R, C> {
    resolver: R,
    connector: C,
    tls: tokio_rustls::TlsConnector,
    deadline: Duration,
    user_agent: HeaderValue,
}

impl Guard<SystemResolver, TcpConnector> {
    /// The production guard: system DNS, TCP, rustls with the Mozilla roots.
    pub fn system() -> Result<Self, rustls::Error> {
        Self::new(SystemResolver, TcpConnector, DEFAULT_DEADLINE)
    }
}

impl<R: Resolve, C: Connect> Guard<R, C> {
    /// A guard over these ports. The policy is not a parameter.
    pub fn new(resolver: R, connector: C, deadline: Duration) -> Result<Self, rustls::Error> {
        let roots = rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth();
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Self {
            resolver,
            connector,
            tls: tokio_rustls::TlsConnector::from(Arc::new(config)),
            deadline,
            user_agent: HeaderValue::from_static("IntentStudio-ReferenceImport/1"),
        })
    }

    /// Fetches `raw`, following redirects, all under the guard's rules and deadline.
    pub async fn fetch(&self, raw: &str, purpose: Purpose) -> Result<Fetched, Refusal> {
        tokio::time::timeout(self.deadline, self.fetch_unbounded(raw, purpose))
            .await
            .unwrap_or(Err(Refusal::Timeout))
    }

    async fn fetch_unbounded(&self, raw: &str, purpose: Purpose) -> Result<Fetched, Refusal> {
        let mut url = Url::parse(raw).map_err(|_| Refusal::InvalidUrl)?;
        let mut redirects = Vec::new();
        loop {
            // `_connection` keeps the connection's task alive while the body is read and aborts it
            // when this iteration ends, whatever happened.
            let (address, response, _connection) = self.request(&url).await?;
            let status = response.status();
            if is_redirect(status) {
                if redirects.len() >= usize::from(MAX_REDIRECTS) {
                    return Err(Refusal::TooManyRedirects);
                }
                let location = response
                    .headers()
                    .get(header::LOCATION)
                    .and_then(|v| v.to_str().ok());
                let next = next_hop(&url, location)?;
                redirects.push(next.clone());
                url = next;
                continue;
            }
            if status != StatusCode::OK {
                return Err(Refusal::HttpStatus(status.as_u16()));
            }
            let max = purpose.max_bytes();
            let declared = response
                .headers()
                .get(header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok());
            if declared.is_some_and(|length| length > max as u64) {
                return Err(Refusal::TooLarge);
            }
            let content_type = response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .filter(|v| v.len() <= 100)
                .map(str::to_owned);
            let mut body = response.into_body();
            let mut bytes = Vec::new();
            while let Some(frame) = body.frame().await {
                let frame = frame.map_err(|_| Refusal::ProtocolError)?;
                if let Some(data) = frame.data_ref() {
                    if bytes.len() + data.len() > max {
                        return Err(Refusal::TooLarge);
                    }
                    bytes.extend_from_slice(data);
                }
            }
            return Ok(Fetched {
                final_url: url,
                content_type,
                body: bytes,
                redirects,
                address,
            });
        }
    }

    /// One request: check the URL, resolve once, check every address, connect to one of them,
    /// send a GET. Nothing connects before every check passed.
    async fn request(
        &self,
        url: &Url,
    ) -> Result<
        (
            SocketAddr,
            hyper::Response<hyper::body::Incoming>,
            ConnectionTask,
        ),
        Refusal,
    > {
        let https = match url.scheme() {
            "https" => true,
            "http" => false,
            _ => return Err(Refusal::SchemeNotAllowed),
        };
        if !url.username().is_empty() || url.password().is_some() {
            return Err(Refusal::CredentialsInUrl);
        }
        // The URL parser removes a port equal to the scheme's default; any port left is a
        // non-standard one.
        if url.port().is_some() {
            return Err(Refusal::PortNotAllowed);
        }
        let port = if https { 443 } else { 80 };
        let host = url.host().ok_or(Refusal::InvalidUrl)?;
        let (addresses, server_name) = match host {
            url::Host::Ipv4(ip) => (vec![IpAddr::V4(ip)], None),
            url::Host::Ipv6(ip) => (vec![IpAddr::V6(ip)], None),
            url::Host::Domain(name) => {
                let addresses = self
                    .resolver
                    .resolve(name)
                    .await
                    .map_err(|_| Refusal::DnsFailed)?;
                (addresses, Some(name.to_owned()))
            }
        };
        if addresses.is_empty() {
            return Err(Refusal::DnsFailed);
        }
        // The whole answer is judged: one private address among public ones is a refusal, not a
        // choice.
        for ip in &addresses {
            policy::check(*ip).map_err(Refusal::AddressForbidden)?;
        }
        let mut stream = None;
        for ip in addresses.iter().take(MAX_CONNECT_ATTEMPTS) {
            let address = SocketAddr::new(*ip, port);
            if let Ok(connected) = self.connector.connect(address).await {
                stream = Some((address, connected));
                break;
            }
        }
        let (address, stream) = stream.ok_or(Refusal::ConnectFailed)?;
        let path = &url[url::Position::BeforePath..url::Position::AfterQuery];
        let authority = url.host_str().ok_or(Refusal::InvalidUrl)?.to_owned();
        let request = Request::get(path)
            .header(header::HOST, authority.as_str())
            .header(header::USER_AGENT, self.user_agent.clone())
            .header(
                header::ACCEPT,
                "text/html,application/xhtml+xml,image/*;q=0.8,*/*;q=0.5",
            )
            .header(header::ACCEPT_ENCODING, "identity")
            .header(header::CONNECTION, "close")
            .body(Empty::<Bytes>::new())
            .map_err(|_| Refusal::InvalidUrl)?;
        let (response, connection) = if https {
            let name = match server_name {
                Some(name) => rustls::pki_types::ServerName::try_from(name)
                    .map_err(|_| Refusal::InvalidUrl)?,
                None => rustls::pki_types::ServerName::IpAddress(address.ip().into()),
            };
            let tls = self
                .tls
                .connect(name, stream)
                .await
                .map_err(|_| Refusal::TlsFailed)?;
            send(tls, request).await?
        } else {
            send(stream, request).await?
        };
        Ok((address, response, connection))
    }
}

/// The task driving one HTTP/1.1 connection; aborted when dropped, so no connection outlives the
/// fetch that opened it.
struct ConnectionTask(tokio::task::JoinHandle<()>);

impl Drop for ConnectionTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Where a redirect leads: `Location` resolved against the current URL, never from `https` down
/// to `http`. Every other rule is checked again when the next hop is requested.
pub fn next_hop(current: &Url, location: Option<&str>) -> Result<Url, Refusal> {
    let next = location
        .and_then(|location| current.join(location).ok())
        .ok_or(Refusal::RedirectInvalid)?;
    if current.scheme() == "https" && next.scheme() == "http" {
        return Err(Refusal::RedirectDowngrade);
    }
    Ok(next)
}

fn is_redirect(status: StatusCode) -> bool {
    matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308)
}

/// HTTP/1.1 over one connection.
async fn send<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    stream: S,
    request: Request<Empty<Bytes>>,
) -> Result<(hyper::Response<hyper::body::Incoming>, ConnectionTask), Refusal> {
    let (mut sender, connection) = hyper::client::conn::http1::Builder::new()
        .max_buf_size(64 * 1024)
        .handshake(TokioIo::new(stream))
        .await
        .map_err(|_| Refusal::ProtocolError)?;
    let task = ConnectionTask(tokio::spawn(async move {
        // Its error is the response's or the body's to report.
        let _ = connection.await;
    }));
    let response = sender
        .send_request(request)
        .await
        .map_err(|_| Refusal::ProtocolError)?;
    Ok((response, task))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirects_resolve_relative_and_never_downgrade() {
        let https = Url::parse("https://site.example/a/b").unwrap();
        assert_eq!(
            next_hop(&https, Some("../c")).unwrap().as_str(),
            "https://site.example/c"
        );
        assert_eq!(
            next_hop(&https, Some("http://site.example/")),
            Err(Refusal::RedirectDowngrade)
        );
        assert_eq!(next_hop(&https, None), Err(Refusal::RedirectInvalid));
        assert_eq!(
            next_hop(&https, Some("http://[not")),
            Err(Refusal::RedirectInvalid)
        );
        let http = Url::parse("http://site.example/").unwrap();
        assert_eq!(
            next_hop(&http, Some("https://site.example/"))
                .unwrap()
                .scheme(),
            "https"
        );
    }
}
