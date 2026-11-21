//! The `generation-fails` drill (I2): every failure the build-and-check path can
//! produce today lands as its named code — never a bare error, never a record
//! that says more than ran — and what a previous version produced stays
//! untouched. Run by `scripts/drills.sh`, which builds `intent-worker` first.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test code: a failed unwrap is a failed test"
)]

#[path = "support/mod.rs"]
mod support;

use std::{path::PathBuf, time::Duration};

use intent_db::{AuditEntry, DEMO_TENANT as DEMO, Db, PoolLimits};
use intent_domain::{
    baseline,
    bundle::Bundle,
    checks::{CheckKind, CheckStatus},
    content::ContentHash,
    fact::FactLookup,
    ids::BriefId,
    page_tree::PageTree,
};
use intent_render::Driver;
use intent_server::checks::{self, CheckError, SuiteOutcome};
use intent_store::ContentStore;
use serde_json::json;
use support::TestDb;
use uuid::Uuid;

const AUDIT: AuditEntry<'static> = AuditEntry {
    action: "test",
    actor: "test",
    address: None,
    request_id: None,
    detail: "{}",
};

fn worker() -> PathBuf {
    let path = PathBuf::from(support::server_binary())
        .parent()
        .unwrap()
        .join("intent-worker");
    assert!(
        path.is_file(),
        "no intent-worker beside intent-server at {}: build it first (scripts/drills.sh does)",
        path.display()
    );
    path
}

fn store() -> ContentStore {
    let dir = std::env::temp_dir().join(format!("intent-checks-{}", uuid::Uuid::now_v7().simple()));
    ContentStore::open(&dir, intent_store::DEFAULT_MAX_BLOB_BYTES).unwrap()
}

fn driver() -> Driver {
    Driver::new(worker(), Duration::from_secs(10), 1)
}

fn config(mutation: Option<&str>) -> intent_server::config::Config {
    let mut env = vec![
        (
            "INTENT_DATABASE_URL",
            "postgres://intent:intent-dev@127.0.0.1:59432/intent",
        ),
        ("INTENT_NOTIFY_PROVIDER", "recorder"),
        ("INTENT_PROFILE", "development"),
        ("INTENT_STORE_DIR", "/tmp/intent-checks-x"),
    ];
    let owned: Vec<(String, String)> = env
        .drain(..)
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .chain(mutation.map(|m| ("INTENT_MUTATION".to_owned(), m.to_owned())))
        .collect();
    intent_server::config::Config::from_lookup(move |key| {
        owned.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    })
    .unwrap()
}

/// A page tree with no claim slots — the baseline needs no facts to fill it.
fn tree() -> PageTree {
    serde_json::from_value(json!({
        "component_set": "quote-request@0",
        "pages": [{
            "path": "/",
            "title": "Quickbeam Heating",
            "description": "Local heating engineers",
            "sections": [
                { "component": "hero", "heading": "Warm homes, fair prices",
                  "action": { "label": "Get a quote", "target": { "anchor": "quote" } } },
                { "component": "quote_form", "id": "quote",
                  "services": ["Boiler service"], "consent_notice": "privacy-v1" }
            ]
        }]
    }))
    .unwrap()
}

fn brief_value() -> intent_domain::brief::Brief {
    let form: intent_domain::brief::BriefForm = serde_json::from_value(json!({
        "business_name": "Quickbeam Heating",
        "goal": "quote_requests",
        "intent": "More boiler-service enquiries",
        "audience": "Home owners within ten miles",
        "services": [{ "name": "Boiler service" }],
        "brand": { "tone": "Plain", "colors": ["#1F4E79"] },
        "domains": ["Quickbeam.example"],
        "references": [],
        "claims": []
    }))
    .unwrap();
    form.validate().unwrap()
}

