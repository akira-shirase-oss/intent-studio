//! The deterministic baseline builder (I2): a validated [`PageTree`] plus the
//! tenant's facts → a byte-stable [`Bundle`], with no model anywhere in the
//! path. The bundle the repair loop is compared against and the floor every
//! generation must beat.
//!
//! Byte-stability is the contract, not a hope:
//!
//! - files live in a [`BTreeMap`], so order is fixed;
//! - nothing wall-clock- or machine-derived reaches the bytes (`today` only
//!   decides *whether* a build may proceed — a stale fact is a refusal, never
//!   different bytes);
//! - facts are resolved by id, never iterated (`HashMap` order cannot leak);
//! - every byte of tree text and fact statement is escaped the same way.
//!
//! The pilot emits semantic HTML with raster/`img` references only — no inline
//! SVG (the check worker's `no_inline_svg` refuses it; [`crate::checks`]) and no
//! free HTML: a tree is the only input a model-shaped caller can ever hand in.

use std::fmt::Write as _;

use time::Date;

use crate::{
    bundle::Bundle,
    fact::{FactLookup, FactStatus},
    ids::FactId,
    page_tree::{Action, FactRef, Page, Section, ServiceItem, Target, Text, ValidatedTree},
};

/// The builder's version, recorded on the candidate (`baseline@2`).
pub const BUILDER: &str = "baseline@2";

/// Every page's fixed first bytes (I2-1): the meta CSP is part of the emitted
/// document, so the check can require this exact prologue rather than trust a
/// policy a hostile bundle rewrote. The stylesheet is a bundle file — inline
/// `<style>` is unhashable attacker surface, so `style-src 'self'` suffices.
pub const PROLOGUE: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
    <meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src 'self'; style-src 'self'; base-uri 'none'; form-action 'self'\">\
    <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">";

/// The stylesheet's serve path — the only CSS file the builder emits.
pub const STYLE_PATH: &str = "style.css";

/// The pilot stylesheet: fixed bytes, no external reference, system fonts — the
/// page must be complete within the bundle's own bytes. Emitted once at
/// [`STYLE_PATH`]; the check requires every `.css` file to equal it exactly.
pub const STYLE: &str = "body{font-family:system-ui,sans-serif;margin:0;color:#111}\
section,footer{max-width:64rem;margin:0 auto;padding:1rem}\
.hero{background:#f4f4f4;padding:3rem 1rem;text-align:center}\
.cta{display:inline-block;background:#111;color:#fff;padding:.75rem 1.5rem;text-decoration:none;min-height:24px}\
ul.services,ul.trust{list-style:none;padding:0}\
.quote-form{display:flex;flex-direction:column;gap:.75rem;max-width:32rem}\
.quote-form label{display:flex;flex-direction:column;gap:.25rem}\
.quote-form .consent{flex-direction:row;align-items:center}\
input,select,textarea,button{min-height:24px;font:inherit}\
blockquote{border-left:3px solid #ccc;margin:1rem 0;padding-left:1rem}";

/// Why a tree cannot become a bundle. Stable codes — an unapproved or absent
/// fact is a refusal, never a substituted sentence.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BuildError {
    /// A referenced fact is not in the lookup (another tenant's reads the same).
    #[error("unknown_fact {0}")]
    UnknownFact(FactId),
    /// A referenced fact is not approved and current today.
    #[error("fact_not_usable {0}")]
    FactNotUsable(FactId),
    /// A fact target names a fact with no link.
    #[error("target_without_link {0}")]
    TargetWithoutLink(FactId),
    /// A page path cannot become a bundle path.
    #[error("bad_path")]
    BadPath,
    /// The bundle's own bound broke ([`crate::bundle::BundleError`]).
    #[error("bundle {0}")]
    Bundle(#[from] crate::bundle::BundleError),
}

