//! The page-tree validator over a fixture brief and fixture facts, with the clock passed in.

use serde_json::json;
use uuid::Uuid;

use super::*;
use crate::{
    brief::BriefForm,
    fact::{FactLink, FactRecord, parse_date},
};

fn id(n: u128) -> FactId {
    FactId::from_uuid(Uuid::from_u128(n))
}

const REVIEW: u128 = 1;
const PRICE: u128 = 2;
const INSURED: u128 = 3;
const AREA: u128 = 4;
const HOURS: u128 = 5;
const PHONE: u128 = 6;
const ADDRESS: u128 = 7; // contact details without a link
const PROPOSED: u128 = 8;
const WITHDRAWN: u128 = 9;
const NOT_IN_BRIEF: u128 = 10;
const OTHER_TENANT: u128 = 11; // never in the lookup

fn today() -> Date {
    parse_date("2026-11-17").unwrap()
}

fn facts() -> FactLookup {
    let approved = Some((
        parse_date("2026-11-10").unwrap(),
        parse_date("2026-12-10").unwrap(),
    ));
    let fact = |n, kind, statement: &str, link, approval, withdrawn| FactRecord {
        id: id(n),
        kind,
        statement: statement.into(),
        link,
        approval,
        withdrawn,
    };
    [
        fact(
            REVIEW,
            FactKind::Testimonial,
            "\"Fixed our boiler the same day.\" — Sam, Leeds",
            None,
            approved,
            false,
        ),
        fact(
            PRICE,
            FactKind::Price,
            "Boiler service from £80",
            None,
            approved,
            false,
        ),
        fact(
            INSURED,
            FactKind::Insurance,
            "Fully insured: £5,000,000 public liability",
            None,
            approved,
            false,
        ),
        fact(
            AREA,
            FactKind::ServiceArea,
            "Leeds and 10 miles around",
            None,
            approved,
            false,
        ),
        fact(
            HOURS,
            FactKind::OpeningHours,
            "Mon–Fri 8:00–18:00",
            None,
            approved,
            false,
        ),
        fact(
            PHONE,
            FactKind::ContactDetails,
            "Call 0113 496 0000",
            Some(FactLink::Tel("01134960000".into())),
            approved,
            false,
        ),
        fact(
            ADDRESS,
            FactKind::ContactDetails,
            "1 High Street, Leeds",
            None,
            approved,
            false,
        ),
        fact(
            PROPOSED,
            FactKind::Testimonial,
            "Not yet approved",
            None,
            None,
            false,
        ),
        fact(
            WITHDRAWN,
            FactKind::Testimonial,
            "Withdrawn",
            None,
            approved,
            true,
        ),
        fact(
            NOT_IN_BRIEF,
            FactKind::Testimonial,
            "Approved but not listed",
            None,
            approved,
            false,
        ),
    ]
    .into_iter()
    .map(|f| (f.id, f))
    .collect()
}

fn brief() -> Brief {
    let mut form: BriefForm = serde_json::from_value(json!({
        "business_name": "Lee Heating",
        "goal": "quote_requests",
        "intent": "More quote requests",
        "audience": "Home owners in Leeds",
        "services": [{ "name": "Boiler service" }, { "name": "Boiler repair" }],
        "domains": ["leeheating.example"],
    }))
    .unwrap();
    form.claims = Some(
        [
            REVIEW, PRICE, INSURED, AREA, HOURS, PHONE, ADDRESS, PROPOSED, WITHDRAWN,
        ]
        .into_iter()
        .map(|n| Uuid::from_u128(n).to_string())
        .collect(),
    );
    form.validate().unwrap()
}

fn fact_ref(n: u128) -> serde_json::Value {
    json!({ "fact": Uuid::from_u128(n) })
}

