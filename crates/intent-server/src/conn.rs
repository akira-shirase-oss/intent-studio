//! Serving connections with bounds (rust-mastery chapter 12: `axum::serve` exposes no
//! connection-level settings, so this drives hyper directly).
//!
//! - **At most `max_connections` open at once.** A permit is taken before `accept`, so when the
//!   cap is reached new connections wait in the kernel's backlog instead of costing a file
//!   descriptor and a task each.
//! - **Header-read timeout.** hyper's HTTP/1 timer runs whenever a connection waits for a
//!   request head: a slowloris that trickles headers and a keep-alive socket left idle are both
//!   closed after it.
//! - **Write-stall deadline.** A client that stops reading cannot hold a response (and its
//!   permit) forever: a write that makes no progress for `write_stall` fails the connection.
//! - **Bounded per connection and per address.** A connection serves at most
//!   `max_requests_per_connection` requests (the last response says `Connection: close`) and
//!   lives at most `max_age`, so a busy socket cannot be held forever; one client address (an
//!   IPv6 /64) holds at most `max_per_address` connections, so a single client cannot take the
//!   whole cap. A trusted proxy is exempt: it carries many clients, which the per-address
//!   request limits then tell apart by `X-Forwarded-For`.
//! - Graceful shutdown: on stop, accepting ends and every connection finishes its current
//!   request and closes; the caller bounds the wait.
//!
//! Request bodies are bounded separately: the body cap and the request timeout in the
//! middleware stack, and intake reads a body only after its cheap refusals (rate limits, shed).

use std::{
    collections::HashMap,
    io,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};

use axum::{
    Router,
    extract::ConnectInfo,
    http::{HeaderValue, header},
};
use hyper::server::conn::http1;
use hyper_util::rt::{TokioIo, TokioTimer};
use intent_domain::ratelimit::AddressKey;
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
    sync::Semaphore,
    time::Sleep,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use tower::ServiceBuilder;

/// Connection limits.
#[derive(Debug, Clone, Copy)]
pub struct ConnectionLimits {
    /// Most connections open at once.
    pub max_connections: usize,
    /// Longest wait for a complete request head, including on an idle keep-alive connection.
    pub header_read_timeout: Duration,
    /// Longest a write may make no progress.
    pub write_stall: Duration,
    /// Requests one connection serves before it is closed.
    pub max_requests_per_connection: u32,
    /// Longest a connection lives; then it finishes its current request and closes.
    pub max_age: Duration,
    /// Connections one client address (an IPv6 /64) may hold; trusted proxies are exempt. By
    /// default a quarter of `max_connections`, at most 64 (D-053): 16 IPv6 /64s, which one
    /// subscriber may well have, fill the default cap of 1,024.
    pub max_per_address: usize,
}

/// Connection counters, shared with `/metrics`.
#[derive(Debug, Default)]
pub struct ConnectionStats {
    /// Connections open now.
    pub open: AtomicU64,
    /// Connections accepted since start.
    pub accepted_total: AtomicU64,
    /// Connections closed because a write stalled.
    pub write_stalled_total: AtomicU64,
    /// Connections closed at once because their address already held its share.
    pub refused_per_address_total: AtomicU64,
    /// Connections closed because they reached their request cap or their age.
    pub retired_total: AtomicU64,
}

/// Open connections per client address. Entries exist only while connections are open, so the
/// table is bounded by the connection cap.
#[derive(Default)]
struct PerAddress(std::sync::Mutex<HashMap<AddressKey, usize>>);

/// One counted connection; dropping it gives the slot back.
struct AddressSlot {
    table: Arc<PerAddress>,
    key: AddressKey,
}

impl PerAddress {
    fn take(self: &Arc<Self>, key: AddressKey, cap: usize) -> Option<AddressSlot> {
        let mut table = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let held = table.entry(key).or_insert(0);
        if *held >= cap {
            return None;
        }
        *held += 1;
        Some(AddressSlot {
            table: self.clone(),
            key,
        })
    }
}

impl Drop for AddressSlot {
    fn drop(&mut self) {
        let mut table = self
            .table
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(held) = table.get_mut(&self.key) {
            *held = held.saturating_sub(1);
            if *held == 0 {
                table.remove(&self.key);
            }
        }
    }
}

