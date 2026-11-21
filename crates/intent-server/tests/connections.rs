//! Connection-level protections against the real binary (F1): slow or idle sockets are closed,
//! open connections are capped, a client that stops reading is cut off, and a client already
//! over its limit is refused before its body is read.
//!
//! Needs the stack (`scripts/stack.sh up`); run by `scripts/drills.sh`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test code: a failed unwrap is a failed test"
)]

mod support;

use std::time::{Duration, Instant};

use intent_provider_fixture::Mode;
use support::{LeadSpec, ProviderFixture, Server, TestDb, client, drill_env, site_for, submit};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

async fn server(db: &TestDb, fixture: &ProviderFixture, extra: &[(&str, &str)]) -> Server {
    let mut env = drill_env(db, &fixture.url);
    for (k, v) in extra {
        env.retain(|(key, _)| key != k);
        env.push(((*k).to_owned(), (*v).to_owned()));
    }
    Server::start(env).await
}

/// Reads until the server closes the connection or `limit` passes; returns how long it took,
/// or `None` if the connection was still open at the limit.
async fn time_until_closed(
    stream: &mut tokio::net::TcpStream,
    limit: Duration,
) -> Option<Duration> {
    let start = Instant::now();
    let mut buf = [0_u8; 4096];
    loop {
        match tokio::time::timeout(limit.saturating_sub(start.elapsed()), stream.read(&mut buf))
            .await
        {
            Err(_) => return None,
            Ok(Ok(0) | Err(_)) => return Some(start.elapsed()),
            Ok(Ok(_)) => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn slow_headers_and_idle_sockets_are_closed() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let server = server(&db, &fixture, &[("INTENT_HEADER_READ_TIMEOUT_MS", "1000")]).await;

    // Slowloris: a request line, then one header byte every 300 ms, forever.
    let mut slow = tokio::net::TcpStream::connect(server.address)
        .await
        .unwrap();
    slow.write_all(b"POST /v1/sites/x/leads HTTP/1.1\r\nhost: x\r\nx-slow: ")
        .await
        .unwrap();
    let trickle = tokio::spawn(async move {
        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(300)).await;
            if slow.write_all(b"a").await.is_err() {
                return true; // the server hung up
            }
        }
        false
    });

    // Fifty sockets that connect and never send a byte.
    let mut idle = Vec::new();
    for _ in 0..50 {
        idle.push(
            tokio::net::TcpStream::connect(server.address)
                .await
                .unwrap(),
        );
    }
    // A keep-alive connection left idle after one complete request.
    let mut kept = tokio::net::TcpStream::connect(server.address)
        .await
        .unwrap();
    kept.write_all(b"GET /healthz HTTP/1.1\r\nhost: x\r\n\r\n")
        .await
        .unwrap();
    let mut first = [0_u8; 512];
    let read = kept.read(&mut first).await.unwrap();
    assert!(String::from_utf8_lossy(&first[..read]).starts_with("HTTP/1.1 200"));

    for (i, socket) in idle.iter_mut().enumerate() {
        let closed = time_until_closed(socket, Duration::from_secs(6)).await;
        assert!(closed.is_some(), "idle socket {i} was still open after 6 s");
    }
    let closed = time_until_closed(&mut kept, Duration::from_secs(6)).await;
    assert!(
        closed.is_some(),
        "an idle keep-alive connection was still open after 6 s"
    );
    let hung_up = tokio::time::timeout(Duration::from_secs(10), trickle).await;
    assert!(
        matches!(hung_up, Ok(Ok(true))),
        "the slowloris connection was never cut off"
    );

    // And the server still serves.
    let health = client()
        .get(format!("{}/healthz", server.base))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), 200);
    db.drop_database().await;
}

