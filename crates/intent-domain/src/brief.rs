//! The brief (guide §Design and facts, D-057): the client's intent, audience, offer, approved
//! claims, brand and reference permissions, in one validated record.
//!
//! [`BriefForm`] is the wire shape, every field optional so a missing one is a named
//! [`Problem`] rather than a serde message. [`BriefForm::validate`] checks shape and bounds;
//! [`Brief::check_claims`] checks the approved claims against the tenant's facts on a given
//! day. A validated [`Brief`] serializes back to exactly the form's shape, so a stored brief is
//! re-read through the same validation and never trusted as-is.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use time::Date;

use crate::{
    fact::FactLookup,
    ids::{AssetId, FactId},
    problem::{Problem, ProblemCode, Problems, checked_text, field, index},
};

/// Longest business name.
pub const BUSINESS_NAME_MAX_CHARS: usize = 120;
/// Longest intent and audience descriptions.
pub const DESCRIPTION_MAX_CHARS: usize = 1_000;
/// Most services a brief offers.
pub const SERVICES_MAX: usize = 20;
/// Longest service name.
pub const SERVICE_NAME_MAX_CHARS: usize = 80;
/// Longest service description.
pub const SERVICE_DESCRIPTION_MAX_CHARS: usize = 500;
/// Longest brand tone note.
pub const TONE_MAX_CHARS: usize = 200;
/// Most brand colours.
pub const COLORS_MAX: usize = 6;
/// Most client domains.
pub const DOMAINS_MAX: usize = 5;
/// Most references.
pub const REFERENCES_MAX: usize = 10;
/// Longest reference URL.
pub const REFERENCE_URL_MAX_CHARS: usize = 2_048;
/// Longest reference notes.
pub const NOTES_MAX_CHARS: usize = 2_000;
/// Most approved claims (fact ids) on one brief.
pub const CLAIMS_MAX: usize = 50;

/// What the site is for. The first release has one pattern (D-009).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Goal {
    /// Qualified quote requests through the site's form.
    QuoteRequests,
}

impl Goal {
    /// The wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::QuoteRequests => "quote_requests",
        }
    }
}

/// What the client allows us to do with a reference (D-057).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// Look only: layout, tone, structure. Nothing is copied from it.
    InspirationOnly,
    /// The client's own site: its text and images may be reused.
    OwnContent,
    /// Images may be reused under the licence the notes name.
    LicensedAssets,
}

impl Permission {
    /// The wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InspirationOnly => "inspiration_only",
            Self::OwnContent => "own_content",
            Self::LicensedAssets => "licensed_assets",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        [
            Self::InspirationOnly,
            Self::OwnContent,
            Self::LicensedAssets,
        ]
        .into_iter()
        .find(|p| p.as_str() == text)
    }
}

/// A service as submitted.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceForm {
    /// What it is called ("Boiler service").
    pub name: Option<String>,
    /// What it involves.
    pub description: Option<String>,
}

/// The brand as submitted.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrandForm {
    /// How the site should sound ("plain, friendly, no jargon").
    pub tone: Option<String>,
    /// `#rrggbb` colours, most important first.
    pub colors: Option<Vec<String>>,
    /// An uploaded asset to use as the logo.
    pub logo: Option<String>,
}

/// A reference as submitted.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceForm {
    /// The page's address (`http` or `https`).
    pub url: Option<String>,
    /// One of [`Permission`]'s wire names.
    pub permission: Option<String>,
    /// What to take from it, and for licensed assets, the licence.
    pub notes: Option<String>,
}

/// A brief as submitted.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BriefForm {
    /// The business, as the site should name it.
    pub business_name: Option<String>,
    /// `quote_requests`.
    pub goal: Option<String>,
    /// What the client wants the site to achieve, in their words.
    pub intent: Option<String>,
    /// Who the site is for.
    pub audience: Option<String>,
    /// What the business offers; at least one.
    pub services: Option<Vec<ServiceForm>>,
    /// Tone, colours, logo.
    pub brand: Option<BrandForm>,
    /// The client's own domains; the only web addresses a page may link to besides contact
    /// facts (D-014).
    pub domains: Option<Vec<String>>,
    /// Sites the client pointed at, each with what we may do with it.
    pub references: Option<Vec<ReferenceForm>>,
    /// The approved facts this brief may use: every claim a page makes comes from these.
    pub claims: Option<Vec<String>>,
}