/// A tree that validates; each test breaks one thing.
fn valid() -> serde_json::Value {
    json!({
        "component_set": "quote-request@0",
        "pages": [{
            "path": "/",
            "title": "Lee Heating — boiler service in Leeds",
            "description": "Friendly local heating engineers.",
            "sections": [
                { "component": "hero", "id": "top", "heading": "Warm homes, honest prices",
                  "subheading": { "text": "Fully insured engineers", "facts": [Uuid::from_u128(INSURED)] },
                  "action": { "label": "Get a quote", "target": { "anchor": "quote" } } },
                { "component": "services", "id": "services", "items": [
                    { "name": "Boiler service", "price": fact_ref(PRICE) },
                    { "name": "Boiler repair", "description": "We find the fault and fix it." }
                ] },
                { "component": "testimonials", "heading": "What customers say", "items": [fact_ref(REVIEW)] },
                { "component": "trust", "items": [fact_ref(INSURED)] },
                { "component": "service_area", "fact": fact_ref(AREA) },
                { "component": "hours", "fact": fact_ref(HOURS) },
                { "component": "contact", "facts": [fact_ref(PHONE), fact_ref(ADDRESS)],
                  "actions": [{ "label": "Call us", "target": { "fact": Uuid::from_u128(PHONE) } }] },
                { "component": "quote_form", "id": "quote", "heading": "Ask for a quote",
                  "services": ["Boiler service", "boiler repair"], "consent_notice": "privacy-v1" },
                { "component": "footer", "links": [
                    { "label": "About us", "target": { "page": "/about" } },
                    { "label": "Our blog", "target": { "url": "https://leeheating.example/blog" } }
                ] }
            ]
        }, {
            "path": "/about",
            "title": "About Lee Heating",
            "description": "Who we are.",
            "sections": [{ "component": "text", "heading": "About us", "body": "A family business.\nWe answer the phone." }]
        }]
    })
}

fn check_at(tree: serde_json::Value, today: Date) -> Vec<(String, ProblemCode)> {
    let tree: PageTree = serde_json::from_value(tree).expect("the tree parses");
    let brief = brief();
    let facts = facts();
    validate(
        &tree,
        SpecContext {
            brief: &brief,
            facts: &facts,
            today,
        },
    )
    .into_iter()
    .map(|p| (p.path, p.code))
    .collect()
}

fn check(tree: serde_json::Value) -> Vec<(String, ProblemCode)> {
    check_at(tree, today())
}

fn set(tree: &mut serde_json::Value, pointer: &str, value: serde_json::Value) {
    *tree
        .pointer_mut(pointer)
        .unwrap_or_else(|| panic!("no {pointer}")) = value;
}

#[test]
fn the_fixture_tree_validates() {
    assert_eq!(check(valid()), vec![]);
}

#[test]
fn a_claim_slot_needs_an_approved_current_fact_the_brief_lists() {
    let cases = [
        (PROPOSED, ProblemCode::FactUnapproved),
        (WITHDRAWN, ProblemCode::FactWithdrawn),
        (NOT_IN_BRIEF, ProblemCode::FactNotInBrief),
        (OTHER_TENANT, ProblemCode::FactUnknown),
        (PRICE, ProblemCode::FactKindMismatch),
    ];
    for (fact, code) in cases {
        let mut tree = valid();
        set(&mut tree, "/pages/0/sections/2/items/0", fact_ref(fact));
        assert_eq!(
            check(tree),
            vec![("pages[0].sections[2].items[0]".into(), code)],
            "fact {fact}"
        );
    }
}

#[test]
fn the_same_tree_goes_stale_when_the_clock_passes_review_by() {
    // Every fact in the fixture lapses after 2026-12-10: the tree that validated on 17 November
    // fails on 11 December, at every slot that uses one.
    assert_eq!(check_at(valid(), parse_date("2026-12-10").unwrap()), vec![]);
    let stale = check_at(valid(), parse_date("2026-12-11").unwrap());
    assert_ne!(stale, vec![]);
    assert!(
        stale
            .iter()
            .all(|(_, code)| *code == ProblemCode::FactStale),
        "{stale:?}"
    );
    assert!(stale.contains(&(
        "pages[0].sections[1].items[0].price".into(),
        ProblemCode::FactStale
    )));
    assert!(stale.contains(&(
        "pages[0].sections[0].subheading.facts[0]".into(),
        ProblemCode::FactStale
    )));
}

