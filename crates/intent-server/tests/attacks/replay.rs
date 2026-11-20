//! Replay: a submission delivered twice stores one lead however the retry is labelled —
//! the `(site, submission_id)` identity is the dedupe, not anything the client controls.

use crate::support::{LeadSpec, ProviderFixture, Server, TestDb, client, drill_env, site_for};
use intent_provider_fixture::Mode;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn a_replayed_submission_stores_one_lead() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let server = Server::start(drill_env(&db, &fixture.url)).await;
    let client = client();
    let site = site_for(0, 1);
    let spec = LeadSpec::new("replay", 1);

    // The same body three times, each under a fresh request id and a different claimed
    // address: one acknowledged lead must exist, however the repeats arrive.
    for attempt in 0..3 {
        let response = client
            .post(format!("{}/v1/sites/{site}/leads", server.base))
            .header("x-request-id", format!("replay-attempt-{attempt}"))
            .json(&spec.json())
            .send()
            .await
            .unwrap();
        assert!(
            response.status().is_success(),
            "attempt {attempt}: {}",
            response.status()
        );
    }

    let leads = db.leads_with_prefix("replay").await;
    let ids = leads.get("replay-n00001");
    assert_eq!(
        ids.map(Vec::len),
        Some(1),
        "the replayed submission stored {ids:?}"
    );
}
