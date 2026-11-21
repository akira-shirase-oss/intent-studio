//! The check job (`intent-worker check`): the static half of the D-022 suite,
//! run inside the same one-process confinement as every other untrusted-bytes
//! job. Input is a bundle's canonical framing; output is a [`CheckReport`] as
//! JSON — checks this job cannot run are reported `pending` with the reason,
//! never passed.
//!
//! The check is a *contract*, not a blocklist (I2-1): the baseline builder
//! emits a fixed prologue with a meta CSP, a single fixed stylesheet file, and
//! pages through a known element set. So a bundle passes only if every file is
//! a kind the builder can produce at all, every `.css` is the fixed stylesheet
//! byte for byte, and every `.html` is valid UTF-8 without a BOM, starts with
//! [`baseline::PROLOGUE`] byte for byte, and survives both parses below.
//!
//! Two parses, deliberately (N1 of the tenth re-review): html5ever's *tree
//! builder* still drops every start tag inside `<select>` except `option`,
//! while Chrome 134+ keeps them — so walking the tree never sees the markup a
//! current browser runs. The check therefore scores every start-tag **token**
//! the tokenizer yields, tracking open elements itself (iteratively — a stack
//! of names, never recursion, and capped before any tree is built). Only once
//! the token pass is clean does a tree build run, and its `dom.errors` must
//! be empty: a document the tree builder had to correct is not builder output.

use std::cell::RefCell;

use html5ever::tendril::{StrTendril, TendrilSink as _};
use html5ever::tokenizer::{
    BufferQueue, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
};
use intent_domain::{
    baseline::{CSP, PROLOGUE, STYLE, VIEWPORT},
    bundle::Bundle,
    checks::{CheckKind, CheckReport, CheckResult, CheckStatus, MAX_DETAIL_BYTES, SUITE},
};
use intent_render::{Processed, WorkerRefusal};
use markup5ever_rcdom::RcDom;

/// Open-element depth the contract tolerates (N3). The builder nests about
/// six deep; forty-eight is headroom, not a target — past it the tree build
/// (the part with quadratic scope scans and a recursive drop) never runs.
const MAX_TAG_DEPTH: usize = 48;

/// Elements HTML never closes — they count nothing toward depth.
const VOID: [&str; 14] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// File extensions that pass uninspected: none. The builder emits text only
/// today; assets join when they are bound by hash (N2).
const BINARY: [&str; 0] = [];

/// Every element the builder emits → the attribute names it may carry. No
/// `script`, `iframe`, `object`, `embed`, `base`, `svg`, `math`, `style`,
/// `noscript`, `template`, `img`, `span`, `em`, `h3`, `h4` — narrower than the
/// vocabulary, because the contract is what the builder *emits*, not what it
/// could. `meta`, `link`, `form`, `select`, `textarea`, `button` and `option`
/// go through the exact-set rules instead; this table covers the rest.
fn element_attrs(el: &str) -> Option<&'static [&'static str]> {
    Some(match el {
        "a" => &["class", "href"],
        "blockquote" | "body" | "h1" | "h2" | "head" | "li" | "strong" | "title" => &[],
        "div" | "label" | "p" | "ul" => &["class"],
        "footer" => &["id"],
        "input" => &["autocomplete", "name", "required", "type"],
        "section" => &["class", "id"],
        _ => return None,
    })
}

/// A value-shape rule per (element, attribute) — the exact strings the
/// builder emits, not a family (N2). Absent → inert escaped text.
fn value_ok(el: &str, attr: &str, value: &str) -> bool {
    match (el, attr) {
        ("a", "href") => nav_ok(value),
        ("a", "class") => matches!(value, "cta" | "contact" | "footer"),
        ("div", "class") => value == "sub",
        ("footer" | "section", "id") => slug(value),
        ("input", "autocomplete") => {
            matches!(value, "name" | "tel" | "email" | "postal-code")
        }
        ("input", "name") => {
            matches!(value, "name" | "phone" | "email" | "area" | "consent")
        }
        ("input" | "textarea", "required") => value.is_empty() || value == "required",
        ("input", "type") => matches!(value, "checkbox" | "email" | "tel"),
        ("label", "class") => value == "consent",
        ("p", "class") => value == "price",
        ("section", "class") => value == "hero",
        ("ul", "class") => matches!(value, "services" | "trust"),
        _ => true,
    }
}

