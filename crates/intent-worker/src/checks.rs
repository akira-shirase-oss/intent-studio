//! The check job (`intent-worker check`): the static half of the D-022 suite,
//! run inside the same one-process confinement as every other untrusted-bytes
//! job. Input is a bundle's canonical framing; output is a [`CheckReport`] as
//! JSON — checks this job cannot run are reported `pending` with the reason,
//! never passed.
//!
//! The check is a *contract*, not a blocklist (I2-1): the baseline builder
//! emits a fixed prologue with a meta CSP, a single fixed stylesheet file, and
//! pages through a known element set. So a bundle passes only if every file
//! ends in an allowed suffix, every `.css` is the fixed stylesheet byte for
//! byte, every `.html` is valid UTF-8 without a BOM, starts with
//! [`baseline::PROLOGUE`] byte for byte, and parses through html5ever with
//! only the elements, attributes and value shapes the builder emits. Nothing
//! is enumerated — anything outside the contract fails.

use html5ever::tendril::TendrilSink as _;
use intent_domain::{
    baseline::{PROLOGUE, STYLE},
    bundle::Bundle,
    checks::{CheckKind, CheckReport, CheckResult, CheckStatus, SUITE},
};
use intent_render::{Processed, WorkerRefusal};
use markup5ever_rcdom::{Handle, NodeData, RcDom};

/// File extensions the builder can ever produce beyond the checked text
/// kinds — raster images and webfonts the asset pipeline already vetted. No
/// `svg`, `xhtml`, `xml`, `js`, no extensionless files: each of those is
/// markup or script an allowlist scanner could miss. Case-sensitive on
/// purpose: `INDEX.HTML` is not a builder path.
const BINARY: [&str; 7] = ["png", "jpg", "jpeg", "webp", "avif", "woff", "woff2"];

/// Every element the builder emits — the allowlist, not a blocklist. No
/// `script`, `iframe`, `object`, `embed`, `base`, `svg`, `math`, `style`,
/// `noscript`, `template`, `form`-adjacent tags beyond the listed.
fn element_attrs(el: &str) -> Option<&'static [&'static str]> {
    Some(match el {
        "a" => &["class", "href"],
        "blockquote" | "body" | "head" | "li" | "title" => &[],
        "option" => &["value"],
        "button" => &["type"],
        "div" | "em" | "h1" | "h2" | "h3" | "h4" | "p" | "span" | "strong" | "ul" => &["class"],
        "footer" | "section" => &["class", "id"],
        "form" => &["action", "class", "method"],
        "html" => &["lang"],
        "img" => &["alt", "class", "src"],
        "input" => &[
            "autocomplete",
            "checked",
            "name",
            "required",
            "type",
            "value",
        ],
        "label" => &["class", "for"],
        "link" => &["href", "rel"],
        "meta" => &["charset", "content", "http-equiv", "name"],
        "select" => &["name"],
        "textarea" => &["name", "required"],
        _ => return None,
    })
}

/// A bundle-relative reference — the only thing `src`, `link href` and
/// `form action` may name. `/x`, never `//x`, never a scheme.
fn bundle_path(raw: &str) -> bool {
    let v = normalized(raw);
    v.starts_with('/') && !v.starts_with("//")
}

/// A navigational target — D-014's structural shapes: on-page anchor, site
/// page, or an `https`/`mailto`/`tel` destination (the brief's domain allowlist
/// was the validator's job; the emitted bytes carry only these forms).
fn nav_ok(raw: &str) -> bool {
    let v = normalized(raw);
    v.starts_with('#')
        || (v.starts_with('/') && !v.starts_with("//"))
        || v.starts_with("https://")
        || v.starts_with("mailto:")
        || v.starts_with("tel:")
}

