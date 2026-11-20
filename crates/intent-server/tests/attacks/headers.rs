//! Spoofed headers: `X-Forwarded-For` is believed only from a trusted proxy (D-019), so a
//! direct client cannot move its limiter key by lying about its address; and `/metrics`
//! answers nobody without its token, however the request describes itself.

use crate::support::{ProviderFixture, Server, TestDb, client, drill_env};
use intent_provider_fixture::Mode;

use crate::auth;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn forwarded_for_does_not_move_the_per_address_budget() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    // No INTENT_TRUSTED_PROXIES: the peer is 127.0.0.1 whoever the request claims to be.
    let mut env = drill_env(&db, &fixture.url);
    env.retain(|(k, _)| k != "INTENT_COOKIE_SECURE");
    env.push(("INTENT_OPERATOR_PASSWORD_HASH".into(), auth::hash()));
    let server = Server::start(env).await;
    let client = client();

    // Five charged attempts are the per-address budget; the sixth is refused — and each
    // request names a different X-Forwarded-For. Were the header believed, every attempt
    // would be a fresh address and no refusal would ever come.
    let mut statuses = Vec::new();
    for i in 0..7u32 {
        let response = client
            .post(format!("{}/v1/operator/session", server.base))
            .header("x-forwarded-for", format!("10.66.0.{i}"))
            .json(&serde_json::json!({ "password": "wrong password" }))
            .send()
            .await
            .unwrap();
        statuses.push(response.status());
    }
    assert!(
        statuses[..5].iter().all(|s| *s == 401),
        "charged attempts: {statuses:?}"
    );
    assert!(
        statuses[5..].iter().all(|s| *s == 429),
        "X-Forwarded-For moved the limiter key: {statuses:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn metrics_refuses_every_spoofed_client_without_the_token() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let mut env = drill_env(&db, &fixture.url);
    env.retain(|(k, _)| k != "INTENT_COOKIE_SECURE");
    env.push((
        "INTENT_METRICS_TOKEN".into(),
        "attack-metrics-token-value".into(),
    ));
    let server = Server::start(env).await;
    let client = client();

    for request in [
        client.get(format!("{}/metrics", server.base)),
        client
            .get(format!("{}/metrics", server.base))
            .header("x-forwarded-for", "127.0.0.1"),
        client
            .get(format!("{}/metrics", server.base))
            .bearer_auth("attack-metrics-token-wrong"),
        client
            .get(format!("{}/metrics", server.base))
            .bearer_auth("attack-metrics-token-wrong")
            .header("x-forwarded-for", "10.0.0.9"),
    ] {
        let status = request.send().await.unwrap().status();
        assert!(
            status.is_client_error(),
            "metrics without the token answered {status}"
        );
    }
    let with_token = client
        .get(format!("{}/metrics", server.base))
        .bearer_auth("attack-metrics-token-value")
        .send()
        .await
        .unwrap();
    assert_eq!(with_token.status(), 200);
}
