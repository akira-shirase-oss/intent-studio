//! Bounded bodies: oversized, deeply nested and mistyped payloads are refused before they
//! can do work, and a slow-dribbled body cannot starve the connections beside it.

use std::time::Duration;

use crate::support::{ProviderFixture, Server, TestDb, client, drill_env, site_for};
use intent_provider_fixture::Mode;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn oversized_nested_and_mistyped_bodies_are_refused() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let server = Server::start(drill_env(&db, &fixture.url)).await;
    let client = client();
    let site = site_for(0, 1);
    let leads = format!("{}/v1/sites/{site}/leads", server.base);

    // Over the intake bound (16 KiB): refused before the body does work.
    let fat = serde_json::json!({
        "submission_id": "fat-body-1", "name": "n", "email": "a@b.c", "phone": "1",
        "message": "x".repeat(64 * 1024), "consent": true, "consent_notice": "v1"
    });
    let status = client
        .post(&leads)
        .json(&fat)
        .send()
        .await
        .unwrap()
        .status();
    assert!(status.is_client_error(), "a 64 KiB lead body got {status}");

    // Deeper than serde's recursion limit: a parse error, never a crash or a hang.
    let deep = "[".repeat(20_000) + &"]".repeat(20_000);
    let deep = client
        .post(&leads)
        .header("content-type", "application/json")
        .body(deep)
        .send()
        .await
        .unwrap();
    assert!(
        deep.status().is_client_error(),
        "20k-deep JSON got {}",
        deep.status()
    );

    // A form post is not a sign-in (also the CSRF shape: no cross-site form can send JSON).
    let form = client
        .post(format!("{}/v1/operator/session", server.base))
        .header("content-type", "application/x-www-form-urlencoded")
        .body("password=x")
        .send()
        .await
        .unwrap();
    assert_eq!(form.status(), 415);
    let form = client
        .post(&leads)
        .header("content-type", "application/x-www-form-urlencoded")
        .body("submission_id=ct-confusion")
        .send()
        .await
        .unwrap();
    assert!(form.status().is_client_error());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn a_slow_body_cannot_starve_the_other_connections() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let server = Server::start(drill_env(&db, &fixture.url)).await;
    let client = client();
    let site = site_for(0, 1);

    // One connection promises 16 KiB and then sends a byte every 150 ms.
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

    // While it dribbles, other clients are served: twenty health checks, all answered.
    let healthz = format!("{}/healthz", server.base);
    for _ in 0..20 {
        slow.write_all(b"x").await.unwrap();
        let status = client.get(&healthz).send().await.unwrap().status();
        assert_eq!(status, 200);
        tokio::time::sleep(Duration::from_millis(150)).await;
    }

    // And the dribble has a deadline: the server must give up on the promised body (the
    // request timeout's default is 5 s) rather than hold the connection open forever.
    let mut buf = [0u8; 512];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        match tokio::time::timeout_at(deadline, slow.read(&mut buf)).await {
            Ok(Ok(0)) => break,
            Ok(Err(_io)) => break,
            Ok(Ok(_)) => {
                slow.write_all(b"x").await.unwrap();
            }
            Err(elapsed) => panic!("the server held a dribbled body open past 15 s: {elapsed}"),
        }
    }
}