async fn brief_and_candidate(
    db: &Db,
    tree: &PageTree,
    bundle_sha256: ContentHash,
) -> intent_db::CandidateRow {
    let today = time::OffsetDateTime::now_utc().date();
    let brief = brief_value();
    let brief_id = BriefId::from_uuid(Uuid::now_v7());
    assert!(!matches!(
        db.save_brief(DEMO, brief_id, None, &brief, today, &AUDIT)
            .await
            .unwrap(),
        intent_db::BriefSave::NotFound
    ));
    db.insert_candidate(
        &intent_db::NewCandidate {
            id: Uuid::now_v7(),
            tenant: DEMO,
            brief: brief_id,
            tree: &serde_json::to_value(tree).unwrap(),
            bundle_sha256,
            builder: baseline::BUILDER,
            model: None,
        },
        &AUDIT,
    )
    .await
    .unwrap()
    .unwrap()
}

/// The only shape `build` accepts: validation's own mint, with the drill's
/// brief and empty facts (the fixture tree references none).
fn vtree() -> intent_domain::page_tree::ValidatedTree {
    let brief = brief_value();
    let facts = FactLookup::new();
    intent_domain::page_tree::ValidatedTree::validate(
        tree(),
        intent_domain::page_tree::SpecContext {
            brief: &brief,
            facts: &facts,
            today: time::OffsetDateTime::now_utc().date(),
        },
    )
    .unwrap_or_else(|problems| panic!("the drill fixture must validate: {problems:?}"))
}