#[test]
fn links_go_only_where_the_allowlist_says() {
    let cases = [
        (
            json!({ "url": "https://elsewhere.example/" }),
            ProblemCode::LinkNotAllowed,
        ),
        (
            json!({ "url": "http://leeheating.example/" }),
            ProblemCode::LinkNotAllowed,
        ),
        (
            json!({ "url": "https://leeheating.example.evil.example/" }),
            ProblemCode::LinkNotAllowed,
        ),
        (
            json!({ "url": "https://user@leeheating.example/" }),
            ProblemCode::LinkNotAllowed,
        ),
        (
            json!({ "url": "javascript:alert(1)" }),
            ProblemCode::LinkNotAllowed,
        ),
        (json!({ "anchor": "nowhere" }), ProblemCode::AnchorUnknown),
        (json!({ "page": "/missing" }), ProblemCode::PageUnknown),
        (
            json!({ "fact": Uuid::from_u128(ADDRESS) }),
            ProblemCode::FactHasNoLink,
        ),
        (
            json!({ "fact": Uuid::from_u128(REVIEW) }),
            ProblemCode::FactKindMismatch,
        ),
    ];
    for (target, code) in cases {
        let mut tree = valid();
        set(
            &mut tree,
            "/pages/0/sections/8/links/1/target",
            target.clone(),
        );
        assert_eq!(
            check(tree),
            vec![("pages[0].sections[8].links[1].target".into(), code)],
            "{target}"
        );
    }
}

#[test]
fn a_target_with_two_destinations_does_not_parse() {
    let mut tree = valid();
    set(
        &mut tree,
        "/pages/0/sections/8/links/1/target",
        json!({ "anchor": "top", "url": "https://elsewhere.example/" }),
    );
    assert!(serde_json::from_value::<PageTree>(tree).is_err());
}

#[test]
fn invented_claims_in_free_text_are_unbound() {
    let mut tree = valid();
    set(
        &mut tree,
        "/pages/0/sections/0/heading",
        json!("Licensed & insured since 1985"),
    );
    assert_eq!(
        check(tree.clone()),
        vec![
            (
                "pages[0].sections[0].heading".into(),
                ProblemCode::UnboundClaim
            ),
            (
                "pages[0].sections[0].heading".into(),
                ProblemCode::UnboundClaim
            ),
            (
                "pages[0].sections[0].heading".into(),
                ProblemCode::UnboundClaim
            ),
        ]
    );
    // Binding the insurance fact supports "insured" only.
    set(
        &mut tree,
        "/pages/0/sections/0/heading",
        json!({ "text": "Licensed & insured since 1985", "facts": [Uuid::from_u128(INSURED)] }),
    );
    let tree_parsed: PageTree = serde_json::from_value(tree).unwrap();
    let brief = brief();
    let facts = facts();
    let found = validate(
        &tree_parsed,
        SpecContext {
            brief: &brief,
            facts: &facts,
            today: today(),
        },
    );
    let tokens: Vec<_> = found
        .iter()
        .map(|p| (p.path.as_str(), p.detail.as_deref()))
        .collect();
    assert_eq!(
        tokens,
        vec![
            ("pages[0].sections[0].heading.text", Some("licensed")),
            ("pages[0].sections[0].heading.text", Some("1985")),
        ]
    );
    // Prices in labels and titles are claims too.
    let mut tree = valid();
    set(
        &mut tree,
        "/pages/0/sections/0/action/label",
        json!("Only $99 today"),
    );
    assert_eq!(
        check(tree),
        vec![(
            "pages[0].sections[0].action.label".into(),
            ProblemCode::UnboundClaim
        )]
    );
}

