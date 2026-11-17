//! Named problems for the studio's records (briefs, facts, page trees), and the shared text rule.
//!
//! A lead's refusals name one of a fixed set of form fields ([`crate::lead::Field`]); a brief or
//! a page tree is nested, so each [`Problem`] carries a path (`services[2].name`) and a stable
//! [`ProblemCode`]. A problem never carries a submitted value, except where naming it is the
//! point and it is the operator's own text: the token an unbound claim was found in (bounded,
//! [`DETAIL_MAX_CHARS`]).

use std::fmt::{self, Write as _};

/// Most problems reported for one record; past it the list ends with a [`ProblemCode::TooManyProblems`].
pub const MAX_PROBLEMS: usize = 100;

/// Longest detail kept on a problem.
pub const DETAIL_MAX_CHARS: usize = 40;

/// Why part of a record was refused. Stable snake-case codes: clients branch on them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ProblemCode {
    /// The JSON did not have the expected shape at this path (wrong type, unknown field).
    Malformed,
    /// Required and missing or blank.
    Required,
    /// Longer than the limit.
    TooLong,
    /// Shorter than the minimum.
    TooShort,
    /// More items than the limit.
    TooMany,
    /// Fewer items than the minimum.
    TooFew,
    /// Control, bidirectional or invisible characters, or characters outside an id's set.
    InvalidCharacters,
    /// Not in the expected format (a date, a colour, a host name, a URL, a phone number).
    InvalidFormat,
    /// Not one of the allowed values.
    UnknownValue,
    /// The same value twice where values must be unique.
    Duplicate,
    /// A date that lies in the future where only the past makes sense.
    InFuture,
    /// A review-by date on or before the approval date.
    ReviewBeforeApproval,
    /// A review-by date later than the kind allows (D-013).
    ReviewTooLate,
    /// No such fact for this tenant.
    FactUnknown,
    /// The fact has not been approved.
    FactUnapproved,
    /// The fact's review-by date has passed (D-013).
    FactStale,
    /// The fact was withdrawn.
    FactWithdrawn,
    /// The fact's kind cannot fill this slot.
    FactKindMismatch,
    /// The page uses a fact the brief does not list among its approved claims.
    FactNotInBrief,
    /// A link target that is a fact without a phone, e-mail or web address.
    FactHasNoLink,
    /// A link to somewhere the allowlist does not include (D-014).
    LinkNotAllowed,
    /// An in-page link to a section id that does not exist on the page.
    AnchorUnknown,
    /// A link to a page path the tree does not have.
    PageUnknown,
    /// A quote form offering a service the brief does not list.
    ServiceUnknown,
    /// The tree has no home page (`/`).
    MissingHome,
    /// The home page has no quote form (the quote-request pattern).
    MissingQuoteForm,
    /// The page tree names a component set this build does not have.
    UnknownComponentSet,
    /// Free text makes a claim (a number, a price, a percentage, a year, a phrase such as
    /// "licensed") that no bound fact supports (D-013).
    UnboundClaim,
    /// No such asset for this tenant.
    AssetUnknown,
    /// The list was cut at [`MAX_PROBLEMS`].
    TooManyProblems,
}

impl ProblemCode {
    /// The wire code.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Malformed => "malformed",
            Self::Required => "required",
            Self::TooLong => "too_long",
            Self::TooShort => "too_short",
            Self::TooMany => "too_many",
            Self::TooFew => "too_few",
            Self::InvalidCharacters => "invalid_characters",
            Self::InvalidFormat => "invalid_format",
            Self::UnknownValue => "unknown_value",
            Self::Duplicate => "duplicate",
            Self::InFuture => "in_future",
            Self::ReviewBeforeApproval => "review_before_approval",
            Self::ReviewTooLate => "review_too_late",
            Self::FactUnknown => "fact_unknown",
            Self::FactUnapproved => "fact_unapproved",
            Self::FactStale => "fact_stale",
            Self::FactWithdrawn => "fact_withdrawn",
            Self::FactKindMismatch => "fact_kind_mismatch",
            Self::FactNotInBrief => "fact_not_in_brief",
            Self::FactHasNoLink => "fact_has_no_link",
            Self::LinkNotAllowed => "link_not_allowed",
            Self::AnchorUnknown => "anchor_unknown",
            Self::PageUnknown => "page_unknown",
            Self::ServiceUnknown => "service_unknown",
            Self::MissingHome => "missing_home",
            Self::MissingQuoteForm => "missing_quote_form",
            Self::UnknownComponentSet => "unknown_component_set",
            Self::UnboundClaim => "unbound_claim",
            Self::AssetUnknown => "asset_unknown",
            Self::TooManyProblems => "too_many_problems",
        }
    }
}

impl fmt::Display for ProblemCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One refused part of a record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// Where: `business_name`, `services[2].name`, `pages[0].sections[3].items[1]`.
    pub path: String,
    /// Why.
    pub code: ProblemCode,
    /// For an unbound claim, the token that made it (the operator's own text, at most
    /// [`DETAIL_MAX_CHARS`] characters); otherwise nothing.
    pub detail: Option<String>,
}

impl Problem {
    /// A problem with no detail.
    pub fn new(path: impl Into<String>, code: ProblemCode) -> Self {
        Self {
            path: path.into(),
            code,
            detail: None,
        }
    }

