//! SVG sanitizing by re-serialization from an allowlist (D-011, D-062).
//!
//! The input is parsed (no DTD, a node limit), and a new document is written containing only
//! allowlisted elements in the SVG namespace and allowlisted attributes, every value escaped.
//! Anything else — `script`, `style`, `foreignObject`, `image`, `a`, animation elements, event
//! handlers, `style` attributes, references outside the document — is dropped and counted.
//! Rebuilding from an allowlist, rather than deleting what a denylist names, is what makes an
//! unknown trick harmless: it is simply never written.

use std::fmt::Write as _;

use intent_render::WorkerRefusal;
use roxmltree::{Document, Node, NodeType, ParsingOptions};

/// Largest SVG accepted (2 MiB).
pub const MAX_SVG_BYTES: usize = 2 * 1024 * 1024;
/// Most XML nodes parsed.
pub const MAX_NODES: u32 = 100_000;
/// Deepest element nesting kept.
pub const MAX_DEPTH: usize = 64;
/// Deepest a `use` may reach through references; a longer chain never finishes expanding
/// cheaply enough (a doubling chain's cost is 2^depth).
pub const MAX_USE_DEPTH: u32 = 8;
/// Most `use` expansions the document may produce in all.
pub const MAX_USE_EXPANSIONS: u32 = 512;

const SVG_NS: &str = "http://www.w3.org/2000/svg";
const XLINK_NS: &str = "http://www.w3.org/1999/xlink";

/// Elements that are drawn or structure drawing, and nothing that runs, loads or embeds.
const ELEMENTS: &[&str] = &[
    "svg",
    "g",
    "defs",
    "title",
    "desc",
    "path",
    "rect",
    "circle",
    "ellipse",
    "line",
    "polyline",
    "polygon",
    "text",
    "tspan",
    "linearGradient",
    "radialGradient",
    "stop",
    "clipPath",
    "mask",
    "symbol",
    "use",
];

/// Geometry and paint, with no attribute that can carry script or load anything.
const ATTRIBUTES: &[&str] = &[
    "id",
    "class",
    "viewBox",
    "width",
    "height",
    "x",
    "y",
    "x1",
    "y1",
    "x2",
    "y2",
    "cx",
    "cy",
    "r",
    "rx",
    "ry",
    "fx",
    "fy",
    "dx",
    "dy",
    "d",
    "points",
    "pathLength",
    "fill",
    "fill-opacity",
    "fill-rule",
    "stroke",
    "stroke-width",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-miterlimit",
    "stroke-dasharray",
    "stroke-dashoffset",
    "stroke-opacity",
    "opacity",
    "transform",
    "gradientUnits",
    "gradientTransform",
    "spreadMethod",
    "offset",
    "stop-color",
    "stop-opacity",
    "clip-path",
    "clip-rule",
    "clipPathUnits",
    "mask",
    "maskUnits",
    "maskContentUnits",
    "font-family",
    "font-size",
    "font-weight",
    "font-style",
    "text-anchor",
    "dominant-baseline",
    "letter-spacing",
    "preserveAspectRatio",
    "version",
    "role",
    "aria-label",
    "aria-hidden",
    "focusable",
    "visibility",
    "display",
];

/// Elements whose text content is kept.
const TEXT_ELEMENTS: &[&str] = &["title", "desc", "text", "tspan"];

/// Elements that may reference another element of the same document.
const HREF_ELEMENTS: &[&str] = &["use", "linearGradient", "radialGradient"];

/// A sanitized SVG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sanitized {
    /// The new document.
    pub svg: String,
    /// Elements and attributes dropped.
    pub removed: u32,
    /// The root's width in pixels, when it states one (or the viewBox's).
    pub width: Option<u32>,
    /// The root's height in pixels, when it states one (or the viewBox's).
    pub height: Option<u32>,
}

