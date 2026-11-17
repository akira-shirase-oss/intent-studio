//! Fact records (D-013, D-056): every factual claim a page may make is one of these.
//!
//! A fact's content — kind, statement, source and contact link — never changes once recorded; a
//! corrected statement is a new fact. What changes is its approval: who approved it on the
//! client's behalf, on which day, and the review-by date after which it is stale and blocks
//! promotion. A withdrawn fact stays on record and can no longer be used.
//!
//! Dates are calendar days (UTC), passed in: the domain has no clock.

use std::fmt;

use serde::{Deserialize, Serialize};
use time::{Date, Month};

use crate::{
    ids::FactId,
    problem::{ProblemCode, Problems, checked_text, field},
};

/// Longest fact statement (the text a page shows).
pub const STATEMENT_MAX_CHARS: usize = 1_000;
/// Longest source note (where the fact came from).
pub const SOURCE_MAX_CHARS: usize = 500;
/// Longest approver name.
pub const APPROVER_MAX_CHARS: usize = 200;
/// Longest web address a contact fact may carry.
pub const URL_MAX_CHARS: usize = 2_048;
/// Furthest back an approval may be recorded, in days (ten years).
pub const APPROVAL_MAX_AGE_DAYS: i64 = 3_650;

/// What a fact asserts (D-013). The kind decides which page slots it may fill and how long an
/// approval lasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactKind {
    /// A customer's words, with who said them.
    Testimonial,
    /// A number about the business ("over 500 boilers fitted").
    Metric,
    /// A price or a price range.
    Price,
    /// A promise about the work ("12-month guarantee on parts").
    Guarantee,
    /// A licence or registration.
    Licence,
    /// A certification or accreditation.
    Certification,
    /// Insurance cover.
    Insurance,
    /// Where the business works.
    ServiceArea,
    /// When the business is open.
    OpeningHours,
    /// A phone number, e-mail or web address, possibly with a link.
    ContactDetails,
    /// A legal promise (terms, a cancellation right).
    LegalPromise,
}

impl FactKind {
    /// Every kind, in the order the workspace lists them.
    pub const ALL: [Self; 11] = [
        Self::Testimonial,
        Self::Metric,
        Self::Price,
        Self::Guarantee,
        Self::Licence,
        Self::Certification,
        Self::Insurance,
        Self::ServiceArea,
        Self::OpeningHours,
        Self::ContactDetails,
        Self::LegalPromise,
    ];

    /// The wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Testimonial => "testimonial",
            Self::Metric => "metric",
            Self::Price => "price",
            Self::Guarantee => "guarantee",
            Self::Licence => "licence",
            Self::Certification => "certification",
            Self::Insurance => "insurance",
            Self::ServiceArea => "service_area",
            Self::OpeningHours => "opening_hours",
            Self::ContactDetails => "contact_details",
            Self::LegalPromise => "legal_promise",
        }
    }

    /// Parses a wire name.
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == text)
    }

    /// The longest an approval lasts (D-013): 30 days for prices and opening hours, which change
    /// most, 90 for everything else. An operator may set an earlier review-by date, never a
    /// later one.
    pub const fn review_days(self) -> i64 {
        match self {
            Self::Price | Self::OpeningHours => 30,
            _ => 90,
        }
    }
}

impl fmt::Display for FactKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where a contact fact may link (D-014): the only external targets a page can have besides the
/// client's own domains.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactLink {
    /// A phone number (`tel:`), digits with an optional leading `+`.
    Tel(String),
    /// An e-mail address (`mailto:`).
    Mailto(String),
    /// An `https` web address (a profile page, a booking page).
    Url(String),
}

impl FactLink {
    /// `tel`, `mailto` or `url`.
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Tel(_) => "tel",
            Self::Mailto(_) => "mailto",
            Self::Url(_) => "url",
        }
    }

    /// The stored value.
    pub fn value(&self) -> &str {
        match self {
            Self::Tel(v) | Self::Mailto(v) | Self::Url(v) => v,
        }
    }

    /// Rebuilds a stored link; `None` for an unknown kind.
    pub fn from_parts(kind: &str, value: String) -> Option<Self> {
        match kind {
            "tel" => Some(Self::Tel(value)),
            "mailto" => Some(Self::Mailto(value)),
            "url" => Some(Self::Url(value)),
            _ => None,
        }
    }

    /// The `href` a page would use.
    pub fn href(&self) -> String {
        match self {
            Self::Tel(v) => format!("tel:{v}"),
            Self::Mailto(v) => format!("mailto:{v}"),
            Self::Url(v) => v.clone(),
        }
    }
}

