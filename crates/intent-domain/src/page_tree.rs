//! The typed page tree (D-002, D-058): what a page may say, as structured data over a fixed
//! component vocabulary — never free HTML.
//!
//! The vocabulary is `quote-request@0`, the first site pattern's components as a specification.
//! Rendering them is the second half of I1 and waits for the category (B-1); validation does
//! not. [`validate`] enforces, against the brief and the tenant's facts on a given day:
//!
//! - every claim slot references a fact of the right kind that the brief lists, approved and
//!   current (D-013);
//! - every link is structural (D-014): an in-page anchor, a page of the tree, a contact fact's
//!   link, or an `https` address on one of the brief's domains;
//! - free text passes the claim lint unless a fact bound to the slot supports each flagged token
//!   ([`crate::claims`]);
//! - bounds on pages, sections, items and text, unique ids and paths, a home page with a quote
//!   form offering only the brief's services.
//!
//! Shape errors (unknown fields, wrong types) are found while parsing; the server reports their
//! path. Everything else is a [`Problem`] here.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use time::Date;

use crate::{
    brief::Brief,
    claims,
    fact::{FactKind, FactLookup},
    ids::FactId,
    problem::{Problem, ProblemCode, Problems, field, index},
};

/// The one component set this build has.
pub const COMPONENT_SET: &str = "quote-request@0";

/// Most pages in a tree.
pub const PAGES_MAX: usize = 5;
/// Most sections on a page.
pub const SECTIONS_MAX: usize = 20;
/// Longest page title.
pub const TITLE_MAX_CHARS: usize = 70;
/// Longest page description.
pub const PAGE_DESCRIPTION_MAX_CHARS: usize = 160;
/// Longest heading.
pub const HEADING_MAX_CHARS: usize = 120;
/// Longest subheading or item description.
pub const SUBHEADING_MAX_CHARS: usize = 300;
/// Longest body text.
pub const BODY_MAX_CHARS: usize = 2_000;
/// Longest button or link label.
pub const LABEL_MAX_CHARS: usize = 40;
/// Most facts bound to one text slot.
pub const BOUND_FACTS_MAX: usize = 4;

/// Free text, optionally bound to facts that support the claims it makes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Text {
    /// Text with no bound fact: it may make no claim.
    Plain(String),
    /// Text with bound facts.
    Bound(BoundText),
}

/// Text and the facts that support it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundText {
    /// The text.
    pub text: String,
    /// Facts whose statements support the tokens the claim lint flags.
    #[serde(default)]
    pub facts: Vec<FactId>,
}

impl Text {
    fn parts(&self) -> (&str, &[FactId]) {
        match self {
            Self::Plain(text) => (text, &[]),
            Self::Bound(bound) => (&bound.text, &bound.facts),
        }
    }
}

/// A claim slot: a reference to a fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactRef {
    /// The fact.
    pub fact: FactId,
}

/// Where a link may go (D-014). There is no "any URL" target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    /// A section id on the same page.
    Anchor(String),
    /// A page of this tree, by path.
    Page(String),
    /// A contact fact's phone, e-mail or web address.
    Fact(FactId),
    /// An `https` address on one of the brief's domains.
    Url(String),
}

/// A button or link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    /// What it says.
    pub label: String,
    /// Where it goes.
    pub target: Target,
}

/// One service on the services section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceItem {
    /// Its name.
    pub name: Text,
    /// What it involves.
    #[serde(default)]
    pub description: Option<Text>,
    /// Its price: a price fact.
    #[serde(default)]
    pub price: Option<FactRef>,
}

