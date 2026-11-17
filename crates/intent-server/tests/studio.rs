//! The studio API against the real binary and database (I1 gate): a brief round-trips; every
//! invalid brief, fact, approval or page tree is a named refusal with paths; a claim slot that
//! references an unapproved, stale, withdrawn or foreign fact cannot validate; tenants stay
//! apart; nothing is read before the session check; every action is audited without content;
//! no record content reaches the log. Run by `scripts/drills.sh`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    reason = "test code: a failed unwrap is a failed test, and a journey reads top to bottom"
)]

mod support;

use intent_provider_fixture::Mode;
use serde_json::{Value, json};
use sqlx::Row as _;
use support::{ProviderFixture, Server, TestDb, client, drill_env};

const PASSWORD: &str = "studio test password 9";
const DEMO_TENANT: &str = "00000000-0000-4000-8000-000000000001";

// Needles for the log scan: record content that must never be logged.
const TESTIMONIAL: &str = "Fixed our boiler the same afternoon, spotless work";
const PRICE: &str = "Boiler service from £80, parts extra";
const APPROVER: &str = "Pat Quickbeam, owner";
const BUSINESS: &str = "Quickbeam & Daughters Heating";
const INTENT: &str = "More boiler-service quote requests from nearby homes";

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

/// A signed-in operator: the session cookie on every request.
struct Operator {
    client: reqwest::Client,
    base: String,
    cookie: String,
}

impl Operator {
    async fn sign_in(base: &str) -> Self {
        let client = client();
        let response = client
            .post(format!("{base}/v1/operator/session"))
            .json(&json!({ "password": PASSWORD }))
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
        Self {
            client,
            base: base.to_owned(),
            cookie,
        }
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> (u16, Value) {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.base))
            .header("cookie", &self.cookie);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let body = response.json().await.unwrap_or(Value::Null);
        (status, body)
    }

    async fn get(&self, path: &str) -> (u16, Value) {
        self.send(reqwest::Method::GET, path, None).await
    }

    async fn post(&self, path: &str, body: &Value) -> (u16, Value) {
        self.send(reqwest::Method::POST, path, Some(body)).await
    }

    async fn put(&self, path: &str, body: &Value) -> (u16, Value) {
        self.send(reqwest::Method::PUT, path, Some(body)).await
    }
}