/// Reads a complete HTTP/1.1 response (head + `content-length` body) or returns what arrived
/// before EOF/error/timeout.
async fn read_response_bytes(stream: &mut tokio::net::TcpStream, limit: Duration) -> Vec<u8> {
    let start = Instant::now();
    let mut seen = Vec::new();
    let mut buf = vec![0_u8; 16 * 1024];
    loop {
        let left = limit.saturating_sub(start.elapsed());
        match tokio::time::timeout(left, stream.read(&mut buf)).await {
            Ok(Ok(n)) if n > 0 => {
                seen.extend_from_slice(&buf[..n]);
                // Complete when the head has arrived and content-length bytes followed.
                let Some(head) = seen.windows(4).position(|w| w == b"\r\n\r\n") else {
                    continue;
                };
                let head_text = String::from_utf8_lossy(&seen[..head]).to_ascii_lowercase();
                let Some(length) = head_text.lines().find_map(|l| {
                    l.strip_prefix("content-length:")?
                        .trim()
                        .parse::<usize>()
                        .ok()
                }) else {
                    continue;
                };
                if seen.len() >= head + 4 + length {
                    return seen;
                }
            }
            _ => return seen,
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn a_reused_idle_connection_gets_a_whole_response_or_a_clean_close() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    // The head-read bound is also the idle keep-alive bound (D-068): a client whose pool
    // holds a connection past it reuses a dead one. The contract every pool relies on is
    // that the server never truncates a response on this path: a request sent after the
    // bound sees a whole response, or a clean close before its bytes are read — which is
    // why keep-alive clients must idle below the bound rather than handle a mid-close.
    let server = server(&db, &fixture, &[("INTENT_HEADER_READ_TIMEOUT_MS", "600")]).await;

    let mut socket = tokio::net::TcpStream::connect(server.address)
        .await
        .unwrap();
    socket
        .write_all(b"GET /healthz HTTP/1.1\r\nhost: x\r\n\r\n")
        .await
        .unwrap();
    let first = read_response_bytes(&mut socket, Duration::from_secs(3)).await;
    assert!(
        String::from_utf8_lossy(&first).starts_with("HTTP/1.1 200"),
        "first response incomplete: {}",
        String::from_utf8_lossy(&first)
    );

    // Idle well past the 600 ms bound, then reuse the connection.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    // The write may fail outright (the FIN landed first) — a clean close, not consumed
    // bytes. Only a read decides which side of the close we landed on.
    let _ = socket
        .write_all(b"GET /healthz HTTP/1.1\r\nhost: x\r\n\r\n")
        .await;
    let second = read_response_bytes(&mut socket, Duration::from_secs(3)).await;
    if !second.is_empty() {
        let text = String::from_utf8_lossy(&second);
        assert!(
            text.starts_with("HTTP/1.1 200"),
            "the reused connection answered with a partial response (a reset mid-request): {text}"
        );
        let head_end = second.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let length: usize = text[..head_end]
            .to_ascii_lowercase()
            .lines()
            .find_map(|l| l.strip_prefix("content-length:")?.trim().parse().ok())
            .expect("a response without content-length");
        assert_eq!(
            second.len() - head_end - 4,
            length,
            "truncated body on a reused connection: {text}"
        );
    }
    // Whether the reuse saw a whole response or a clean close, the server still serves a
    // fresh connection.
    let health = client()
        .get(format!("{}/healthz", server.base))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), 200);
    db.drop_database().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn open_connections_are_capped() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let server = server(
        &db,
        &fixture,
        &[
            ("INTENT_MAX_CONNECTIONS", "8"),
            // All eight from one address: this test is about the global cap.
            ("INTENT_MAX_CONNECTIONS_PER_ADDRESS", "8"),
            ("INTENT_HEADER_READ_TIMEOUT_MS", "1500"),
        ],
    )
    .await;
    // Eight idle sockets fill the cap; no ninth connection is served until one of them goes.
    let mut idle = Vec::new();
    for _ in 0..8 {
        idle.push(
            tokio::net::TcpStream::connect(server.address)
                .await
                .unwrap(),
        );
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let start = Instant::now();
    let health = client()
        .get(format!("{}/healthz", server.base))
        .send()
        .await
        .unwrap();
    let waited = start.elapsed();
    assert_eq!(health.status(), 200);
    assert!(
        waited >= Duration::from_millis(1000),
        "the ninth connection was served after {waited:?}: the cap of 8 did not hold"
    );
    assert!(
        waited < Duration::from_secs(5),
        "served only after {waited:?}"
    );
    let metrics = client()
        .get(format!("{}/metrics", server.base))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(metrics.contains("intent_connections_open "), "{metrics}");
    db.drop_database().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn a_client_over_its_limit_is_refused_before_its_body_is_read() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    // No trusted proxy: every request is from 127.0.0.1, which has 5 leads a minute.
    let server = server(&db, &fixture, &[]).await;
    let client = client();
    let site = site_for(0, 1);
    for n in 0..5 {
        let outcome = submit(
            &client,
            &server.base,
            site,
            &LeadSpec::new("slowbody", n).json(),
            None,
            n,
            Instant::now(),
        )
        .await;
        assert_eq!(outcome.status, Some(201));
    }
    // The sixth declares 2,000 bytes and sends 100: it must be refused at once, not after the
    // server has waited for (and read) the rest.
    let mut socket = tokio::net::TcpStream::connect(server.address)
        .await
        .unwrap();
    let head = format!(
        "POST /v1/sites/{site}/leads HTTP/1.1\r\nhost: x\r\ncontent-type: application/json\r\ncontent-length: 2000\r\n\r\n"
    );
    socket.write_all(head.as_bytes()).await.unwrap();
    socket.write_all(&[b' '; 100]).await.unwrap();
    let start = Instant::now();
    let mut response = vec![0_u8; 1024];
    let read = tokio::time::timeout(Duration::from_secs(2), socket.read(&mut response)).await;
    let Ok(Ok(read)) = read else {
        panic!(
            "no answer within 2 s: the server was waiting for the body of a rate-limited request"
        );
    };
    let text = String::from_utf8_lossy(&response[..read]);
    assert!(text.starts_with("HTTP/1.1 429"), "{text}");
    assert!(start.elapsed() < Duration::from_secs(1));
    db.drop_database().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn a_client_that_stops_reading_is_cut_off() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let server = server(&db, &fixture, &[("INTENT_WRITE_STALL_TIMEOUT_MS", "1000")]).await;
    // Ask for a response much larger than the socket buffers (the fixture's long body is the
    // workspace's 200-lead page in production; here a metrics page repeated is enough), never read.
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.set_recv_buffer_size(4096).unwrap();
    let mut stalled = socket.connect(server.address).await.unwrap();
    let mut requests = String::new();
    for _ in 0..2_000 {
        requests.push_str("GET /metrics HTTP/1.1\r\nhost: x\r\n\r\n");
    }
    stalled.write_all(requests.as_bytes()).await.unwrap();
    // Never read. Within a few seconds the server gives up on this connection.
    let start = Instant::now();
    let probe = client();
    loop {
        let metrics = probe
            .get(format!("{}/metrics", server.base))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        if metrics.contains("intent_connections_write_stalled_total 1") {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "a connection that stopped reading was never cut off"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    drop(stalled);
    db.drop_database().await;
}

/// Reads HTTP/1.1 responses from `stream` until the server closes it (or `limit` passes);
/// returns how many responses arrived and whether the last one said `connection: close`.
async fn count_responses(stream: &mut tokio::net::TcpStream, limit: Duration) -> (usize, bool) {
    let start = Instant::now();
    let mut seen = Vec::new();
    let mut buf = vec![0_u8; 16 * 1024];
    loop {
        let left = limit.saturating_sub(start.elapsed());
        match tokio::time::timeout(left, stream.read(&mut buf)).await {
            Ok(Ok(n)) if n > 0 => seen.extend_from_slice(&buf[..n]),
            _ => break,
        }
    }
    let text = String::from_utf8_lossy(&seen).to_ascii_lowercase();
    let responses = text.matches("http/1.1 200").count();
    let closing = text
        .rsplit("http/1.1 200")
        .next()
        .is_some_and(|last| last.contains("connection: close"));
    (responses, closing)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn a_connection_serves_a_bounded_number_of_requests() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let server = server(
        &db,
        &fixture,
        &[("INTENT_MAX_REQUESTS_PER_CONNECTION", "50")],
    )
    .await;
    let mut socket = tokio::net::TcpStream::connect(server.address)
        .await
        .unwrap();
    let mut pipelined = String::new();
    for _ in 0..100 {
        pipelined.push_str("GET /healthz HTTP/1.1\r\nhost: x\r\n\r\n");
    }
    socket.write_all(pipelined.as_bytes()).await.unwrap();
    let (responses, closing) = count_responses(&mut socket, Duration::from_secs(5)).await;
    assert_eq!(
        responses, 50,
        "one connection answered {responses} requests"
    );
    assert!(closing, "the 50th response must carry connection: close");
    db.drop_database().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn a_busy_connection_is_closed_at_its_maximum_age() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let server = server(&db, &fixture, &[("INTENT_MAX_CONNECTION_AGE_MS", "2000")]).await;
    // Keep-alive pings every 300 ms: never idle, so only an age limit can end it.
    let mut socket = tokio::net::TcpStream::connect(server.address)
        .await
        .unwrap();
    let start = Instant::now();
    let mut buf = [0_u8; 4096];
    let closed_after = loop {
        if socket
            .write_all(b"GET /healthz HTTP/1.1\r\nhost: x\r\n\r\n")
            .await
            .is_err()
        {
            break start.elapsed();
        }
        if let Ok(Ok(0) | Err(_)) =
            tokio::time::timeout(Duration::from_secs(2), socket.read(&mut buf)).await
        {
            break start.elapsed();
        }
        assert!(
            start.elapsed() < Duration::from_secs(8),
            "a pinging connection was never closed"
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
    };
    assert!(
        closed_after >= Duration::from_millis(1800),
        "closed too early: {closed_after:?}"
    );
    db.drop_database().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn one_address_cannot_hold_every_connection() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let server = server(
        &db,
        &fixture,
        &[
            ("INTENT_MAX_CONNECTIONS", "20"),
            ("INTENT_MAX_CONNECTIONS_PER_ADDRESS", "8"),
        ],
    )
    .await;
    // Twelve sockets from one address: eight are kept, the other four are closed at once.
    let mut sockets = Vec::new();
    for _ in 0..12 {
        sockets.push(
            tokio::net::TcpStream::connect(server.address)
                .await
                .unwrap(),
        );
    }
    let mut closed = 0;
    for socket in &mut sockets {
        if time_until_closed(socket, Duration::from_millis(500))
            .await
            .is_some()
        {
            closed += 1;
        }
    }
    assert_eq!(
        closed, 4,
        "{closed} of 12 sockets from one address were closed (cap 8)"
    );
    drop(sockets);
    // A trusted proxy carries many clients: it is not capped per address.
    let mut env = drill_env(&db, &fixture.url);
    env.push(("INTENT_MAX_CONNECTIONS".into(), "20".into()));
    env.push(("INTENT_MAX_CONNECTIONS_PER_ADDRESS".into(), "8".into()));
    env.push(("INTENT_TRUSTED_PROXIES".into(), "127.0.0.1".into()));
    let proxied = Server::start(env).await;
    let mut sockets = Vec::new();
    for _ in 0..12 {
        sockets.push(
            tokio::net::TcpStream::connect(proxied.address)
                .await
                .unwrap(),
        );
    }
    let mut closed = 0;
    for socket in &mut sockets {
        if time_until_closed(socket, Duration::from_millis(300))
            .await
            .is_some()
        {
            closed += 1;
        }
    }
    assert_eq!(closed, 0, "the trusted proxy's connections were capped");
    db.drop_database().await;
}