/// Sanitizes `input` or refuses it.
pub fn sanitize(input: &[u8]) -> Result<Sanitized, WorkerRefusal> {
    if input.len() > MAX_SVG_BYTES {
        return Err(WorkerRefusal::TooLarge);
    }
    let text = std::str::from_utf8(input).map_err(|_| WorkerRefusal::Malformed)?;
    let trimmed = text.trim_start_matches('\u{feff}').trim_start();
    if !(trimmed.starts_with('<')) {
        return Err(WorkerRefusal::TypeMismatch);
    }
    let options = ParsingOptions {
        allow_dtd: false,
        nodes_limit: MAX_NODES,
        ..ParsingOptions::default()
    };
    let document = Document::parse_with_options(trimmed, options).map_err(|error| match error {
        roxmltree::Error::NodesLimitReached => WorkerRefusal::TooLarge,
        _ => WorkerRefusal::Malformed,
    })?;
    let root = document.root_element();
    if root.tag_name().name() != "svg" || root.tag_name().namespace() != Some(SVG_NS) {
        return Err(WorkerRefusal::TypeMismatch);
    }
    let mut writer = Writer {
        out: String::with_capacity(trimmed.len()),
        removed: 0,
        ids: std::collections::HashSet::new(),
    };
    writer.element(root, 0, true);
    // The expansion bound is checked against what the browser will actually expand: the
    // sanitised document re-parsed. Checking the input tree would see elements the writer
    // dropped (a `filter` shadowing a referenced `id`) and attributes it rewrote.
    let emitted = writer.out;
    let reparsed = Document::parse_with_options(
        &emitted,
        ParsingOptions {
            allow_dtd: false,
            nodes_limit: MAX_NODES,
            ..ParsingOptions::default()
        },
    )
    .map_err(|_| WorkerRefusal::Malformed)?;
    let sanitized_root = reparsed.root_element();
    check_use_expansion(&sanitized_root)?;
    let size = |name: &str| root.attribute(name).and_then(pixels);
    let view_box: Option<Vec<f64>> = root.attribute("viewBox").map(|v| {
        v.split(|c: char| c == ',' || c.is_whitespace())
            .filter(|p| !p.is_empty())
            .filter_map(|p| p.parse().ok())
            .collect()
    });
    let from_box = |i: usize| {
        view_box
            .as_ref()
            .filter(|b| b.len() == 4)
            .and_then(|b| whole(b[i]))
    };
    let width = size("width").or_else(|| from_box(2));
    let height = size("height").or_else(|| from_box(3));
    writer.out = emitted;
    Ok(Sanitized {
        svg: writer.out,
        removed: writer.removed,
        width,
        height,
    })
}

/// A plain or `px` length as whole pixels.
fn pixels(value: &str) -> Option<u32> {
    let number = value.trim().strip_suffix("px").unwrap_or(value.trim());
    number.parse::<f64>().ok().and_then(whole)
}

fn whole(value: f64) -> Option<u32> {
    (value.is_finite() && value >= 1.0 && value <= f64::from(u32::MAX)).then(|| {
        // The value is checked to be within u32's range above.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "bounded to 1..=u32::MAX just above"
        )]
        let rounded = value.round() as u32;
        rounded
    })
}

/// The functions an attribute value may invoke (transforms and paints); `url` is the only one
/// that can fetch, and it is pinned to a same-document fragment by the check below.
const FUNCTIONS: &[&str] = &[
    "url",
    "translate",
    "scale",
    "rotate",
    "skewx",
    "skewy",
    "matrix",
    "rgb",
    "rgba",
    "hsl",
    "hsla",
    "hwb",
    "oklch",
    "lab",
    "color",
    "var",
];

/// Presentation attributes whose values a browser resolves as a paint, transform or resource
/// — the ones a hostile value could make fetch. Other attributes (a font name, an aria-label)
/// get the looser check: no escapes, no scheme, but text parentheses are fine.
const CSS_ATTRIBUTES: &[&str] = &[
    "fill",
    "stroke",
    "clip-path",
    "mask",
    "transform",
    "gradientTransform",
    "stop-color",
];