/// One section of a page: a component of `quote-request@0` with its slots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "component", rename_all = "snake_case", deny_unknown_fields)]
pub enum Section {
    /// The opening: a heading, a line under it, one action.
    Hero {
        /// Section id, for anchors.
        #[serde(default)]
        id: Option<String>,
        /// The main heading.
        heading: Text,
        /// The line under it.
        #[serde(default)]
        subheading: Option<Text>,
        /// The main button.
        #[serde(default)]
        action: Option<Action>,
    },
    /// What the business offers.
    Services {
        /// Section id.
        #[serde(default)]
        id: Option<String>,
        /// Heading.
        #[serde(default)]
        heading: Option<Text>,
        /// 1–20 services.
        items: Vec<ServiceItem>,
    },
    /// Customers' words: testimonial facts only.
    Testimonials {
        /// Section id.
        #[serde(default)]
        id: Option<String>,
        /// Heading.
        #[serde(default)]
        heading: Option<Text>,
        /// 1–6 testimonial facts.
        items: Vec<FactRef>,
    },
    /// Reasons to trust: metrics, licences, certifications, insurance, guarantees.
    Trust {
        /// Section id.
        #[serde(default)]
        id: Option<String>,
        /// Heading.
        #[serde(default)]
        heading: Option<Text>,
        /// 1–8 facts.
        items: Vec<FactRef>,
    },
    /// Where the business works.
    ServiceArea {
        /// Section id.
        #[serde(default)]
        id: Option<String>,
        /// Heading.
        #[serde(default)]
        heading: Option<Text>,
        /// A service-area fact.
        fact: FactRef,
    },
    /// When it is open.
    Hours {
        /// Section id.
        #[serde(default)]
        id: Option<String>,
        /// Heading.
        #[serde(default)]
        heading: Option<Text>,
        /// An opening-hours fact.
        fact: FactRef,
    },
    /// How to get in touch.
    Contact {
        /// Section id.
        #[serde(default)]
        id: Option<String>,
        /// Heading.
        #[serde(default)]
        heading: Option<Text>,
        /// 1–4 contact facts.
        facts: Vec<FactRef>,
        /// Up to 3 actions.
        #[serde(default)]
        actions: Vec<Action>,
    },
    /// The quote request form: posts to the site's lead endpoint.
    QuoteForm {
        /// Section id (usually the hero's anchor target).
        #[serde(default)]
        id: Option<String>,
        /// Heading.
        #[serde(default)]
        heading: Option<Text>,
        /// The services the form offers: names from the brief.
        #[serde(default)]
        services: Vec<String>,
        /// The privacy-notice version the form shows (D-044).
        consent_notice: String,
        /// The submit button's label.
        #[serde(default)]
        submit_label: Option<String>,
    },
    /// A heading and a paragraph.
    Text {
        /// Section id.
        #[serde(default)]
        id: Option<String>,
        /// Heading.
        #[serde(default)]
        heading: Option<Text>,
        /// The paragraph.
        body: Text,
    },
    /// The page's end: legal promises and links.
    Footer {
        /// Section id.
        #[serde(default)]
        id: Option<String>,
        /// Up to 4 legal-promise facts.
        #[serde(default)]
        legal: Vec<FactRef>,
        /// Up to 6 links.
        #[serde(default)]
        links: Vec<Action>,
    },
}

impl Section {
    fn id(&self) -> Option<&str> {
        match self {
            Self::Hero { id, .. }
            | Self::Services { id, .. }
            | Self::Testimonials { id, .. }
            | Self::Trust { id, .. }
            | Self::ServiceArea { id, .. }
            | Self::Hours { id, .. }
            | Self::Contact { id, .. }
            | Self::QuoteForm { id, .. }
            | Self::Text { id, .. }
            | Self::Footer { id, .. } => id.as_deref(),
        }
    }
}

/// One page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    /// `/` or `/<lowercase-slug>`.
    pub path: String,
    /// The `<title>`.
    pub title: Text,
    /// The meta description.
    pub description: Text,
    /// 1–20 sections.
    pub sections: Vec<Section>,
}

/// A whole site's page tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageTree {
    /// Must be [`COMPONENT_SET`].
    pub component_set: String,
    /// 1–5 pages, one of them `/`.
    pub pages: Vec<Page>,
}

/// What [`validate`] needs besides the tree.
#[derive(Debug, Clone, Copy)]
pub struct SpecContext<'a> {
    /// The brief the tree was built from.
    pub brief: &'a Brief,
    /// The tenant's facts (another tenant's are absent and read as unknown).
    pub facts: &'a FactLookup,
    /// The server's calendar day.
    pub today: Date,
}

