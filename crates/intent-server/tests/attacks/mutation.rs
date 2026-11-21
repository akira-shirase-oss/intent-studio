//! Mutation proofs: every attack in this suite earns its place by going red once the
//! guard it exercises is switched off. Each test boots the server with
//! `INTENT_MUTATION=<guard>` — a development-only seam the config refuses outside
//! development — and asserts the *vulnerable* outcome the paired attack makes impossible.
//! A proof that goes red means the seam, not the protection, is what broke.

use std::time::Duration;

use crate::support::{LeadSpec, ProviderFixture, Server, TestDb, client, drill_env, site_for};
use intent_provider_fixture::Mode;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::auth;

/// A server with one guard switched off by name.
async fn mutated(db: &TestDb, fixture_url: &str, mutation: &str, password: bool) -> Server {
    let mut env = drill_env(db, fixture_url);
    env.retain(|(k, _)| k != "INTENT_COOKIE_SECURE");
    env.push(("INTENT_MUTATION".into(), mutation.into()));
    if password {
        env.push(("INTENT_OPERATOR_PASSWORD_HASH".into(), auth::hash()));
    }
    Server::start(env).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn no_session_check_answers_every_cookie() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let server = mutated(&db, &fixture.url, "no-session-check", true).await;
    let client = client();
    // The forged-cookie attack asserts 401; with the check off it gets a real route.
    let forged = client
        .get(format!("{}/v1/operator/leads", server.base))
        .header(
            "cookie",
            format!("__Host-intent_operator={}", "0".repeat(64)),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(forged.status(), 200, "the session check still refused");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn no_session_delete_leaves_the_session_live() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let server = mutated(&db, &fixture.url, "no-session-delete", true).await;
    let client = client();
    let response = client
        .post(format!("{}/v1/operator/session", server.base))
        .json(&json!({ "password": auth::PASSWORD }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let out = client
        .delete(format!("{}/v1/operator/session", server.base))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert!(out.status().is_success());
    // The dead-session attack asserts 401 on replay; with the delete skipped it passes.
    let replay = client
        .get(format!("{}/v1/operator/leads", server.base))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 200, "the session was deleted anyway");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn trust_any_forwarded_moves_the_limiter() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let server = mutated(&db, &fixture.url, "trust-any-forwarded", true).await;
    let client = client();
    // The XFF attack asserts rotation never moves the key; believed, every attempt is a
    // fresh address and the quota never binds.
    for i in 0..7u32 {
        let status = client
            .post(format!("{}/v1/operator/session", server.base))
            .header("x-forwarded-for", format!("10.66.0.{i}"))
            .json(&json!({ "password": "wrong password" }))
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, 401, "attempt {i}: the limiter moved anyway");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn no_metrics_token_opens_metrics() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    // A token is configured so the guard is real: without the seam the request is 401 (M1).
    let mut env = drill_env(&db, &fixture.url);
    env.retain(|(k, _)| k != "INTENT_COOKIE_SECURE");
    env.push(("INTENT_MUTATION".into(), "no-metrics-token".into()));
    env.push(("INTENT_METRICS_TOKEN".into(), "proof-metrics-token".into()));
    let server = Server::start(env).await;
    let client = client();
    let status = client
        .get(format!("{}/metrics", server.base))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 200, "metrics still asked for a token");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn no_request_bound_reads_the_fat_body() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let server = mutated(&db, &fixture.url, "no-request-bound", false).await;
    let client = client();
    let site = site_for(0, 1);
    // The oversized attack asserts a refusal; with the bound off the body is read and the
    // (valid) lead is stored.
    let fat = serde_json::json!({
        "submission_id": "fat-mutation-1", "name": "n", "email": "a@b.c", "phone": "1",
        "message": "x".repeat(64 * 1024), "consent": true, "consent_notice": "v1"
    });
    let status = client
        .post(format!("{}/v1/sites/{site}/leads", server.base))
        .json(&fat)
        .send()
        .await
        .unwrap()
        .status();
    assert!(
        status.is_success(),
        "the 64 KiB body was still refused: {status}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn no_request_bound_holds_a_dribbled_body() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let server = mutated(&db, &fixture.url, "no-request-bound", false).await;
    let site = site_for(0, 1);
    // The slow-body attack asserts the server gives up inside its request timeout; with
    // the bound off the dribble is still open well past it.
    let mut slow = tokio::net::TcpStream::connect(server.address)
        .await
        .unwrap();
    slow.write_all(
        format!(
            "POST /v1/sites/{site}/leads HTTP/1.1\r\nhost: x\r\ncontent-type: application/json\r\ncontent-length: 16384\r\nconnection: close\r\n\r\n"
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let mut buf = [0u8; 512];
    // Forty quiet 200 ms windows — far past the default request timeout — and the
    // connection must still be waiting on the body the whole time.
    for _ in 0..40 {
        match tokio::time::timeout(Duration::from_millis(200), slow.read(&mut buf)).await {
            Err(_elapsed) => {} // still open, nothing yet — keep dribbling
            Ok(Ok(0)) => panic!("the server gave up on the body anyway"),
            Ok(Ok(_)) => panic!("the server answered without the body anyway"),
            Ok(Err(_io)) => panic!("the server dropped the connection anyway"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn no_content_type_accepts_a_form() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let server = mutated(&db, &fixture.url, "no-content-type", true).await;
    let client = client();
    // The content-type attack asserts 415; with the check off the form reaches the parser
    // and dies a 400, not a media-type refusal.
    let status = client
        .post(format!("{}/v1/operator/session", server.base))
        .header("content-type", "application/x-www-form-urlencoded")
        .body("password=x")
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(
        status, 400,
        "with the media check off the form reaches the JSON parser: {status}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn no_egress_guard_fetches_loopback() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let server = mutated(&db, &fixture.url, "no-egress-guard", true).await;
    let client = client();
    let cookie = {
        let response = client
            .post(format!("{}/v1/operator/session", server.base))
            .json(&json!({ "password": auth::PASSWORD }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned()
    };
    let authed = |body: &Value| {
        client
            .post(format!("{}/v1/operator/tenants", server.base))
            .header("cookie", &cookie)
            .json(body)
    };
    let tenant = authed(&json!({ "name": "mut" }))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let brief = client
        .post(format!(
            "{}/v1/operator/tenants/{tenant}/briefs",
            server.base
        ))
        .header("cookie", &cookie)
        .json(&json!({
            "business_name": "mut", "goal": "quote_requests", "intent": "quote",
            "audience": "owners", "services": [{ "name": "s" }],
            "references": [{ "url": fixture.url, "permission": "own_content" }],
            "claims": [],
        }))
        .send()
        .await
        .unwrap();
    let brief_id = brief.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    // The SSRF attack asserts 422 fetch_refused on a private address; loopback is private.
    let fetched = client
        .post(format!(
            "{}/v1/operator/tenants/{tenant}/briefs/{brief_id}/snapshots",
            server.base
        ))
        .header("cookie", &cookie)
        .json(&json!({ "reference": 0 }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        fetched.status(),
        201,
        "the egress guard still refused a private address"
    );
    let stored = fetched.json::<Value>().await.unwrap();
    assert!(
        stored["id"].as_str().is_some_and(|id| !id.is_empty()),
        "no snapshot row for the fetched private address: {stored}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn no_lead_dedup_stores_every_retry() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let server = mutated(&db, &fixture.url, "no-lead-dedup", false).await;
    let client = client();
    let site = site_for(0, 1);
    let spec = LeadSpec::new("dupmut", 1);
    // The replay attack asserts one stored lead; with dedupe off every retry is a row.
    for attempt in 0..3 {
        let response = client
            .post(format!("{}/v1/sites/{site}/leads", server.base))
            .header("x-request-id", format!("dup-{attempt}"))
            .json(&spec.json())
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
    }
    // With the key deduped out each retry is a row of its own, keyed `c:m<uuid>`.
    let leads = db.leads_with_prefix("m").await;
    let stored: usize = leads.values().map(Vec::len).sum();
    assert_eq!(stored, 3, "dedup still merged them: {leads:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn no_content_type_accepts_a_bare_withdrawal() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let server = mutated(&db, &fixture.url, "no-content-type", true).await;
    let client = client();
    let response = client
        .post(format!("{}/v1/operator/session", server.base))
        .json(&json!({ "password": auth::PASSWORD }))
        .send()
        .await
        .unwrap();
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    // The CSRF attack asserts the marker is required: a bare POST carries neither
    // application/json nor x-intent, so guarded it dies 415; with the seam on it reaches
    // the handler and fails only because the fact is unknown — 404, not a media refusal.
    let status = client
        .post(format!(
            "{}/v1/operator/tenants/00000000-0000-0000-0000-000000000000/facts/00000000-0000-0000-0000-000000000000/withdrawal",
            server.base
        ))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 404, "the CSRF marker was still enforced: {status}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn client_session_token_issues_the_client_chosen_token() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let server = mutated(&db, &fixture.url, "client-session-token", true).await;
    let client = client();
    let chosen = "b".repeat(64);
    // The token-choice attack asserts the mint ignores a client field; with the seam on the
    // response carries exactly the token the client named — proof the guard was real.
    let response = client
        .post(format!("{}/v1/operator/session", server.base))
        .json(&json!({ "password": auth::PASSWORD, "session": chosen }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .to_owned();
    let issued = cookie.split('=').nth(1).and_then(|v| v.split(';').next());
    assert_eq!(
        issued,
        Some(chosen.as_str()),
        "the mint ignored the client token"
    );
    let replay = client
        .get(format!("{}/v1/operator/leads", server.base))
        .header("cookie", cookie.split(';').next().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(
        replay.status(),
        200,
        "the chosen token did not open the session"
    );
}
