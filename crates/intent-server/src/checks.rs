//! The check-suite runner (I2): a candidate's bundle through the confined check
//! worker, its report committed as `check_records` rows bound to the exact
//! bundle hash (D-010, D-022). No transaction spans the worker call — the
//! candidate is read, the job runs, the records commit after.
//!
//! What a caller learns is the [`SuiteOutcome`]: every failure is a named code
//! from the D-021 taxonomy, never a bare error. `Pending` means the suite is
//! incomplete (the browser backend is not built) — it is not a pass and can
//! never promote.

use intent_db::{CandidateRow, Db, DbError};
use intent_domain::{
    checks::{
        CheckBackend, CheckKind, CheckReport, CheckResult, CheckStatus, CheckVerdict, Engine, SUITE,
    },
    content::ContentHash,
};
use intent_render::{AssetKind, Driver, WorkerRefusal};
use intent_store::ContentStore;
use uuid::Uuid;

use crate::config::Config;

/// Why a suite run produced no outcome a caller may rely on. The `code()`s are
/// D-021's taxonomy.
#[derive(Debug, Clone, thiserror::Error)]
pub enum CheckError {
    /// The candidate is not this tenant's (or does not exist).
    #[error("candidate_unknown")]
    CandidateUnknown,
    /// The store does not hold the bundle the candidate names.
    #[error("candidate_bundle_missing")]
    BundleMissing,
    /// The check worker died, timed out or answered outside the protocol.
    #[error("render_crashed")]
    RenderCrashed,
    /// The stored bytes are not a canonical bundle — a deterministic defect,
    /// never a retryable crash (D-021, I2-6).
    #[error("bundle_malformed")]
    BundleMalformed,
    /// The worker could not run the job (busy, unavailable, oversized).
    #[error("check_unavailable")]
    CheckUnavailable,
    /// The worker's report does not parse or does not name the suite.
    #[error("report_malformed")]
    ReportMalformed,
    /// The record write was refused — the pair `(candidate, hash)` is the
    /// binding; a wrong hash cannot land (the `check-any-hash` seam reaches
    /// this same refusal, proving the schema holds when the app layer is off).
    #[error("check_binding_refused")]
    CheckBinding,
    /// The database faulted.
    #[error("db {0}")]
    Db(String),
}

/// How the run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuiteOutcome {
    /// Every check ran and passed. Not reachable until the browser backend
    /// lands — a `Pending` in the report is never laundered.
    Pass,
    /// A check failed; `checks_failed` in the D-021 taxonomy.
    ChecksFailed,
    /// Nothing failed but at least one check could not run.
    Pending,
}

const AUDIT: intent_db::AuditEntry<'static> = intent_db::AuditEntry {
    action: "checks.run",
    actor: "system",
    address: None,
    request_id: None,
    detail: "{}",
};

/// Runs `suite` over the candidate's bundle and records the evidence.
///
/// `requested` is the hash a future caller might try to record under: it is
/// ignored — the record binds the candidate's own hash — *except* under the
/// `check-any-hash` mutation seam, where it reaches the insert to prove the
/// composite foreign key refuses the pair.
#[cfg_attr(
    not(feature = "mutation-seams"),
    expect(unused_variables, reason = "the seams are feature-gated")
)]
pub async fn run(
    config: &Config,
    db: &Db,
    store: &ContentStore,
    driver: &Driver,
    tenant: intent_domain::ids::TenantId,
    candidate_id: Uuid,
    requested: Option<ContentHash>,
) -> Result<SuiteOutcome, CheckError> {
    let candidate = db
        .candidate(tenant, candidate_id)
        .await
        .map_err(|e| db_error(&e))?
        .ok_or(CheckError::CandidateUnknown)?;
    let blob = store
        .get(candidate.bundle_sha256)
        .await
        .map_err(|_| CheckError::BundleMissing)?
        .ok_or(CheckError::BundleMissing)?;
    let answer = driver
        .run(AssetKind::Check, &blob)
        .await
        .map_err(worker_error)?;
    let mut report: CheckReport =
        serde_json::from_slice(&answer.output).map_err(|_| CheckError::ReportMalformed)?;
    if report.suite != SUITE {
        return Err(CheckError::ReportMalformed);
    }
    // The report is untrusted worker output: it must answer for every check the
    // suite version names — a missing check is a laundered failure, not a skip.
    // `allow-inline-svg`: the seam drops the check's result — a bundle that
    // should fail is recorded as merely incomplete, which is exactly what the
    // drill refuses to let stand.
    if crate::mutated!(config, crate::config::Mutation::AllowInlineSvg) {
        report.results.retain(|r| r.check != CheckKind::NoInlineSvg);
    }
    // Until a browser engine produces them, every browser check must be
    // Pending: the static worker may not claim a browser pass, and a verdict
    // exists only over complete, well-formed evidence (I2-3, I2-4).
    if report
        .results
        .iter()
        .any(|r| r.check.backend() == CheckBackend::Browser && r.status != CheckStatus::Pending)
    {
        return Err(CheckError::ReportMalformed);
    }
    let verdict = CheckVerdict::of(&report).ok_or(CheckError::ReportMalformed)?;
    // The record binds the candidate's own hash. The seam substitutes a
    // caller-supplied one — the composite FK turns it into a refused write.
    let hash = if crate::mutated!(config, crate::config::Mutation::CheckAnyHash) {
        requested.unwrap_or(candidate.bundle_sha256)
    } else {
        candidate.bundle_sha256
    };
    write_record(db, &candidate, hash, &report.results).await?;
    Ok(match verdict {
        CheckVerdict::Pass => SuiteOutcome::Pass,
        CheckVerdict::Fail => SuiteOutcome::ChecksFailed,
        CheckVerdict::Pending => SuiteOutcome::Pending,
    })
}