/// Entities are already decoded by the parser; what remains is what a browser
/// would navigate — strip ASCII whitespace and controls (`java\tscript:`),
/// fold `\` to `/` (`\host`, `\\host`), lowercase for the scheme check.
fn normalized(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_ascii_whitespace() && !c.is_control())
        .map(|c| {
            if c == '\\' {
                '/'
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect()
}

/// A value-shape rule per (element, attribute); absent → the attribute exists
/// only to carry inert escaped text.
fn value_ok(el: &str, attr: &str, value: &str) -> bool {
    match (el, attr) {
        ("a", "href") => nav_ok(value),
        ("link", "href") | ("img", "src") | ("form", "action") => bundle_path(value),
        ("link", "rel") => value == "stylesheet",
        ("form", "method") => value == "post",
        ("button", "type") => matches!(value, "submit" | "button"),
        ("html", "lang") => value == "en",
        ("meta", "charset") => value == "utf-8",
        ("meta", "http-equiv") => value == "Content-Security-Policy",
        ("meta", "name") => matches!(value, "viewport" | "description"),
        ("input", "type") => matches!(
            value,
            "checkbox"
                | "radio"
                | "hidden"
                | "text"
                | "tel"
                | "email"
                | "search"
                | "url"
                | "number"
                | "password"
                | "date"
                | "time"
                | "submit"
        ),
        ("input" | "textarea", "required") => value.is_empty() || value == "required",
        _ => true,
    }
}

/// What the contract walk found — the first SVG-family violation and the
/// first other, kept apart so each check reports its own finding.
struct Findings {
    svg: Option<String>,
    external: Option<String>,
}

impl Findings {
    fn svg(&mut self, detail: String) {
        self.svg.get_or_insert(detail);
    }
    fn external(&mut self, detail: String) {
        self.external.get_or_insert(detail);
    }
}

/// Walks the parsed DOM of one `.html` file against the contract.
fn walk(node: &Handle, path: &str, findings: &mut Findings) {
    if let NodeData::Element { name, attrs, .. } = &node.data {
        let el = name.local.as_ref();
        if el.contains("svg") || el == "math" {
            findings.svg(format!("<{el}> in {path}"));
        } else {
            match element_attrs(el) {
                None => findings.external(format!("<{el}> in {path}")),
                Some(allowed) => {
                    for attr in attrs.borrow().iter() {
                        let (an, av) = (attr.name.local.as_ref(), attr.value.as_ref());
                        if !allowed.contains(&an) {
                            findings.external(format!("<{el} {an}> in {path}"));
                        } else if !value_ok(el, an, av) {
                            findings.external(format!("<{el} {an}=\"…\"> in {path}"));
                        }
                    }
                }
            }
        }
    }
    for child in node.children.borrow().iter() {
        walk(child, path, findings);
    }
}

/// One `.html` file under the contract: UTF-8, no BOM, the fixed prologue,
/// then the allowlist walk. The first violation of each family is reported.
fn check_html(path: &str, bytes: &[u8], findings: &mut Findings) {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF])
        || bytes.starts_with(&[0xFF, 0xFE])
        || bytes.starts_with(&[0xFE, 0xFF])
    {
        findings.external(format!("byte-order mark in {path}"));
        return;
    }
    let Ok(html) = std::str::from_utf8(bytes) else {
        findings.external(format!("invalid utf-8 in {path}"));
        return;
    };
    if !html.starts_with(PROLOGUE) {
        findings.external(format!("missing prologue in {path}"));
        return;
    }
    let opts = html5ever::ParseOpts::default();
    let Ok(dom) = html5ever::parse_document(RcDom::default(), opts)
        .from_utf8()
        .read_from(&mut html.as_bytes())
    else {
        findings.external(format!("unparseable in {path}"));
        return;
    };
    walk(&dom.document, path, findings);
}

/// Runs the contract over the bundle; first violation per check wins.
fn contract(bundle: &Bundle) -> Findings {
    let mut findings = Findings {
        svg: None,
        external: None,
    };
    for (path, bytes) in bundle.files() {
        let ext = path.rsplit_once('.').map(|(_, e)| e);
        match ext {
            Some("html") => check_html(path, bytes, &mut findings),
            Some("css") => {
                if bytes != STYLE.as_bytes() {
                    findings.external(format!("stylesheet changed in {path}"));
                }
            }
            Some(e) if BINARY.contains(&e) => {}
            _ => findings.external(format!("file kind {path}")),
        }
    }
    findings
}