/// A section/footer id: the validator's slug, re-proved on the bytes.
fn slug(value: &str) -> bool {
    (1..=40).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
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

/// Entities are already decoded by the tokenizer; what remains is what a
/// browser would navigate — strip ASCII whitespace and controls
/// (`java\tscript:`), fold `\` to `/` (`\host`, `\\host`), lowercase for the
/// scheme check.
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

/// A detail that can never overflow the report's bound (N4): tag and
/// attribute names are attacker-sized, the column is not.
fn clipped(text: String) -> String {
    if text.len() <= MAX_DETAIL_BYTES {
        return text;
    }
    let mut end = MAX_DETAIL_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

/// What the contract found — the first SVG-family violation and the first
/// other, kept apart so each check reports its own finding.
struct Findings {
    svg: Option<String>,
    external: Option<String>,
}

impl Findings {
    fn svg(&mut self, detail: String) {
        self.svg.get_or_insert_with(|| clipped(detail));
    }
    fn external(&mut self, detail: String) {
        self.external.get_or_insert_with(|| clipped(detail));
    }
}

/// The exact attribute sets for elements the builder emits in exactly one
/// shape (N2). `have` is the token's attributes in order; a set fails on an
/// unknown name, a duplicated name (browsers differ on which wins), or a
/// value that is not the builder's. `"*"` marks the one free slot.
fn exact(have: &[(&str, &str)], want: &[(&str, &str)]) -> bool {
    if have.len() != want.len() {
        return false;
    }
    let mut names: Vec<&str> = have.iter().map(|(n, _)| *n).collect();
    names.sort_unstable();
    names.dedup();
    names.len() == have.len()
        && have.iter().all(|(an, av)| {
            want.iter()
                .any(|(wn, wv)| wn == an && (wv == &"*" || wv == av))
        })
}

/// The four `<meta>` shapes the builder emits — charset, the fixed CSP, the
/// fixed viewport, the page description. Anything else (a refresh, a second
/// CSP, a different viewport) is not a builder tag.
fn meta_ok(have: &[(&str, &str)]) -> bool {
    exact(have, &[("charset", "utf-8")])
        || exact(
            have,
            &[("http-equiv", "Content-Security-Policy"), ("content", CSP)],
        )
        || exact(have, &[("name", "viewport"), ("content", VIEWPORT)])
        || exact(have, &[("name", "description"), ("content", "*")])
}

/// One start tag against the contract. `open` is the elements above it —
/// the tokenizer's view, because the tree builder's cannot be trusted (N1).
fn check_tag(el: &str, have: &[(&str, &str)], st: &mut TokState, path: &str, f: &mut Findings) {
    if el.contains("svg") || el == "math" {
        f.svg(format!("<{el}> in {path}"));
        return;
    }
    if st.open.iter().any(|o| o == "select") && el != "option" {
        f.external(format!("<{el}> inside <select> in {path}"));
        return;
    }
    if el == "option" && !st.open.iter().any(|o| o == "select") {
        f.external(format!("<option> outside <select> in {path}"));
        return;
    }
    let ok = match el {
        "meta" => meta_ok(have),
        "link" => exact(have, &[("rel", "stylesheet"), ("href", "/style.css")]),
        "form" => exact(
            have,
            &[
                ("method", "post"),
                ("action", "/v1/leads"),
                ("class", "quote-form"),
            ],
        ),
        "select" => exact(have, &[("name", "service")]),
        "textarea" => exact(have, &[("name", "message"), ("required", "")]),
        "button" => exact(have, &[("type", "submit")]),
        "option" => have.is_empty() || exact(have, &[("value", "")]),
        "html" => exact(have, &[("lang", "en")]),
        _ => {
            let Some(allowed) = element_attrs(el) else {
                f.external(format!("<{el}> in {path}"));
                return;
            };
            let mut names: Vec<&str> = have.iter().map(|(n, _)| *n).collect();
            names.sort_unstable();
            names.dedup();
            if names.len() != have.len() {
                f.external(format!("duplicated attribute in <{el}> in {path}"));
                return;
            }
            if let Some((an, _)) = have
                .iter()
                .find(|(an, av)| !allowed.contains(an) || !value_ok(el, an, av))
            {
                f.external(clipped(format!("<{el} {an}> in {path}")));
                return;
            }
            return;
        }
    };
    if !ok {
        f.external(format!("<{el}> in {path}"));
    }
}

/// The token pass's mutable state — open elements by name (the depth cap is
/// their length), so `<select>` context and mismatched ends are visible.
#[derive(Default)]
struct TokState {
    open: Vec<String>,
    doctypes: u32,
    too_deep: bool,
}

/// The sink html5ever drives: every start-tag token through the contract.
struct TokSink<'a> {
    path: &'a str,
    st: RefCell<TokState>,
    findings: RefCell<&'a mut Findings>,
}

impl TokenSink for TokSink<'_> {
    type Handle = ();
    fn process_token(&self, token: Token, _line: u64) -> TokenSinkResult<()> {
        let mut st = self.st.borrow_mut();
        let mut f = self.findings.borrow_mut();
        match token {
            Token::TagToken(tag) => {
                let el = tag.name.as_ref().to_owned();
                match tag.kind {
                    TagKind::StartTag => {
                        let attrs: Vec<(String, String)> = tag
                            .attrs
                            .iter()
                            .map(|a| {
                                (
                                    a.name.local.as_ref().to_owned(),
                                    a.value.as_ref().to_owned(),
                                )
                            })
                            .collect();
                        let pairs: Vec<(&str, &str)> = attrs
                            .iter()
                            .map(|(n, v)| (n.as_str(), v.as_str()))
                            .collect();
                        check_tag(&el, &pairs, &mut st, self.path, &mut f);
                        // html5ever treats a self-closing mark on non-void
                        // HTML as open — the count must match the browser's
                        // depth, not the token's suggestion.
                        if !VOID.contains(&el.as_str()) {
                            st.open.push(el.clone());
                            if st.open.len() > MAX_TAG_DEPTH {
                                st.too_deep = true;
                            }
                        }
                    }
                    TagKind::EndTag => {
                        // Pop through the named element if it is open —
                        // mismatches are the dom.errors check's business.
                        if let Some(pos) = st.open.iter().rposition(|o| *o == el) {
                            st.open.truncate(pos);
                        }
                    }
                }
            }
            Token::DoctypeToken(d) => {
                st.doctypes += 1;
                if st.doctypes > 1
                    || d.force_quirks
                    || d.name.as_deref() != Some("html")
                    || d.public_id.is_some()
                    || d.system_id.is_some()
                {
                    f.external(format!("doctype in {}", self.path));
                }
            }
            Token::ParseError(_) => {
                f.external(format!("tokenizer parse error in {}", self.path));
            }
            _ => {}
        }
        TokenSinkResult::Continue
    }
}