/// A contact link as submitted: exactly one of the three.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkForm {
    /// A phone number.
    pub tel: Option<String>,
    /// An e-mail address.
    pub mailto: Option<String>,
    /// An `https` address.
    pub url: Option<String>,
}

/// A fact as submitted by the operator.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactForm {
    /// One of [`FactKind`]'s wire names.
    pub kind: Option<String>,
    /// The text a page shows.
    pub statement: Option<String>,
    /// Where it came from ("the client's e-mail of 10 November", "their insurer's certificate").
    pub source: Option<String>,
    /// Contact facts only: where a page may link.
    pub link: Option<LinkForm>,
}

/// A fact whose content passed validation, not yet stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewFact {
    /// The kind.
    pub kind: FactKind,
    /// The statement.
    pub statement: String,
    /// The source note.
    pub source: String,
    /// The contact link, for contact facts.
    pub link: Option<FactLink>,
}

impl FactForm {
    /// Applies every rule; reports every problem.
    pub fn validate(self) -> Result<NewFact, Vec<crate::problem::Problem>> {
        let mut problems = Problems::new();
        let kind = match self.kind.as_deref().map(str::trim) {
            None | Some("") => {
                problems.add("kind", ProblemCode::Required);
                None
            }
            Some(text) => {
                let kind = FactKind::parse(text);
                if kind.is_none() {
                    problems.add("kind", ProblemCode::UnknownValue);
                }
                kind
            }
        };
        let statement = checked_text(
            &mut problems,
            "statement",
            self.statement.as_deref(),
            STATEMENT_MAX_CHARS,
            true,
            true,
        );
        let source = checked_text(
            &mut problems,
            "source",
            self.source.as_deref(),
            SOURCE_MAX_CHARS,
            true,
            true,
        );
        let link = self
            .link
            .and_then(|form| link(&mut problems, "link", &form, kind));
        match (kind, statement, source) {
            (Some(kind), Some(statement), Some(source)) if problems.is_empty() => Ok(NewFact {
                kind,
                statement,
                source,
                link,
            }),
            _ => Err(problems.into_vec()),
        }
    }
}

fn link(
    problems: &mut Problems,
    path: &str,
    form: &LinkForm,
    kind: Option<FactKind>,
) -> Option<FactLink> {
    let given = [&form.tel, &form.mailto, &form.url]
        .into_iter()
        .filter(|v| v.as_deref().is_some_and(|v| !v.trim().is_empty()))
        .count();
    if given == 0 {
        return None;
    }
    if kind.is_some_and(|k| k != FactKind::ContactDetails) {
        problems.add(path, ProblemCode::LinkNotAllowed);
        return None;
    }
    if given > 1 {
        problems.add(path, ProblemCode::TooMany);
        return None;
    }
    if let Some(tel) = form.tel.as_deref() {
        if let Ok(Some(value)) = crate::lead::phone(Some(tel)) {
            // Stored as dialled: digits with an optional leading `+`.
            let plus = value.starts_with('+');
            let digits: String = value.chars().filter(char::is_ascii_digit).collect();
            return Some(FactLink::Tel(if plus {
                format!("+{digits}")
            } else {
                digits
            }));
        }
        problems.add(field(path, "tel"), ProblemCode::InvalidFormat);
        return None;
    }
    if let Some(mailto) = form.mailto.as_deref() {
        if let Ok(Some(value)) = crate::lead::email(Some(mailto)) {
            return Some(FactLink::Mailto(value));
        }
        problems.add(field(path, "mailto"), ProblemCode::InvalidFormat);
        return None;
    }
    let url = form.url.as_deref().unwrap_or_default();
    match https_url(url) {
        Ok(value) => Some(FactLink::Url(value)),
        Err(code) => {
            problems.add(field(path, "url"), code);
            None
        }
    }
}