/// Whether an attribute value could load, run or reference something outside the document.
/// `strict` selects the CSS-value rules ([`CSS_ATTRIBUTES`]); a `false` value still refuses
/// escapes and scheme words but lets a `(` be ordinary text.
fn value_is_safe(value: &str, strict: bool) -> bool {
    // A CSS escape can spell anything the checks below look for (`\75rl(` is `url(`), so the
    // escape character itself is refused rather than unescaped into a bigger check surface.
    if value.contains('\\') {
        return false;
    }
    // Quoted spans come out before the scan: `(` inside '…' or "…" is text (a font name), not
    // a call. An unterminated quote hides the rest of the value — refuse the whole attribute.
    let mut unquoted = String::with_capacity(value.len());
    let mut quote = None;
    for c in value.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '\'' || c == '"' => quote = Some(c),
            None => unquoted.push(c),
        }
    }
    if quote.is_some() {
        return false;
    }
    let lower: String = unquoted
        .chars()
        .filter(|c| !c.is_whitespace() && !c.is_control())
        .collect::<String>()
        .to_ascii_lowercase();
    if lower.contains("javascript:")
        || lower.contains("data:")
        || lower.contains("expression(")
        || lower.contains("@import")
        || lower.contains('<')
    {
        return false;
    }
    if !strict {
        // A value that is not a CSS paint/transform must never smuggle a resource load.
        return !lower.contains("url(");
    }
    // Every `(` opens a function: the name before it must be on the short list, and `url` must
    // name a same-document fragment — `url(#id)`, optionally quoted.
    let mut rest = lower.as_str();
    while let Some(open) = rest.find('(') {
        // The name is the run of name characters before `(`; taking it with rsplit (a char
        // scan, not byte math) means a non-ASCII byte can never be sliced mid-character.
        let name = rest[..open]
            .rsplit(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
            .next()
            .unwrap_or("");
        if !FUNCTIONS.contains(&name) {
            return false;
        }
        if name == "url" {
            let target = rest[open + 1..].trim_start_matches(['"', '\'']);
            if !target.starts_with('#') {
                return false;
            }
        }
        rest = &rest[open + 1..];
    }
    true
}

/// A `use` element's target, if it has a same-document fragment reference that resolves.
fn use_target<'a, 'input>(
    node: Node<'a, 'input>,
    by_id: &std::collections::HashMap<&'input str, Node<'a, 'input>>,
) -> Option<Node<'a, 'input>> {
    let tag = node.tag_name();
    if !(node.is_element() && tag.name() == "use" && tag.namespace() == Some(SVG_NS)) {
        return None;
    }
    // A browser prefers `href` over `xlink:href` when both are present; resolve the same one
    // it will expand — the writer emits only this attribute, so the rendered tree agrees.
    let href = node
        .attributes()
        .find_map(|a| (a.namespace().is_none() && a.name() == "href").then(|| a.value()))
        .or_else(|| {
            node.attributes().find_map(|a| {
                (a.namespace() == Some(XLINK_NS) && a.name() == "href").then(|| a.value())
            })
        })?;
    if !is_fragment(href) {
        return None;
    }
    by_id.get(&href[1..]).copied()
}

/// `use` chains are bounded before a byte is written: a renderer expands each reference,
/// and a chain of `use`-inside-`use` (or a cycle) makes that expansion exponential — or
/// infinite. The cost of one element is itself plus the cost of every use inside it;
/// a document over the depth or total bound is refused.
fn check_use_expansion(root: &Node<'_, '_>) -> Result<(), WorkerRefusal> {
    use std::collections::{HashMap, HashSet};
    let mut by_id: HashMap<&str, Node<'_, '_>> = HashMap::new();
    for node in root.descendants() {
        if node.is_element()
            && let Some(id) = node.attribute("id")
        {
            by_id.entry(id).or_insert(node);
        }
    }
    let mut memo = HashMap::new();
    let (expansions, height) = use_cost(*root, &by_id, &mut memo, &mut HashSet::new())?;
    if height > MAX_USE_DEPTH || expansions > MAX_USE_EXPANSIONS {
        return Err(WorkerRefusal::TooLarge);
    }
    Ok(())
}

