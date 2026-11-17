//! The egress-guard security case (`END_GOAL.md`, D-012): loopback, RFC 1918, link-local and
//! cloud-metadata addresses, DNS rebinding and redirects to private targets are each refused with
//! a named code before any connection to them is opened. The resolver and the connector are
//! fixtures (an in-memory pipe to an in-process HTTP server); the policy is the real one.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unused_async_trait_impl,
    reason = "test code: a failed unwrap is a failed test, and the fixtures answer at once"
)]

use std::{
    collections::HashMap,
    convert::Infallible,
    io,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use http_body_util::{BodyExt as _, Full};
use hyper::{
    Request, Response,
    body::{Bytes, Frame},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use intent_egress::{Connect, Forbidden, Guard, Purpose, Refusal, Resolve};
use tokio::io::DuplexStream;

const PUBLIC: &str = "93.184.215.14";
const PUBLIC_2: &str = "93.184.215.15";

/// Answers per host, in order; the last answer repeats.
#[derive(Default, Clone)]
struct Resolver {
    answers: Arc<Mutex<HashMap<String, Vec<Vec<IpAddr>>>>>,
    calls: Arc<Mutex<Vec<String>>>,
}

impl Resolver {
    fn with(self, host: &str, answers: &[&[&str]]) -> Self {
        self.answers.lock().unwrap().insert(
            host.into(),
            answers
                .iter()
                .map(|a| a.iter().map(|ip| ip.parse().unwrap()).collect())
                .collect(),
        );
        self
    }
}

impl Resolve for Resolver {
    async fn resolve(&self, host: &str) -> io::Result<Vec<IpAddr>> {
        self.calls.lock().unwrap().push(host.into());
        let mut answers = self.answers.lock().unwrap();
        let queue = answers
            .get_mut(host)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such host"))?;
        Ok(if queue.len() > 1 {
            queue.remove(0)
        } else {
            queue[0].clone()
        })
    }
}

/// Every connection becomes an in-memory pipe to the fixture server, recorded by address.
#[derive(Default, Clone)]
struct Connector {
    seen: Arc<Mutex<Vec<SocketAddr>>>,
    refuse: Arc<Mutex<Vec<SocketAddr>>>,
}

impl Connect for Connector {
    type Stream = DuplexStream;

    async fn connect(&self, address: SocketAddr) -> io::Result<DuplexStream> {
        self.seen.lock().unwrap().push(address);
        if self.refuse.lock().unwrap().contains(&address) {
            return Err(io::Error::new(io::ErrorKind::ConnectionRefused, "refused"));
        }
        let (client, server) = tokio::io::duplex(64 * 1024);
        tokio::spawn(async move {
            let _ = http1::Builder::new()
                .serve_connection(TokioIo::new(server), service_fn(respond))
                .await;
        });
        Ok(client)
    }
}

type Body = http_body_util::combinators::BoxBody<Bytes, Infallible>;

fn full(status: u16, body: &'static str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", "text/html; charset=utf-8")
        .body(Full::new(Bytes::from_static(body.as_bytes())).boxed())
        .unwrap()
}

fn redirect(to: &str) -> Response<Body> {
    Response::builder()
        .status(302)
        .header("location", to)
        .body(Full::new(Bytes::new()).boxed())
        .unwrap()
}

async fn respond(request: Request<hyper::body::Incoming>) -> Result<Response<Body>, Infallible> {
    let path = request.uri().path().to_owned();
    Ok(match path.as_str() {
        "/ok" => full(200, "<h1>reference</h1>"),
        "/relative" => redirect("/ok"),
        "/to-private" => redirect("http://10.0.0.5/admin"),
        "/to-metadata" => redirect("http://169.254.169.254/latest/meta-data/"),
        "/again" => redirect("http://rebind.example/ok"),
        "/missing" => full(404, "no"),
        // Declares 6 MiB (a body of unknown length, so the server sends the header as given): the
        // guard refuses on the header without reading.
        "/declared-big" => Response::builder()
            .header("content-length", (6 * 1024 * 1024).to_string())
            .body(Stalled(false).boxed())
            .unwrap(),
        "/streamed-big" => {
            let chunks = (0..100)
                .map(|_| Ok::<_, Infallible>(Frame::data(Bytes::from(vec![b'x'; 64 * 1024]))));
            Response::new(futures_stream(chunks).boxed())
        }
        "/hang" => {
            tokio::time::sleep(Duration::from_secs(60)).await;
            full(200, "late")
        }
        p if p.starts_with("/loop/") => {
            let n: u32 = p[6..].parse().unwrap();
            redirect(&format!("/loop/{}", n + 1))
        }
        _ => full(500, "?"),
    })
}

/// A stream from an iterator, without a futures dependency.
fn futures_stream<I>(
    iter: I,
) -> impl hyper::body::Body<Data = Bytes, Error = Infallible> + Send + Sync + 'static
where
    I: Iterator<Item = Result<Frame<Bytes>, Infallible>> + Send + Sync + Unpin + 'static,
{
    struct Iter<I>(I);
    impl<I: Iterator<Item = Result<Frame<Bytes>, Infallible>> + Unpin> hyper::body::Body for Iter<I> {
        type Data = Bytes;
        type Error = Infallible;
        fn poll_frame(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Result<Frame<Bytes>, Infallible>>> {
            std::task::Poll::Ready(self.0.next())
        }
    }
    Iter(iter)
}

/// One byte, then nothing ever again: the head is flushed, the body never ends.
struct Stalled(bool);

impl hyper::body::Body for Stalled {
    type Data = Bytes;
    type Error = Infallible;
    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        if self.0 {
            std::task::Poll::Pending
        } else {
            self.0 = true;
            std::task::Poll::Ready(Some(Ok(Frame::data(Bytes::from_static(b"x")))))
        }
    }
}

fn guard(resolver: Resolver, connector: Connector) -> Guard<Resolver, Connector> {
    Guard::new(resolver, connector, Duration::from_secs(5)).unwrap()
}

fn public_resolver() -> Resolver {
    Resolver::default()
        .with("public.example", &[&[PUBLIC]])
        .with("localhost", &[&["127.0.0.1", "::1"]])
        .with("metadata.example", &[&["169.254.169.254"]])
        .with("mixed.example", &[&[PUBLIC, "10.1.2.3"]])
        .with("empty.example", &[&[]])
        .with("rebind.example", &[&[PUBLIC], &["127.0.0.1"]])
}

#[tokio::test]
async fn private_targets_are_refused_before_any_connection() {
    let connector = Connector::default();
    let g = guard(public_resolver(), connector.clone());
    let cases = [
        ("http://127.0.0.1/", Forbidden::Loopback),
        ("http://2130706433/", Forbidden::Loopback),
        ("http://0x7f.0.0.1/", Forbidden::Loopback),
        ("http://[::1]/", Forbidden::Loopback),
        ("http://[::ffff:127.0.0.1]/", Forbidden::Loopback),
        ("http://localhost/", Forbidden::Loopback),
        ("http://10.0.0.1/", Forbidden::Private),
        ("https://192.168.1.1/", Forbidden::Private),
        ("http://172.20.0.1/", Forbidden::Private),
        (
            "http://169.254.169.254/latest/meta-data/",
            Forbidden::LinkLocal,
        ),
        ("http://metadata.example/", Forbidden::LinkLocal),
        ("http://[fd00:ec2::254]/", Forbidden::UniqueLocal),
        ("http://[fe80::1]/", Forbidden::LinkLocal),
        ("http://100.100.100.200/", Forbidden::SharedAddressSpace),
        ("http://0.0.0.0/", Forbidden::Unspecified),
        ("http://mixed.example/", Forbidden::Private),
    ];
    for (url, why) in cases {
        assert_eq!(
            g.fetch(url, Purpose::Page).await.map(|_| ()),
            Err(Refusal::AddressForbidden(why)),
            "{url}"
        );
    }
    assert_eq!(
        *connector.seen.lock().unwrap(),
        vec![],
        "a refused address was connected to"
    );
}

#[tokio::test]
async fn only_plain_web_addresses_are_fetched() {
    let connector = Connector::default();
    let g = guard(public_resolver(), connector.clone());
    for (url, code) in [
        ("ftp://public.example/", Refusal::SchemeNotAllowed),
        ("file:///etc/passwd", Refusal::SchemeNotAllowed),
        ("gopher://public.example/", Refusal::SchemeNotAllowed),
        (
            "http://user:pw@public.example/ok",
            Refusal::CredentialsInUrl,
        ),
        ("http://public.example:8080/ok", Refusal::PortNotAllowed),
        ("https://public.example:22/", Refusal::PortNotAllowed),
        ("not a url", Refusal::InvalidUrl),
        ("http://nowhere.example/", Refusal::DnsFailed),
        ("http://empty.example/", Refusal::DnsFailed),
    ] {
        assert_eq!(
            g.fetch(url, Purpose::Page).await.map(|_| ()),
            Err(code),
            "{url}"
        );
    }
    assert_eq!(*connector.seen.lock().unwrap(), vec![]);
}

#[tokio::test]
async fn a_public_page_is_fetched_through_a_relative_redirect() {
    let connector = Connector::default();
    let g = guard(public_resolver(), connector.clone());
    let fetched = g
        .fetch("http://public.example/relative", Purpose::Page)
        .await
        .unwrap();
    assert_eq!(fetched.final_url.as_str(), "http://public.example/ok");
    assert_eq!(fetched.body, b"<h1>reference</h1>");
    assert_eq!(fetched.redirects.len(), 1);
    assert_eq!(
        fetched.content_type.as_deref(),
        Some("text/html; charset=utf-8")
    );
    let public: SocketAddr = format!("{PUBLIC}:80").parse().unwrap();
    assert_eq!(*connector.seen.lock().unwrap(), vec![public, public]);
}

#[tokio::test]
async fn a_redirect_to_a_private_target_is_refused_before_it_is_followed() {
    let connector = Connector::default();
    let g = guard(public_resolver(), connector.clone());
    assert_eq!(
        g.fetch("http://public.example/to-private", Purpose::Page)
            .await
            .map(|_| ()),
        Err(Refusal::AddressForbidden(Forbidden::Private))
    );
    assert_eq!(
        g.fetch("http://public.example/to-metadata", Purpose::Page)
            .await
            .map(|_| ()),
        Err(Refusal::AddressForbidden(Forbidden::LinkLocal))
    );
    let public: SocketAddr = format!("{PUBLIC}:80").parse().unwrap();
    assert_eq!(
        *connector.seen.lock().unwrap(),
        vec![public, public],
        "only the public hops connected"
    );
}

#[tokio::test]
async fn dns_rebinding_cannot_reach_a_private_address() {
    // The name answers a public address first and loopback afterwards. The first hop connects to
    // the address it checked (pinned); the redirect back to the same name is resolved and
    // checked again, and refused.
    let resolver = public_resolver();
    let connector = Connector::default();
    let g = guard(resolver.clone(), connector.clone());
    assert_eq!(
        g.fetch("http://rebind.example/again", Purpose::Page)
            .await
            .map(|_| ()),
        Err(Refusal::AddressForbidden(Forbidden::Loopback))
    );
    let public: SocketAddr = format!("{PUBLIC}:80").parse().unwrap();
    assert_eq!(*connector.seen.lock().unwrap(), vec![public]);
    assert_eq!(
        *resolver.calls.lock().unwrap(),
        vec!["rebind.example".to_owned(), "rebind.example".to_owned()],
        "one lookup per hop: the connection used the answer that was checked"
    );
}

#[tokio::test]
async fn redirects_sizes_statuses_and_time_are_bounded() {
    let g = guard(public_resolver(), Connector::default());
    assert_eq!(
        g.fetch("http://public.example/loop/0", Purpose::Page)
            .await
            .map(|_| ()),
        Err(Refusal::TooManyRedirects)
    );
    assert_eq!(
        g.fetch("http://public.example/declared-big", Purpose::Page)
            .await
            .map(|_| ()),
        Err(Refusal::TooLarge)
    );
    assert_eq!(
        g.fetch("http://public.example/streamed-big", Purpose::Page)
            .await
            .map(|_| ()),
        Err(Refusal::TooLarge)
    );
    // The same 6.25 MiB body fits an asset's 20 MiB.
    assert_eq!(
        g.fetch("http://public.example/streamed-big", Purpose::Asset)
            .await
            .unwrap()
            .body
            .len(),
        100 * 64 * 1024
    );
    assert_eq!(
        g.fetch("http://public.example/missing", Purpose::Page)
            .await
            .map(|_| ()),
        Err(Refusal::HttpStatus(404))
    );
    let quick = Guard::new(
        public_resolver(),
        Connector::default(),
        Duration::from_millis(300),
    )
    .unwrap();
    let started = Instant::now();
    assert_eq!(
        quick
            .fetch("http://public.example/hang", Purpose::Page)
            .await
            .map(|_| ()),
        Err(Refusal::Timeout)
    );
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn tls_is_verified_and_failures_are_named() {
    // The fixture speaks plain HTTP: an https fetch fails its handshake.
    let g = guard(public_resolver(), Connector::default());
    assert_eq!(
        g.fetch("https://public.example/ok", Purpose::Page)
            .await
            .map(|_| ()),
        Err(Refusal::TlsFailed)
    );
    // Both addresses refuse: connect_failed; the second address is tried.
    let connector = Connector::default();
    let first: SocketAddr = format!("{PUBLIC}:80").parse().unwrap();
    let second: SocketAddr = format!("{PUBLIC_2}:80").parse().unwrap();
    connector.refuse.lock().unwrap().extend([first, second]);
    let resolver = Resolver::default().with("down.example", &[&[PUBLIC, PUBLIC_2]]);
    let g = guard(resolver, connector.clone());
    assert_eq!(
        g.fetch("http://down.example/ok", Purpose::Page)
            .await
            .map(|_| ()),
        Err(Refusal::ConnectFailed)
    );
    assert_eq!(*connector.seen.lock().unwrap(), vec![first, second]);
}