/// Builds a [`ValidatedTree`] into a bundle with `facts` resolved on `today`.
/// Validation is a type, not a convention — a tree that skipped it cannot
/// reach this function. The builder still refuses a fact it cannot use — it is
/// the last enforcement point before bytes.
pub fn build(tree: &ValidatedTree, facts: &FactLookup, today: Date) -> Result<Bundle, BuildError> {
    let mut bundle = Bundle::new();
    bundle.put(STYLE_PATH, STYLE.as_bytes().to_vec())?;
    let mut pages: Vec<(&String, &Page)> = tree.tree().pages.iter().map(|p| (&p.path, p)).collect();
    pages.sort_by(|a, b| a.0.cmp(b.0));
    for (path, page) in pages {
        let file = if path == "/" {
            "index.html".to_owned()
        } else {
            format!("{}/index.html", path.trim_start_matches('/'))
        };
        bundle.put(&file, render_page(page, facts, today)?.into_bytes())?;
    }
    Ok(bundle)
}

/// `& < > " '` → entities — text nodes and attribute values share the five;
/// the templates never interpolate into a raw `<script>`/`<style>` context.
fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(c),
        }
    }
    out
}

/// One fact's statement, escaped — or the named refusal. A fact's status is
/// decided on `today` exactly as the validator's is.
fn statement(facts: &FactLookup, id: FactId, today: Date) -> Result<&str, BuildError> {
    let record = facts.get(&id).ok_or(BuildError::UnknownFact(id))?;
    match record.status(today) {
        FactStatus::Approved { .. } => Ok(&record.statement),
        _ => Err(BuildError::FactNotUsable(id)),
    }
}

fn fact_ref(facts: &FactLookup, today: Date, fact: &FactRef) -> Result<String, BuildError> {
    Ok(esc(statement(facts, fact.fact, today)?))
}

/// A link target → `href`: structural only, as D-014 validated.
fn href(facts: &FactLookup, today: Date, target: &Target) -> Result<String, BuildError> {
    let raw = match target {
        Target::Anchor(id) => format!("#{id}"),
        Target::Page(path) => path.clone(),
        Target::Fact(id) => {
            statement(facts, *id, today)?; // approved, or the link may not point at it
            let record = facts.get(id).ok_or(BuildError::UnknownFact(*id))?;
            record
                .link
                .as_ref()
                .ok_or(BuildError::TargetWithoutLink(*id))?
                .href()
        }
        Target::Url(url) => url.clone(),
    };
    Ok(esc(&raw))
}

fn text_of(text: &Text) -> &str {
    match text {
        Text::Plain(t) => t,
        Text::Bound(b) => &b.text,
    }
}

fn heading(text: &Text, level: u8) -> String {
    format!("<h{level}>{}</h{level}>", esc(text_of(text)))
}

fn opt_heading(text: Option<&Text>, level: u8) -> String {
    text.map(|t| heading(t, level)).unwrap_or_default()
}

fn action(
    facts: &FactLookup,
    today: Date,
    action: &Action,
    class: &str,
) -> Result<String, BuildError> {
    Ok(format!(
        "<a class=\"{class}\" href=\"{}\">{}</a>",
        href(facts, today, &action.target)?,
        esc(&action.label)
    ))
}

/// The form's fixed fields — the pilot's quote request. `action` is the site's
/// own lead intake path (D-022 checks the success-on-201 behaviour in a real
/// browser); field names are the intake schema's.
fn quote_form(services: &[String], consent_notice: &str, submit_label: Option<&str>) -> String {
    let mut out = String::from("<form method=\"post\" action=\"/v1/leads\" class=\"quote-form\">");
    out.push_str(
        "<label>Name <input name=\"name\" required autocomplete=\"name\"></label>\
         <label>Phone <input name=\"phone\" type=\"tel\" autocomplete=\"tel\"></label>\
         <label>E-mail <input name=\"email\" type=\"email\" autocomplete=\"email\"></label>\
         <label>Postcode or area <input name=\"area\" autocomplete=\"postal-code\"></label>",
    );
    out.push_str("<label>Service <select name=\"service\"><option value=\"\">—</option>");
    for service in services {
        let _ = write!(out, "<option>{}</option>", esc(service));
    }
    out.push_str("</select></label>");
    out.push_str(
        "<label>What do you need? <textarea name=\"message\" required></textarea></label>",
    );
    let _ = write!(
        out,
        "<label class=\"consent\"><input type=\"checkbox\" name=\"consent\" required> {}</label>",
        esc(consent_notice)
    );
    let label = submit_label.unwrap_or("Request a quote");
    let _ = write!(
        out,
        "<button type=\"submit\">{}</button></form>",
        esc(label)
    );
    out
}