fn result(check: CheckKind, detail: Option<String>) -> CheckResult {
    match detail {
        Some(detail) => CheckResult {
            check,
            status: CheckStatus::Fail,
            detail: Some(detail),
        },
        None => CheckResult {
            check,
            status: CheckStatus::Pass,
            detail: None,
        },
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
    let findings = contract(&bundle);
    let mut results = vec![
        result(CheckKind::NoInlineSvg, findings.svg),
        result(CheckKind::NoExternalResources, findings.external),
    ];
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A page in the builder's emitted shape — the prologue is fixed, so any
    /// violation must come from `body` (or from the file's own suffix).
    fn page(body: &str) -> Vec<u8> {
        format!(
            "{PROLOGUE}<title>x</title><meta name=\"description\" content=\"x\">\
             <link rel=\"stylesheet\" href=\"/style.css\"></head><body>{body}</body></html>"
        )
        .into_bytes()
    }

    fn report_for(path: &str, bytes: Vec<u8>) -> CheckReport {
        let mut bundle = Bundle::new();
        bundle.put("style.css", STYLE.as_bytes().to_vec()).unwrap();
        bundle.put(path, bytes).unwrap();
        let out = check(&bundle.encode()).unwrap();
        serde_json::from_slice(&out.output).unwrap()
    }

    fn fails(report: &CheckReport, check: CheckKind) -> bool {
        report
            .results
            .iter()
            .any(|r| r.check == check && r.status == CheckStatus::Fail)
    }

    #[test]
    fn a_builder_page_passes_and_browsers_stay_pending() {
        let report = report_for(
            "index.html",
            page(
                "<section class=\"hero\"><h1>x</h1><a class=\"cta\" href=\"#quote\">y</a>\
                  </section><a href=\"/about\">a</a><a href=\"https://ok.example\">b</a>\
                  <form method=\"post\" action=\"/v1/leads\"></form>",
            ),
        );
        assert!(!fails(&report, CheckKind::NoInlineSvg));
        assert!(!fails(&report, CheckKind::NoExternalResources));
        assert!(
            report
                .results
                .iter()
                .filter(|r| r.check.backend() == intent_domain::checks::CheckBackend::Browser)
                .all(|r| r.status == CheckStatus::Pending),
            "no browser result may claim pass from this worker"
        );
    }

    /// Every bypass the ninth review named — each passed the byte-pattern
    /// blocklist; each must fail the contract. One case per row.
    #[expect(
        clippy::too_many_lines,
        reason = "a table of named attack fixtures reads as one test"
    )]
    #[test]
    fn every_bypass_fixture_fails_the_contract() {
        let utf16 = {
            let mut b = vec![0xFF, 0xFE];
            for u in page("<svg></svg>") {
                b.push(u);
                b.push(0);
            }
            b
        };
        let invalid_utf8 = {
            let mut b = page("<svg></svg>");
            b.insert(200, 0xFF);
            b
        };
        let mut utf8_bom = vec![0xEF, 0xBB, 0xBF];
        utf8_bom.extend_from_slice(&page("<svg></svg>"));

        let cases: Vec<(&str, Vec<u8>, CheckKind)> = vec![
            // srcset was never scanned at all.
            (
                "x.html",
                page(r#"<img srcset="https://e/x.png 1x">"#),
                CheckKind::NoExternalResources,
            ),
            // `src=` with spaces/newline around `=` and the quote.
            (
                "x.html",
                page("<img src =\n\"https://e/x.png\">"),
                CheckKind::NoExternalResources,
            ),
            (
                "x.html",
                page("<img src = \"https://e/x.png\">"),
                CheckKind::NoExternalResources,
            ),
            // Entity-encoded schemes — decoded by the parser, then refused.
            (
                "x.html",
                page(r#"<img src="&#104;ttps://e/x.png">"#),
                CheckKind::NoExternalResources,
            ),
            (
                "x.html",
                page(r#"<a href="javas&#99;ript:alert(1)">x</a>"#),
                CheckKind::NoExternalResources,
            ),
            (
                "x.html",
                page(r#"<a href="javas&#99;ript&colon;alert(1)">x</a>"#),
                CheckKind::NoExternalResources,
            ),
            // Backslash-host, slash-host and `http:` schemes.
            (
                "x.html",
                page(r#"<a href="\\evil.example/x">x</a>"#),
                CheckKind::NoExternalResources,
            ),
            (
                "x.html",
                page(r#"<a href="/\evil.example/x">x</a>"#),
                CheckKind::NoExternalResources,
            ),
            (
                "x.html",
                page(r#"<a href="//evil.example/x">x</a>"#),
                CheckKind::NoExternalResources,
            ),
            (
                "x.html",
                page(r#"<a href="http:evil.example">x</a>"#),
                CheckKind::NoExternalResources,
            ),
            (
                "x.html",
                page(r#"<a href="http://evil.example">x</a>"#),
                CheckKind::NoExternalResources,
            ),
            // Elements outside the contract — loads the old patterns missed.
            (
                "x.html",
                page(r#"<object data="https://e/x"></object>"#),
                CheckKind::NoExternalResources,
            ),
            (
                "x.html",
                page(r#"<video poster="https://e/x.jpg"></video>"#),
                CheckKind::NoExternalResources,
            ),
            (
                "x.html",
                page(r#"<base href="https://e/">"#),
                CheckKind::NoExternalResources,
            ),
            (
                "x.html",
                page(r#"<meta http-equiv="refresh" content="0;url=https://e">"#),
                CheckKind::NoExternalResources,
            ),
            (
                "x.html",
                page(r#"<iframe srcdoc="&lt;svg onload=alert(1)&gt;"></iframe>"#),
                CheckKind::NoExternalResources,
            ),
            (
                "x.html",
                page(r#"<script src="https://e/x.js"></script>"#),
                CheckKind::NoExternalResources,
            ),
            (
                "x.html",
                page(r#"<embed src="https://e/x.swf">"#),
                CheckKind::NoExternalResources,
            ),
            // data: in src=.
            (
                "x.html",
                page(r#"<img src="data:image/svg+xml;base64,PHN2Zz4=">"#),
                CheckKind::NoExternalResources,
            ),
            // Event handlers and javascript: URLs — never checked before.
            (
                "x.html",
                page(r##"<a onclick="steal()" href="#">x</a>"##),
                CheckKind::NoExternalResources,
            ),
            (
                "x.html",
                page(r#"<p onmouseover="steal()">x</p>"#),
                CheckKind::NoExternalResources,
            ),
            (
                "x.html",
                page(r#"<a href="javascript:alert(1)">x</a>"#),
                CheckKind::NoExternalResources,
            ),
            (
                "x.html",
                page(r#"<form action="javascript:alert(1)" method="post"></form>"#),
                CheckKind::NoExternalResources,
            ),
            // File kinds the suffix list never scanned.
            (
                "x.svg",
                page("<svg onload=alert(1)></svg>"),
                CheckKind::NoExternalResources,
            ),
            (
                "x.xhtml",
                page("<s:svg onload=alert(1)></s:svg>"),
                CheckKind::NoExternalResources,
            ),
            (
                "x.xml",
                page("<h:script>alert(1)</h:script>"),
                CheckKind::NoExternalResources,
            ),
            ("x.js", b"alert(1)".to_vec(), CheckKind::NoExternalResources),
            ("x", page("<svg></svg>"), CheckKind::NoExternalResources),
            // Prefixed elements inside a scanned suffix.
            (
                "x.html",
                page("<s:svg onload=alert(1)></s:svg>"),
                CheckKind::NoInlineSvg,
            ),
            (
                "x.html",
                page("<h:script>alert(1)</h:script>"),
                CheckKind::NoExternalResources,
            ),
            // Encodings that defeated byte-run checks.
            ("x.html", utf16, CheckKind::NoExternalResources),
            ("x.html", utf8_bom, CheckKind::NoExternalResources),
            ("x.html", invalid_utf8, CheckKind::NoExternalResources),
            // CSS the byte scanner never caught: image-set, \75 rl(, @\69mport —
            // now any .css that is not the fixed stylesheet fails.
            (
                "x.css",
                b"a{background:image-set('https://e/1x.png')}".to_vec(),
                CheckKind::NoExternalResources,
            ),
            (
                "x.css",
                b"a{background:\\75 rl('https://e/x.png')}".to_vec(),
                CheckKind::NoExternalResources,
            ),
            (
                "x.css",
                b"@\\69mport 'https://e/x.css';".to_vec(),
                CheckKind::NoExternalResources,
            ),
            // A changed prologue or a missing one.
            (
                "x.html",
                page("<svg></svg>")[..200].to_vec(),
                CheckKind::NoExternalResources,
            ),
        ];
        for (index, (path, bytes, kind)) in cases.into_iter().enumerate() {
            let report = report_for(path, bytes);
            assert!(
                fails(&report, kind),
                "case {index} ({path}) must fail {kind:?}, got {report:?}"
            );
        }
    }

    /// `no_inline_svg` keeps its own name: an inline `<svg>` (prefixed or not)
    /// inside an otherwise-clean page fails the svg check itself.
    #[test]
    fn inline_svg_fails_its_own_check() {
        let report = report_for("index.html", page("<svg onload=alert(1)></svg>"));
        assert!(fails(&report, CheckKind::NoInlineSvg));
        assert!(!fails(&report, CheckKind::NoExternalResources));
    }
}
