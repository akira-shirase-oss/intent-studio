//! Operator sign-in (F6, D-015): a global budget on top of the per-address one, an audit row
//! for every attempt carrying actor, address and request id, a `__Host-` cookie, and sessions
//! that end when idle. Run by `scripts/drills.sh`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test code: a failed unwrap is a failed test"
)]

mod support;

use std::time::Duration;

use intent_provider_fixture::Mode;
use sqlx::Row as _;
use support::{ProviderFixture, Server, TestDb, client, drill_env};

const PASSWORD: &str = "operator test password 7";

fn hash() -> String {
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
        .write_all(format!("{PASSWORD}\n").as_bytes())
        .unwrap();
    String::from_utf8(child.wait_with_output().unwrap().stdout)
        .unwrap()
        .trim()
        .to_owned()
}

async fn sign_in(
    client: &reqwest::Client,
    base: &str,
    password: &str,
    address: &str,
    request_id: &str,
) -> reqwest::Response {
    client
        .post(format!("{base}/v1/operator/session"))
        .header("x-forwarded-for", address)
        .header("x-request-id", request_id)
        .json(&serde_json::json!({ "password": password }))
        .send()
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn sign_in_is_audited_uses_a_host_cookie_and_expires_when_idle() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let mut env = drill_env(&db, &fixture.url);
    env.retain(|(k, _)| k != "INTENT_COOKIE_SECURE");
    env.push(("INTENT_OPERATOR_PASSWORD_HASH".into(), hash()));
    env.push(("INTENT_TRUSTED_PROXIES".into(), "127.0.0.1".into()));
    env.push(("INTENT_SESSION_IDLE_SECS".into(), "2".into()));
    let server = Server::start(env).await;
    let client = client();

    let wrong = sign_in(&client, &server.base, "wrong", "10.70.0.1", "req-failed-1").await;
    assert_eq!(wrong.status(), 401);
    let right = sign_in(&client, &server.base, PASSWORD, "10.70.0.2", "req-signin-1").await;
    assert_eq!(right.status(), 200);
    let cookie = right.headers()["set-cookie"].to_str().unwrap().to_owned();
    assert!(cookie.starts_with("__Host-intent_operator="), "{cookie}");
    assert!(
        cookie.contains("; Secure") && cookie.contains("Path=/") && !cookie.contains("Domain"),
        "{cookie}"
    );
    let session = cookie.split(';').next().unwrap().to_owned();

    let inbox = format!("{}/v1/operator/leads", server.base);
    let page = client
        .get(&inbox)
        .header("cookie", &session)
        .header("x-request-id", "req-read-1")
        .header("x-forwarded-for", "10.70.0.2")
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 200);

    let mut conn = db.connect().await;
    let rows =
        sqlx::query("SELECT action, actor, address, request_id FROM operator_audit ORDER BY id")
            .fetch_all(&mut conn)
            .await
            .unwrap();
    let audit: Vec<(String, String, String, String)> = rows
        .iter()
        .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
        .collect();
    assert_eq!(
        audit,
        vec![
            (
                "operator.sign_in_failed".into(),
                "unknown".into(),
                "10.70.0.1".into(),
                "req-failed-1".into()
            ),
            (
                "operator.sign_in".into(),
                "operator".into(),
                "10.70.0.2".into(),
                "req-signin-1".into()
            ),
            (
                "operator.inbox_read".into(),
                "operator".into(),
                "10.70.0.2".into(),
                "req-read-1".into()
            ),
        ]
    );

    // Idle for longer than the idle timeout: the session is over.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let idle = client
        .get(&inbox)
        .header("cookie", &session)
        .send()
        .await
        .unwrap();
    assert_eq!(idle.status(), 401, "an idle session still worked");
    db.drop_database().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn sign_in_has_a_global_budget_beyond_the_per_address_one() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let mut env = drill_env(&db, &fixture.url);
    env.push(("INTENT_OPERATOR_PASSWORD_HASH".into(), hash()));
    env.push(("INTENT_TRUSTED_PROXIES".into(), "127.0.0.1".into()));
    let server = Server::start(env).await;
    let client = client();
    // Thirty guesses, each from a different address: the per-address limit never trips, the
    // global budget (20 a minute) does.
    let started = std::time::Instant::now();
    let mut statuses = Vec::new();
    for n in 0..30 {
        let response = sign_in(
            &client,
            &server.base,
            "guess",
            &format!("10.71.0.{n}"),
            &format!("req-{n}"),
        )
        .await;
        statuses.push(response.status().as_u16());
    }
    // The budget refills one verification every 3 s; exact counts are the unit tests' job
    // (an injected clock), here a slow machine may let the refills of its own duration through.
    let refills = usize::try_from(started.elapsed().as_secs() / 3).unwrap() + 1;
    let failed = statuses.iter().filter(|s| **s == 401).count();
    let limited = statuses.iter().filter(|s| **s == 429).count();
    assert!((20..=20 + refills).contains(&failed), "{statuses:?}");
    assert_eq!(failed + limited, 30, "{statuses:?}");
    db.drop_database().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn a_flood_from_one_address_cannot_lock_the_operator_out() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let mut env = drill_env(&db, &fixture.url);
    env.push(("INTENT_OPERATOR_PASSWORD_HASH".into(), hash()));
    env.push(("INTENT_TRUSTED_PROXIES".into(), "127.0.0.1".into()));
    let server = Server::start(env).await;
    let client = client();
    // 25 junk posts from one address: refused per address (or for their content type) before
    // any password is checked, so none may spend the global budget.
    for n in 0..25 {
        let response = client
            .post(format!("{}/v1/operator/session", server.base))
            .header("x-forwarded-for", "10.72.0.1")
            .header("content-type", "text/plain")
            .body(format!("guess {n}"))
            .send()
            .await
            .unwrap();
        assert!(
            matches!(response.status().as_u16(), 415 | 429),
            "{}",
            response.status()
        );
    }
    let operator = sign_in(&client, &server.base, PASSWORD, "10.72.0.2", "req-op-1").await;
    assert_eq!(
        operator.status(),
        200,
        "the operator was locked out by one address's flood"
    );
    db.drop_database().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn the_operators_known_address_gets_through_a_distributed_flood() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let mut env = drill_env(&db, &fixture.url);
    env.push(("INTENT_OPERATOR_PASSWORD_HASH".into(), hash()));
    env.push(("INTENT_TRUSTED_PROXIES".into(), "127.0.0.1".into()));
    let server = Server::start(env).await;
    let client = client();
    // The operator works from 10.73.0.1 and has signed in from there before.
    assert_eq!(
        sign_in(&client, &server.base, PASSWORD, "10.73.0.1", "req-known")
            .await
            .status(),
        200
    );
    // Thirty wrong guesses, one from each of thirty addresses, drain the global budget.
    let mut limited = 0;
    for n in 0..30 {
        let status = sign_in(
            &client,
            &server.base,
            "guess",
            &format!("10.74.0.{n}"),
            "req-flood",
        )
        .await
        .status();
        limited += usize::from(status == 429);
    }
    assert!(limited > 0, "the global budget never ran out");
    // From the known address, the right password still gets in.
    let known = sign_in(&client, &server.base, PASSWORD, "10.73.0.1", "req-known-2").await;
    assert_eq!(
        known.status(),
        200,
        "the operator's known address was locked out"
    );
    // From an address never used before, the global budget applies (the documented cost).
    let unknown = sign_in(&client, &server.base, PASSWORD, "10.75.0.1", "req-new").await;
    assert_eq!(unknown.status(), 429);
    db.drop_database().await;
}