/// One service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Service {
    /// Its name.
    pub name: String,
    /// Its description, if given.
    pub description: Option<String>,
}

/// The brand.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Brand {
    /// The tone note.
    pub tone: Option<String>,
    /// Lowercase `#rrggbb` colours.
    pub colors: Vec<String>,
    /// The logo asset.
    pub logo: Option<AssetId>,
}

/// One reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Reference {
    /// The normalized address.
    pub url: String,
    /// What we may do with it.
    pub permission: Permission,
    /// The notes.
    pub notes: Option<String>,
}

/// A brief whose every field passed validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Brief {
    /// The business name.
    pub business_name: String,
    /// What the site is for.
    pub goal: Goal,
    /// The client's intent.
    pub intent: String,
    /// The audience.
    pub audience: String,
    /// The services, names unique ignoring case.
    pub services: Vec<Service>,
    /// The brand.
    pub brand: Brand,
    /// Lowercase host names.
    pub domains: Vec<String>,
    /// The references.
    pub references: Vec<Reference>,
    /// The approved claims.
    pub claims: Vec<FactId>,
}

impl BriefForm {
    /// Applies every shape and bound rule; reports every problem. Whether the claims are
    /// approved facts of the tenant is [`Brief::check_claims`].
    pub fn validate(self) -> Result<Brief, Vec<Problem>> {
        let mut problems = Problems::new();
        let business_name = checked_text(
            &mut problems,
            "business_name",
            self.business_name.as_deref(),
            BUSINESS_NAME_MAX_CHARS,
            false,
            true,
        );
        let goal = match self.goal.as_deref().map(str::trim) {
            None | Some("") => {
                problems.add("goal", ProblemCode::Required);
                None
            }
            Some("quote_requests") => Some(Goal::QuoteRequests),
            Some(_) => {
                problems.add("goal", ProblemCode::UnknownValue);
                None
            }
        };
        let intent = checked_text(
            &mut problems,
            "intent",
            self.intent.as_deref(),
            DESCRIPTION_MAX_CHARS,
            true,
            true,
        );
        let audience = checked_text(
            &mut problems,
            "audience",
            self.audience.as_deref(),
            DESCRIPTION_MAX_CHARS,
            true,
            true,
        );

        let mut services = Vec::new();
        let forms = self.services.unwrap_or_default();
        if forms.is_empty() {
            problems.add("services", ProblemCode::TooFew);
        } else if forms.len() > SERVICES_MAX {
            problems.add("services", ProblemCode::TooMany);
        } else {
            let mut names = HashSet::new();
            for (i, form) in forms.into_iter().enumerate() {
                let path = index("services", i);
                let name = checked_text(
                    &mut problems,
                    &field(&path, "name"),
                    form.name.as_deref(),
                    SERVICE_NAME_MAX_CHARS,
                    false,
                    true,
                );
                let description = checked_text(
                    &mut problems,
                    &field(&path, "description"),
                    form.description.as_deref(),
                    SERVICE_DESCRIPTION_MAX_CHARS,
                    true,
                    false,
                );
                if let Some(name) = name {
                    if !names.insert(name.to_lowercase()) {
                        problems.add(field(&path, "name"), ProblemCode::Duplicate);
                    }
                    services.push(Service { name, description });
                }
            }
        }

        let brand = brand(&mut problems, self.brand.unwrap_or_default());
        let domains = domains(&mut problems, &self.domains.unwrap_or_default());
        let references = references(&mut problems, self.references.unwrap_or_default());
        let claims = claims(&mut problems, &self.claims.unwrap_or_default());

        match (business_name, goal, intent, audience) {
            (Some(business_name), Some(goal), Some(intent), Some(audience))
                if problems.is_empty() =>
            {
                Ok(Brief {
                    business_name,
                    goal,
                    intent,
                    audience,
                    services,
                    brand,
                    domains,
                    references,
                    claims,
                })
            }
            _ => Err(problems.into_vec()),
        }
    }
}