#[expect(
    clippy::too_many_lines,
    reason = "one arm per component, read top to bottom"
)]
fn emit_section(
    out: &mut String,
    facts: &FactLookup,
    today: Date,
    section: &Section,
) -> Result<(), BuildError> {
    let id = match section_id(section) {
        Some(id) => format!(" id=\"{}\"", esc(id)),
        None => String::new(),
    };
    match section {
        Section::Hero {
            heading: h,
            subheading,
            action: a,
            ..
        } => {
            let _ = write!(
                out,
                "<section{id} class=\"hero\">{}<div class=\"sub\">",
                heading(h, 1)
            );
            if let Some(sub) = subheading {
                let _ = write!(out, "<p>{}</p>", esc(text_of(sub)));
            }
            out.push_str("</div>");
            if let Some(a) = a {
                out.push_str(&action(facts, today, a, "cta")?);
            }
            out.push_str("</section>");
        }
        Section::Services {
            heading: h, items, ..
        } => {
            let _ = write!(
                out,
                "<section{id}>{}<ul class=\"services\">",
                opt_heading(h.as_ref(), 2)
            );
            for item in items {
                service_item(out, facts, today, item)?;
            }
            out.push_str("</ul></section>");
        }
        Section::Testimonials {
            heading: h, items, ..
        } => {
            let _ = write!(out, "<section{id}>{}", opt_heading(h.as_ref(), 2));
            for item in items {
                let _ = write!(
                    out,
                    "<blockquote>{}</blockquote>",
                    fact_ref(facts, today, item)?
                );
            }
            out.push_str("</section>");
        }
        Section::Trust {
            heading: h, items, ..
        } => {
            let _ = write!(
                out,
                "<section{id}>{}<ul class=\"trust\">",
                opt_heading(h.as_ref(), 2)
            );
            for item in items {
                let _ = write!(out, "<li>{}</li>", fact_ref(facts, today, item)?);
            }
            out.push_str("</ul></section>");
        }
        Section::ServiceArea {
            heading: h, fact, ..
        }
        | Section::Hours {
            heading: h, fact, ..
        } => {
            let _ = write!(
                out,
                "<section{id}>{}<p>{}</p></section>",
                opt_heading(h.as_ref(), 2),
                fact_ref(facts, today, fact)?
            );
        }
        Section::Contact {
            heading: h,
            facts: contact,
            actions,
            ..
        } => {
            let _ = write!(out, "<section{id}>{}", opt_heading(h.as_ref(), 2));
            for f in contact {
                let statement = fact_ref(facts, today, f)?;
                let record = facts.get(&f.fact).ok_or(BuildError::UnknownFact(f.fact))?;
                match &record.link {
                    Some(link) => {
                        let _ = write!(
                            out,
                            "<p><a href=\"{}\">{}</a></p>",
                            esc(&link.href()),
                            statement
                        );
                    }
                    None => {
                        let _ = write!(out, "<p>{statement}</p>");
                    }
                }
            }
            for a in actions {
                out.push_str(&action(facts, today, a, "contact")?);
            }
            out.push_str("</section>");
        }
        Section::QuoteForm {
            heading: h,
            services,
            consent_notice,
            submit_label,
            ..
        } => {
            let _ = write!(out, "<section{id}>{}", opt_heading(h.as_ref(), 2));
            out.push_str(&quote_form(
                services,
                consent_notice,
                submit_label.as_deref(),
            ));
            out.push_str("</section>");
        }
        Section::Text {
            heading: h, body, ..
        } => {
            let _ = write!(
                out,
                "<section{id}>{}<p>{}</p></section>",
                opt_heading(h.as_ref(), 2),
                esc(text_of(body))
            );
        }
        Section::Footer { legal, links, .. } => {
            let _ = write!(out, "<footer{id}><ul>");
            for f in legal {
                let _ = write!(out, "<li>{}</li>", fact_ref(facts, today, f)?);
            }
            out.push_str("</ul>");
            for a in links {
                out.push_str(&action(facts, today, a, "footer")?);
            }
            out.push_str("</footer>");
        }
    }
    Ok(())
}

