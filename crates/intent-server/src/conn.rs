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
//! - Graceful shutdown: on stop, accepting ends and every connection finishes its current
//!   request and closes; the caller bounds the wait.
//!
//! Request bodies are bounded separately: the body cap and the request timeout in the
//! middleware stack, and intake reads a body only after its cheap refusals (rate limits, shed).

use std::{
    io,
    net::SocketAddr,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};

use axum::{Router, extract::ConnectInfo};
use hyper::server::conn::http1;
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
    sync::Semaphore,
    time::Sleep,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use tower::ServiceExt as _;

/// Connection limits.
#[derive(Debug, Clone, Copy)]
pub struct ConnectionLimits {
    /// Most connections open at once.
    pub max_connections: usize,
    /// Longest wait for a complete request head, including on an idle keep-alive connection.
    pub header_read_timeout: Duration,
    /// Longest a write may make no progress.
    pub write_stall: Duration,
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
}

/// Accepts and serves until `stop` fires; returns the tracker of open connections, which the
/// caller waits on (with its own deadline) after telling them to finish.
pub async fn serve(
    listener: TcpListener,
    router: Router,
    limits: ConnectionLimits,
    stats: Arc<ConnectionStats>,
    stop: CancellationToken,
) -> TaskTracker {
    let permits = Arc::new(Semaphore::new(limits.max_connections));
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
        let _ = stream.set_nodelay(true);
        stats.accepted_total.fetch_add(1, Ordering::Relaxed);
        stats.open.fetch_add(1, Ordering::Relaxed);
        let service = router
            .clone()
            .map_request(move |mut request: axum::extract::Request<_>| {
                request
                    .extensions_mut()
                    .insert(ConnectInfo::<SocketAddr>(peer));
                request
            });
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
        let (stop, stats) = (stop.clone(), stats.clone());
        connections.spawn(async move {
            let _permit = permit;
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