impl PageTree {
    /// Every fact the tree references, deduplicated: what the caller must load.
    pub fn referenced_facts(&self) -> Vec<FactId> {
        let mut ids = Vec::new();
        for page in &self.pages {
            ids.extend_from_slice(page.title.parts().1);
            ids.extend_from_slice(page.description.parts().1);
            for section in &page.sections {
                visit_section(section, &mut |slot| match slot {
                    Slot::Text(t) => ids.extend_from_slice(t.parts().1),
                    Slot::Fact(r) => ids.push(r.fact),
                    Slot::Target(Target::Fact(id)) => ids.push(*id),
                    Slot::Target(_) => {}
                });
            }
        }
        let mut seen = HashSet::new();
        ids.retain(|id| seen.insert(*id));
        ids
    }
}

enum Slot<'a> {
    Text(&'a Text),
    Fact(&'a FactRef),
    Target(&'a Target),
}

const TRUST_KINDS: &[FactKind] = &[
    FactKind::Metric,
    FactKind::Licence,
    FactKind::Certification,
    FactKind::Insurance,
    FactKind::Guarantee,
];

fn visit_section<'a>(section: &'a Section, visit: &mut dyn FnMut(Slot<'a>)) {
    let opt = |t: &'a Option<Text>, visit: &mut dyn FnMut(Slot<'a>)| {
        if let Some(t) = t {
            visit(Slot::Text(t));
        }
    };
    match section {
        Section::Hero {
            heading,
            subheading,
            action,
            ..
        } => {
            visit(Slot::Text(heading));
            opt(subheading, visit);
            if let Some(action) = action {
                visit(Slot::Target(&action.target));
            }
        }
        Section::Services { heading, items, .. } => {
            opt(heading, visit);
            for item in items {
                visit(Slot::Text(&item.name));
                opt(&item.description, visit);
                if let Some(price) = &item.price {
                    visit(Slot::Fact(price));
                }
            }
        }
        Section::Testimonials { heading, items, .. } | Section::Trust { heading, items, .. } => {
            opt(heading, visit);
            for item in items {
                visit(Slot::Fact(item));
            }
        }
        Section::ServiceArea { heading, fact, .. } | Section::Hours { heading, fact, .. } => {
            opt(heading, visit);
            visit(Slot::Fact(fact));
        }
        Section::Contact {
            heading,
            facts,
            actions,
            ..
        } => {
            opt(heading, visit);
            for fact in facts {
                visit(Slot::Fact(fact));
            }
            for action in actions {
                visit(Slot::Target(&action.target));
            }
        }
        Section::QuoteForm { heading, .. } => opt(heading, visit),
        Section::Text { heading, body, .. } => {
            opt(heading, visit);
            visit(Slot::Text(body));
        }
        Section::Footer { legal, links, .. } => {
            for fact in legal {
                visit(Slot::Fact(fact));
            }
            for link in links {
                visit(Slot::Target(&link.target));
            }
        }
    }
}

/// A tree [`validate`] approved with zero problems — the only form a builder
/// may consume, so an unacceptable link target is unbuildable, not escaped
/// into bytes (I2-2). The field is private: nothing outside this module can
/// mint one.
#[derive(Debug, Clone)]
pub struct ValidatedTree(PageTree);

impl ValidatedTree {
    /// The only constructor: `tree` under `ctx`, problem-free. The problems
    /// come back as the error, unchanged.
    pub fn validate(tree: PageTree, ctx: SpecContext<'_>) -> Result<Self, Vec<Problem>> {
        let problems = validate(&tree, ctx);
        if problems.is_empty() {
            Ok(Self(tree))
        } else {
            Err(problems)
        }
    }

    /// The approved tree.
    pub fn tree(&self) -> &PageTree {
        &self.0
    }

    /// Unwraps back to the raw tree — for callers that record it, never for
    /// building around the guarantee.
    pub fn into_tree(self) -> PageTree {
        self.0
    }
}

/// Every problem with `tree` under `ctx`; empty when it validates.
pub fn validate(tree: &PageTree, ctx: SpecContext<'_>) -> Vec<Problem> {
    let mut v = Validator {
        ctx,
        problems: Problems::new(),
        brief_claims: ctx.brief.claims.iter().copied().collect(),
    };
    v.tree(tree);
    v.problems.into_vec()
}

struct Validator<'a> {
    ctx: SpecContext<'a>,
    problems: Problems,
    brief_claims: HashSet<FactId>,
}

