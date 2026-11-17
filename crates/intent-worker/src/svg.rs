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
    };
    writer.element(root, 0, true);
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
    Ok(Sanitized {
        svg: writer.out,
        removed: writer.removed,
        width: size("width").or_else(|| from_box(2)),
        height: size("height").or_else(|| from_box(3)),
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

/// Whether an attribute value could load, run or reference something outside the document.
fn value_is_safe(value: &str) -> bool {
    let lower: String = value
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
    // `url(...)` is allowed only as a same-document fragment: `url(#id)`.
    let mut rest = lower.as_str();
    while let Some(at) = rest.find("url(") {
        let after = &rest[at + 4..];
        let target = after.trim_start_matches(['"', '\'']);
        if !target.starts_with('#') {
            return false;
        }
        rest = after;
    }
    true
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
        for attribute in node.attributes() {
            let key = attribute.name();
            let value = attribute.value();
            let keep = match attribute.namespace() {
                None if key == "href" => HREF_ELEMENTS.contains(&name) && is_fragment(value),
                Some(XLINK_NS) if key == "href" => {
                    HREF_ELEMENTS.contains(&name) && is_fragment(value)
                }
                None => ATTRIBUTES.contains(&key) && value_is_safe(value) && value.len() <= 100_000,
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