/// The token pass, exposed so a test can prove an arm without the
/// `dom.errors` backstop overlapping it. Returns true when the nesting cap
/// fired — the caller must not build a tree for such a file.
fn token_pass(path: &str, html: &str, findings: &mut Findings) -> bool {
    let sink = TokSink {
        path,
        st: RefCell::new(TokState::default()),
        findings: RefCell::new(findings),
    };
    let input = BufferQueue::default();
    input.push_back(StrTendril::from_slice(html));
    let tok = Tokenizer::new(sink, TokenizerOpts::default());
    let _ = tok.feed(&input);
    tok.end();
    let sink = tok.sink;
    let st = sink.st.into_inner();
    let findings = sink.findings.into_inner();
    if st.too_deep {
        findings.external(format!("nesting deeper than {MAX_TAG_DEPTH} in {path}"));
    }
    st.too_deep
}

/// One `.html` file under the contract: UTF-8, no BOM, the fixed prologue,
/// the token pass, then — only when the tokens are clean and shallow — the
/// tree-builder parse whose `dom.errors` must be empty.
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
    if token_pass(path, html, findings) || findings.svg.is_some() || findings.external.is_some() {
        // The cap fired before any tree was built (N3) — nothing deep is ever
        // constructed or dropped; or the token pass already failed the file.
        return;
    }
    // The tokens are clean: the tree build now exists only to prove the
    // document needed no correction — dom.errors must be empty (N1).
    let opts = html5ever::ParseOpts::default();
    let result = html5ever::parse_document(RcDom::default(), opts)
        .from_utf8()
        .read_from(&mut html.as_bytes());
    match result {
        Ok(dom) => {
            if !dom.errors.borrow().is_empty() {
                findings.external(format!("nonconformant markup in {path}"));
            }
        }
        Err(_) => findings.external(format!("unparseable in {path}")),
    }
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
#[allow(clippy::unwrap_used, reason = "test fixtures are fixed inputs")]
mod tests {
    use super::*;
    use intent_domain::{
        baseline,
        brief::BriefForm,
        bundle::Bundle,
        checks::CheckBackend,
        fact::FactLookup,
        page_tree::{COMPONENT_SET, PageTree, SpecContext, ValidatedTree},
    };
    use time::{Date, Month};

