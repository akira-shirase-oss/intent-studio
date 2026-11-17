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
