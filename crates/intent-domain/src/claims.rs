//! The claim lint for free text (D-013, `claim-lint v1`, D-059).
//!
//! Deterministic and deliberately eager: it flags every number, price, percentage and year and a
//! fixed list of claim words in text that is not a fact. A flagged token is acceptable only when
//! the text slot binds a fact whose statement contains the same token (case-insensitive);
//! otherwise it is an unbound claim and the page cannot validate. False positives cost the
//! operator a re-word; that is the price of a page that cannot quietly invent "licensed and
//! insured since 1985". It is a second line behind human approval, not a guarantee.

use std::fmt;

/// The lint's version; changing a rule changes this (it is a design-regression surface).
pub const LINT_VERSION: &str = "claim-lint v1";

/// Claim words, matched as whole words by prefix (`guarantee` also catches `guaranteed`): the
/// D-013 list plus the accreditation and award words that make the same kind of promise.
const CLAIM_WORDS: [&str; 14] = [
    "licensed",
    "licence",
    "license",
    "insured",
    "insurance",
    "certified",
    "certificat",
    "accredit",
    "guarantee",
    "warrant",
    "award",
    "best",
    "#1",
    "approved",
];

/// Words that are claims only as whole words (no prefix match: `best` is not `bestow`).
const WHOLE_ONLY: [&str; 3] = ["best", "#1", "approved"];

/// Currency signs that make a number a price.
const CURRENCY: [char; 7] = ['£', '$', '€', '¥', '₹', '₽', '₩'];

/// What kind of claim a token looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClaimCategory {
    /// A number ("500+", "24/7", "10,000").
    Number,
    /// A price ("£80", "$1,200").
    Currency,
    /// A percentage ("98%", "50 percent").
    Percent,
    /// A four-digit year from 1900 to 2099 ("since 1985").
    Year,
    /// A claim word ("licensed", "guaranteed", "best").
    Phrase,
}

impl ClaimCategory {
    /// The wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Number => "number",
            Self::Currency => "currency",
            Self::Percent => "percent",
            Self::Year => "year",
            Self::Phrase => "phrase",
        }
    }
}

impl fmt::Display for ClaimCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One flagged token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// What it looks like.
    pub category: ClaimCategory,
    /// The token as written (numbers without their sign or suffix, so a fact saying "over 500
    /// jobs" binds the text "500+ jobs").
    pub token: String,
}

/// Every claim-looking token in `text`, in order.
pub fn lint(text: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_digit() {
            let start = i;
            while i < chars.len()
                && (chars[i].is_ascii_digit()
                    || (matches!(chars[i], ',' | '.')
                        && chars.get(i + 1).is_some_and(char::is_ascii_digit)))
            {
                i += 1;
            }
            let token: String = chars[start..i].iter().collect();
            let before = chars[..start].iter().rev().find(|c| !c.is_whitespace());
            let after_space = chars[i..]
                .iter()
                .position(|c| !c.is_whitespace())
                .map(|p| i + p);
            let category = if before.is_some_and(|b| CURRENCY.contains(b)) {
                ClaimCategory::Currency
            } else if chars.get(i) == Some(&'%')
                || after_space.is_some_and(|p| word_at(&chars, p).eq_ignore_ascii_case("percent"))
            {
                ClaimCategory::Percent
            } else if token.len() == 4
                && token
                    .parse::<u16>()
                    .is_ok_and(|y| (1900..=2099).contains(&y))
            {
                ClaimCategory::Year
            } else {
                ClaimCategory::Number
            };
            findings.push(Finding { category, token });
            continue;
        }
        if c.is_alphabetic() || c == '#' {
            let word = word_at(&chars, i);
            let length = word.chars().count().max(1);
            let preceded = i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '#');
            if !preceded && let Some(stem) = claim_word(&word) {
                findings.push(Finding {
                    category: ClaimCategory::Phrase,
                    token: if WHOLE_ONLY.contains(&stem) {
                        word.to_lowercase()
                    } else {
                        stem.to_owned()
                    },
                });
            }
            i += length;
            continue;
        }
        i += 1;
    }
    findings
}

/// The word starting at `start`: letters, digits, `#` (for `#1`) and inner apostrophes.
fn word_at(chars: &[char], start: usize) -> String {
    chars[start..]
        .iter()
        .take_while(|c| c.is_alphanumeric() || **c == '#' || **c == '\'')
        .collect()
}

