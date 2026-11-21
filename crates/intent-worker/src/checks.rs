//! The check job (`intent-worker check`): the static half of the D-022 suite,
//! run inside the same one-process confinement as every other untrusted-bytes
//! job. Input is a bundle's canonical framing; output is a [`CheckReport`] as
//! JSON — checks this job cannot run are reported `pending` with the reason,
//! never passed.

use intent_domain::{
    bundle::Bundle,
    checks::{CheckKind, CheckReport, CheckResult, CheckStatus, SUITE},
};
use intent_render::{Processed, WorkerRefusal};

/// The file suffixes whose bytes are served as markup or styles — the only
/// places an SVG or an external load can hide. A bundle with none of these is
/// still checked: its paths are the claim.
const TEXT_SUFFIXES: [&str; 3] = [".html", ".htm", ".css"];

/// Every byte of `bytes` lowercased ASCII — tags and attributes are ASCII.
fn lower(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().map(u8::to_ascii_lowercase).collect()
}

/// `<svg` anywhere in a served file — element open only; `xlink` names and
/// namespaces still contain the byte run that matters.
fn no_inline_svg(bundle: &Bundle) -> CheckResult {
    for (path, bytes) in bundle.files() {
        if lower(bytes).windows(4).any(|w| w == b"<svg") {
            return CheckResult {
                check: CheckKind::NoInlineSvg,
                status: CheckStatus::Fail,
                detail: Some(format!("inline svg in {path}")),
            };
        }
    }
    CheckResult {
        check: CheckKind::NoInlineSvg,
        status: CheckStatus::Pass,
        detail: None,
    }
}

/// What follows `=` or `(` before a URL: whitespace, a quote, either.
fn value_after(bytes: &[u8], mut at: usize) -> &str {
    while at < bytes.len() && matches!(bytes[at], b' ' | b'\t' | b'"' | b'\'') {
        at += 1;
    }
    std::str::from_utf8(&bytes[at..]).unwrap_or("")
}

fn loads_externally(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://") || value.starts_with("//")
}

/// A render must never reach outside the bundle. Loads, not links: `src`,
/// `srcset`, `url()`, `@import`, `<link>`/`<script>`/`<img>` references. An
/// `<a href>` to an allowlisted address is a user's click, not a load.
fn no_external_resources(bundle: &Bundle) -> CheckResult {
    for (path, bytes) in bundle.files() {
        if !TEXT_SUFFIXES.iter().any(|s| path.ends_with(s)) {
            continue;
        }
        let text = lower(bytes);
        let mut at = 0;
        for attr in [b"src=" as &[u8], b"srcset="] {
            while let Some(found) = text[at..]
                .windows(attr.len())
                .position(|w| w == attr)
                .map(|p| at + p + attr.len())
            {
                if loads_externally(value_after(&text, found)) {
                    return fail(
                        CheckKind::NoExternalResources,
                        format!("external {attr:?} in {path}"),
                    );
                }
                at = found;
            }
        }
        let mut at = 0;
        while let Some(found) = text[at..]
            .windows(4)
            .position(|w| w == b"url(")
            .map(|p| at + p + 4)
        {
            let value = value_after(&text, found);
            if loads_externally(value) || value.starts_with("data:") {
                return fail(
                    CheckKind::NoExternalResources,
                    format!("external or embedded url() in {path}"),
                );
            }
            at = found;
        }
        for needle in [b"@import" as &[u8], b"<script"] {
            if text.windows(needle.len()).any(|w| w == needle) {
                return fail(
                    CheckKind::NoExternalResources,
                    format!("{} in {path}", String::from_utf8_lossy(needle)),
                );
            }
        }
        if let Some(pos) = text.windows(5).position(|w| w == b"<link") {
            let rest = &text[pos..];
            let end = rest.iter().position(|b| *b == b'>').unwrap_or(rest.len());
            let tag = &rest[..end.min(rest.len())];
            if tag
                .windows(5)
                .position(|w| w == b"href=")
                .is_some_and(|h| loads_externally(value_after(tag, h + 5)))
            {
                return fail(
                    CheckKind::NoExternalResources,
                    format!("external <link> in {path}"),
                );
            }
        }
    }
    CheckResult {
        check: CheckKind::NoExternalResources,
        status: CheckStatus::Pass,
        detail: None,
    }
}

fn fail(check: CheckKind, detail: String) -> CheckResult {
    CheckResult {
        check,
        status: CheckStatus::Fail,
        detail: Some(detail),
    }
}

/// A browser check this build cannot run — honest, never a pass.
fn pending(check: CheckKind) -> CheckResult {
    CheckResult {
        check,
        status: CheckStatus::Pending,
        detail: Some("browser backend not built".into()),
    }
}

/// Runs the static suite over a bundle's canonical framing.
pub fn check(input: &[u8]) -> Result<Processed, WorkerRefusal> {
    let bundle = Bundle::decode(input).map_err(|_| WorkerRefusal::Malformed)?;
    let mut results = vec![no_inline_svg(&bundle), no_external_resources(&bundle)];
    for kind in CheckKind::ALL {
        if kind.backend() == intent_domain::checks::CheckBackend::Browser {
            results.push(pending(kind));
        }
    }
    let report = CheckReport {
        suite: SUITE.to_owned(),
        results,
    };
    let output = serde_json::to_vec(&report).map_err(|_| WorkerRefusal::Malformed)?;
    Ok(Processed {
        media_type: "application/json".into(),
        width: None,
        height: None,
        removed_parts: 0,
        dropped_parts: Vec::new(),
        output,
    })
}