    /// Files → canonical framing → the report a runner would parse.
    fn report_for(files: &[(&str, Vec<u8>)]) -> CheckReport {
        let mut bundle = Bundle::new();
        for (path, content) in files {
            bundle.put(path, content.clone()).unwrap();
        }
        let out = check(&bundle.encode()).unwrap().output;
        serde_json::from_slice(&out).unwrap()
    }

    /// A page conformant apart from the payload under test: the real
    /// prologue, then a minimal head/body around `body`.
    fn page(body: &str) -> Vec<u8> {
        format!("{PROLOGUE}<title>t</title></head><body>{body}</body></html>").into_bytes()
    }

    fn day() -> Date {
        Date::from_calendar_date(2026, Month::November, 25).unwrap()
    }

    /// The real builder's output — brief, tree and facts at their smallest —
    /// so the contract is proved against emitted bytes, not a lookalike.
    fn real_bundle() -> Bundle {
        let brief = serde_json::from_value::<BriefForm>(serde_json::json!({
            "business_name": "Quickbeam Heating",
            "goal": "quote_requests",
            "intent": "More enquiries",
            "audience": "Home owners",
            "services": [{ "name": "Boiler service" }],
            "domains": ["quickbeam.example"],
            "references": [],
            "claims": []
        }))
        .unwrap()
        .validate()
        .unwrap();
        let facts = FactLookup::new();
        let tree: PageTree = serde_json::from_value(serde_json::json!({
            "component_set": COMPONENT_SET,
            "pages": [{
                "path": "/",
                "title": "Quickbeam Heating",
                "description": "Boilers, Glasgow",
                "sections": [
                    { "component": "hero", "heading": "Quickbeam Heating",
                      "subheading": "Heating engineers in Glasgow",
                      "action": { "label": "Get a quote",
                                  "target": { "anchor": "quote" } } },
                    { "component": "quote_form", "id": "quote",
                      "services": ["Boiler service"], "consent_notice": "privacy-v1",
                      "submit_label": "Send" },
                    { "component": "services",
                      "items": [{ "name": "Boiler service",
                                  "description": "Annual check" }] },
                    { "component": "footer", "legal": [], "links": [
                        { "label": "Quote", "target": { "anchor": "quote" } }
                    ] }
                ]
            }]
        }))
        .unwrap();
        let vt = ValidatedTree::validate(
            tree,
            SpecContext {
                brief: &brief,
                facts: &facts,
                today: day(),
            },
        )
        .unwrap();
        baseline::build(&vt).unwrap()
    }