/// A flood of correctly-shaped but random session cookies from one address must not turn into
/// unbounded database writes: the check is charged per address on failure and refunded on a
/// live session, so the address hears 429 after a small quota — and lead intake, on a pool the
/// flood cannot see, keeps answering (S2 review: the session check was an unpriced UPDATE on
/// intake's own pool).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn a_flood_of_bad_session_cookies_is_limited_and_leaves_intake_alone() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let mut env = drill_env(&db, &fixture.url);
    env.retain(|(k, _)| k != "INTENT_COOKIE_SECURE");
    env.push(("INTENT_OPERATOR_PASSWORD_HASH".into(), hash()));
    env.push(("INTENT_TRUSTED_PROXIES".into(), "127.0.0.1".into()));
    let server = Server::start(env).await;
    let client = client();
    let session = format!("{}/v1/operator/session", server.base);

    let mut statuses = std::collections::HashMap::<u16, usize>::new();
    for n in 0..40_u32 {
        let response = client
            .get(&session)
            .header("cookie", format!("__Host-intent_operator={n:064x}"))
            .header("x-forwarded-for", "10.80.0.1")
            .send()
            .await
            .unwrap();
        *statuses.entry(response.status().as_u16()).or_default() += 1;
    }
    let refused = statuses.get(&429).copied().unwrap_or(0);
    let unauthorized = statuses.get(&401).copied().unwrap_or(0);
    assert!(refused >= 10, "the flood was never limited: {statuses:?}");
    assert!(
        unauthorized <= 30,
        "more session checks than the quota reached the database: {statuses:?}"
    );

    // A known session is not drained with the bucket: the operator signs in first (the digest
    // is marked at sign-in), the flood drains the address, the verified trust lapses, the
    // flood re-drains the refill — and the operator's cookie still answers 200, re-checked
    // for free. Without the known-digest path the last request is another 429.
    let signed = sign_in(&client, &server.base, PASSWORD, "10.80.0.1", "req-known-1").await;
    assert_eq!(signed.status(), 200);
    let real = signed.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let valid = || {
        client
            .get(&session)
            .header("cookie", &real)
            .header("x-forwarded-for", "10.80.0.1")
    };
    // Inside the trust window the digest answers without even a charge.
    assert_eq!(valid().send().await.unwrap().status(), 200);
    tokio::time::sleep(std::time::Duration::from_secs(75)).await;
    let mut drained = 0_u32;
    for n in 0..25_u32 {
        let status = client
            .get(&session)
            .header("cookie", format!("__Host-intent_operator={n:064x}"))
            .header("x-forwarded-for", "10.80.0.1")
            .send()
            .await
            .unwrap()
            .status();
        if status == 429 {
            drained += 1;
        }
    }
    assert!(drained > 0, "the re-flood never re-drained the bucket");
    // Past the TTL, against a re-drained bucket: the known digest re-checks uncharged.
    assert_eq!(
        valid().send().await.unwrap().status(),
        200,
        "a known session was held at 429 by strangers on its address"
    );

    // And the same flood must not keep the operator from ending the session.
    assert_sign_out_survives(&client, &server.base, &real).await;
    // Whichever it answered, the session is dead — the tombstone refuses before any cache.
    assert_eq!(valid().send().await.unwrap().status(), 401);

    // Intake never saw the flood: a lead still lands.
    let lead = client
        .post(format!(
            "{}/v1/sites/00000000-0000-4000-9000-000000000000/leads",
            server.base
        ))
        .header("x-forwarded-for", "10.80.0.9")
        .json(&serde_json::json!({
            "submission_id": "session-flood-1", "name": "Session Flood", "email": "flood@example.test",
            "consent": true, "consent_notice": "privacy-v1"
        }))
        .send()
        .await
        .unwrap();
    assert!(
        lead.status().is_success(),
        "intake starved behind the session flood: {}",
        lead.status()
    );

    // A real session on a fresh address is untouched by the flood's drained bucket.
    let signed = sign_in(
        &client,
        &server.base,
        PASSWORD,
        "10.80.0.2",
        "req-flood-signin",
    )
    .await;
    assert_eq!(signed.status(), 200);
    db.drop_database().await;
}