/// An absolute `https` URL with a host, no credentials, at most [`URL_MAX_CHARS`]; returned in
/// its normalized form.
pub fn https_url(raw: &str) -> Result<String, ProblemCode> {
    let raw = raw.trim();
    if raw.chars().count() > URL_MAX_CHARS {
        return Err(ProblemCode::TooLong);
    }
    let parsed = url::Url::parse(raw).map_err(|_| ProblemCode::InvalidFormat)?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none_or(str::is_empty)
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(ProblemCode::InvalidFormat);
    }
    Ok(parsed.to_string())
}

/// An approval as submitted: who approved, on which day, and optionally an earlier review-by
/// date than the kind's default.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalForm {
    /// Who approved it on the client's side ("Pat Lee, owner").
    pub approved_by: Option<String>,
    /// `YYYY-MM-DD`.
    pub approved_on: Option<String>,
    /// `YYYY-MM-DD`; at most [`FactKind::review_days`] after `approved_on`.
    pub review_by: Option<String>,
}

/// A validated approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approval {
    /// Who approved.
    pub approved_by: String,
    /// The day they approved.
    pub approved_on: Date,
    /// The last day the approval holds.
    pub review_by: Date,
}

impl ApprovalForm {
    /// Applies every rule for a fact of `kind`, `today` being the server's calendar day.
    pub fn validate(
        self,
        kind: FactKind,
        today: Date,
    ) -> Result<Approval, Vec<crate::problem::Problem>> {
        let mut problems = Problems::new();
        let approved_by = checked_text(
            &mut problems,
            "approved_by",
            self.approved_by.as_deref(),
            APPROVER_MAX_CHARS,
            false,
            true,
        );
        let approved_on = match self.approved_on.as_deref().map(str::trim) {
            None | Some("") => {
                problems.add("approved_on", ProblemCode::Required);
                None
            }
            Some(text) => match parse_date(text) {
                None => {
                    problems.add("approved_on", ProblemCode::InvalidFormat);
                    None
                }
                Some(day) if day > today => {
                    problems.add("approved_on", ProblemCode::InFuture);
                    None
                }
                Some(day) if (today - day).whole_days() > APPROVAL_MAX_AGE_DAYS => {
                    problems.add("approved_on", ProblemCode::InvalidFormat);
                    None
                }
                Some(day) => Some(day),
            },
        };
        let latest = approved_on.map(|day| add_days(day, kind.review_days()));
        let review_by = match self.review_by.as_deref().map(str::trim) {
            None | Some("") => latest,
            Some(text) => match (parse_date(text), approved_on, latest) {
                (None, ..) => {
                    problems.add("review_by", ProblemCode::InvalidFormat);
                    None
                }
                (Some(day), Some(approved), _) if day <= approved => {
                    problems.add("review_by", ProblemCode::ReviewBeforeApproval);
                    None
                }
                (Some(day), _, Some(latest)) if day > latest => {
                    problems.add("review_by", ProblemCode::ReviewTooLate);
                    None
                }
                (Some(day), ..) => Some(day),
            },
        };
        match (approved_by, approved_on, review_by) {
            (Some(approved_by), Some(approved_on), Some(review_by)) if problems.is_empty() => {
                Ok(Approval {
                    approved_by,
                    approved_on,
                    review_by,
                })
            }
            _ => Err(problems.into_vec()),
        }
    }
}

/// `day` plus `days`, saturating at the calendar's ends.
pub fn add_days(day: Date, days: i64) -> Date {
    day.checked_add(time::Duration::days(days))
        .unwrap_or(Date::MAX)
}

/// A strict `YYYY-MM-DD` (four-digit year, two-digit month and day, a real calendar date).
pub fn parse_date(text: &str) -> Option<Date> {
    let bytes = text.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let number = |range: std::ops::Range<usize>| -> Option<u16> {
        let part = text.get(range)?;
        if part.bytes().all(|b| b.is_ascii_digit()) {
            part.parse().ok()
        } else {
            None
        }
    };
    let year = i32::from(number(0..4)?);
    let month = Month::try_from(u8::try_from(number(5..7)?).ok()?).ok()?;
    let day = u8::try_from(number(8..10)?).ok()?;
    Date::from_calendar_date(year, month, day).ok()
}