    /// N1/N2/N3 red-first: every named bypass must fail the contract — a
    /// hostile bundle can never ride a browser-different parse to a pass.
    /// Svg arms land on `no_inline_svg`; the rest on `no_external_resources`.
    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one row per named bypass — the table is the proof"
    )]
    fn every_bypass_fixture_fails_the_contract() {
        let deep = format!(
            "{}{}",
            "<div class=\"sub\">".repeat(64),
            "</div>".repeat(64)
        );
        let mut second_doctype = page("");
        second_doctype.splice(15..15, b"<!doctype html>".iter().copied());
        for (name, files, check) in [
            // --- bytes before markup ---
            (
                "utf-16 BOM",
                vec![("index.html", b"\xff\xfe<\x00".to_vec())],
                CheckKind::NoExternalResources,
            ),
            (
                "utf-8 BOM",
                vec![("index.html", b"\xef\xbb\xbf<!doctype html>".to_vec())],
                CheckKind::NoExternalResources,
            ),
            (
                "invalid utf-8",
                vec![("index.html", b"<!doctype \xff".to_vec())],
                CheckKind::NoExternalResources,
            ),
            (
                "no prologue",
                vec![("index.html", b"<html></html>".to_vec())],
                CheckKind::NoExternalResources,
            ),
            (
                "different prologue",
                vec![(
                    "index.html",
                    String::from_utf8(page(""))
                        .unwrap()
                        .replacen("default-src 'none'", "default-src *", 1)
                        .into_bytes(),
                )],
                CheckKind::NoExternalResources,
            ),
            (
                "two doctypes",
                vec![("index.html", second_doctype)],
                CheckKind::NoExternalResources,
            ),
            // --- the tree-builder differential (N1): html5ever's tree drops
            // these inside <select>; Chrome 134+ runs them ---
            (
                "meta refresh inside select",
                vec![(
                    "index.html",
                    page(
                        "<form method=\"post\" action=\"/v1/leads\" class=\"quote-form\">\
                         <select name=\"service\"><meta http-equiv=\"refresh\" \
                         content=\"0;url=https://evil.example\"></select></form>",
                    ),
                )],
                CheckKind::NoExternalResources,
            ),
            (
                "svg inside select",
                vec![(
                    "index.html",
                    page("<select name=\"service\"><svg onload=alert(1)></svg></select>"),
                )],
                CheckKind::NoInlineSvg,
            ),
            (
                "preconnect inside select",
                vec![(
                    "index.html",
                    page(
                        "<select name=\"service\"><link rel=\"preconnect\" href=\"https://x\">\
                         </select>",
                    ),
                )],
                CheckKind::NoExternalResources,
            ),
            (
                "option outside select",
                vec![("index.html", page("<option>stowaway</option>"))],
                CheckKind::NoExternalResources,
            ),
            (
                // `name="name"` is a fully valid input anywhere else — only
                // the select-context arm can refuse it here.
                "valid input inside select",
                vec![(
                    "index.html",
                    page("<select name=\"service\"><input name=\"name\"></select>"),
                )],
                CheckKind::NoExternalResources,
            ),
            // --- resource surfaces ---
            (
                "img srcset",
                vec![("index.html", page("<img srcset=\"https://e/x.png 1x\">"))],
                CheckKind::NoExternalResources,
            ),
            (
                "img src",
                vec![("index.html", page("<img src=\"/x.png\">"))],
                CheckKind::NoExternalResources,
            ),
            (
                "object data",
                vec![("index.html", page("<object data=\"https://e/x\"></object>"))],
                CheckKind::NoExternalResources,
            ),
            (
                "iframe srcdoc",
                vec![(
                    "index.html",
                    page("<iframe srcdoc=\"&lt;svg onload=x&gt;\"></iframe>"),
                )],
                CheckKind::NoExternalResources,
            ),
            (
                "base href",
                vec![("index.html", page("<base href=\"https://evil.example/\">"))],
                CheckKind::NoExternalResources,
            ),
            (
                "meta refresh at top level",
                vec![(
                    "index.html",
                    page("<meta http-equiv=\"refresh\" content=\"0;url=https://e\">"),
                )],
                CheckKind::NoExternalResources,
            ),
            (
                "video poster",
                vec![(
                    "index.html",
                    page("<video poster=\"https://e/x.png\"></video>"),
                )],
                CheckKind::NoExternalResources,
            ),
            // --- scheme bypasses ---
            (
                "javascript href",
                vec![("index.html", page("<a href=\"javascript:alert(1)\">x</a>"))],
                CheckKind::NoExternalResources,
            ),
            (
                "entity-encoded javascript",
                vec![(
                    "index.html",
                    page("<a href=\"&#106;avascript:alert(1)\">x</a>"),
                )],
                CheckKind::NoExternalResources,
            ),
            (
                "data href",
                vec![("index.html", page("<a href=\"data:text/html,x\">x</a>"))],
                CheckKind::NoExternalResources,
            ),
            (
                "protocol-relative",
                vec![("index.html", page("<a href=\"//e/x\">x</a>"))],
                CheckKind::NoExternalResources,
            ),
            (
                "backslash host",
                vec![("index.html", page("<a href=\"\\\\e/x\">x</a>"))],
                CheckKind::NoExternalResources,
            ),
            (
                "http href",
                vec![("index.html", page("<a href=\"http://e/x\">x</a>"))],
                CheckKind::NoExternalResources,
            ),
            // --- scripts and handlers: elements the builder never emits ---
            (
                "script element",
                vec![("index.html", page("<script src=\"/x.js\"></script>"))],
                CheckKind::NoExternalResources,
            ),
            (
                "prefixed script",
                vec![("index.html", page("<h:script>x</h:script>"))],
                CheckKind::NoExternalResources,
            ),
            (
                "event handler",
                vec![("index.html", page("<p onclick=\"x()\">t</p>"))],
                CheckKind::NoExternalResources,
            ),
            (
                "style attribute",
                vec![(
                    "index.html",
                    page("<p style=\"background:url(https://e)\">t</p>"),
                )],
                CheckKind::NoExternalResources,
            ),
            (
                "style element",
                vec![(
                    "index.html",
                    page("<style>@import 'https://e/x.css'</style>"),
                )],
                CheckKind::NoExternalResources,
            ),
            (
                "svg family",
                vec![("index.html", page("<svg><script>x</script></svg>"))],
                CheckKind::NoInlineSvg,
            ),
            (
                "prefixed svg",
                vec![("index.html", page("<s:svg></s:svg>"))],
                CheckKind::NoInlineSvg,
            ),
            (
                "math element",
                vec![("index.html", page("<math><mtext>x</mtext></math>"))],
                CheckKind::NoInlineSvg,
            ),
            (
                "noscript",
                vec![(
                    "index.html",
                    page("<noscript><meta http-equiv=\"refresh\" content=\"0\"></noscript>"),
                )],
                CheckKind::NoExternalResources,
            ),
            (
                "template",
                vec![(
                    "index.html",
                    page("<template><img src=https://e/x></template>"),
                )],
                CheckKind::NoExternalResources,
            ),
            // --- values narrower than the vocabulary (N2): the builder only
            // ever emits these exact strings ---
            (
                "password input",
                vec![(
                    "index.html",
                    page(
                        "<form method=\"post\" action=\"/v1/leads\" class=\"quote-form\">\
                         <input type=\"password\" name=\"x\"></form>",
                    ),
                )],
                CheckKind::NoExternalResources,
            ),
            (
                "hidden input",
                vec![(
                    "index.html",
                    page(
                        "<form method=\"post\" action=\"/v1/leads\" class=\"quote-form\">\
                         <input type=\"hidden\" name=\"x\" value=\"y\"></form>",
                    ),
                )],
                CheckKind::NoExternalResources,
            ),
            (
                "other form action",
                vec![(
                    "index.html",
                    page(
                        "<form method=\"post\" action=\"/v1/admin\" class=\"quote-form\">\
                         </form>",
                    ),
                )],
                CheckKind::NoExternalResources,
            ),
            (
                "mailto form action",
                vec![(
                    "index.html",
                    page(
                        "<form method=\"post\" action=\"mailto:e@x\" class=\"quote-form\">\
                         </form>",
                    ),
                )],
                CheckKind::NoExternalResources,
            ),
            (
                "generic rel",
                vec![("index.html", page("<link rel=\"preload\" href=\"/x\">"))],
                CheckKind::NoExternalResources,
            ),
            (
                "second csp meta",
                vec![(
                    "index.html",
                    page(
                        "<meta http-equiv=\"Content-Security-Policy\" \
                         content=\"default-src 'none'; script-src https://e\">",
                    ),
                )],
                CheckKind::NoExternalResources,
            ),
            (
                "img element",
                vec![("index.html", page("<img alt=\"x\">"))],
                CheckKind::NoExternalResources,
            ),
            (
                "h3 element",
                vec![("index.html", page("<h3>x</h3>"))],
                CheckKind::NoExternalResources,
            ),
            (
                "span element",
                vec![("index.html", page("<span>x</span>"))],
                CheckKind::NoExternalResources,
            ),
            // --- file kinds (N2: nothing passes uninspected) ---
            (
                "image file",
                vec![("logo.png", b"\x89PNG".to_vec())],
                CheckKind::NoExternalResources,
            ),
            (
                "font file",
                vec![("f.woff2", b"wOF2".to_vec())],
                CheckKind::NoExternalResources,
            ),
            (
                "extensionless file",
                vec![("data", b"x".to_vec())],
                CheckKind::NoExternalResources,
            ),
            (
                "svg file",
                vec![("icon.svg", b"<svg></svg>".to_vec())],
                CheckKind::NoExternalResources,
            ),
            (
                "changed stylesheet",
                vec![("style.css", b"body{background:url(https://e)}".to_vec())],
                CheckKind::NoExternalResources,
            ),
            (
                // A tokenizer-level ParseError inside otherwise clean markup
                // — the arm the tree-builder errors alone cannot cover.
                "out-of-range charref",
                vec![("index.html", page("<p>&#x110000;</p>"))],
                CheckKind::NoExternalResources,
            ),
            // --- resource bounds (N3) ---
            (
                "deep nesting",
                vec![("index.html", page(&deep))],
                CheckKind::NoExternalResources,
            ),
        ] {
            let report = report_for(&files);
            let result = report
                .results
                .iter()
                .find(|r| r.check == check)
                .unwrap_or_else(|| panic!("{name}: no {check:?} result"));
            assert_eq!(
                result.status,
                CheckStatus::Fail,
                "{name}: bypass must fail, got {result:?}"
            );
        }
    }

    /// N1: the real builder's pages tokenize clean, tree-build with an empty
    /// `dom.errors`, and pass — the conformance bar cannot be gamed by a page
    /// the parser had to repair.
    #[test]
    fn builder_output_is_conformant_and_passes() {
        let bundle = real_bundle();
        for (path, bytes) in bundle.files() {
            if std::path::Path::new(path)
                .extension()
                .is_some_and(|e| e == "html")
            {
                let mut findings = Findings {
                    svg: None,
                    external: None,
                };
                check_html(path, bytes, &mut findings);
                assert!(
                    findings.svg.is_none() && findings.external.is_none(),
                    "{path} must pass the contract: {:?}",
                    findings.external
                );
            }
        }
        let out = check(&bundle.encode()).unwrap().output;
        let report: CheckReport = serde_json::from_slice(&out).unwrap();
        for result in &report.results {
            match result.check.backend() {
                CheckBackend::Static => assert_eq!(
                    result.status,
                    CheckStatus::Pass,
                    "static {:?} must pass on real builder output",
                    result.check
                ),
                CheckBackend::Browser => assert_eq!(result.status, CheckStatus::Pending),
            }
        }
    }

    /// N1: a document the tree builder has to repair is not builder output —
    /// only allowed elements, but mis-nested so the parse-error arm is what
    /// catches it (`<p>` cannot contain `<section>`).
    #[test]
    fn markup_needing_repair_fails() {
        let report = report_for(&[(
            "index.html",
            page("<p><section id=\"x\"><p>t</p></section></p>"),
        )]);
        let result = report
            .results
            .iter()
            .find(|r| r.check == CheckKind::NoExternalResources)
            .unwrap();
        assert_eq!(result.status, CheckStatus::Fail);
    }

    /// N4: attacker-sized names can never push a detail past the report bound.
    #[test]
    fn findings_stay_inside_the_report_bound() {
        let long = "x".repeat(4 * MAX_DETAIL_BYTES);
        for files in [
            vec![("index.html", page(&format!("<{long}>")))],
            vec![("index.html", page(&format!("<p {long}=\"v\">t</p>")))],
        ] {
            let report = report_for(&files);
            for result in &report.results {
                if let Some(detail) = &result.detail {
                    assert!(
                        detail.len() <= MAX_DETAIL_BYTES,
                        "detail {}B exceeds the bound",
                        detail.len()
                    );
                }
            }
        }
    }

    /// N1: the select-context arm on its own — a fully valid element in the
    /// one place the contract forbids it. `dom.errors` backstops the rule,
    /// but the token pass must flag it independently.
    #[test]
    fn only_option_survives_inside_select_at_the_token_level() {
        for body in [
            "<select name=\"service\"><input name=\"name\"></select>",
            "<select name=\"service\"><div class=\"sub\"></div></select>",
            "<select name=\"service\"><a href=\"#x\">x</a></select>",
            "<select name=\"service\"><p>t</p></select>",
        ] {
            let mut findings = Findings {
                svg: None,
                external: None,
            };
            let html = String::from_utf8(page(body)).unwrap();
            token_pass("index.html", &html, &mut findings);
            assert!(
                findings.external.is_some(),
                "{body}: a non-option element inside <select> must fail at the token level"
            );
        }
        // And the allowed shape — option inside select — stays clean.
        let mut findings = Findings {
            svg: None,
            external: None,
        };
        let html = String::from_utf8(page(
            "<select name=\"service\"><option value=\"\">—</option><option>x</option></select>",
        ))
        .unwrap();
        token_pass("index.html", &html, &mut findings);
        assert!(
            findings.external.is_none() && findings.svg.is_none(),
            "option inside select is the builder's own shape"
        );
    }

    /// Every check kind is reported — a missing result can never be read as
    /// a pass — and the browser half stays honestly pending.
    #[test]
    fn every_check_kind_is_reported() {
        let report = report_for(&[("index.html", page(""))]);
        let kinds: Vec<CheckKind> = report.results.iter().map(|r| r.check).collect();
        for kind in CheckKind::ALL {
            assert!(kinds.contains(&kind), "{kind:?} missing from the report");
        }
    }
}