/// Under a still-drained bucket the operator's sign-out must still land: the delete of a
/// known digest runs on the connection no unauthenticated path reaches, so the answer is
/// 204 — or an honest 503 saying unsure — never a 2xx that leaves the row live. A 204
/// clears the cookie; a 503 keeps it, because the session is still live and the client
/// must be able to retry (N7 residual).
async fn assert_sign_out_survives(client: &reqwest::Client, base: &str, cookie: &str) {
    let out = client
        .delete(format!("{base}/v1/operator/session"))
        .header("cookie", cookie)
        .header("x-forwarded-for", "10.80.0.1")
        .send()
        .await
        .unwrap();
    let cleared = out
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("Max-Age=0"));
    match out.status().as_u16() {
        204 => assert!(cleared, "a confirmed sign-out did not clear the cookie"),
        503 => {
            assert!(
                !cleared,
                "an unconfirmed sign-out dropped the live cookie — the client could never retry"
            );
            assert_eq!(
                out.json::<serde_json::Value>().await.unwrap()["signed_out"],
                false
            );
        }
        status => panic!("sign-out under the flood answered {status}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn a_sign_out_the_database_cannot_confirm_stays_revoked_until_the_retry_lands() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let mut env = drill_env(&db, &fixture.url);
    env.retain(|(k, _)| k != "INTENT_COOKIE_SECURE");
    env.push(("INTENT_OPERATOR_PASSWORD_HASH".into(), hash()));
    env.push(("INTENT_TRUSTED_PROXIES".into(), "127.0.0.1".into()));
    let server = Server::start(env).await;
    let client = client();
    let cookie = sign_in(&client, &server.base, PASSWORD, "10.81.0.1", "req-lock").await;
    assert_eq!(cookie.status(), 200);
    let cookie = cookie
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let session = format!("{}/v1/operator/session", server.base);
    let valid = || client.get(&session).header("cookie", &cookie);

    // Hold the session table hostage: the delete must fail, the answer must be honest, and
    // the tombstone must refuse the still-live row for longer than any old cache window.
    let mut lock = db.connect().await;
    sqlx::query("BEGIN").execute(&mut lock).await.unwrap();
    sqlx::query("LOCK TABLE operator_sessions IN ACCESS EXCLUSIVE MODE")
        .execute(&mut lock)
        .await
        .unwrap();
    let out = client
        .delete(&session)
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(out.status(), 503, "a blocked delete was still claimed");
    assert!(
        out.headers()
            .get("set-cookie")
            .is_none_or(|v| !v.to_str().unwrap().contains("Max-Age=0")),
        "an unconfirmed sign-out dropped the live cookie"
    );
    // The row is still live — prove it before the tombstone's claim alone gets credit.
    let mut rows = db.connect().await;
    let live: i64 = sqlx::query_scalar("SELECT count(*) FROM operator_sessions")
        .fetch_one(&mut rows)
        .await
        .unwrap();
    assert_eq!(live, 1, "the row died behind the failed delete");
    assert_eq!(
        valid().send().await.unwrap().status(),
        401,
        "a still-live row answered after the failed sign-out"
    );

    // Free the table: the tracked retry must finish the delete without another request.
    sqlx::query("ROLLBACK").execute(&mut lock).await.unwrap();
    drop(lock);
    let mut gone = false;
    for _ in 0..40 {
        let live: i64 = sqlx::query_scalar("SELECT count(*) FROM operator_sessions")
            .fetch_one(&mut rows)
            .await
            .unwrap();
        if live == 0 {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(gone, "the background retry never deleted the row");
    assert_eq!(valid().send().await.unwrap().status(), 401);
    db.drop_database().await;
}
