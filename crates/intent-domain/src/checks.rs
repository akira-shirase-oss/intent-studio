//! The D-022 check suite: what a bundle must survive before an approval may bind
//! its hash. The seven kinds are the interface; each names the backend that can
//! run it (`static` — bytes only, no browser; `browser` — a real engine over a
//! loopback serve). A check that cannot run is [`CheckStatus::Pending`], never a
//! pass: the verdict is only as strong as the checks that actually ran.

use serde::{Deserialize, Serialize};

/// The suite's version, bumped when a check's meaning changes.
pub const SUITE: &str = "d-022@0";

/// The viewports the suite runs each browser check at (D-022).
pub const VIEWPORTS: [Viewport; 3] = [
    Viewport {
        width: 360,
        height: 800,
    },
    Viewport {
        width: 390,
        height: 844,
    },
    Viewport {
        width: 1280,
        height: 800,
    },
];

/// The engines the suite runs each browser check under (D-022).
pub const ENGINES: [Engine; 2] = [Engine::Chromium, Engine::WebKit];

/// A viewport, CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Viewport {
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
}

impl Viewport {
    /// `360x800` — the record's stable name.
    pub fn as_str(&self) -> String {
        format!("{}x{}", self.width, self.height)
    }
}

/// A browser engine. `Static` is the no-browser backend for bytes-only checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    /// Chromium, via its debug protocol.
    Chromium,
    /// `WebKit`, via its debug protocol.
    WebKit,
    /// No engine: byte-level checks over the bundle itself.
    Static,
}

impl Engine {
    /// The record's stable name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Chromium => "chromium",
            Self::WebKit => "webkit",
            Self::Static => "static",
        }
    }
}

/// The backend that runs a check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckBackend {
    /// Bundle bytes only — the check worker, under the existing confinement.
    Static,
    /// A real browser over a loopback serve — owed until the browser backend
    /// lands (its own confinement: no socket/exec fits the asset worker's).
    Browser,
}

/// What the suite checks. The wire names are the record's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckKind {
    /// No `<svg` element in any served page — the pilot serves raster/`img`
    /// only; inline SVG is an unparsed attack surface.
    NoInlineSvg,
    /// Nothing a render would *load* from outside the bundle: no external
    /// `src`/`srcset`, `url()` or `@import`. Links out are fine — loads are
    /// what break the check worker's no-network claim.
    NoExternalResources,
    /// `document.scrollWidth <= viewport width` at every viewport (D-022).
    NoHorizontalOverflow,
    /// The form posts to the site's intake and shows success only on HTTP 201.
    FormSubmits201,
    /// Required-field failures set `aria-invalid`/`aria-describedby` + a live
    /// region (D-022).
    RequiredFieldsAnnounced,
    /// Zero critical/serious axe-core violations (D-022).
    AxeCoreViolations,
    /// Every interactive target is at least 24×24 CSS px (D-022).
    TapTargets,
    /// Every interactive is Tab-reachable with visible focus and the form is
    /// keyboard-completable (D-022).
    KeyboardCompletion,
}

impl CheckKind {
    /// Every kind, in suite order.
    pub const ALL: [Self; 8] = [
        Self::NoInlineSvg,
        Self::NoExternalResources,
        Self::NoHorizontalOverflow,
        Self::FormSubmits201,
        Self::RequiredFieldsAnnounced,
        Self::AxeCoreViolations,
        Self::TapTargets,
        Self::KeyboardCompletion,
    ];

    /// The record's stable name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoInlineSvg => "no_inline_svg",
            Self::NoExternalResources => "no_external_resources",
            Self::NoHorizontalOverflow => "no_horizontal_overflow",
            Self::FormSubmits201 => "form_submits_201",
            Self::RequiredFieldsAnnounced => "required_fields_announced",
            Self::AxeCoreViolations => "axe_core_violations",
            Self::TapTargets => "tap_targets",
            Self::KeyboardCompletion => "keyboard_completion",
        }
    }

    /// Which backend runs this check.
    pub const fn backend(self) -> CheckBackend {
        match self {
            Self::NoInlineSvg | Self::NoExternalResources => CheckBackend::Static,
            Self::NoHorizontalOverflow
            | Self::FormSubmits201
            | Self::RequiredFieldsAnnounced
            | Self::AxeCoreViolations
            | Self::TapTargets
            | Self::KeyboardCompletion => CheckBackend::Browser,
        }
    }
}