/// `YYYY-MM-DD`.
pub fn format_date(day: Date) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        day.year(),
        u8::from(day.month()),
        day.day()
    )
}

/// A stored fact, as the validators see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactRecord {
    /// The fact.
    pub id: FactId,
    /// Its kind.
    pub kind: FactKind,
    /// Its statement.
    pub statement: String,
    /// Its contact link, if any.
    pub link: Option<FactLink>,
    /// The current approval: approved on, review by.
    pub approval: Option<(Date, Date)>,
    /// Whether it was withdrawn.
    pub withdrawn: bool,
}

/// Where a fact stands on a given day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactStatus {
    /// Recorded, never approved.
    Proposed,
    /// Approved and current until `review_by` (inclusive).
    Approved {
        /// The last day it holds.
        review_by: Date,
    },
    /// Approved, but `review_by` has passed: it blocks promotion until re-confirmed (D-013).
    Stale {
        /// The day it lapsed after.
        review_by: Date,
    },
    /// Withdrawn: never usable again.
    Withdrawn,
}

impl FactStatus {
    /// The wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Approved { .. } => "approved",
            Self::Stale { .. } => "stale",
            Self::Withdrawn => "withdrawn",
        }
    }

    /// Why this fact cannot back a claim today, or `None` when it can.
    pub const fn refusal(self) -> Option<ProblemCode> {
        match self {
            Self::Approved { .. } => None,
            Self::Proposed => Some(ProblemCode::FactUnapproved),
            Self::Stale { .. } => Some(ProblemCode::FactStale),
            Self::Withdrawn => Some(ProblemCode::FactWithdrawn),
        }
    }
}

impl FactRecord {
    /// The status on `today`. The review-by day itself is still current.
    pub fn status(&self, today: Date) -> FactStatus {
        if self.withdrawn {
            return FactStatus::Withdrawn;
        }
        match self.approval {
            None => FactStatus::Proposed,
            Some((_, review_by)) if today > review_by => FactStatus::Stale { review_by },
            Some((_, review_by)) => FactStatus::Approved { review_by },
        }
    }
}

/// The facts a validator may consult: one tenant's, by id. A fact of another tenant is simply
/// absent, so it reads as [`ProblemCode::FactUnknown`].
pub type FactLookup = std::collections::HashMap<FactId, FactRecord>;

#[cfg(test)]
mod tests {
    use super::*;

    fn day(text: &str) -> Date {
        parse_date(text).unwrap()
    }

    fn codes(
        result: Result<impl fmt::Debug, Vec<crate::problem::Problem>>,
    ) -> Vec<(String, ProblemCode)> {
        result
            .expect_err("expected problems")
            .into_iter()
            .map(|p| (p.path, p.code))
            .collect()
    }

    #[test]
    fn a_fact_needs_kind_statement_and_source() {
        assert_eq!(
            codes(FactForm::default().validate()),
            vec![
                ("kind".into(), ProblemCode::Required),
                ("statement".into(), ProblemCode::Required),
                ("source".into(), ProblemCode::Required),
            ]
        );
        let unknown = FactForm {
            kind: Some("rumour".into()),
            statement: Some("x".into()),
            source: Some("y".into()),
            link: None,
        };
        assert_eq!(
            codes(unknown.validate()),
            vec![("kind".into(), ProblemCode::UnknownValue)]
        );
    }

