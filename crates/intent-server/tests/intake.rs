//! Lead intake through the real binary: idempotency, bounded and malformed bodies, abuse limits,
//! the honeypot, the operator inbox, and the log-capture check over every scenario.
//!
//! Needs the stack (`scripts/stack.sh up`); run by `scripts/drills.sh`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    reason = "test code: a failed unwrap is a failed test, and a drill reads top to bottom as one scenario"
)]

mod support;

use std::time::{Duration, Instant};

use intent_provider_fixture::Mode;
use support::{
    LeadSpec, ProviderFixture, Server, TestDb, assert_no_leaks, client, drill_env, site_for, submit,
};

async fn trusted_server(db: &TestDb, fixture: &ProviderFixture) -> Server {
    let mut env = drill_env(db, &fixture.url);
    env.push(("INTENT_TRUSTED_PROXIES".into(), "127.0.0.1".into()));
    Server::start(env).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn a_retried_submit_is_one_lead_sequentially_and_concurrently() {
    let db = TestDb::create(5).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let server = trusted_server(&db, &fixture).await;
    let client = client();
    let clock = Instant::now();
    let spec = LeadSpec::new("dup", 1);
    let site = site_for(1, 5);

    let first = submit(
        &client,
        &server.base,
        site,
        &spec.json(),
        Some("10.0.0.1"),
        1,
        clock,
    )
    .await;
    assert_eq!(first.status, Some(201));
    assert!(!first.duplicate);
    let again = submit(
        &client,
        &server.base,
        site,
        &spec.json(),
        Some("10.0.0.2"),
        1,
        clock,
    )
    .await;
    assert_eq!(again.status, Some(200));
    assert!(again.duplicate);
    assert_eq!(again.lead_id, first.lead_id);

    // Twenty identical submits at once: one lead, every answer names it.
    let racing = LeadSpec::new("dup", 2);
    let mut tasks = tokio::task::JoinSet::new();
    for i in 0..20_u64 {
        let (client, base, body) = (client.clone(), server.base.clone(), racing.json());
        let address = format!("10.0.1.{i}");
        tasks.spawn(
            async move { submit(&client, &base, site, &body, Some(&address), 2, clock).await },
        );
    }
    let mut leads = std::collections::HashSet::new();
    let mut created = 0;
    while let Some(outcome) = tasks.join_next().await {
        let outcome = outcome.unwrap();
        assert!(outcome.acknowledged(), "{outcome:?}");
        created += usize::from(outcome.status == Some(201));
        leads.insert(outcome.lead_id.unwrap());
    }
    assert_eq!(created, 1, "exactly one submit created the lead");
    assert_eq!(leads.len(), 1);
    assert_eq!(db.scalar("SELECT count(*) FROM leads").await, 2);
    assert_eq!(
        db.scalar("SELECT count(*) FROM notification_outbox").await,
        2
    );

    // The same id with different content is refused, never merged.
    let mut changed = racing.json();
    changed["message"] = "something else".into();
    let conflict = submit(
        &client,
        &server.base,
        site,
        &changed,
        Some("10.0.2.1"),
        2,
        clock,
    )
    .await;
    assert_eq!(conflict.status, Some(409));
    assert_eq!(conflict.code.as_deref(), Some("submission_conflict"));

    // A no-script form post without a submission id: the server derives the key, so a double
    // click is one lead too.
    let form = "name=Formname+Person&phone=%2B44+20+7946+0001&message=hello&consent=on";
    let post = |client: reqwest::Client| {
        let url = format!("{}/v1/sites/{site}/leads", server.base);
        async move {
            client
                .post(url)
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-forwarded-for", "10.0.3.1")
                .body(form)
                .send()
                .await
                .unwrap()
        }
    };
    let one: serde_json::Value = post(client.clone()).await.json().await.unwrap();
    let two = post(client.clone()).await;
    assert_eq!(two.status(), 200);
    let two: serde_json::Value = two.json().await.unwrap();
    assert_eq!(one["lead_id"], two["lead_id"]);
    assert_eq!(one["submission_id"].as_str().unwrap().len(), 32);

    let mut needles: Vec<&str> = spec.needles().to_vec();
    needles.extend(racing.needles());
    needles.extend(["Formname", "7946 0001"]);
    assert_no_leaks(&server.log(), &needles);
    db.drop_database().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn bad_bodies_are_refused_with_named_codes_and_store_nothing() {
    let db = TestDb::create(2).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let server = trusted_server(&db, &fixture).await;
    let client = client();
    let site = site_for(0, 2);
    let url = format!("{}/v1/sites/{site}/leads", server.base);
    let send = |body: Vec<u8>, content_type: &'static str, address: &'static str| {
        let (client, url) = (client.clone(), url.clone());
        async move {
            let response = client
                .post(url)
                .header("content-type", content_type)
                .header("x-forwarded-for", address)
                .body(body)
                .send()
                .await
                .unwrap();
            let status = response.status().as_u16();
            let body: serde_json::Value = response.json().await.unwrap_or_default();
            (
                status,
                body["error"]["code"].as_str().unwrap_or("").to_owned(),
                body,
            )
        }
    };

    // Oversized, declared: refused before the body is read.
    let big = vec![b'a'; 17 * 1024];
    let (status, code, _) = send(big, "application/json", "10.1.0.1").await;
    assert_eq!((status, code.as_str()), (413, "payload_too_large"));
    // Oversized, streamed without a length (chunked): refused at the cap, over a raw socket so
    // no client library can add a Content-Length.
    let streamed =
        raw_chunked_post(server.address, &format!("/v1/sites/{site}/leads"), 40, 1024).await;
    assert!(streamed.starts_with("HTTP/1.1 413"), "{streamed}");
    let (status, code, _) = send(b"{\"name\": ".to_vec(), "application/json", "10.1.0.3").await;
    assert_eq!((status, code.as_str()), (400, "malformed_body"));
    let (status, code, _) = send(
        b"{\"name\":\"a\",\"isAdmin\":true}".to_vec(),
        "application/json",
        "10.1.0.4",
    )
    .await;
    assert_eq!((status, code.as_str()), (400, "malformed_body"));
    let (status, code, _) = send(b"<lead/>".to_vec(), "application/xml", "10.1.0.5").await;
    assert_eq!((status, code.as_str()), (415, "unsupported_media_type"));
    let (status, code, body) = send(
        br#"{"name":"  ","email":"not-an-email","consent":false}"#.to_vec(),
        "application/json",
        "10.1.0.6",
    )
    .await;
    assert_eq!((status, code.as_str()), (422, "validation_failed"));
    let fields: Vec<(String, String)> = body["error"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["field"].as_str().unwrap().to_owned(),
                f["code"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert!(
        fields.contains(&("email".into(), "invalid_email".into())),
        "{fields:?}"
    );
    assert!(
        fields.contains(&("consent".into(), "consent_required".into())),
        "{fields:?}"
    );
    assert!(
        !body.to_string().contains("not-an-email"),
        "a refusal must not echo input"
    );

    // The honeypot: refused, unspecific, nothing stored.
    let mut bot = LeadSpec::new("bot", 1).json();
    bot["website"] = "http://spam.example".into();
    let (status, code, _) =
        send(bot.to_string().into_bytes(), "application/json", "10.1.0.7").await;
    assert_eq!((status, code.as_str()), (400, "rejected"));

    // Unknown and closed sites.
    let unknown = client
        .post(format!(
            "{}/v1/sites/{}/leads",
            server.base,
            uuid::Uuid::now_v7()
        ))
        .header("x-forwarded-for", "10.1.0.8")
        .json(&LeadSpec::new("nosite", 1).json())
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), 404);
    let garbage = client
        .post(format!("{}/v1/sites/not-a-site/leads", server.base))
        .json(&LeadSpec::new("nosite", 2).json())
        .send()
        .await
        .unwrap();
    assert_eq!(garbage.status(), 404);
    let mut conn = db.connect().await;
    sqlx::query("UPDATE sites SET accepting_leads = false WHERE id = $1")
        .bind(site_for(1, 2))
        .execute(&mut conn)
        .await
        .unwrap();
    let closed = submit(
        &client,
        &server.base,
        site_for(1, 2),
        &LeadSpec::new("closed", 1).json(),
        Some("10.1.0.9"),
        1,
        Instant::now(),
    )
    .await;
    assert_eq!(closed.status, Some(404));

    // A wrong method gets the same envelope.
    let get = client.get(&url).send().await.unwrap();
    assert_eq!(get.status(), 405);
    let body: serde_json::Value = get.json().await.unwrap();
    assert_eq!(body["error"]["code"], "method_not_allowed");
    assert!(body["error"]["request_id"].is_string());

    assert_eq!(
        db.scalar("SELECT count(*) FROM leads").await,
        0,
        "no refused submission was stored"
    );
    let needles = LeadSpec::new("bot", 1);
    assert_no_leaks(
        &server.log(),
        &[needles.email.as_str(), "not-an-email", "spam.example"],
    );
    db.drop_database().await;
}

/// Posts `chunks` chunks of `size` bytes with `Transfer-Encoding: chunked`; returns the status
/// line.
async fn raw_chunked_post(
    address: std::net::SocketAddr,
    path: &str,
    chunks: usize,
    size: usize,
) -> String {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    let head = format!(
        "POST {path} HTTP/1.1\r\nhost: {address}\r\ncontent-type: application/json\r\nx-forwarded-for: 10.1.0.2\r\ntransfer-encoding: chunked\r\n\r\n"
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    let chunk = format!("{size:x}\r\n{}\r\n", " ".repeat(size));
    for _ in 0..chunks {
        // The server may answer and close before every chunk is written; that is the point.
        if stream.write_all(chunk.as_bytes()).await.is_err() {
            break;
        }
    }
    let _ = stream.write_all(b"0\r\n\r\n").await;
    let mut response = vec![0_u8; 512];
    let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut response))
        .await
        .unwrap()
        .unwrap();
    String::from_utf8_lossy(&response[..read])
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn abuse_limits_hold_per_address_and_per_site() {
    let db = TestDb::create(3).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    // No trusted proxy: every request comes from 127.0.0.1 and X-Forwarded-For is ignored.
    let server = Server::start(drill_env(&db, &fixture.url)).await;
    let client = client();
    let clock = Instant::now();
    let site = site_for(0, 3);
    let mut statuses = Vec::new();
    for n in 0..8_u64 {
        let spec = LeadSpec::new("abuse", n);
        // A forged header changes nothing for an untrusted peer.
        let outcome = submit(
            &client,
            &server.base,
            site,
            &spec.json(),
            Some(&format!("10.9.9.{n}")),
            n,
            clock,
        )
        .await;
        statuses.push((outcome.status, outcome.retry_after.clone()));
    }
    let accepted = statuses.iter().filter(|(s, _)| *s == Some(201)).count();
    assert_eq!(
        accepted, 5,
        "5 leads a minute per address (D-019): {statuses:?}"
    );
    for (status, retry_after) in &statuses[5..] {
        assert_eq!(*status, Some(429));
        let secs: u64 = retry_after.as_deref().unwrap().parse().unwrap();
        assert!((1..=12).contains(&secs), "Retry-After {secs}");
    }
    drop(server);

    // Behind a trusted proxy each client address has its own bucket, and the site's bucket
    // (60 a minute, D-019) still bounds the site as a whole.
    let mut env = drill_env(&db, &fixture.url);
    env.push(("INTENT_TRUSTED_PROXIES".into(), "127.0.0.1".into()));
    let server = Server::start(env).await;
    let busy = site_for(1, 3);
    let mut created = 0;
    let mut site_limited = 0;
    for n in 100..170_u64 {
        let outcome = submit(
            &client,
            &server.base,
            busy,
            &LeadSpec::new("abuse", n).json(),
            Some(&format!("10.8.{}.{}", n / 256, n % 256)),
            n,
            clock,
        )
        .await;
        match outcome.status {
            Some(201) => created += 1,
            Some(429) => site_limited += 1,
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(created, 60, "the per-site burst");
    assert_eq!(site_limited, 10);
    let metrics = client
        .get(format!("{}/metrics", server.base))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        metrics.contains("intent_intake_rate_limited_site_total 10\n"),
        "{metrics}"
    );
    assert_no_leaks(&server.log(), &[&LeadSpec::new("abuse", 100).email]);
    db.drop_database().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn the_operator_inbox_needs_a_session_and_shows_notification_state() {
    let db = TestDb::create(2).await;
    let fixture = ProviderFixture::start(Mode::Unavailable).await;
    let password = "correct horse battery staple 42";
    let hash = {
        use std::io::Write as _;
        let mut child = std::process::Command::new(support::server_binary())
            .arg("hash-password")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(format!("{password}\n").as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    assert!(hash.starts_with("$argon2id$"));
    let mut env = drill_env(&db, &fixture.url);
    env.push(("INTENT_OPERATOR_PASSWORD_HASH".into(), hash.clone()));
    env.push(("INTENT_TRUSTED_PROXIES".into(), "127.0.0.1".into()));
    let server = Server::start(env).await;
    let client = client();
    let spec = LeadSpec::new("inbox", 1);
    let stored = submit(
        &client,
        &server.base,
        site_for(1, 2),
        &spec.json(),
        Some("10.2.0.1"),
        1,
        Instant::now(),
    )
    .await;
    assert_eq!(stored.status, Some(201));

    let inbox = format!("{}/v1/operator/leads", server.base);
    assert_eq!(client.get(&inbox).send().await.unwrap().status(), 401);
    let forged = client
        .get(&inbox)
        .header("cookie", format!("intent_operator={}", "a".repeat(64)))
        .send()
        .await
        .unwrap();
    assert_eq!(forged.status(), 401);
    let session = format!("{}/v1/operator/session", server.base);
    let wrong = client
        .post(&session)
        .json(&serde_json::json!({ "password": "wrong" }))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 401);
    let right = client
        .post(&session)
        .json(&serde_json::json!({ "password": password }))
        .send()
        .await
        .unwrap();
    assert_eq!(right.status(), 200);
    let cookie = right.headers()["set-cookie"].to_str().unwrap().to_owned();
    assert!(
        cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"),
        "{cookie}"
    );
    let token = cookie.split(';').next().unwrap().to_owned();

    // Let the notifier fail a few times against the refusing provider first.
    tokio::time::sleep(Duration::from_millis(400)).await;
    let page: serde_json::Value = client
        .get(&inbox)
        .header("cookie", &token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let lead = &page["leads"][0];
    assert_eq!(lead["email"], spec.email.as_str());
    assert_eq!(lead["id"], stored.lead_id.unwrap().to_string());
    assert_eq!(lead["state"], "notification_pending");
    assert_eq!(lead["notification"]["last_error"], "provider_unavailable");
    assert!(lead["notification"]["attempts"].as_u64().unwrap() >= 1);
    assert_eq!(
        db.scalar("SELECT count(*) FROM operator_audit WHERE action = 'operator.inbox_read'")
            .await,
        1
    );

    // Recovery: the inbox shows the delivery.
    fixture.fixture.set_mode(Mode::Ok);
    db.wait_outbox_terminal(Duration::from_secs(20)).await;
    let page: serde_json::Value = client
        .get(&inbox)
        .header("cookie", &token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(page["leads"][0]["state"], "delivered");

    let out = client
        .delete(&session)
        .header("cookie", &token)
        .send()
        .await
        .unwrap();
    assert_eq!(out.status(), 204);
    assert_eq!(
        client
            .get(&inbox)
            .header("cookie", &token)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );

    let session_token = token.trim_start_matches("intent_operator=").to_owned();
    let mut needles: Vec<&str> = spec.needles().to_vec();
    needles.extend([password, hash.as_str(), session_token.as_str()]);
    assert_no_leaks(&server.log(), &needles);
    db.drop_database().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn sigterm_drains_and_exits_cleanly() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let mut server = Server::start(drill_env(&db, &fixture.url)).await;
    let status = server.terminate();
    assert!(status.success(), "exit status {status:?}");
    let log = server.log();
    assert!(log.contains("readiness is now false"), "{log}");
    assert!(log.contains("\"outcome\":\"Graceful\""), "{log}");
    db.drop_database().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn only_storable_leads_from_known_sites_spend_the_site_budget() {
    let db = TestDb::create(2).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let mut env = drill_env(&db, &fixture.url);
    env.push(("INTENT_TRUSTED_PROXIES".into(), "127.0.0.1".into()));
    let server = Server::start(env).await;
    let client = client();
    let clock = Instant::now();
    let site = site_for(0, 2);
    let metrics = |client: reqwest::Client, base: String| async move {
        client
            .get(format!("{base}/metrics"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    };

    // Random site ids: refused, and no rate-limit row for any of them.
    for n in 0..20_u64 {
        let outcome = submit(
            &client,
            &server.base,
            uuid::Uuid::now_v7(),
            &LeadSpec::new("ghost", n).json(),
            Some(&format!("10.40.0.{n}")),
            n,
            clock,
        )
        .await;
        assert_eq!(outcome.status, Some(404));
    }
    let text = metrics(client.clone(), server.base.clone()).await;
    assert!(
        text.contains("intent_rate_table_site_keys 0\n"),
        "unknown sites created rows:\n{text}"
    );

    // 60 honeypot posts and 60 invalid ones (12 + 12 addresses, 5 each), then a genuine lead:
    // none of the refused ones may have spent the site's 60 tokens.
    for n in 0..60_u64 {
        let mut bot = LeadSpec::new("bot", n).json();
        bot["website"] = "http://spam.example".into();
        let outcome = submit(
            &client,
            &server.base,
            site,
            &bot,
            Some(&format!("10.41.0.{}", n / 5)),
            n,
            clock,
        )
        .await;
        assert_eq!(outcome.status, Some(400));
        let mut invalid = LeadSpec::new("invalid", n).json();
        invalid["consent"] = false.into();
        let outcome = submit(
            &client,
            &server.base,
            site,
            &invalid,
            Some(&format!("10.42.0.{}", n / 5)),
            n,
            clock,
        )
        .await;
        assert_eq!(outcome.status, Some(422));
    }
    let genuine = submit(
        &client,
        &server.base,
        site,
        &LeadSpec::new("genuine", 1).json(),
        Some("10.43.0.1"),
        1,
        clock,
    )
    .await;
    assert_eq!(
        genuine.status,
        Some(201),
        "a genuine lead was refused after bots spent the site budget"
    );

    // Replays of a stored lead do not spend it either: 70 resubmissions, then a new lead.
    for n in 0..70_u64 {
        let replay = submit(
            &client,
            &server.base,
            site,
            &LeadSpec::new("genuine", 1).json(),
            Some(&format!("10.44.{}.{}", n / 5, n % 5)),
            1,
            clock,
        )
        .await;
        assert_eq!(replay.status, Some(200), "replay {n}");
    }
    let next = submit(
        &client,
        &server.base,
        site,
        &LeadSpec::new("genuine", 2).json(),
        Some("10.45.0.1"),
        2,
        clock,
    )
    .await;
    assert_eq!(next.status, Some(201), "replays drained the site budget");
    let text = metrics(client.clone(), server.base.clone()).await;
    assert!(text.contains("intent_rate_table_site_keys 1\n"), "{text}");
    db.drop_database().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn every_forwarded_for_line_counts_so_a_client_cannot_pick_its_own_address() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let server = trusted_server(&db, &fixture).await;
    let client = client();
    let site = site_for(0, 1);
    // The client sends a forged first line; the trusted proxy adds the real address as a second
    // line. Read together, right to left, the client is 10.60.0.1 every time.
    let mut statuses = Vec::new();
    for n in 0..7_u64 {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.append("x-forwarded-for", format!("10.50.0.{n}").parse().unwrap());
        headers.append("x-forwarded-for", "10.60.0.1".parse().unwrap());
        let response = client
            .post(format!("{}/v1/sites/{site}/leads", server.base))
            .headers(headers)
            .json(&LeadSpec::new("xff", n).json())
            .send()
            .await
            .unwrap();
        statuses.push(response.status().as_u16());
    }
    assert_eq!(
        statuses,
        vec![201, 201, 201, 201, 201, 429, 429],
        "the forged first line chose the rate-limit key"
    );
    db.drop_database().await;
}
