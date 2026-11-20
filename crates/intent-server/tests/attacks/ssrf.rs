//! Server-side request forgery: a brief's `references` name URLs the operator asks the
//! platform to fetch (`POST .../snapshots` pulls them through `intent-egress`, D-012). An
//! untrusted brief — an imported page, a pasted form — that points at loopback, link-local
//! or the instance-metadata address must never produce a fetch: the refusal is either at
//! brief validation or `422 fetch_refused` from the guard, and never a stored snapshot.

use crate::support::{ProviderFixture, Server, TestDb, client, drill_env};
use intent_provider_fixture::Mode;
use serde_json::{Value, json};

use crate::auth;

/// Targets a client-side URL fetch must never reach.
const PRIVATE_TARGETS: [&str; 5] = [
    "http://169.254.169.254/latest/meta-data",
    "http://127.0.0.1:59432/",
    "http://[::1]/v1/operator/leads",
    "http://10.255.255.1/",
    "http://192.168.1.1:8080/internal",
];

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn a_snapshot_of_a_private_reference_is_never_fetched() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let mut env = drill_env(&db, &fixture.url);
    env.retain(|(k, _)| k != "INTENT_COOKIE_SECURE");
    env.push(("INTENT_OPERATOR_PASSWORD_HASH".into(), auth::hash()));
    let server = Server::start(env).await;
    let client = client();

    let cookie = client
        .post(format!("{}/v1/operator/session", server.base))
        .json(&json!({ "password": "operator test password 7" }))
        .send()
        .await
        .unwrap()
        .headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let authed = |method: reqwest::Method, path: String, body: Option<&Value>| {
        let request = client
            .request(method, format!("{}{path}", server.base))
            .header("cookie", &cookie);
        match body {
            Some(body) => request.json(body),
            None => request,
        }
    };

    let response = authed(
        reqwest::Method::POST,
        "/v1/operator/tenants".into(),
        Some(&json!({ "name": "SSRF client" })),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(response.status(), 201);
    let tenant = response.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();

    for url in PRIVATE_TARGETS {
        let brief = json!({
            "business_name": "SSRF client",
            "goal": "quote_requests",
            "intent": "quote",
            "audience": "owners",
            "services": [{ "name": "Servicing" }],
            "references": [{ "url": url, "permission": "own_content" }],
            "claims": [],
        });
        let created = authed(
            reqwest::Method::POST,
            format!("/v1/operator/tenants/{tenant}/briefs"),
            Some(&brief),
        )
        .send()
        .await
        .unwrap();
        let status = created.status();
        if status.is_client_error() {
            // Refused at validation: the private URL never even entered a brief.
            continue;
        }
        assert_eq!(status, 201, "{url}: brief create got {status}");
        let brief_id = created.json::<Value>().await.unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();

        let fetched = authed(
            reqwest::Method::POST,
            format!("/v1/operator/tenants/{tenant}/briefs/{brief_id}/snapshots"),
            Some(&json!({ "reference": 0 })),
        )
        .send()
        .await
        .unwrap();
        assert_eq!(
            fetched.status(),
            422,
            "{url}: the egress guard let the fetch run"
        );
        let body = fetched.json::<Value>().await.unwrap_or(Value::Null);
        assert_eq!(body["error"]["code"].as_str(), Some("fetch_refused"));
    }
}