/// One `static` cell's row; browser cells get rows when their backend exists.
/// The verdict is the DB layer's own derivation from the results — never
/// caller-supplied (I2-4).
async fn write_record(
    db: &Db,
    candidate: &CandidateRow,
    hash: ContentHash,
    results: &[CheckResult],
) -> Result<(), CheckError> {
    let results = serde_json::to_value(results).map_err(|_| CheckError::ReportMalformed)?;
    db.insert_check_record(
        &intent_db::NewCheckRecord {
            id: Uuid::now_v7(),
            candidate_id: candidate.id,
            bundle_sha256: hash,
            suite: SUITE,
            engine: Engine::Static.as_str(),
            viewport: "static",
            results: &results,
        },
        &AUDIT,
    )
    .await
    .map(|_| ())
    .map_err(|error| {
        if error.sqlstate().as_deref() == Some("23503") {
            CheckError::CheckBinding
        } else {
            db_error(&error)
        }
    })
}

fn worker_error(refusal: WorkerRefusal) -> CheckError {
    match refusal {
        WorkerRefusal::WorkerCrashed | WorkerRefusal::WorkerTimeout => CheckError::RenderCrashed,
        WorkerRefusal::Malformed => CheckError::BundleMalformed,
        _ => CheckError::CheckUnavailable,
    }
}

fn db_error(error: &DbError) -> CheckError {
    CheckError::Db(error.summary())
}

/// The confinement profile a browser job runs under (docs/BROWSER-CONFINEMENT.md):
/// every bound and wall on, every hole off — unless `INTENT_MUTATION` armed one,
/// which is exactly what the confinement proofs exercise. Browser binaries are
/// still absent, so the six D-022 checks stay `Pending`; this builder exists so
/// the profile — and its seams — are real code the proofs run, not prose.
#[allow(
    unused_mut,
    clippy::needless_update,
    reason = "in a shipped build no seam exists, so the job stays as constructed"
)]
pub fn browser_job(
    config: &Config,
    program: &str,
    args: &[String],
    bundle_dir: &std::path::Path,
) -> intent_render::browser::BrowserJob {
    let mut job = intent_render::browser::BrowserJob {
        program: program.to_owned(),
        args: args.to_vec(),
        bundle_dir: bundle_dir.to_path_buf(),
        ..Default::default()
    };
    #[cfg(feature = "mutation-seams")]
    {
        use crate::config::Mutation;
        job.allow_net = crate::mutated!(config, Mutation::BrowserAllowNet);
        job.host_mounts = crate::mutated!(config, Mutation::BrowserHostMounts);
        job.unbounded_procs = crate::mutated!(config, Mutation::BrowserUnboundedProcs);
        job.unbounded_mem = crate::mutated!(config, Mutation::BrowserUnboundedMem);
        job.no_deadline = crate::mutated!(config, Mutation::BrowserNoDeadline);
        job.unbounded_output = crate::mutated!(config, Mutation::BrowserUnboundedOutput);
    }
    let _ = config;
    job
}