fn brand(problems: &mut Problems, form: BrandForm) -> Brand {
    let tone = checked_text(
        problems,
        "brand.tone",
        form.tone.as_deref(),
        TONE_MAX_CHARS,
        false,
        false,
    );
    let mut colors = Vec::new();
    let given = form.colors.unwrap_or_default();
    if given.len() > COLORS_MAX {
        problems.add("brand.colors", ProblemCode::TooMany);
    } else {
        for (i, raw) in given.iter().enumerate() {
            let color = raw.trim().to_ascii_lowercase();
            let shaped = color.len() == 7
                && color.starts_with('#')
                && color[1..].bytes().all(|b| b.is_ascii_hexdigit());
            if !shaped {
                problems.add(index("brand.colors", i), ProblemCode::InvalidFormat);
            } else if colors.contains(&color) {
                problems.add(index("brand.colors", i), ProblemCode::Duplicate);
            } else {
                colors.push(color);
            }
        }
    }
    let logo = match form.logo.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(text) => {
            let parsed = text.parse::<AssetId>().ok();
            if parsed.is_none() {
                problems.add("brand.logo", ProblemCode::InvalidFormat);
            }
            parsed
        }
    };
    Brand { tone, colors, logo }
}

/// A host name: lowercase ASCII (international names in their `xn--` form), at least two
/// labels, each 1–63 of `a-z 0-9 -` not starting or ending with `-`, at most 253 in all, and
/// not an IP address.
pub fn host_name(raw: &str) -> Option<String> {
    let host = raw.trim().trim_end_matches('.').to_ascii_lowercase();
    let labels: Vec<&str> = host.split('.').collect();
    let shaped = host.len() <= 253
        && labels.len() >= 2
        && labels.iter().all(|label| {
            (1..=63).contains(&label.len())
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        // The last label of a real name is never all digits; this also refuses `10.0.0.1`.
        && labels
            .last()
            .is_some_and(|tld| !tld.bytes().all(|b| b.is_ascii_digit()));
    shaped.then_some(host)
}

fn domains(problems: &mut Problems, given: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    if given.len() > DOMAINS_MAX {
        problems.add("domains", ProblemCode::TooMany);
        return out;
    }
    for (i, raw) in given.iter().enumerate() {
        match host_name(raw) {
            None => problems.add(index("domains", i), ProblemCode::InvalidFormat),
            Some(host) if out.contains(&host) => {
                problems.add(index("domains", i), ProblemCode::Duplicate);
            }
            Some(host) => out.push(host),
        }
    }
    out
}

fn references(problems: &mut Problems, given: Vec<ReferenceForm>) -> Vec<Reference> {
    let mut out = Vec::new();
    if given.len() > REFERENCES_MAX {
        problems.add("references", ProblemCode::TooMany);
        return out;
    }
    for (i, form) in given.into_iter().enumerate() {
        let path = index("references", i);
        let url = match form.url.as_deref().map(str::trim) {
            None | Some("") => {
                problems.add(field(&path, "url"), ProblemCode::Required);
                None
            }
            Some(raw) => match reference_url(raw) {
                Ok(url) => Some(url),
                Err(code) => {
                    problems.add(field(&path, "url"), code);
                    None
                }
            },
        };
        let permission = match form.permission.as_deref().map(str::trim) {
            None | Some("") => {
                problems.add(field(&path, "permission"), ProblemCode::Required);
                None
            }
            Some(text) => {
                let permission = Permission::parse(text);
                if permission.is_none() {
                    problems.add(field(&path, "permission"), ProblemCode::UnknownValue);
                }
                permission
            }
        };
        let notes = checked_text(
            problems,
            &field(&path, "notes"),
            form.notes.as_deref(),
            NOTES_MAX_CHARS,
            true,
            // Licensed assets must say which licence.
            permission == Some(Permission::LicensedAssets),
        );
        if let (Some(url), Some(permission)) = (url, permission) {
            if out.iter().any(|r: &Reference| r.url == url) {
                problems.add(field(&path, "url"), ProblemCode::Duplicate);
            }
            out.push(Reference {
                url,
                permission,
                notes,
            });
        }
    }
    out
}

/// An absolute `http` or `https` address with a host and no credentials, normalized. Whether
/// it may be fetched is the egress guard's decision (D-012), not the brief's.
pub fn reference_url(raw: &str) -> Result<String, ProblemCode> {
    if raw.chars().count() > REFERENCE_URL_MAX_CHARS {
        return Err(ProblemCode::TooLong);
    }
    let parsed = url::Url::parse(raw).map_err(|_| ProblemCode::InvalidFormat)?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none_or(str::is_empty)
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(ProblemCode::InvalidFormat);
    }
    Ok(parsed.to_string())
}

fn claims(problems: &mut Problems, given: &[String]) -> Vec<FactId> {
    let mut out = Vec::new();
    if given.len() > CLAIMS_MAX {
        problems.add("claims", ProblemCode::TooMany);
        return out;
    }
    for (i, raw) in given.iter().enumerate() {
        match raw.trim().parse::<FactId>() {
            Err(_) => problems.add(index("claims", i), ProblemCode::InvalidFormat),
            Ok(id) if out.contains(&id) => problems.add(index("claims", i), ProblemCode::Duplicate),
            Ok(id) => out.push(id),
        }
    }
    out
}

impl Brief {
    /// The claims that are not approved, current facts of this tenant on `today`: each is a
    /// problem at `claims[i]`. `facts` holds only the tenant's facts, so another tenant's fact
    /// reads as unknown.
    pub fn check_claims(&self, facts: &FactLookup, today: Date) -> Vec<Problem> {
        let mut problems = Problems::new();
        for (i, id) in self.claims.iter().enumerate() {
            let code = match facts.get(id) {
                None => Some(ProblemCode::FactUnknown),
                Some(fact) => fact.status(today).refusal(),
            };
            if let Some(code) = code {
                problems.add(index("claims", i), code);
            }
        }
        problems.into_vec()
    }

    /// Whether `name` is one of the brief's services (ignoring case).
    pub fn offers(&self, name: &str) -> bool {
        self.services
            .iter()
            .any(|s| s.name.to_lowercase() == name.trim().to_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fact::{FactKind, FactRecord, parse_date};

    pub(crate) fn sample() -> BriefForm {
        BriefForm {
            business_name: Some("Lee & Daughters Heating".into()),
            goal: Some("quote_requests".into()),
            intent: Some("More boiler-service quote requests from nearby homes.".into()),
            audience: Some("Home owners within 10 miles.".into()),
            services: Some(vec![
                ServiceForm {
                    name: Some("Boiler service".into()),
                    description: Some("Annual check and clean.".into()),
                },
                ServiceForm {
                    name: Some("Boiler repair".into()),
                    description: None,
                },
            ]),
            brand: Some(BrandForm {
                tone: Some("Plain and friendly".into()),
                colors: Some(vec!["#1F4E79".into()]),
                logo: None,
            }),
            domains: Some(vec!["LeeHeating.example".into()]),
            references: Some(vec![ReferenceForm {
                url: Some("https://old-site.example/".into()),
                permission: Some("own_content".into()),
                notes: Some("Their current site: reuse the photos.".into()),
            }]),
            claims: Some(vec![]),
        }
    }

    fn codes(result: Result<Brief, Vec<Problem>>) -> Vec<(String, ProblemCode)> {
        result
            .expect_err("expected problems")
            .into_iter()
            .map(|p| (p.path, p.code))
            .collect()
    }

    #[test]
    fn a_valid_brief_is_normalized() {
        let brief = sample().validate().unwrap();
        assert_eq!(brief.domains, vec!["leeheating.example"]);
        assert_eq!(brief.brand.colors, vec!["#1f4e79"]);
        assert_eq!(brief.references[0].url, "https://old-site.example/");
        assert!(brief.offers("boiler SERVICE"));
    }

    #[test]
    fn an_empty_brief_names_every_missing_part() {
        assert_eq!(
            codes(BriefForm::default().validate()),
            vec![
                ("business_name".into(), ProblemCode::Required),
                ("goal".into(), ProblemCode::Required),
                ("intent".into(), ProblemCode::Required),
                ("audience".into(), ProblemCode::Required),
                ("services".into(), ProblemCode::TooFew),
            ]
        );
    }

    #[test]
    fn every_nested_problem_has_its_path() {
        let mut form = sample();
        form.goal = Some("bookings".into());
        form.services.as_mut().unwrap()[1].name = Some("boiler service".into());
        form.brand.as_mut().unwrap().colors = Some(vec!["blue".into()]);
        form.brand.as_mut().unwrap().logo = Some("not-an-id".into());
        form.domains = Some(vec!["10.0.0.1".into(), "localhost".into()]);
        form.references = Some(vec![
            ReferenceForm {
                url: Some("ftp://x.example/".into()),
                permission: Some("anything".into()),
                notes: None,
            },
            ReferenceForm {
                url: Some("https://user:pw@x.example/".into()),
                permission: Some("licensed_assets".into()),
                notes: None,
            },
        ]);
        form.claims = Some(vec!["nope".into()]);
        assert_eq!(
            codes(form.validate()),
            vec![
                ("goal".into(), ProblemCode::UnknownValue),
                ("services[1].name".into(), ProblemCode::Duplicate),
                ("brand.colors[0]".into(), ProblemCode::InvalidFormat),
                ("brand.logo".into(), ProblemCode::InvalidFormat),
                ("domains[0]".into(), ProblemCode::InvalidFormat),
                ("domains[1]".into(), ProblemCode::InvalidFormat),
                ("references[0].url".into(), ProblemCode::InvalidFormat),
                ("references[0].permission".into(), ProblemCode::UnknownValue),
                ("references[1].url".into(), ProblemCode::InvalidFormat),
                ("references[1].notes".into(), ProblemCode::Required),
                ("claims[0]".into(), ProblemCode::InvalidFormat),
            ]
        );
    }

    #[test]
    fn claims_must_be_approved_current_facts_of_the_tenant() {
        let today = parse_date("2026-11-17").unwrap();
        let fact = |n: u128, approval: Option<(&str, &str)>, withdrawn: bool| FactRecord {
            id: FactId::from_uuid(uuid::Uuid::from_u128(n)),
            kind: FactKind::Testimonial,
            statement: "Great work".into(),
            link: None,
            approval: approval.map(|(a, b)| (parse_date(a).unwrap(), parse_date(b).unwrap())),
            withdrawn,
        };
        let facts: FactLookup = [
            fact(1, Some(("2026-11-01", "2027-01-30")), false),
            fact(2, None, false),
            fact(3, Some(("2026-06-01", "2026-08-30")), false),
            fact(4, Some(("2026-11-01", "2027-01-30")), true),
        ]
        .into_iter()
        .map(|f| (f.id, f))
        .collect();
        let mut form = sample();
        form.claims = Some(
            (1..=5)
                .map(|n| uuid::Uuid::from_u128(n).to_string())
                .collect(),
        );
        let brief = form.validate().unwrap();
        let found: Vec<_> = brief
            .check_claims(&facts, today)
            .into_iter()
            .map(|p| (p.path, p.code))
            .collect();
        assert_eq!(
            found,
            vec![
                ("claims[1]".into(), ProblemCode::FactUnapproved),
                ("claims[2]".into(), ProblemCode::FactStale),
                ("claims[3]".into(), ProblemCode::FactWithdrawn),
                ("claims[4]".into(), ProblemCode::FactUnknown),
            ]
        );
    }

    #[test]
    fn a_stored_brief_reads_back_through_the_same_validation() {
        let brief = sample().validate().unwrap();
        let stored = serde_json::to_string(&brief).unwrap();
        let again: BriefForm = serde_json::from_str(&stored).unwrap();
        assert_eq!(again.validate().unwrap(), brief);
    }

    #[test]
    fn unknown_fields_are_refused() {
        assert!(serde_json::from_str::<BriefForm>(r#"{"business_name":"x","extra":1}"#).is_err());
    }
}