/// `(path, code)` of every problem in a refusal.
fn problems(body: &Value) -> Vec<(String, String)> {
    body["error"]["problems"]
        .as_array()
        .unwrap_or_else(|| panic!("no problems in {body}"))
        .iter()
        .map(|p| {
            (
                p["path"].as_str().unwrap().to_owned(),
                p["code"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn days_ago(days: i64) -> String {
    let day = time::OffsetDateTime::now_utc().date() - time::Duration::days(days);
    intent_domain::fact::format_date(day)
}

fn brief(claims: &[&str]) -> Value {
    json!({
        "business_name": BUSINESS,
        "goal": "quote_requests",
        "intent": INTENT,
        "audience": "Home owners within ten miles",
        "services": [
            { "name": "Boiler service", "description": "Annual check and clean" },
            { "name": "Boiler repair" }
        ],
        "brand": { "tone": "Plain and friendly", "colors": ["#1F4E79"] },
        "domains": ["Quickbeam.example"],
        "references": [{
            "url": "https://old.quickbeam.example/",
            "permission": "own_content",
            "notes": "Their current site: keep the van photo"
        }],
        "claims": claims,
    })
}

fn tree(testimonial: &str, price: &str) -> Value {
    json!({
        "component_set": "quote-request@0",
        "pages": [{
            "path": "/",
            "title": "Quickbeam Heating",
            "description": "Local heating engineers",
            "sections": [
                { "component": "hero", "heading": "Warm homes, fair prices",
                  "action": { "label": "Get a quote", "target": { "anchor": "quote" } } },
                { "component": "services", "items": [{ "name": "Boiler service", "price": { "fact": price } }] },
                { "component": "testimonials", "items": [{ "fact": testimonial }] },
                { "component": "quote_form", "id": "quote", "services": ["Boiler service"], "consent_notice": "privacy-v1" }
            ]
        }]
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn briefs_and_facts_round_trip_and_refuse_by_name() {
    let db = TestDb::create(1).await;
    let fixture = ProviderFixture::start(Mode::Ok).await;
    let mut env = drill_env(&db, &fixture.url);
    env.push(("INTENT_OPERATOR_PASSWORD_HASH".into(), hash()));
    let server = Server::start(env).await;
    let base = server.base.clone();
    let tenant_path = format!("/v1/operator/tenants/{DEMO_TENANT}");

    // Nothing is read before the session check: a large body to a studio route without a
    // session is refused 401 at once.
    let anonymous = client()
        .post(format!("{base}{tenant_path}/briefs"))
        .header("content-type", "application/json")
        .body(vec![b' '; 100_000])
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), 401);

    let op = Operator::sign_in(&base).await;
    let (status, tenants) = op.get("/v1/operator/tenants").await;
    assert_eq!(status, 200);
    assert!(
        tenants["tenants"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == DEMO_TENANT)
    );

    // Writes are JSON only.
    let form = op
        .client
        .post(format!("{base}{tenant_path}/facts"))
        .header("cookie", &op.cookie)
        .header("content-type", "application/x-www-form-urlencoded")
        .body("kind=price")
        .send()
        .await
        .unwrap();
    assert_eq!(form.status(), 415);

    // Facts: recorded as proposed; an approval makes them approved for the kind's days; an
    // approval recorded long ago is already stale.
    let (status, review) = op
        .post(&format!("{tenant_path}/facts"), &json!({
            "kind": "testimonial", "statement": TESTIMONIAL, "source": "Google review, 2 November"
        }))
        .await;
    assert_eq!(status, 201, "{review}");
    assert_eq!(review["status"], "proposed");
    let review_id = review["id"].as_str().unwrap().to_owned();
    let (status, approved) = op
        .post(
            &format!("{tenant_path}/facts/{review_id}/approval"),
            &json!({ "approved_by": APPROVER, "approved_on": days_ago(1) }),
        )
        .await;
    assert_eq!(status, 200, "{approved}");
    assert_eq!(approved["status"], "approved");
    assert_eq!(approved["review_by"], days_ago(1 - 90));

    let (_, price) = op
        .post(
            &format!("{tenant_path}/facts"),
            &json!({
                "kind": "price", "statement": PRICE, "source": "Price list, November"
            }),
        )
        .await;
    let price_id = price["id"].as_str().unwrap().to_owned();
    let (status, stale) = op
        .post(
            &format!("{tenant_path}/facts/{price_id}/approval"),
            &json!({ "approved_by": APPROVER, "approved_on": days_ago(40) }),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        stale["status"], "stale",
        "a price approval lasts 30 days: {stale}"
    );

    // A proposed fact, and another tenant's approved fact.
    let (_, proposed) = op
        .post(&format!("{tenant_path}/facts"), &json!({
            "kind": "testimonial", "statement": "Not yet approved by the client", "source": "Phone call"
        }))
        .await;
    let proposed_id = proposed["id"].as_str().unwrap().to_owned();
    let (status, other) = op
        .post("/v1/operator/tenants", &json!({ "name": "Other client" }))
        .await;
    assert_eq!(status, 201);
    let other_tenant = other["id"].as_str().unwrap().to_owned();
    let (_, foreign) = op
        .post(&format!("/v1/operator/tenants/{other_tenant}/facts"), &json!({
            "kind": "testimonial", "statement": "Another client's review", "source": "Their e-mail"
        }))
        .await;
    let foreign_id = foreign["id"].as_str().unwrap().to_owned();
    op.post(
        &format!("/v1/operator/tenants/{other_tenant}/facts/{foreign_id}/approval"),
        &json!({ "approved_by": "Someone else", "approved_on": days_ago(1) }),
    )
    .await;
    // Another tenant's fact is not found under this tenant, for any action.
    let (status, _) = op
        .post(
            &format!("{tenant_path}/facts/{foreign_id}/withdrawal"),
            &json!({}),
        )
        .await;
    assert_eq!(status, 404);

    // Invalid facts and approvals are named refusals with paths.
    let (status, body) = op
        .post(
            &format!("{tenant_path}/facts"),
            &json!({ "kind": "rumour", "statement": "", "source": "x" }),
        )
        .await;
    assert_eq!(status, 422);
    assert_eq!(body["error"]["code"], "fact_invalid");
    assert_eq!(
        problems(&body),
        vec![
            ("kind".into(), "unknown_value".into()),
            ("statement".into(), "required".into()),
        ]
    );
    let (status, body) = op
        .post(
            &format!("{tenant_path}/facts/{review_id}/approval"),
            &json!({ "approved_by": APPROVER, "approved_on": days_ago(-3) }),
        )
        .await;
    assert_eq!(status, 422);
    assert_eq!(body["error"]["code"], "approval_invalid");
    assert_eq!(
        problems(&body),
        vec![("approved_on".into(), "in_future".into())]
    );

    // The brief round-trips: what comes back is the normalized brief that was sent.
    let (status, created) = op
        .post(&format!("{tenant_path}/briefs"), &brief(&[&review_id]))
        .await;
    assert_eq!(status, 201, "{created}");
    assert_eq!(created["revision"], 1);
    let brief_id = created["id"].as_str().unwrap().to_owned();
    let (status, read) = op.get(&format!("{tenant_path}/briefs/{brief_id}")).await;
    assert_eq!(status, 200);
    let mut expected = brief(&[&review_id]);
    expected["domains"] = json!(["quickbeam.example"]);
    expected["brand"] =
        json!({ "tone": "Plain and friendly", "colors": ["#1f4e79"], "logo": null });
    expected["services"][1]["description"] = Value::Null;
    assert_eq!(read["brief"], expected);
    assert_eq!(read["revision"], 1);
    let (_, list) = op.get(&format!("{tenant_path}/briefs")).await;
    assert_eq!(list["briefs"][0]["business_name"], BUSINESS);
    // Not under another tenant.
    let (status, _) = op
        .get(&format!(
            "/v1/operator/tenants/{other_tenant}/briefs/{brief_id}"
        ))
        .await;
    assert_eq!(status, 404);

    // Every invalid brief is a named refusal with paths.
    let (status, body) = op
        .post(
            &format!("{tenant_path}/briefs"),
            &json!({ "goal": "quote_requests" }),
        )
        .await;
    assert_eq!(status, 422);
    assert_eq!(body["error"]["code"], "brief_invalid");
    assert_eq!(
        problems(&body),
        vec![
            ("business_name".into(), "required".into()),
            ("intent".into(), "required".into()),
            ("audience".into(), "required".into()),
            ("services".into(), "too_few".into()),
        ]
    );
    let (status, body) = op
        .post(
            &format!("{tenant_path}/briefs"),
            &json!({ "services": [{ "name": 7 }] }),
        )
        .await;
    assert_eq!(
        (status, problems(&body)),
        (422, vec![("services[0].name".into(), "malformed".into())])
    );
    let (status, body) = op
        .post(
            &format!("{tenant_path}/briefs"),
            &json!({ "business_name": "x", "colour": "red" }),
        )
        .await;
    assert_eq!(
        (status, body["error"]["code"].clone()),
        (422, json!("brief_invalid"))
    );
    // Claims must be approved, current facts of this tenant.
    let (status, body) = op
        .post(
            &format!("{tenant_path}/briefs"),
            &brief(&[&review_id, &price_id, &proposed_id, &foreign_id]),
        )
        .await;
    assert_eq!(status, 422);
    assert_eq!(
        problems(&body),
        vec![
            ("claims[1]".into(), "fact_stale".into()),
            ("claims[2]".into(), "fact_unapproved".into()),
            ("claims[3]".into(), "fact_unknown".into()),
        ]
    );

    // Revisions: a change names the revision it was made from; a stale one is refused.
    let mut changed = brief(&[&review_id]);
    changed["audience"] = json!("Home owners and landlords");
    let (status, saved) = op
        .put(
            &format!("{tenant_path}/briefs/{brief_id}"),
            &json!({ "expected_revision": 1, "brief": changed }),
        )
        .await;
    assert_eq!(
        (status, saved["revision"].clone()),
        (200, json!(2)),
        "{saved}"
    );
    let (status, body) = op
        .put(
            &format!("{tenant_path}/briefs/{brief_id}"),
            &json!({ "expected_revision": 1, "brief": changed }),
        )
        .await;
    assert_eq!(status, 409);
    assert_eq!(body["error"]["code"], "revision_conflict");
    assert_eq!(body["error"]["problems"][0]["latest_revision"], 2);
    let (_, first) = op
        .get(&format!("{tenant_path}/briefs/{brief_id}?revision=1"))
        .await;
    assert_eq!(first["brief"]["audience"], "Home owners within ten miles");
    assert_eq!(first["latest_revision"], 2);
    let (status, body) = op
        .put(
            &format!("{tenant_path}/briefs/{brief_id}"),
            &json!({ "expected_revision": 2, "brief": { "business_name": "" } }),
        )
        .await;
    assert_eq!(status, 422);
    assert!(problems(&body).contains(&("brief.business_name".into(), "required".into())));

    // The page tree: valid against the brief, then refused once its facts are not usable. The
    // price was stale when the brief was saved, so revision 2 does not list it.
    let spec = format!("{tenant_path}/briefs/{brief_id}/spec-check");
    let mut valid = tree(&review_id, &price_id);
    valid["pages"][0]["sections"]
        .as_array_mut()
        .unwrap()
        .remove(1);
    let (status, body) = op.post(&spec, &valid).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["valid"], true);
    let (status, body) = op.post(&spec, &tree(&review_id, &price_id)).await;
    assert_eq!(status, 422);
    assert_eq!(body["error"]["code"], "spec_invalid");
    assert_eq!(
        problems(&body),
        vec![(
            "pages[0].sections[1].items[0].price".into(),
            "fact_not_in_brief".into()
        )]
    );
    let (status, body) = op.post(&spec, &tree(&foreign_id, &price_id)).await;
    assert_eq!(status, 422);
    assert!(problems(&body).contains(&(
        "pages[0].sections[2].items[0]".into(),
        "fact_unknown".into()
    )));
    let mut claim = valid.clone();
    claim["pages"][0]["sections"][0]["heading"] = json!("Licensed engineers since 1985");
    let (_, body) = op.post(&spec, &claim).await;
    assert_eq!(
        problems(&body),
        vec![
            (
                "pages[0].sections[0].heading".into(),
                "unbound_claim".into()
            ),
            (
                "pages[0].sections[0].heading".into(),
                "unbound_claim".into()
            ),
        ]
    );
    assert_eq!(body["error"]["problems"][1]["detail"], "1985");
    let mut unknown = valid.clone();
    unknown["pages"][0]["sections"][0]["component"] = json!("marquee");
    let (status, body) = op.post(&spec, &unknown).await;
    assert_eq!(
        (status, problems(&body)),
        (
            422,
            vec![("pages[0].sections[0].component".into(), "malformed".into())]
        )
    );

    // Withdrawing the testimonial makes the same tree fail with fact_withdrawn, and the fact
    // can no longer be approved.
    let (status, withdrawn) = op
        .post(
            &format!("{tenant_path}/facts/{review_id}/withdrawal"),
            &json!({}),
        )
        .await;
    assert_eq!(
        (status, withdrawn["status"].clone()),
        (200, json!("withdrawn"))
    );
    let (_, body) = op.post(&spec, &valid).await;
    assert_eq!(
        problems(&body),
        vec![(
            "pages[0].sections[1].items[0]".into(),
            "fact_withdrawn".into()
        )]
    );
    let (status, body) = op
        .post(
            &format!("{tenant_path}/facts/{review_id}/approval"),
            &json!({ "approved_by": APPROVER, "approved_on": days_ago(0) }),
        )
        .await;
    assert_eq!(
        (status, body["error"]["code"].clone()),
        (409, json!("fact_withdrawn"))
    );

    // The fact list reports each status for today.
    let (_, facts) = op.get(&format!("{tenant_path}/facts")).await;
    let statuses: Vec<(String, String)> = facts["facts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["id"].as_str().unwrap().to_owned(),
                f["status"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert!(statuses.contains(&(review_id.clone(), "withdrawn".into())));
    assert!(statuses.contains(&(price_id.clone(), "stale".into())));
    assert!(statuses.contains(&(proposed_id.clone(), "proposed".into())));
    assert!(
        !statuses.iter().any(|(id, _)| *id == foreign_id),
        "tenants stay apart"
    );

    // Every action is on record, with ids and counts only.
    let mut conn = db.connect().await;
    let rows = sqlx::query("SELECT action, detail::text FROM operator_audit ORDER BY id")
        .fetch_all(&mut conn)
        .await
        .unwrap();
    let actions: Vec<String> = rows.iter().map(|r| r.get::<String, _>(0)).collect();
    for action in [
        "tenants.read",
        "tenant.create",
        "fact.create",
        "fact.approve",
        "fact.withdraw",
        "brief.create",
        "brief.revise",
        "brief.read",
        "briefs.read",
        "spec.check",
        "facts.read",
    ] {
        assert!(
            actions.iter().any(|a| a == action),
            "no {action} in {actions:?}"
        );
    }
    for row in &rows {
        let detail: String = row.get(1);
        for needle in [TESTIMONIAL, PRICE, APPROVER, BUSINESS, INTENT] {
            assert!(
                !detail.contains(needle),
                "audit detail holds content: {detail}"
            );
        }
    }
    // Refused writes left nothing behind: one brief, two revisions.
    let revisions: i64 = sqlx::query_scalar("SELECT count(*) FROM brief_revisions")
        .fetch_one(&mut conn)
        .await
        .unwrap();
    assert_eq!(revisions, 2);

    drop(op);
    let log = server.log();
    support::assert_no_leaks(&log, &[TESTIMONIAL, PRICE, APPROVER, BUSINESS, INTENT]);
    drop(server);
    db.drop_database().await;
}