/// One node's answer for [`check_use_expansion`]: (its expansion cost, the longest chain
/// below it). Both are absolute, so memoizing them is sound — `g` costs the same whether
/// it is reached first or at the end of a chain.
fn use_cost<'a, 'input>(
    node: Node<'a, 'input>,
    by_id: &std::collections::HashMap<&'input str, Node<'a, 'input>>,
    memo: &mut std::collections::HashMap<roxmltree::NodeId, (u32, u32)>,
    visiting: &mut std::collections::HashSet<roxmltree::NodeId>,
) -> Result<(u32, u32), WorkerRefusal> {
    if visiting.contains(&node.id()) {
        return Err(WorkerRefusal::TooLarge);
    }
    if let Some(&cached) = memo.get(&node.id()) {
        return Ok(cached);
    }
    visiting.insert(node.id());
    let mut total = 1_u32;
    let mut height = 1_u32;
    for descendant in node.descendants() {
        // A `use` inside a `defs`/`symbol` subtree is never painted directly — it expands only
        // when its subtree is instantiated by a reference, and it is counted in that call.
        // Skipping it here keeps a use inside an unreferenced defs from counting twice.
        if descendant.is_element()
            && descendant.tag_name().name() == "use"
            && descendant.tag_name().namespace() == Some(SVG_NS)
            && descendant
                .ancestors()
                .take_while(|a| a.id() != node.id())
                .any(|a| {
                    a.tag_name().namespace() == Some(SVG_NS)
                        && matches!(a.tag_name().name(), "defs" | "symbol")
                })
        {
            continue;
        }
        if let Some(target) = use_target(descendant, by_id) {
            let (expansions, chain) = use_cost(target, by_id, memo, visiting)?;
            total = total.saturating_add(expansions);
            height = height.max(chain.saturating_add(1));
        }
    }
    visiting.remove(&node.id());
    memo.insert(node.id(), (total, height));
    Ok((total, height))
}

/// A same-document reference: `#` followed by an id's characters.
fn is_fragment(value: &str) -> bool {
    value.strip_prefix('#').is_some_and(|id| {
        !id.is_empty()
            && id.len() <= 200
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
    })
}

struct Writer {
    out: String,
    removed: u32,
    /// `id` values already emitted: a browser resolves a fragment to the first element with
    /// the id, so a later duplicate `id` is dropped, never written.
    ids: std::collections::HashSet<String>,
}

impl Writer {
    fn element(&mut self, node: Node<'_, '_>, depth: usize, is_root: bool) {
        let tag = node.tag_name();
        let allowed = tag.namespace() == Some(SVG_NS) && ELEMENTS.contains(&tag.name());
        if !allowed || depth > MAX_DEPTH {
            self.removed += 1;
            return;
        }
        let name = tag.name();
        let _ = write!(self.out, "<{name}");
        if is_root {
            self.out.push_str(" xmlns=\"http://www.w3.org/2000/svg\"");
        }
        // Exactly one href survives on an element that may carry one: a browser prefers
        // `href` over `xlink:href`, so that is the one emitted — emitting both would let a
        // `use` resolve a different target than the expansion check counted.
        let mut href_emitted = false;
        if HREF_ELEMENTS.contains(&name) {
            let chosen = node
                .attributes()
                .find_map(|a| {
                    (a.namespace().is_none() && a.name() == "href" && is_fragment(a.value()))
                        .then(|| a.value())
                })
                .or_else(|| {
                    node.attributes().find_map(|a| {
                        (a.namespace() == Some(XLINK_NS)
                            && a.name() == "href"
                            && is_fragment(a.value()))
                        .then(|| a.value())
                    })
                });
            if let Some(chosen) = chosen {
                let _ = write!(self.out, " href=\"");
                escape(&mut self.out, chosen, true);
                self.out.push('"');
                href_emitted = true;
            }
        }
        for attribute in node.attributes() {
            let key = attribute.name();
            let value = attribute.value();
            let keep = match attribute.namespace() {
                // Handled above: either emitted once or dropped.
                None | Some(XLINK_NS) if key == "href" => false,
                None if key == "id" => ATTRIBUTES.contains(&key) && self.ids.insert(value.into()),
                None => {
                    ATTRIBUTES.contains(&key)
                        && value.len() <= 100_000
                        && value_is_safe(value, CSS_ATTRIBUTES.contains(&key))
                }
                Some(_) => false,
            };
            if keep {
                let _ = write!(self.out, " {key}=\"");
                escape(&mut self.out, value, true);
                self.out.push('"');
            } else {
                self.removed += 1;
            }
        }
        if href_emitted {
            // The normalized href above stands for one of the source attributes the loop
            // counted as dropped; the rest really were.
            self.removed = self.removed.saturating_sub(1);
        }
        self.out.push('>');
        let keeps_text = TEXT_ELEMENTS.contains(&name);
        for child in node.children() {
            match child.node_type() {
                NodeType::Element => self.element(child, depth + 1, false),
                NodeType::Text if keeps_text => {
                    escape(&mut self.out, child.text().unwrap_or_default(), false);
                }
                // Whitespace between elements, comments and processing instructions carry
                // nothing a picture needs.
                _ => {}
            }
        }
        let _ = write!(self.out, "</{name}>");
    }
}