#[test]
fn structure_rules_are_named() {
    let mut tree = valid();
    set(&mut tree, "/component_set", json!("gallery@3"));
    assert_eq!(
        check(tree),
        vec![("component_set".into(), ProblemCode::UnknownComponentSet)]
    );

    let mut tree = valid();
    set(&mut tree, "/pages/0/path", json!("/home"));
    assert_eq!(
        check(tree),
        vec![("pages".into(), ProblemCode::MissingHome)]
    );

    let mut tree = valid();
    set(&mut tree, "/pages/1/path", json!("/"));
    let found = check(tree);
    assert!(
        found.contains(&("pages[1].path".into(), ProblemCode::Duplicate)),
        "{found:?}"
    );

    let mut tree = valid();
    tree["pages"][0]["sections"]
        .as_array_mut()
        .unwrap()
        .remove(7);
    assert_eq!(
        check(tree),
        vec![
            ("pages[0].sections".into(), ProblemCode::MissingQuoteForm),
            // The hero's anchor pointed at the form.
            (
                "pages[0].sections[0].action.target".into(),
                ProblemCode::AnchorUnknown
            ),
        ]
    );

    let mut tree = valid();
    set(&mut tree, "/pages/0/sections/1/id", json!("top"));
    assert_eq!(
        check(tree),
        vec![("pages[0].sections[1].id".into(), ProblemCode::Duplicate)]
    );

    let mut tree = valid();
    set(
        &mut tree,
        "/pages/0/sections/7/services/1",
        json!("Roof repair"),
    );
    assert_eq!(
        check(tree),
        vec![(
            "pages[0].sections[7].services[1]".into(),
            ProblemCode::ServiceUnknown
        )]
    );

    let mut tree = valid();
    set(
        &mut tree,
        "/pages/0/sections/7/consent_notice",
        json!("privacy notice!"),
    );
    assert_eq!(
        check(tree),
        vec![(
            "pages[0].sections[7].consent_notice".into(),
            ProblemCode::InvalidFormat
        )]
    );

    let mut tree = valid();
    set(&mut tree, "/pages/0/sections/2/items", json!([]));
    assert_eq!(
        check(tree),
        vec![("pages[0].sections[2].items".into(), ProblemCode::TooFew)]
    );

    let mut tree = valid();
    let page = tree["pages"][1].clone();
    let pages = tree["pages"].as_array_mut().unwrap();
    for n in 0..5 {
        let mut extra = page.clone();
        extra["path"] = json!(format!("/extra-{n}"));
        pages.push(extra);
    }
    assert_eq!(check(tree), vec![("pages".into(), ProblemCode::TooMany)]);

    let mut tree = valid();
    set(&mut tree, "/pages/0/title", json!("x".repeat(71)));
    assert_eq!(
        check(tree),
        vec![("pages[0].title".into(), ProblemCode::TooLong)]
    );

    let mut tree = valid();
    set(&mut tree, "/pages/0/title", json!("Hidden\u{202E}text"));
    assert_eq!(
        check(tree),
        vec![("pages[0].title".into(), ProblemCode::InvalidCharacters)]
    );
}

#[test]
fn unknown_components_and_fields_do_not_parse() {
    for broken in [
        json!({ "component": "carousel", "items": [] }),
        json!({ "component": "hero", "heading": "Hi", "script": "alert(1)" }),
        json!({ "component": "text", "body": { "text": "Hi", "html": "<b>x</b>" } }),
    ] {
        let mut tree = valid();
        set(&mut tree, "/pages/1/sections/0", broken.clone());
        assert!(
            serde_json::from_value::<PageTree>(tree).is_err(),
            "{broken}"
        );
    }
}

#[test]
fn referenced_facts_lists_each_once() {
    let tree: PageTree = serde_json::from_value(valid()).unwrap();
    let ids = tree.referenced_facts();
    assert_eq!(
        ids,
        vec![
            id(INSURED),
            id(PRICE),
            id(REVIEW),
            id(AREA),
            id(HOURS),
            id(PHONE),
            id(ADDRESS)
        ]
    );
}