    #[test]
    fn only_contact_facts_carry_one_valid_link() {
        let form = |kind: &str, link: LinkForm| FactForm {
            kind: Some(kind.into()),
            statement: Some("Call us".into()),
            source: Some("client".into()),
            link: Some(link),
        };
        let tel = form(
            "contact_details",
            LinkForm {
                tel: Some("+44 (0)20 7946 0018".into()),
                ..LinkForm::default()
            },
        )
        .validate()
        .unwrap();
        assert_eq!(tel.link, Some(FactLink::Tel("+4402079460018".into())));
        assert_eq!(
            codes(
                form(
                    "testimonial",
                    LinkForm {
                        url: Some("https://example.test".into()),
                        ..LinkForm::default()
                    }
                )
                .validate()
            ),
            vec![("link".into(), ProblemCode::LinkNotAllowed)]
        );
        assert_eq!(
            codes(
                form(
                    "contact_details",
                    LinkForm {
                        url: Some("http://example.test".into()),
                        ..LinkForm::default()
                    }
                )
                .validate()
            ),
            vec![("link.url".into(), ProblemCode::InvalidFormat)]
        );
        assert_eq!(
            codes(
                form(
                    "contact_details",
                    LinkForm {
                        url: Some("https://user:pw@example.test/".into()),
                        ..LinkForm::default()
                    }
                )
                .validate()
            ),
            vec![("link.url".into(), ProblemCode::InvalidFormat)]
        );
        assert_eq!(
            codes(
                form(
                    "contact_details",
                    LinkForm {
                        tel: Some("12".into()),
                        mailto: Some("a@b.test".into()),
                        url: None,
                    }
                )
                .validate()
            ),
            vec![("link".into(), ProblemCode::TooMany)]
        );
    }

    #[test]
    fn approvals_last_30_or_90_days_and_never_longer() {
        let today = day("2026-11-17");
        let form = |on: &str, by: Option<&str>| ApprovalForm {
            approved_by: Some("Pat Lee, owner".into()),
            approved_on: Some(on.into()),
            review_by: by.map(str::to_owned),
        };
        let price = form("2026-11-10", None)
            .validate(FactKind::Price, today)
            .unwrap();
        assert_eq!(price.review_by, day("2026-12-10"));
        let review = form("2026-11-10", None)
            .validate(FactKind::Testimonial, today)
            .unwrap();
        assert_eq!(review.review_by, day("2027-02-08"));
        let earlier = form("2026-11-10", Some("2026-11-30"))
            .validate(FactKind::Price, today)
            .unwrap();
        assert_eq!(earlier.review_by, day("2026-11-30"));
        assert_eq!(
            codes(form("2026-11-10", Some("2026-12-11")).validate(FactKind::Price, today)),
            vec![("review_by".into(), ProblemCode::ReviewTooLate)]
        );
        assert_eq!(
            codes(form("2026-11-10", Some("2026-11-10")).validate(FactKind::Price, today)),
            vec![("review_by".into(), ProblemCode::ReviewBeforeApproval)]
        );
        assert_eq!(
            codes(form("2026-11-18", None).validate(FactKind::Price, today)),
            vec![("approved_on".into(), ProblemCode::InFuture)]
        );
        assert_eq!(
            codes(form("17/11/2026", None).validate(FactKind::Price, today)),
            vec![("approved_on".into(), ProblemCode::InvalidFormat)]
        );
    }

    #[test]
    fn a_fact_is_current_through_its_review_day_and_stale_after() {
        let mut fact = FactRecord {
            id: FactId::from_uuid(uuid::Uuid::nil()),
            kind: FactKind::Price,
            statement: "Boiler service from £80".into(),
            link: None,
            approval: None,
            withdrawn: false,
        };
        let review_by = day("2026-12-10");
        assert_eq!(fact.status(day("2026-11-17")), FactStatus::Proposed);
        fact.approval = Some((day("2026-11-10"), review_by));
        assert_eq!(
            fact.status(day("2026-12-10")),
            FactStatus::Approved { review_by }
        );
        assert_eq!(
            fact.status(day("2026-12-11")),
            FactStatus::Stale { review_by }
        );
        assert_eq!(
            fact.status(day("2026-12-11")).refusal(),
            Some(ProblemCode::FactStale)
        );
        fact.withdrawn = true;
        assert_eq!(fact.status(day("2026-11-17")), FactStatus::Withdrawn);
    }

    #[test]
    fn dates_are_strict() {
        assert_eq!(format_date(day("2028-02-29")), "2028-02-29");
        for bad in [
            "2027-02-29",
            "2026-13-01",
            "2026-1-01",
            "+2026-01-01",
            "2026-01-01T00",
            "",
        ] {
            assert!(parse_date(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn kinds_round_trip() {
        for kind in FactKind::ALL {
            assert_eq!(FactKind::parse(kind.as_str()), Some(kind));
        }
    }
}