fn escape(out: &mut String, text: &str, attribute: bool) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attribute => out.push_str("&quot;"),
            c if c.is_control() && !matches!(c, '\n' | '\t') => {}
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(inner: &str) -> Vec<u8> {
        format!("<svg xmlns=\"{SVG_NS}\">{inner}</svg>").into_bytes()
    }

    fn sanitized(inner: &str) -> String {
        sanitize(&doc(inner)).unwrap().svg
    }

    #[test]
    fn a_css_escape_cannot_disguise_a_fetch_or_a_handler() {
        // \75 decodes to `u`: what a stylesheet reads as url(https://…) the literal scan
        // missed. The attribute must not survive.
        assert!(
            !sanitized("<rect fill=\"\\75rl(https://beacon.example/x.png)\"/>")
                .contains("https://"),
            "an escaped url( reached the output"
        );
        // The same trick on the scheme: \6a avascript:.
        assert!(
            !sanitized("<use href=\"#a\" fill=\"\\6a avascript:alert(1)\"/>").contains("vascript"),
            "an escaped scheme reached the output"
        );
        // A backslash is never part of a legitimate paint value at all: none survives.
        assert!(
            !sanitized("<rect fill=\"u\\72 l(#a)\"/>").contains('\\'),
            "a backslash reached the output"
        );
    }

    #[test]
    fn parentheses_only_survive_inside_a_named_function() {
        // The allowed forms still pass.
        let out = sanitized(
            "<defs><linearGradient id=\"g\"/></defs>\
             <rect fill=\"url(#g)\" stroke=\"rgb(1, 2, 3)\" transform=\"translate(10, 20) rotate(5)\"/>",
        );
        assert!(out.contains("fill=\"url(#g)\""));
        assert!(out.contains("stroke=\"rgb(1, 2, 3)\""));
        assert!(out.contains("translate(10, 20) rotate(5)"));
        // A url() that is not a fragment is dropped, and so is any other unknown function.
        assert!(!sanitized("<rect fill=\"url(https://b.example/)\"/>").contains("https://"));
        assert!(!sanitized("<rect fill=\"expression(alert(1))\"/>").contains("expression"));
        assert!(
            !sanitized("<rect fill=\"-moz-image-rect(url(#g),0,0,1,1)\"/>").contains("image-rect")
        );
    }

    #[test]
    fn use_chains_are_bounded() {
        // A chain nine references deep refuses: the document expands past the bound.
        let mut chain = String::from("<defs><rect id=\"a\" width=\"1\" height=\"1\"/>");
        for i in 0..9 {
            let from = char::from(b'a' + i);
            let to = char::from(b'a' + i + 1);
            let _ = write!(chain, "<g id=\"{to}\"><use href=\"#{from}\"/></g>");
        }
        chain.push_str("</defs><use href=\"#j\"/>");
        assert_eq!(
            sanitize(&doc(&chain)),
            Err(WorkerRefusal::TooLarge),
            "a nine-deep use chain passed"
        );
        // A cycle never finishes expanding: refused too.
        let cycle = "<defs><g id=\"x\"><use href=\"#x\"/></g></defs><use href=\"#x\"/>";
        assert_eq!(sanitize(&doc(cycle)), Err(WorkerRefusal::TooLarge));
        // A modest chain still passes.
        let ok = "<defs><g id=\"p\"><rect/></g><g id=\"q\"><use href=\"#p\"/></g></defs>\
                  <use href=\"#q\"/><use href=\"#q\"/>";
        assert!(sanitize(&doc(ok)).is_ok(), "a two-deep use chain refused");
        // Uses inside a defs nothing references do not spend the budget.
        let mut dormant = String::from("<defs><g>");
        for _ in 0..600 {
            let _ = write!(dormant, "<use href=\"#p\"/>");
        }
        dormant.push_str("</g><g id=\"p\"><rect/></g></defs><rect/>");
        assert!(
            sanitize(&doc(&dormant)).is_ok(),
            "unreferenced uses spent the budget"
        );
    }

    #[test]
    fn a_non_ascii_byte_before_a_paren_is_just_an_unknown_function() {
        // The fuzzer's minimal panic: a four-byte char before `(` made the function-name
        // slice land mid-character. Now the value is simply refused, never a panic.
        let out = sanitize(&doc("<rect fill=\"\u{10700}(x)\"/>")).unwrap().svg;
        assert!(!out.contains('\u{10700}'), "{out}");
    }

    #[test]
    fn the_bound_checks_the_rendered_tree_not_the_input() {
        // A dropped element can still shadow an id in the input: `filter` is not allowlisted,
        // so the rendered `#a` is the `g` with ten uses in it. 60 references × 11 expansions
        // each is 660 — but the input check resolved `#a` to the empty filter and counted 70.
        let mut bomb = String::from("<defs><filter id=\"a\"/><g id=\"a\">");
        for _ in 0..10 {
            let _ = write!(bomb, "<use href=\"#r\"/>");
        }
        bomb.push_str("</g><rect id=\"r\"/></defs>");
        for _ in 0..60 {
            let _ = write!(bomb, "<use href=\"#a\"/>");
        }
        assert_eq!(
            sanitize(&doc(&bomb)),
            Err(WorkerRefusal::TooLarge),
            "a dropped element's id hid the real expansion"
        );
        // Both href spellings on one `use`: the browser prefers `href`, so `xlink:href`
        // cannot name a cheaper target than the one expanded. `#a` is a nine-deep chain.
        let mut dual = String::from(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\">\
             <defs><rect id=\"cheap\"/><g id=\"a\"><use href=\"#b\"/></g>",
        );
        for k in b'b'..b'j' {
            let id = k as char;
            let next = (k + 1) as char;
            let _ = write!(dual, "<g id=\"{id}\"><use href=\"#{next}\"/></g>");
        }
        dual.push_str(
            "<g id=\"j\"><rect/></g></defs><use xlink:href=\"#cheap\" href=\"#a\"/></svg>",
        );
        assert_eq!(
            sanitize(dual.as_bytes()),
            Err(WorkerRefusal::TooLarge),
            "xlink:href shadowed the href a browser would use"
        );
    }

    #[test]
    fn text_parentheses_and_new_paint_functions_are_kept() {
        let out = sanitized(
            "<text aria-label=\"Call us (soon)\">x</text>\
             <rect fill=\"oklch(0.5 0.1 30)\" stroke=\"var(--p, url(#g))\"/>",
        );
        assert!(out.contains("aria-label=\"Call us (soon)\""), "{out}");
        assert!(out.contains("oklch(0.5 0.1 30)"), "{out}");
        assert!(out.contains("var(--p, url(#g))"), "{out}");
        // A url() inside the relaxed check is still refused.
        assert!(!sanitized("<text aria-label=\"url(http://x)\">x</text>").contains("url("));
    }
}
