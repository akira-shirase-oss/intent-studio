//! Operator sign-in (F6, D-015): a global budget on top of the per-address one, an audit row
//! for every attempt carrying actor, address and request id, a `__Host-` cookie, and sessions
//! that end when idle. Run by `scripts/drills.sh`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    reason = "test code: a failed unwrap is a failed test; a drill is a linear narrative"
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
#[cfg(feature = "mutation-seams")] // POST /__test/clock exists only in feature builds (K).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn a_flood_of_bad_session_cookies_is_limited_and_leaves_intake_alone() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let mut env = drill_env(&db, &fixture.url);
    env.retain(|(k, _)| k != "INTENT_COOKIE_SECURE");
    env.push(("INTENT_OPERATOR_PASSWORD_HASH".into(), hash()));
    env.push(("INTENT_TRUSTED_PROXIES".into(), "127.0.0.1".into()));
    env.push(("INTENT_TEST_CLOCK".into(), "true".into()));
    let server = Server::start(env).await;
    let client = client();
    let session = format!("{}/v1/operator/session", server.base);

    // Without INTENT_TEST_CLOCK there is no route at all — a request can never move
    // the clock in production.
    let unclocked = Server::start(drill_env(&db, &fixture.url)).await;
    let refused = client
        .post(format!("{}/__test/clock", unclocked.base))
        .json(&serde_json::json!({ "advance_ms": 1 }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        refused.status(),
        404,
        "the test clock answered without its env"
    );
    drop(unclocked);

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
    // The flood's bucket refills over its window; advancing the injected clock ages it
    // (and the verified-digest trust) without a wall-clock wait (K).
    let advance = client
        .post(format!("{}/__test/clock", server.base))
        .json(&serde_json::json!({ "advance_ms": 75_000 }))
        .send()
        .await
        .unwrap();
    assert_eq!(advance.status(), 204, "the test clock refused the advance");
    // A no-op advance leaves the bucket empty and every re-flood request is another 429:
    // the assertions must see both charged answers and fresh refusals to prove the
    // bucket actually refilled and re-drained.
    let mut charged = 0_u32;
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
        charged += u32::from(status == 401);
        drained += u32::from(status == 429);
    }
    assert!(charged > 0, "the advance never refilled the bucket");
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
#[cfg(feature = "mutation-seams")] // only the clock-injected flood drill calls it
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
async fn a_sign_out_the_database_cannot_confirm_is_an_honest_503_and_the_cookie_stays() {
    // D-069: sign-out either confirms the delete or says so. A blocked delete is a 503
    // with the cookie KEPT — no tombstone for an unconfirmed delete, no background
    // retry; the session stays live until a later attempt confirms.
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

    // Hold the session row hostage: the delete must fail and the answer must be honest.
    // A row lock, not ACCESS EXCLUSIVE: a table lock blocks the plain SELECTs below too
    // and the drill deadlocks on its own assertion (NEW-2).
    let mut lock = db.connect().await;
    sqlx::query("BEGIN").execute(&mut lock).await.unwrap();
    sqlx::query("SELECT token_digest FROM operator_sessions FOR UPDATE")
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
    // The row is still live — and the session honestly still works: only a confirmed
    // delete earns a tombstone (D-069).
    let mut rows = db.connect().await;
    let live: i64 = sqlx::query_scalar("SELECT count(*) FROM operator_sessions")
        .fetch_one(&mut rows)
        .await
        .unwrap();
    assert_eq!(live, 1, "the row died behind the failed delete");
    assert_eq!(
        valid().send().await.unwrap().status(),
        200,
        "an unconfirmed delete broke the still-live session — it must stay valid"
    );
    // A second attempt while the row is held is another honest 503 — every attempt is
    // real; nothing is answered from a pending set that no longer exists.
    let again = client
        .delete(&session)
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(
        again.status(),
        503,
        "the held row must not confirm a delete"
    );

    // Free the row and retry — the operator's retry, not a background task: the delete
    // confirms, the row is gone, the digest is tombstoned.
    sqlx::query("ROLLBACK").execute(&mut lock).await.unwrap();
    drop(lock);
    let settled = client
        .delete(&session)
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(
        settled.status(),
        204,
        "the retried sign-out did not confirm"
    );
    let live: i64 = sqlx::query_scalar("SELECT count(*) FROM operator_sessions")
        .fetch_one(&mut rows)
        .await
        .unwrap();
    assert_eq!(live, 0, "the row survived a confirmed delete");
    assert_eq!(valid().send().await.unwrap().status(), 401);
    // The confirmed delete is answered from the tombstone's memory: a thief looping the
    // revoked cookie gets no uncharged DELETE on the one connection — and the counter is
    // what proves the DB was never asked (H).
    let settled = client
        .delete(&session)
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(settled.status(), 204);
    let metrics = client
        .get(format!("{}/metrics", server.base))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        metrics.contains("intent_signout_tombstoned_answers_total 1\n"),
        "a confirmed delete hit the database again — the memory answer was not counted"
    );
    db.drop_database().await;
}