fn service_item(
    out: &mut String,
    facts: &FactLookup,
    today: Date,
    item: &ServiceItem,
) -> Result<(), BuildError> {
    let _ = write!(out, "<li><strong>{}</strong>", esc(text_of(&item.name)));
    if let Some(description) = &item.description {
        let _ = write!(out, "<p>{}</p>", esc(text_of(description)));
    }
    if let Some(price) = &item.price {
        let _ = write!(
            out,
            "<p class=\"price\">{}</p>",
            fact_ref(facts, today, price)?
        );
    }
    out.push_str("</li>");
    Ok(())
}

fn section_id(section: &Section) -> Option<&str> {
    match section {
        Section::Hero { id, .. }
        | Section::Services { id, .. }
        | Section::Testimonials { id, .. }
        | Section::Trust { id, .. }
        | Section::ServiceArea { id, .. }
        | Section::Hours { id, .. }
        | Section::Contact { id, .. }
        | Section::QuoteForm { id, .. }
        | Section::Text { id, .. }
        | Section::Footer { id, .. } => id.as_deref(),
    }
}

fn render_page(page: &Page, facts: &FactLookup, today: Date) -> Result<String, BuildError> {
    let mut out = String::from(PROLOGUE);
    let _ = write!(
        out,
        "<title>{}</title><meta name=\"description\" content=\"{}\">\
         <link rel=\"stylesheet\" href=\"/{STYLE_PATH}\"></head><body>",
        esc(text_of(&page.title)),
        esc(text_of(&page.description))
    );
    for section in &page.sections {
        emit_section(&mut out, facts, today, section)?;
    }
    out.push_str("</body></html>");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        brief::Brief,
        fact::{FactKind, FactRecord},
        page_tree::{BoundText, COMPONENT_SET, PageTree, SpecContext},
    };
    use time::Month;

    fn day() -> Date {
        Date::from_calendar_date(2026, Month::November, 25).unwrap()
    }

    fn brief(claims: &[FactId]) -> Brief {
        let claims: Vec<String> = claims.iter().map(|f| f.as_uuid().to_string()).collect();
        serde_json::from_value::<crate::brief::BriefForm>(serde_json::json!({
            "business_name": "Quickbeam Heating",
            "goal": "quote_requests",
            "intent": "More enquiries",
            "audience": "Home owners",
            "services": [{ "name": "Boiler service" }],
            "domains": ["quickbeam.example"],
            "references": [],
            "claims": claims
        }))
        .unwrap()
        .validate()
        .unwrap()
    }

    /// The only door to [`build`]: `validate` must mint the tree clean.
    fn vtree_with(tree: PageTree, facts: &FactLookup, brief: &Brief) -> ValidatedTree {
        ValidatedTree::validate(
            tree,
            SpecContext {
                brief,
                facts,
                today: day(),
            },
        )
        .unwrap_or_else(|problems| panic!("fixture should validate: {problems:?}"))
    }

    fn vtree(tree: PageTree, facts: &FactLookup) -> ValidatedTree {
        vtree_with(tree, facts, &brief(&[]))
    }

    fn fact(id: FactId, kind: FactKind, statement: &str) -> FactRecord {
        FactRecord {
            id,
            kind,
            statement: statement.into(),
            link: None,
            approval: Some((day(), day().saturating_add(time::Duration::days(365)))),
            withdrawn: false,
        }
    }

    fn lookup(facts: Vec<FactRecord>) -> FactLookup {
        facts.into_iter().map(|f| (f.id, f)).collect()
    }

    // Two pages — a one-file bundle would canonicalize trivially and could not
    // prove ordering is by the bytes, not luck.
    fn tree() -> PageTree {
        serde_json::from_value(serde_json::json!({
            "component_set": COMPONENT_SET,
            "pages": [
                {
                    "path": "/",
                    "title": "Raeburn Plumbing",
                    "description": "Boiler and plumbing, Edinburgh",
                    "sections": [
                        { "component": "hero", "heading": "Raeburn Plumbing",
                          "subheading": "Plumbing and heating in Edinburgh",
                          "action": { "label": "Get a quote", "target": { "anchor": "quote" } } },
                        { "component": "quote_form", "id": "quote",
                          "services": ["Boiler service"], "consent_notice": "privacy-v1",
                          "submit_label": "Send" }
                    ]
                },
                {
                    "path": "/services",
                    "title": "Services",
                    "description": "What we do",
                    "sections": [
                        { "component": "services",
                          "items": [{ "name": "Boiler service",
                                      "description": "Annual check and clean" }] }
                    ]
                }
            ]
        }))
        .unwrap()
    }

    #[test]
    fn the_same_input_builds_byte_identical_bundles() {
        let facts = lookup(vec![]);
        let a = build(&vtree(tree(), &facts), &facts, day()).unwrap();
        let b = build(&vtree(tree(), &facts), &facts, day()).unwrap();
        assert_eq!(a.encode(), b.encode());
        assert_eq!(a.hash(), b.hash());
        // Every page carries the fixed prologue — the check's byte-for-byte bar.
        for file in a.files().values() {
            assert!(
                file.starts_with(PROLOGUE.as_bytes()) || file == STYLE.as_bytes(),
                "a bundle file is a prologue page or the stylesheet"
            );
        }
    }

    #[test]
    fn a_javascript_target_is_unbuildable() {
        let mut t = tree();
        let Section::Hero { action, .. } = &mut t.pages[0].sections[0] else {
            panic!("hero");
        };
        *action = Some(Action {
            label: "x".into(),
            target: Target::Url("javascript:alert(1)".into()),
        });
        let facts = lookup(vec![]);
        let brief = brief(&[]);
        assert!(
            ValidatedTree::validate(
                t,
                SpecContext {
                    brief: &brief,
                    facts: &facts,
                    today: day()
                }
            )
            .is_err(),
            "a javascript: target must fail validation — the builder never sees it"
        );
    }

    #[test]
    fn fact_slots_are_filled_and_escaped() {
        let id = FactId::from_uuid(uuid::Uuid::from_u128(7));
        let mut t = tree();
        t.pages[0].sections.insert(
            1,
            Section::Testimonials {
                id: None,
                heading: None,
                items: vec![FactRef { fact: id }],
            },
        );
        let facts = lookup(vec![fact(
            id,
            FactKind::Testimonial,
            "Fixed it <fast> & well",
        )]);
        let brief = brief(&[id]);
        let bundle = build(&vtree_with(t, &facts, &brief), &facts, day()).unwrap();
        let html = String::from_utf8(bundle.files()["index.html"].clone()).unwrap();
        assert!(html.contains("<blockquote>Fixed it &lt;fast&gt; &amp; well</blockquote>"));
        assert!(
            !html.contains("<svg"),
            "the baseline never emits inline svg"
        );
    }

    #[test]
    fn unapproved_or_absent_facts_are_refused() {
        let id = FactId::from_uuid(uuid::Uuid::from_u128(7));
        let mut t = tree();
        t.pages[0].sections.push(Section::Hours {
            id: None,
            heading: None,
            fact: FactRef { fact: id },
        });
        let brief = brief(&[]);
        // Absent entirely — validation refuses the tree, so the build door
        // is never reached. The builder's `BuildError` remains as the second
        // line of defense for any caller that outlives its validation.
        let empty = lookup(vec![]);
        assert!(
            ValidatedTree::validate(
                t.clone(),
                SpecContext {
                    brief: &brief,
                    facts: &empty,
                    today: day()
                }
            )
            .is_err()
        );
        // Proposed, never approved.
        let mut proposed = fact(id, FactKind::OpeningHours, "Mon–Fri");
        proposed.approval = None;
        let proposed = lookup(vec![proposed]);
        assert!(
            ValidatedTree::validate(
                t,
                SpecContext {
                    brief: &brief,
                    facts: &proposed,
                    today: day()
                }
            )
            .is_err()
        );
    }

    #[test]
    fn tree_text_cannot_inject_markup() {
        let mut t = tree();
        t.pages[0].title = Text::Bound(BoundText {
            text: "O'Malley & Sons \"Plumbing\"".into(),
            facts: vec![],
        });
        let facts = lookup(vec![]);
        let bundle = build(&vtree(t, &facts), &facts, day()).unwrap();
        let html = String::from_utf8(bundle.files()["index.html"].clone()).unwrap();
        assert!(html.contains("O&#x27;Malley &amp; Sons &quot;Plumbing&quot;"));
    }
}