/// The claim stem `word` matches, if any.
fn claim_word(word: &str) -> Option<&'static str> {
    let lower = word.to_lowercase();
    CLAIM_WORDS.into_iter().find(|stem| {
        if WHOLE_ONLY.contains(stem) {
            lower == *stem
        } else {
            lower.starts_with(stem)
        }
    })
}

/// The findings in `text` that none of `statements` supports: a finding is supported when a
/// statement contains its token, ignoring case (for claim words, a statement containing the
/// same stem).
pub fn unbound<'a>(text: &str, statements: impl IntoIterator<Item = &'a str>) -> Vec<Finding> {
    let lowered: Vec<String> = statements.into_iter().map(str::to_lowercase).collect();
    lint(text)
        .into_iter()
        .filter(|finding| {
            let token = finding.token.to_lowercase();
            !lowered
                .iter()
                .any(|statement| contains_token(statement, &token))
        })
        .collect()
}

/// Whether `haystack` contains `token` as a whole number (for digits) or anywhere (for words).
fn contains_token(haystack: &str, token: &str) -> bool {
    if !token.starts_with(|c: char| c.is_ascii_digit()) {
        return haystack.contains(token);
    }
    // "500" must not be supported by "1500": the match may not touch other digits.
    haystack.match_indices(token).any(|(at, _)| {
        let before = haystack[..at].chars().next_back();
        let after = haystack[at + token.len()..].chars().next();
        !before.is_some_and(|c| c.is_ascii_digit()) && !after.is_some_and(|c| c.is_ascii_digit())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(text: &str) -> Vec<(ClaimCategory, String)> {
        lint(text)
            .into_iter()
            .map(|f| (f.category, f.token))
            .collect()
    }

    #[test]
    fn the_d013_fixtures_are_all_flagged() {
        use ClaimCategory::*;
        assert_eq!(
            tokens("Licensed & insured since 1985"),
            vec![
                (Phrase, "licensed".into()),
                (Phrase, "insured".into()),
                (Year, "1985".into())
            ]
        );
        assert_eq!(
            tokens("Boiler service from $99"),
            vec![(Currency, "99".into())]
        );
        assert_eq!(
            tokens("Boiler service from £ 1,200"),
            vec![(Currency, "1,200".into())]
        );
        assert_eq!(tokens("98% of customers"), vec![(Percent, "98".into())]);
        assert_eq!(tokens("50 percent off"), vec![(Percent, "50".into())]);
        assert_eq!(
            tokens("Over 500 jobs, 24/7"),
            vec![
                (Number, "500".into()),
                (Number, "24".into()),
                (Number, "7".into())
            ]
        );
        assert_eq!(tokens("The #1 plumber"), vec![(Phrase, "#1".into())]);
        assert_eq!(tokens("Best in town"), vec![(Phrase, "best".into())]);
        assert_eq!(
            tokens("Fully guaranteed work"),
            vec![(Phrase, "guarantee".into())]
        );
        assert_eq!(tokens("Award-winning team"), vec![(Phrase, "award".into())]);
        assert_eq!(
            tokens("Gas Safe certification"),
            vec![(Phrase, "certificat".into())]
        );
    }

    #[test]
    fn plain_copy_passes() {
        for text in [
            "Friendly local plumbers who turn up when they say.",
            "We bestow care on every job.",
            "Get a quote",
            "",
        ] {
            assert_eq!(lint(text), vec![], "{text}");
        }
    }

    #[test]
    fn a_bound_fact_that_says_the_same_thing_supports_the_claim() {
        let insured = "Fully insured: £5,000,000 public liability";
        assert_eq!(unbound("Fully insured", [insured]), vec![]);
        assert_eq!(unbound("£5,000,000 cover", [insured]), vec![]);
        let left = unbound("Licensed and insured", [insured]);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].token, "licensed");
        // A number is supported only as a whole number: 500 is not in 1500.
        assert_eq!(unbound("500 jobs", ["1500 jobs done"]).len(), 1);
        assert_eq!(unbound("500+ jobs", ["Over 500 jobs completed"]), vec![]);
        // Nothing bound: everything flagged stays.
        assert_eq!(unbound("Since 1985", std::iter::empty()).len(), 1);
    }
}