#[cfg(feature = "mutation-seams")] // POST /__test/clock exists only in feature builds (K, F2).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn a_restart_seeds_only_in_window_sign_ins_under_the_current_credential() {
    // A boot seed must carry the audit's own window — `at + 30d`, never a fresh
    // `now + 30d` — and only sign-ins under the current credential (D, D-069).
    // Three seeded rows separate the cases. Then the GLOBAL minute budget is
    // spent from FOUR OTHER addresses so only the day budget can let a sign-in
    // through — the 5/min per-address quota would refuse a sixth attempt on its
    // own and hide the difference; the flood is what makes the test red.
    //
    // Under INTENT_TEST_CLOCK the drill ages the server clock deterministically:
    // the global bucket's refill runs on the injected time, so twenty Argon2
    // verifications — however slow the machine — can never refill it mid-flood,
    // and the edge row's expiry is one advance away, not a wall-clock sleep (F2).
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let mut env = drill_env(&db, &fixture.url);
    env.retain(|(k, _)| k != "INTENT_COOKIE_SECURE");
    let hash = hash();
    env.push(("INTENT_OPERATOR_PASSWORD_HASH".into(), hash.clone()));
    env.push(("INTENT_TRUSTED_PROXIES".into(), "127.0.0.1".into()));
    env.push(("INTENT_TEST_CLOCK".into(), "true".into()));
    // The fingerprint the server computes from the env's hash — the drill seeds
    // audit rows the server's own sign-in would have written.
    let fp = {
        let d = <sha2::Sha256 as sha2::Digest>::digest(hash.as_bytes());
        hex::encode(&d[..16])
    };
    let mut conn = db.connect().await;
    // 10.83.9.9  — deep inside the window under the current credential: stays known.
    // 10.83.9.10 — an hour left on its audit window: the mark must die on the
    //              audit's own clock, not renew for 30 days at boot (the renewal
    //              bug). An hour survives the slowest boot; the advance crosses it.
    // 10.83.9.11 — fresh but under a rotated-out credential: rotation evicts it.
    sqlx::query(
        "INSERT INTO operator_audit (action, address, at, detail) VALUES
         ('operator.sign_in', '10.83.9.9',  now() - interval '1 minute',
            jsonb_build_object('credential_fp', $1::text)),
         ('operator.sign_in', '10.83.9.10', now() - interval '30 days' + interval '1 hour',
            jsonb_build_object('credential_fp', $1::text)),
         ('operator.sign_in', '10.83.9.11', now() - interval '1 minute',
            jsonb_build_object('credential_fp', 'a-rotated-out-fingerprint'))",
    )
    .bind(&fp)
    .execute(&mut conn)
    .await
    .unwrap();
    let server = Server::start(env).await;
    let client = client();
    // Advance past .10's hour but nowhere near .9's month: .10's mark expires on
    // the audit's own schedule while .9's stays. The marks were seeded at skew 0,
    // so this moves the check-side clock only — a no-op advance leaves .10 known
    // and turns red on the first assert below.
    let advance = client
        .post(format!("{}/__test/clock", server.base))
        .json(&serde_json::json!({ "advance_ms": 75 * 60 * 1000 }))
        .send()
        .await
        .unwrap();
    assert_eq!(advance.status(), 204, "the test clock refused the advance");

    // Spend the GLOBAL 20/minute verification budget from four other addresses —
    // five wrong passwords each, the per-address admit bound. After this, only an
    // address carrying the day budget (a seeded known address) can verify at all.
    for host in 1..=4 {
        for attempt in 0..5 {
            let bad = sign_in(
                &client,
                &server.base,
                "a wrong password",
                &format!("10.83.20.{host}"),
                &format!("req-flood-{host}-{attempt}"),
            )
            .await;
            assert_eq!(bad.status(), 401, "the flood was not even admitted");
        }
    }

    // Legitimately known: its own day budget answers 200 — this proves the flood
    // actually spent the global budget and that knownness still bypasses it.
    let known = sign_in(&client, &server.base, PASSWORD, "10.83.9.9", "req-known").await;
    assert_eq!(
        known.status(),
        200,
        "a legitimately-known address did not keep its day budget"
    );
    // Window passed on the audit's own clock: under `now + window` the restart
    // renewed it and this is a 200 — the red line the review demanded.
    let stale = sign_in(&client, &server.base, PASSWORD, "10.83.9.10", "req-stale").await;
    assert_eq!(
        stale.status(),
        429,
        "the restart renewed an expired window — the address kept a day budget it had lost"
    );
    // Rotated-out credential: a fresh sign-in under the old password seeds nothing.
    let rotated = sign_in(&client, &server.base, PASSWORD, "10.83.9.11", "req-rotated").await;
    assert_eq!(
        rotated.status(),
        429,
        "a sign-in under a rotated-out credential still conferred the day budget"
    );
    db.drop_database().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn a_rotated_credential_ends_the_sessions_it_issued() {
    // F1: a session is bound to the credential that issued it — rotating the
    // password hash ends the sessions the rotated-out hash created. Before
    // credential_fp the check knew only the digest: a stolen cookie kept full
    // access across a rotation until absolute expiry. Red today on the same
    // assert: the row is still live, the check finds it, 200.
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let hash_a = hash();
    let hash_b = hash();
    assert_ne!(
        hash_a, hash_b,
        "argon2's random salt must give the rotation drill two credentials"
    );
    let mut env = drill_env(&db, &fixture.url);
    env.retain(|(k, _)| k != "INTENT_COOKIE_SECURE");
    env.push(("INTENT_OPERATOR_PASSWORD_HASH".into(), hash_a));
    env.push(("INTENT_TRUSTED_PROXIES".into(), "127.0.0.1".into()));
    let mut server = Server::start(env).await;
    let client = client();
    let cookie = sign_in(&client, &server.base, PASSWORD, "10.84.0.1", "req-f1").await;
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

    // Rotation = a boot carrying a different hash: the same session row is still
    // in the table, and the new process must not count it.
    server.kill();
    let mut env = drill_env(&db, &fixture.url);
    env.retain(|(k, _)| k != "INTENT_COOKIE_SECURE");
    env.push(("INTENT_OPERATOR_PASSWORD_HASH".into(), hash_b));
    env.push(("INTENT_TRUSTED_PROXIES".into(), "127.0.0.1".into()));
    let server2 = Server::start(env).await;
    let out = client
        .get(format!("{}/v1/operator/session", server2.base))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(
        out.status(),
        401,
        "a session issued under the rotated-out credential answered after rotation"
    );
    // Control: the password still signs in — the session ended, not the operator.
    let fresh = sign_in(&client, &server2.base, PASSWORD, "10.84.0.1", "req-f1-new").await;
    assert_eq!(fresh.status(), 200);
    db.drop_database().await;
}