/// How one check ended. `Pending` is an honest "not run" — never a pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    /// Ran and held.
    Pass,
    /// Ran and broke — `detail` names what.
    Fail,
    /// Could not run in this build — `detail` names why.
    Pending,
}

/// `detail`'s bound — a result explains itself in a sentence, never content.
pub const MAX_DETAIL_BYTES: usize = 512;

/// One check's outcome on a bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckResult {
    /// Which check.
    pub check: CheckKind,
    /// How it ended.
    pub status: CheckStatus,
    /// What failed or why it is pending, bounded. Never content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// The check worker's report: suite version plus one result per kind — the kinds
/// it ran honestly and the kinds it cannot (pending with the named reason).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckReport {
    /// [`SUITE`].
    pub suite: String,
    /// One entry per [`CheckKind`], any order.
    pub results: Vec<CheckResult>,
}

/// A suite run's verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckVerdict {
    /// Every check ran and passed.
    Pass,
    /// At least one check failed.
    Fail,
    /// Nothing failed but at least one check did not run. Never promotable.
    Pending,
}

impl CheckVerdict {
    /// The record's stable name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Pending => "pending",
        }
    }

    /// The verdict over a complete report — `None` when the evidence is
    /// incomplete: a missing kind, a duplicate, an extra, or an oversized
    /// `detail`. An empty result set has no path to [`Self::Pass`]: pass is
    /// derived from evidence, never assumed (I2-4).
    pub fn of(report: &CheckReport) -> Option<Self> {
        let results = &report.results;
        let complete = CheckKind::ALL
            .iter()
            .all(|kind| results.iter().filter(|r| r.check == *kind).count() == 1)
            && results.len() == CheckKind::ALL.len()
            && results
                .iter()
                .all(|r| r.detail.as_deref().map_or(0, str::len) <= MAX_DETAIL_BYTES);
        if !complete {
            return None;
        }
        Some(Self::of_results(results))
    }

    /// The verdict over results already proved complete — what the DB layer
    /// derives for itself so no caller can supply a verdict the evidence
    /// doesn't support.
    #[must_use]
    pub fn of_results(results: &[CheckResult]) -> Self {
        if results.iter().any(|r| r.status == CheckStatus::Fail) {
            Self::Fail
        } else if results.iter().any(|r| r.status == CheckStatus::Pending) {
            Self::Pending
        } else {
            Self::Pass
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(results: Vec<CheckResult>) -> CheckReport {
        CheckReport {
            suite: SUITE.into(),
            results,
        }
    }

    #[test]
    fn the_verdict_never_launders_a_pending() {
        let pass = |check| CheckResult {
            check,
            status: CheckStatus::Pass,
            detail: None,
        };
        assert_eq!(
            CheckVerdict::of(&report(CheckKind::ALL.map(pass).to_vec())),
            Some(CheckVerdict::Pass)
        );
        let mut results = CheckKind::ALL.map(pass).to_vec();
        results[3].status = CheckStatus::Pending;
        results[3].detail = Some("browser backend not built".into());
        assert_eq!(
            CheckVerdict::of(&report(results.clone())),
            Some(CheckVerdict::Pending)
        );
        results[3].status = CheckStatus::Fail;
        assert_eq!(CheckVerdict::of(&report(results)), Some(CheckVerdict::Fail));
    }

    #[test]
    fn no_evidence_no_pass() {
        // An empty or short report has no path to Pass (I2-4).
        assert_eq!(CheckVerdict::of(&report(vec![])), None);
        let pass = |check| CheckResult {
            check,
            status: CheckStatus::Pass,
            detail: None,
        };
        // Missing one kind, a duplicate, an extra, an oversized detail.
        assert_eq!(
            CheckVerdict::of(&report(CheckKind::ALL.map(pass)[..7].to_vec())),
            None
        );
        let mut dup = CheckKind::ALL.map(pass).to_vec();
        dup.push(pass(CheckKind::NoInlineSvg));
        assert_eq!(CheckVerdict::of(&report(dup)), None);
        let mut big = CheckKind::ALL.map(pass).to_vec();
        big[0].detail = Some("x".repeat(MAX_DETAIL_BYTES + 1));
        assert_eq!(CheckVerdict::of(&report(big)), None);
    }

    #[test]
    fn check_result_refuses_unknown_fields() {
        assert!(
            serde_json::from_str::<CheckResult>(
                r#"{"check":"no_inline_svg","status":"pass","extra":1}"#
            )
            .is_err()
        );
    }
}