/// Builds the baseline and puts its blob in `store` — the path every candidate
/// takes: the same tree gives the same bytes, proved byte-for-byte.
async fn baseline_bundle(store: &ContentStore) -> (Bundle, ContentHash) {
    let today = time::OffsetDateTime::now_utc().date();
    let bundle = baseline::build(&vtree(), &FactLookup::new(), today).unwrap();
    let again = baseline::build(&vtree(), &FactLookup::new(), today).unwrap();
    assert_eq!(
        bundle.encode(),
        again.encode(),
        "the same input must build byte-identical output"
    );
    let hash = store.put(bundle.encode()).await.unwrap();
    assert_eq!(
        hash,
        bundle.hash(),
        "the store's key is the bundle's own hash"
    );
    (bundle, hash)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn a_clean_bundle_is_pending_until_the_browser_backend_lands() {
    let test = TestDb::create(1).await;
    let db = Db::connect_lazy(&test.url, PoolLimits::default()).unwrap();
    let store = store();
    let (_, hash) = baseline_bundle(&store).await;
    let candidate = brief_and_candidate(&db, &tree(), hash).await;

    let outcome = checks::run(
        &config(None),
        &db,
        &store,
        &driver(),
        DEMO,
        candidate.id,
        None,
    )
    .await
    .unwrap();
    // Honest: the statics passed, the six browser checks could not run — and
    // `pending` is what the record says, never `pass`.
    assert_eq!(outcome, SuiteOutcome::Pending);
    let records = db.check_records(candidate.id).await.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].verdict, "pending");
    assert_eq!(records[0].bundle_sha256, candidate.bundle_sha256);
    assert_eq!(records[0].suite, intent_domain::checks::SUITE);
    let results: Vec<intent_domain::checks::CheckResult> =
        serde_json::from_value(records[0].results.clone()).unwrap();
    let pass = |kind| {
        results
            .iter()
            .any(|r| r.check == kind && r.status == CheckStatus::Pass)
    };
    assert!(pass(CheckKind::NoInlineSvg));
    assert!(pass(CheckKind::NoExternalResources));
    let pending = results
        .iter()
        .filter(|r| r.status == CheckStatus::Pending)
        .count();
    assert_eq!(pending, 6, "the six browser checks stay honestly pending");
    test.drop_database().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn inline_svg_is_checks_failed_and_the_record_names_it() {
    let test = TestDb::create(1).await;
    let db = Db::connect_lazy(&test.url, PoolLimits::default()).unwrap();
    let store = store();
    let today = time::OffsetDateTime::now_utc().date();
    let mut bundle = baseline::build(&vtree(), &FactLookup::new(), today).unwrap();
    // What a hostile or buggy generator would ship: the check must catch it in
    // the bytes, not trust that the builder never would. The page carries the
    // fixed prologue so `no_inline_svg` itself — not the contract's prologue
    // arm — is what names the failure.
    bundle
        .put(
            "index.html",
            format!(
                "{}{}",
                intent_domain::baseline::PROLOGUE,
                "<title>x</title><meta name=\"description\" content=\"x\">\
                 <link rel=\"stylesheet\" href=\"/style.css\"></head><body>\
                 <svg onload=alert(1)><rect/></svg></body></html>"
            )
            .into_bytes(),
        )
        .unwrap();
    let hash = store.put(bundle.encode()).await.unwrap();
    let candidate = brief_and_candidate(&db, &tree(), hash).await;

    let outcome = checks::run(
        &config(None),
        &db,
        &store,
        &driver(),
        DEMO,
        candidate.id,
        None,
    )
    .await
    .unwrap();
    assert_eq!(outcome, SuiteOutcome::ChecksFailed);
    let records = db.check_records(candidate.id).await.unwrap();
    assert_eq!(records[0].verdict, "fail");
    let results: Vec<intent_domain::checks::CheckResult> =
        serde_json::from_value(records[0].results.clone()).unwrap();
    let svg = results
        .iter()
        .find(|r| r.check == CheckKind::NoInlineSvg)
        .unwrap();
    assert_eq!(svg.status, CheckStatus::Fail);
    test.drop_database().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn a_worker_that_dies_is_render_crashed_and_writes_nothing() {
    let test = TestDb::create(1).await;
    let db = Db::connect_lazy(&test.url, PoolLimits::default()).unwrap();
    let store = store();
    let (_, hash) = baseline_bundle(&store).await;
    let candidate = brief_and_candidate(&db, &tree(), hash).await;
    // A worker that exits immediately — every job is a crash.
    let dead = Driver::new(PathBuf::from("/usr/bin/false"), Duration::from_secs(5), 1);
    let error = checks::run(&config(None), &db, &store, &dead, DEMO, candidate.id, None)
        .await
        .unwrap_err();
    assert!(
        matches!(error, CheckError::RenderCrashed),
        "expected render_crashed, got {error}"
    );
    assert!(
        db.check_records(candidate.id).await.unwrap().is_empty(),
        "a crashed run records nothing"
    );
    test.drop_database().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn a_check_record_cannot_describe_a_different_hash() {
    let test = TestDb::create(1).await;
    let db = Db::connect_lazy(&test.url, PoolLimits::default()).unwrap();
    let store = store();
    let (_, hash_a) = baseline_bundle(&store).await;
    // A second candidate over different bytes: the wrong-hash temptation.
    let today = time::OffsetDateTime::now_utc().date();
    let mut other = baseline::build(&vtree(), &FactLookup::new(), today).unwrap();
    other.put("index.html", b"different".to_vec()).unwrap();
    let hash_b = store.put(other.encode()).await.unwrap();
    let candidate = brief_and_candidate(&db, &tree(), hash_a).await;

    // The write layer itself: a record for candidate A naming B's bundle is
    // refused by the pair foreign key, whatever asked for it. Real results —
    // the verdict is derived, not supplied (I2-4).
    let error = db
        .insert_check_record(
            &intent_db::NewCheckRecord {
                id: Uuid::now_v7(),
                candidate_id: candidate.id,
                bundle_sha256: hash_b,
                suite: intent_domain::checks::SUITE,
                engine: "static",
                viewport: "static",
                results: &json!([{ "check": "no_inline_svg", "status": "pass" }]),
            },
            &AUDIT,
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.sqlstate().as_deref(),
        Some("23503"),
        "a record for a different hash must be refused by the binding, not by luck"
    );
    // And an empty results array has no path to a `pass` row: the evidence
    // itself refuses before the foreign key is ever consulted.
    assert!(
        db.insert_check_record(
            &intent_db::NewCheckRecord {
                id: Uuid::now_v7(),
                candidate_id: candidate.id,
                bundle_sha256: hash_a,
                suite: intent_domain::checks::SUITE,
                engine: "static",
                viewport: "static",
                results: &json!([]),
            },
            &AUDIT,
        )
        .await
        .is_err(),
        "empty evidence cannot be written as a pass"
    );
    test.drop_database().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn malformed_model_output_is_refused_and_the_previous_version_is_untouched() {
    let test = TestDb::create(1).await;
    let db = Db::connect_lazy(&test.url, PoolLimits::default()).unwrap();
    let store = store();
    let (_, hash_a) = baseline_bundle(&store).await;
    let candidate = brief_and_candidate(&db, &tree(), hash_a).await;
    checks::run(
        &config(None),
        &db,
        &store,
        &driver(),
        DEMO,
        candidate.id,
        None,
    )
    .await
    .unwrap();
    let before = db.check_records(candidate.id).await.unwrap().len();

    // `model_invalid_output` — bytes that are not a page tree are a parse
    // refusal; a tree that is schema-shaped but unusable is a named refusal.
    // Neither writes a candidate.
    assert!(serde_json::from_slice::<PageTree>(b"not a tree").is_err());
    let mut bad_tree = tree();
    bad_tree.component_set = "other@9".into();
    assert!(
        intent_domain::page_tree::validate(
            &bad_tree,
            intent_domain::page_tree::SpecContext {
                brief: &brief_value(),
                facts: &FactLookup::new(),
                today: time::OffsetDateTime::now_utc().date(),
            }
        )
        .iter()
        .any(|p| {
            matches!(
                p.code,
                intent_domain::problem::ProblemCode::UnknownComponentSet
            )
        }),
        "an out-of-vocabulary tree is a named refusal, not a retry-forever"
    );
    // The previous version: its blob is still there, still hashing to the
    // candidate's hash; its records are exactly what they were.
    let stored = store.get(hash_a).await.unwrap().unwrap();
    assert_eq!(ContentHash::of(&stored), hash_a);
    assert_eq!(
        db.check_records(candidate.id).await.unwrap().len(),
        before,
        "a failed generation touches nothing already recorded"
    );
    test.drop_database().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
async fn a_report_missing_a_check_is_malformed_not_a_record() {
    let test = TestDb::create(1).await;
    let db = Db::connect_lazy(&test.url, PoolLimits::default()).unwrap();
    let store = store();
    let (_, hash) = baseline_bundle(&store).await;
    let candidate = brief_and_candidate(&db, &tree(), hash).await;

    // A worker answering the protocol but omitting checks: the runner treats an
    // incomplete report as malformed — a missing check can never launder a fail.
    let dir = std::env::temp_dir().join(format!(
        "intent-fake-worker-{}",
        uuid::Uuid::now_v7().simple()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("fake-check-worker.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\nbody='{\"suite\":\"d-022@0\",\"results\":[]}'\nprintf '{\"ok\":true,\"media_type\":\"application/json\",\"length\":%d}\\n%s' \"${#body}\" \"$body\"\n",
    )
    .unwrap();
    std::fs::set_permissions(
        &script,
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
    )
    .unwrap();
    let fake = Driver::new(script, Duration::from_secs(5), 1);

    let error = checks::run(&config(None), &db, &store, &fake, DEMO, candidate.id, None)
        .await
        .unwrap_err();
    assert!(
        matches!(error, CheckError::ReportMalformed),
        "an incomplete report must be malformed, got {error}"
    );
    assert!(db.check_records(candidate.id).await.unwrap().is_empty());
    test.drop_database().await;
}

/// A tree schema parse that cannot be tricked: no `ValidatedTree` mints from
/// it, so the build door is never reached (I2-2).
#[test]
fn a_tree_referencing_an_unapproved_fact_is_refused_by_name() {
    let mut t = tree();
    let id = intent_domain::ids::FactId::from_uuid(Uuid::from_u128(9));
    t.pages[0]
        .sections
        .push(intent_domain::page_tree::Section::Hours {
            id: None,
            heading: None,
            fact: intent_domain::page_tree::FactRef { fact: id },
        });
    let brief = brief_value();
    let facts = FactLookup::new();
    assert!(
        intent_domain::page_tree::ValidatedTree::validate(
            t,
            intent_domain::page_tree::SpecContext {
                brief: &brief,
                facts: &facts,
                today: time::OffsetDateTime::now_utc().date(),
            },
        )
        .is_err(),
        "an unapproved fact reference must fail validation — no tree to build"
    );
}

/// The mutation proofs, in the same file the protections live in: the seam only
/// exists under `mutation-seams`, so these are compiled out of the shipped
/// build — and `check-seams.sh` proves it.
#[cfg(feature = "mutation-seams")]
mod seams {
    use super::*;

    /// `allow-inline-svg`: the seam drops the check's result — the report is
    /// now incomplete, and completeness is enforced: the run is refused as
    /// `report_malformed`, no record is written. The fixture fails ONLY
    /// `no_inline_svg` (a valid prologue page with one `<svg>`), so nothing
    /// else can produce the refusal — the dropped evidence is what did it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
    async fn allow_inline_svg_is_caught_by_report_completeness() {
        let test = TestDb::create(1).await;
        let db = Db::connect_lazy(&test.url, PoolLimits::default()).unwrap();
        let store = store();
        let today = time::OffsetDateTime::now_utc().date();
        let mut bundle = baseline::build(&vtree(), &FactLookup::new(), today).unwrap();
        let payload = format!(
            "{}{}",
            intent_domain::baseline::PROLOGUE,
            "<title>x</title><meta name=\"description\" content=\"x\">\
             <link rel=\"stylesheet\" href=\"/style.css\"></head><body>\
             <svg onload=alert(1)></svg></body></html>"
        );
        bundle.put("index.html", payload.into_bytes()).unwrap();
        let hash = store.put(bundle.encode()).await.unwrap();
        let candidate = brief_and_candidate(&db, &tree(), hash).await;

        let error = checks::run(
            &config(Some("allow-inline-svg")),
            &db,
            &store,
            &driver(),
            DEMO,
            candidate.id,
            None,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(error, CheckError::ReportMalformed),
            "a dropped check result must be malformed, got {error}"
        );
        assert!(db.check_records(candidate.id).await.unwrap().is_empty());
        test.drop_database().await;
    }

    /// `check-any-hash`: a caller-supplied hash reaches the record write — and
    /// the pair foreign key refuses it. The record layer's binding is what the
    /// drill relies on, and the schema, not the code, is the proof.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "needs the stack: scripts/stack.sh up (run by scripts/drills.sh)"]
    async fn check_any_hash_reaches_the_write_and_the_binding_refuses() {
        let test = TestDb::create(1).await;
        let db = Db::connect_lazy(&test.url, PoolLimits::default()).unwrap();
        let store = store();
        let (_, hash_a) = baseline_bundle(&store).await;
        let today = time::OffsetDateTime::now_utc().date();
        let mut other = baseline::build(&vtree(), &FactLookup::new(), today).unwrap();
        other.put("index.html", b"other".to_vec()).unwrap();
        let hash_b = store.put(other.encode()).await.unwrap();
        let candidate = brief_and_candidate(&db, &tree(), hash_a).await;

        let error = checks::run(
            &config(Some("check-any-hash")),
            &db,
            &store,
            &driver(),
            DEMO,
            candidate.id,
            Some(hash_b), // under the seam, this is what the record claims ran
        )
        .await
        .unwrap_err();
        assert!(
            matches!(error, CheckError::CheckBinding),
            "the wrong-hash write must be refused by the binding, got {error}"
        );
        assert!(db.check_records(candidate.id).await.unwrap().is_empty());
        test.drop_database().await;
    }
}