/// A section or page id: 1–40 of `a-z 0-9 -`.
fn is_slug(text: &str) -> bool {
    (1..=40).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `/` or `/<slug>`.
fn is_page_path(path: &str) -> bool {
    path == "/" || path.strip_prefix('/').is_some_and(is_slug)
}

impl Validator<'_> {
    fn tree(&mut self, tree: &PageTree) {
        if tree.component_set != COMPONENT_SET {
            self.problems
                .add("component_set", ProblemCode::UnknownComponentSet);
            return;
        }
        if tree.pages.is_empty() {
            self.problems.add("pages", ProblemCode::TooFew);
            return;
        }
        if tree.pages.len() > PAGES_MAX {
            self.problems.add("pages", ProblemCode::TooMany);
            return;
        }
        let mut paths = HashSet::new();
        for (i, page) in tree.pages.iter().enumerate() {
            let at = field(&index("pages", i), "path");
            if !is_page_path(&page.path) {
                self.problems.add(at, ProblemCode::InvalidFormat);
            } else if !paths.insert(page.path.as_str()) {
                self.problems.add(at, ProblemCode::Duplicate);
            }
        }
        match tree.pages.iter().position(|p| p.path == "/") {
            None => self.problems.add("pages", ProblemCode::MissingHome),
            Some(home) => {
                let has_form = tree.pages[home]
                    .sections
                    .iter()
                    .any(|s| matches!(s, Section::QuoteForm { .. }));
                if !has_form {
                    self.problems.add(
                        field(&index("pages", home), "sections"),
                        ProblemCode::MissingQuoteForm,
                    );
                }
            }
        }
        for (i, page) in tree.pages.iter().enumerate() {
            self.page(&index("pages", i), page, &paths);
        }
    }

    fn page(&mut self, path: &str, page: &Page, pages: &HashSet<&str>) {
        self.text(&field(path, "title"), &page.title, TITLE_MAX_CHARS, true);
        self.text(
            &field(path, "description"),
            &page.description,
            PAGE_DESCRIPTION_MAX_CHARS,
            true,
        );
        let sections_path = field(path, "sections");
        if page.sections.is_empty() {
            self.problems.add(&sections_path, ProblemCode::TooFew);
            return;
        }
        if page.sections.len() > SECTIONS_MAX {
            self.problems.add(&sections_path, ProblemCode::TooMany);
            return;
        }
        let mut ids: HashMap<&str, usize> = HashMap::new();
        for (i, section) in page.sections.iter().enumerate() {
            if let Some(id) = section.id() {
                let at = field(&index(&sections_path, i), "id");
                if !is_slug(id) {
                    self.problems.add(at, ProblemCode::InvalidFormat);
                } else if ids.insert(id, i).is_some() {
                    self.problems.add(at, ProblemCode::Duplicate);
                }
            }
        }
        let anchors: HashSet<&str> = ids.keys().copied().collect();
        for (i, section) in page.sections.iter().enumerate() {
            self.section(&index(&sections_path, i), section, &anchors, pages);
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one arm per component, read top to bottom"
    )]
    fn section(
        &mut self,
        path: &str,
        section: &Section,
        anchors: &HashSet<&str>,
        pages: &HashSet<&str>,
    ) {
        match section {
            Section::Hero {
                heading,
                subheading,
                action,
                ..
            } => {
                self.text(&field(path, "heading"), heading, HEADING_MAX_CHARS, true);
                self.opt_text(
                    &field(path, "subheading"),
                    subheading.as_ref(),
                    SUBHEADING_MAX_CHARS,
                );
                if let Some(action) = action {
                    self.action(&field(path, "action"), action, anchors, pages);
                }
            }
            Section::Services { heading, items, .. } => {
                self.opt_text(&field(path, "heading"), heading.as_ref(), HEADING_MAX_CHARS);
                let items_path = field(path, "items");
                if self.count(&items_path, items.len(), 1, 20) {
                    for (i, item) in items.iter().enumerate() {
                        let at = index(&items_path, i);
                        self.text(&field(&at, "name"), &item.name, HEADING_MAX_CHARS, true);
                        self.opt_text(
                            &field(&at, "description"),
                            item.description.as_ref(),
                            SUBHEADING_MAX_CHARS,
                        );
                        if let Some(price) = &item.price {
                            self.fact(&field(&at, "price"), price.fact, &[FactKind::Price]);
                        }
                    }
                }
            }
            Section::Testimonials { heading, items, .. } => {
                self.opt_text(&field(path, "heading"), heading.as_ref(), HEADING_MAX_CHARS);
                self.facts(&field(path, "items"), items, 1, 6, &[FactKind::Testimonial]);
            }
            Section::Trust { heading, items, .. } => {
                self.opt_text(&field(path, "heading"), heading.as_ref(), HEADING_MAX_CHARS);
                self.facts(&field(path, "items"), items, 1, 8, TRUST_KINDS);
            }
            Section::ServiceArea { heading, fact, .. } => {
                self.opt_text(&field(path, "heading"), heading.as_ref(), HEADING_MAX_CHARS);
                self.fact(&field(path, "fact"), fact.fact, &[FactKind::ServiceArea]);
            }
            Section::Hours { heading, fact, .. } => {
                self.opt_text(&field(path, "heading"), heading.as_ref(), HEADING_MAX_CHARS);
                self.fact(&field(path, "fact"), fact.fact, &[FactKind::OpeningHours]);
            }
            Section::Contact {
                heading,
                facts,
                actions,
                ..
            } => {
                self.opt_text(&field(path, "heading"), heading.as_ref(), HEADING_MAX_CHARS);
                self.facts(
                    &field(path, "facts"),
                    facts,
                    1,
                    4,
                    &[FactKind::ContactDetails],
                );
                let actions_path = field(path, "actions");
                if self.count(&actions_path, actions.len(), 0, 3) {
                    for (i, action) in actions.iter().enumerate() {
                        self.action(&index(&actions_path, i), action, anchors, pages);
                    }
                }
            }
            Section::QuoteForm {
                heading,
                services,
                consent_notice,
                submit_label,
                ..
            } => {
                self.opt_text(&field(path, "heading"), heading.as_ref(), HEADING_MAX_CHARS);
                let services_path = field(path, "services");
                if self.count(&services_path, services.len(), 0, 20) {
                    let mut seen = HashSet::new();
                    for (i, name) in services.iter().enumerate() {
                        let at = index(&services_path, i);
                        if !self.ctx.brief.offers(name) {
                            self.problems.add(at, ProblemCode::ServiceUnknown);
                        } else if !seen.insert(name.to_lowercase()) {
                            self.problems.add(at, ProblemCode::Duplicate);
                        }
                    }
                }
                let notice_ok = (1..=crate::limits::CONSENT_NOTICE_MAX_CHARS)
                    .contains(&consent_notice.len())
                    && consent_notice.bytes().all(|b| {
                        b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-')
                    });
                if !notice_ok {
                    self.problems
                        .add(field(path, "consent_notice"), ProblemCode::InvalidFormat);
                }
                if let Some(label) = submit_label {
                    self.label(&field(path, "submit_label"), label);
                }
            }
            Section::Text { heading, body, .. } => {
                self.opt_text(&field(path, "heading"), heading.as_ref(), HEADING_MAX_CHARS);
                self.text(&field(path, "body"), body, BODY_MAX_CHARS, true);
            }
            Section::Footer { legal, links, .. } => {
                self.facts(
                    &field(path, "legal"),
                    legal,
                    0,
                    4,
                    &[FactKind::LegalPromise],
                );
                let links_path = field(path, "links");
                if self.count(&links_path, links.len(), 0, 6) {
                    for (i, link) in links.iter().enumerate() {
                        self.action(&index(&links_path, i), link, anchors, pages);
                    }
                }
            }
        }
    }

    /// Records a problem unless `min <= len <= max`; returns whether the items should be read.
    fn count(&mut self, path: &str, len: usize, min: usize, max: usize) -> bool {
        if len < min {
            self.problems.add(path, ProblemCode::TooFew);
            false
        } else if len > max {
            self.problems.add(path, ProblemCode::TooMany);
            false
        } else {
            true
        }
    }

    fn facts(&mut self, path: &str, items: &[FactRef], min: usize, max: usize, kinds: &[FactKind]) {
        if self.count(path, items.len(), min, max) {
            let mut seen = HashSet::new();
            for (i, item) in items.iter().enumerate() {
                let at = index(path, i);
                if seen.insert(item.fact) {
                    self.fact(&at, item.fact, kinds);
                } else {
                    self.problems.add(at, ProblemCode::Duplicate);
                }
            }
        }
    }

    /// The fact exists for the tenant, is in the brief, is approved and current, and has an
    /// allowed kind. Returns it when it passed.
    fn fact(&mut self, path: &str, id: FactId, kinds: &[FactKind]) -> bool {
        let Some(fact) = self.ctx.facts.get(&id) else {
            self.problems.add(path, ProblemCode::FactUnknown);
            return false;
        };
        if !self.brief_claims.contains(&id) {
            self.problems.add(path, ProblemCode::FactNotInBrief);
            return false;
        }
        if let Some(code) = fact.status(self.ctx.today).refusal() {
            self.problems.add(path, code);
            return false;
        }
        if !kinds.contains(&fact.kind) {
            self.problems.add(path, ProblemCode::FactKindMismatch);
            return false;
        }
        true
    }

    fn opt_text(&mut self, path: &str, text: Option<&Text>, max: usize) {
        if let Some(text) = text {
            self.text(path, text, max, true);
        }
    }

    /// Length, characters, the bound facts, then the claim lint against them.
    fn text(&mut self, path: &str, text: &Text, max: usize, required: bool) {
        let (body, bound) = text.parts();
        let at = match text {
            Text::Plain(_) => path.to_owned(),
            Text::Bound(_) => field(path, "text"),
        };
        match crate::problem::text(Some(body), max, max > SUBHEADING_MAX_CHARS) {
            Ok(None) => {
                if required {
                    self.problems.add(at, ProblemCode::Required);
                }
                return;
            }
            Ok(Some(_)) => {}
            Err(code) => {
                self.problems.add(at, code);
                return;
            }
        }
        let facts_path = field(path, "facts");
        if bound.len() > BOUND_FACTS_MAX {
            self.problems.add(facts_path, ProblemCode::TooMany);
            return;
        }
        let mut statements = Vec::new();
        let mut all_usable = true;
        for (i, id) in bound.iter().enumerate() {
            if self.fact(&index(&facts_path, i), *id, &FactKind::ALL)
                && let Some(fact) = self.ctx.facts.get(id)
            {
                statements.push(fact.statement.as_str());
            } else {
                all_usable = false;
            }
        }
        // A bound fact that cannot be used is the problem to fix first; linting against the
        // rest would report the same claim twice.
        if !all_usable {
            return;
        }
        for finding in claims::unbound(body, statements) {
            self.problems.push(Problem::with_detail(
                at.clone(),
                ProblemCode::UnboundClaim,
                &finding.token,
            ));
        }
    }

    /// A label is plain text that may make no claim.
    fn label(&mut self, path: &str, label: &str) {
        self.text(path, &Text::Plain(label.to_owned()), LABEL_MAX_CHARS, true);
    }

    fn action(
        &mut self,
        path: &str,
        action: &Action,
        anchors: &HashSet<&str>,
        pages: &HashSet<&str>,
    ) {
        self.label(&field(path, "label"), &action.label);
        let at = field(path, "target");
        match &action.target {
            Target::Anchor(anchor) => {
                if !anchors.contains(anchor.as_str()) {
                    self.problems.add(at, ProblemCode::AnchorUnknown);
                }
            }
            Target::Page(page) => {
                if !pages.contains(page.as_str()) {
                    self.problems.add(at, ProblemCode::PageUnknown);
                }
            }
            Target::Fact(id) => {
                if self.fact(&at, *id, &[FactKind::ContactDetails])
                    && self.ctx.facts.get(id).is_some_and(|f| f.link.is_none())
                {
                    self.problems.add(at, ProblemCode::FactHasNoLink);
                }
            }
            Target::Url(url) => {
                if !self.own_domain(url) {
                    self.problems.add(at, ProblemCode::LinkNotAllowed);
                }
            }
        }
    }

    /// An `https` address, without credentials, whose host is exactly one of the brief's
    /// domains.
    fn own_domain(&self, raw: &str) -> bool {
        let Ok(url) = url::Url::parse(raw) else {
            return false;
        };
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && url
                .host_str()
                .is_some_and(|host| self.ctx.brief.domains.iter().any(|d| d == host))
    }
}

#[cfg(test)]
mod tests;
