//! Authentication surface: every operator route refuses forged, absent or dead session
//! material, and a client cannot choose its own session token.

use crate::support::{PASSWORD, ProviderFixture, Server, TestDb, client, drill_env, hash};
use intent_provider_fixture::Mode;

async fn start(db: &TestDb, fixture_url: &str) -> Server {
    let mut env = drill_env(db, fixture_url);
    env.retain(|(k, _)| k != "INTENT_COOKIE_SECURE");
    env.push(("INTENT_OPERATOR_PASSWORD_HASH".into(), hash()));
    Server::start(env).await
}

/// A signed-in cookie for the running server.
async fn sign_in(client: &reqwest::Client, base: &str) -> String {
    let response = client
        .post(format!("{base}/v1/operator/session"))
        .json(&serde_json::json!({ "password": PASSWORD }))
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
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn forged_and_absent_cookies_get_no_operator_route() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let server = start(&db, &fixture.url).await;
    let client = client();

    // Well-formed digests that were never issued, and shapes the parser must refuse.
    let bad_cookies = [
        None,
        Some("__Host-intent_operator=".to_owned()),
        Some(format!("__Host-intent_operator={}", "0".repeat(63))),
        Some(format!("__Host-intent_operator={}", "0".repeat(65))),
        Some(format!("__Host-intent_operator={}", "z".repeat(64))),
        Some(format!("__Host-intent_operator={}", "0".repeat(64))),
        Some(format!("intent_operator={}", "0".repeat(64))),
    ];
    for cookie in bad_cookies {
        for request in [
            client.get(format!("{}/v1/operator/leads", server.base)),
            client.get(format!("{}/v1/operator/tenants", server.base)),
            client
                .post(format!("{}/v1/operator/tenants", server.base))
                .json(&serde_json::json!({"name": "tenant-forged"})),
            client.delete(format!("{}/v1/operator/session", server.base)),
        ] {
            let request = match &cookie {
                Some(cookie) => request.header("cookie", cookie),
                None => request,
            };
            let status = request.send().await.unwrap().status();
            assert_eq!(status, 401, "cookie {cookie:?} got {status}");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn a_client_cannot_choose_its_session_token() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let server = start(&db, &fixture.url).await;
    let client = client();

    let chosen = "a".repeat(64);
    let response = client
        .post(format!("{}/v1/operator/session", server.base))
        .json(&serde_json::json!({ "password": PASSWORD, "session": chosen }))
        .send()
        .await
        .unwrap();
    let status = response.status();
    if status == 200 {
        let cookie = response.headers()["set-cookie"].to_str().unwrap();
        assert_ne!(
            cookie.split('=').nth(1).and_then(|v| v.split(';').next()),
            Some(chosen.as_str()),
            "the issued session is the client's own token"
        );
    } else {
        // Unknown fields refused outright: still no session the client named.
        assert!(status.is_client_error(), "{status}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn a_deleted_session_cannot_be_replayed() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let server = start(&db, &fixture.url).await;
    let client = client();

    let cookie = sign_in(&client, &server.base).await;
    let out = client
        .delete(format!("{}/v1/operator/session", server.base))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert!(out.status().is_success());
    let reused = client
        .get(format!("{}/v1/operator/leads", server.base))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(reused.status(), 401, "a dead session still read the inbox");
}