/// Accepts and serves until `stop` fires; returns the tracker of open connections, which the
/// caller waits on (with its own deadline) after telling them to finish.
pub async fn serve(
    listener: TcpListener,
    router: Router,
    limits: ConnectionLimits,
    trusted_proxies: Arc<[IpAddr]>,
    stats: Arc<ConnectionStats>,
    stop: CancellationToken,
) -> TaskTracker {
    let permits = Arc::new(Semaphore::new(limits.max_connections));
    let per_address = Arc::new(PerAddress::default());
    let connections = TaskTracker::new();
    loop {
        let permit = tokio::select! {
            () = stop.cancelled() => break,
            permit = permits.clone().acquire_owned() => match permit {
                Ok(permit) => permit,
                Err(_) => break,
            },
        };
        let (stream, peer) = tokio::select! {
            () = stop.cancelled() => break,
            accepted = listener.accept() => match accepted {
                Ok(accepted) => accepted,
                Err(error) => {
                    // Usually out of file descriptors: back off instead of spinning.
                    tracing::warn!(%error, "accept failed");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
            },
        };
        stats.accepted_total.fetch_add(1, Ordering::Relaxed);
        let slot = if trusted_proxies.contains(&peer.ip()) {
            None
        } else if let Some(slot) =
            per_address.take(AddressKey::of(peer.ip()), limits.max_per_address)
        {
            Some(slot)
        } else {
            // This address already holds its share: close at once, before any task or buffer.
            stats
                .refused_per_address_total
                .fetch_add(1, Ordering::Relaxed);
            drop(stream);
            continue;
        };
        let _ = stream.set_nodelay(true);
        stats.open.fetch_add(1, Ordering::Relaxed);
        let served = Arc::new(AtomicU32::new(0));
        let cap = limits.max_requests_per_connection;
        let retired = stats.clone();
        let service = ServiceBuilder::new()
            .map_request(
                move |mut request: axum::extract::Request<hyper::body::Incoming>| {
                    request
                        .extensions_mut()
                        .insert(ConnectInfo::<SocketAddr>(peer));
                    request
                },
            )
            .map_response(move |mut response: axum::response::Response| {
                // The request that reaches the cap is answered, then the connection closes.
                if served.fetch_add(1, Ordering::Relaxed).saturating_add(1) >= cap {
                    response
                        .headers_mut()
                        .insert(header::CONNECTION, HeaderValue::from_static("close"));
                    retired.retired_total.fetch_add(1, Ordering::Relaxed);
                }
                response
            })
            .service(router.clone());
        let io = TokioIo::new(WriteDeadline::new(
            stream,
            limits.write_stall,
            stats.clone(),
        ));
        let connection = http1::Builder::new()
            .timer(TokioTimer::new())
            .header_read_timeout(limits.header_read_timeout)
            .keep_alive(true)
            .serve_connection(io, hyper_util::service::TowerToHyperService::new(service));
        let (stop, stats, max_age) = (stop.clone(), stats.clone(), limits.max_age);
        connections.spawn(async move {
            let (_permit, _slot) = (permit, slot);
            let mut connection = std::pin::pin!(connection);
            tokio::select! {
                result = connection.as_mut() => {
                    if let Err(error) = result {
                        tracing::debug!(%error, "connection ended with an error");
                    }
                }
                () = stop.cancelled() => {
                    connection.as_mut().graceful_shutdown();
                    let _ = connection.await;
                }
                () = tokio::time::sleep(max_age) => {
                    // Old enough: finish the request in flight, then close.
                    stats.retired_total.fetch_add(1, Ordering::Relaxed);
                    connection.as_mut().graceful_shutdown();
                    let _ = connection.await;
                }
            }
            stats.open.fetch_sub(1, Ordering::Relaxed);
        });
    }
    connections.close();
    connections
}

/// A stream whose writes fail after `stall` without progress.
pub struct WriteDeadline {
    inner: TcpStream,
    stall: Duration,
    timer: Option<Pin<Box<Sleep>>>,
    stats: Arc<ConnectionStats>,
}

impl WriteDeadline {
    fn new(inner: TcpStream, stall: Duration, stats: Arc<ConnectionStats>) -> Self {
        Self {
            inner,
            stall,
            timer: None,
            stats,
        }
    }

    /// While a write is pending, run the stall timer; any progress resets it.
    fn pending_or_stalled<T>(
        &mut self,
        cx: &mut Context<'_>,
        poll: Poll<io::Result<T>>,
    ) -> Poll<io::Result<T>> {
        match poll {
            Poll::Ready(result) => {
                self.timer = None;
                Poll::Ready(result)
            }
            Poll::Pending => {
                let stall = self.stall;
                let timer = self
                    .timer
                    .get_or_insert_with(|| Box::pin(tokio::time::sleep(stall)));
                if timer.as_mut().poll(cx).is_ready() {
                    self.timer = None;
                    self.stats
                        .write_stalled_total
                        .fetch_add(1, Ordering::Relaxed);
                    tracing::debug!("closing a connection whose client stopped reading");
                    Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "write stalled",
                    )))
                } else {
                    Poll::Pending
                }
            }
        }
    }
}

impl AsyncRead for WriteDeadline {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for WriteDeadline {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let poll = Pin::new(&mut self.inner).poll_write(cx, buf);
        self.pending_or_stalled(cx, poll)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let poll = Pin::new(&mut self.inner).poll_write_vectored(cx, bufs);
        self.pending_or_stalled(cx, poll)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let poll = Pin::new(&mut self.inner).poll_flush(cx);
        self.pending_or_stalled(cx, poll)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let poll = Pin::new(&mut self.inner).poll_shutdown(cx);
        self.pending_or_stalled(cx, poll)
    }
}