    /// A problem with a detail, cut to [`DETAIL_MAX_CHARS`].
    pub fn with_detail(path: impl Into<String>, code: ProblemCode, detail: &str) -> Self {
        Self {
            path: path.into(),
            code,
            detail: Some(detail.chars().take(DETAIL_MAX_CHARS).collect()),
        }
    }
}

/// Collects problems, at most [`MAX_PROBLEMS`] of them.
#[derive(Debug, Default)]
pub struct Problems(Vec<Problem>);

impl Problems {
    /// An empty list.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one; past the cap, the list ends with a single [`ProblemCode::TooManyProblems`].
    pub fn push(&mut self, problem: Problem) {
        if self.0.len() < MAX_PROBLEMS {
            self.0.push(problem);
        } else if self.0.len() == MAX_PROBLEMS {
            self.0.push(Problem::new("", ProblemCode::TooManyProblems));
        }
    }

    /// Adds `code` at `path`.
    pub fn add(&mut self, path: impl Into<String>, code: ProblemCode) {
        self.push(Problem::new(path, code));
    }

    /// Whether nothing was found.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The list.
    pub fn into_vec(self) -> Vec<Problem> {
        self.0
    }

    /// `Ok(value)` when nothing was found.
    pub fn finish<T>(self, value: T) -> Result<T, Vec<Problem>> {
        if self.0.is_empty() {
            Ok(value)
        } else {
            Err(self.0)
        }
    }
}

/// `parent.child`, or `child` at the root.
pub fn field(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_owned()
    } else {
        format!("{parent}.{child}")
    }
}

/// `parent[index]`.
pub fn index(parent: &str, index: usize) -> String {
    let mut path = parent.to_owned();
    let _ = write!(path, "[{index}]");
    path
}

/// The shared rule for studio text: trimmed; blank is `None`; at most `max` characters; no
/// control characters (newlines and tabs only when `multiline`) and no bidirectional or
/// invisible characters (D-042, the same set leads refuse).
pub fn text(raw: Option<&str>, max: usize, multiline: bool) -> Result<Option<String>, ProblemCode> {
    let Some(value) = raw.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if value.chars().count() > max {
        return Err(ProblemCode::TooLong);
    }
    let allowed = |c: char| {
        !crate::lead::is_deceptive(c, multiline)
            && (!c.is_control() || (multiline && matches!(c, '\n' | '\r' | '\t')))
    };
    if !value.chars().all(allowed) {
        return Err(ProblemCode::InvalidCharacters);
    }
    Ok(Some(value.to_owned()))
}

/// [`text`], recording a problem at `path` and returning `None` on one.
pub fn checked_text(
    problems: &mut Problems,
    path: &str,
    raw: Option<&str>,
    max: usize,
    multiline: bool,
    required: bool,
) -> Option<String> {
    match text(raw, max, multiline) {
        Ok(Some(value)) => Some(value),
        Ok(None) => {
            if required {
                problems.add(path, ProblemCode::Required);
            }
            None
        }
        Err(code) => {
            problems.add(path, code);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_snake_case_and_unique() {
        use ProblemCode::*;
        let all = [
            Malformed,
            Required,
            TooLong,
            TooShort,
            TooMany,
            TooFew,
            InvalidCharacters,
            InvalidFormat,
            UnknownValue,
            Duplicate,
            InFuture,
            ReviewBeforeApproval,
            ReviewTooLate,
            FactUnknown,
            FactUnapproved,
            FactStale,
            FactWithdrawn,
            FactKindMismatch,
            FactNotInBrief,
            FactHasNoLink,
            LinkNotAllowed,
            AnchorUnknown,
            PageUnknown,
            ServiceUnknown,
            MissingHome,
            MissingQuoteForm,
            UnknownComponentSet,
            UnboundClaim,
            AssetUnknown,
            TooManyProblems,
        ];
        let mut seen = std::collections::HashSet::new();
        for code in all {
            let text = code.as_str();
            assert!(
                text.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
                "{text}"
            );
            assert!(seen.insert(text), "duplicate {text}");
        }
    }

    #[test]
    fn the_list_is_capped_and_says_so() {
        let mut problems = Problems::new();
        for i in 0..250 {
            problems.add(index("items", i), ProblemCode::Required);
        }
        let list = problems.into_vec();
        assert_eq!(list.len(), MAX_PROBLEMS + 1);
        assert_eq!(list[MAX_PROBLEMS].code, ProblemCode::TooManyProblems);
    }

    #[test]
    fn text_refuses_what_leads_refuse() {
        assert_eq!(text(Some("  plain  "), 10, false), Ok(Some("plain".into())));
        assert_eq!(text(Some("   "), 10, false), Ok(None));
        assert_eq!(text(Some("abcdef"), 5, false), Err(ProblemCode::TooLong));
        assert_eq!(
            text(Some("a\u{202E}b"), 10, true),
            Err(ProblemCode::InvalidCharacters)
        );
        assert_eq!(
            text(Some("two\nlines"), 20, false),
            Err(ProblemCode::InvalidCharacters)
        );
        assert_eq!(
            text(Some("two\nlines"), 20, true),
            Ok(Some("two\nlines".into()))
        );
    }

    #[test]
    fn details_are_cut() {
        let long = "x".repeat(500);
        let problem = Problem::with_detail("a", ProblemCode::UnboundClaim, &long);
        assert_eq!(problem.detail.unwrap().chars().count(), DETAIL_MAX_CHARS);
    }
}
