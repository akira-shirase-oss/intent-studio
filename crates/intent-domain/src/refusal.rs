//! The stable codes every refusal carries. Clients branch on these; renaming one is a breaking
//! change. The HTTP status for each lives in `intent-server`.

/// Why the server did not store (or did not acknowledge) a submission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RefusalCode {
    /// The body is not valid JSON or form data for a lead.
    MalformedBody,
    /// The body exceeds the 16 KiB cap (D-019).
    PayloadTooLarge,
    /// Neither `application/json` nor `application/x-www-form-urlencoded`.
    UnsupportedMediaType,
    /// One or more fields broke a rule; the response lists each field and code.
    ValidationFailed,
    /// The honeypot was filled. Deliberately unspecific.
    Rejected,
    /// No such site, or it is not accepting leads.
    UnknownSite,
    /// Too many leads from this address or for this site; `Retry-After` says when.
    RateLimited,
    /// This submission id was already used for different content.
    SubmissionConflict,
    /// The database cannot take the lead right now. Nothing was acknowledged; retrying with the
    /// same submission id is safe.
    Unavailable,
    /// Too many requests in flight; shed before any work was done.
    Overloaded,
    /// The request did not finish before its deadline. Not acknowledged; retry is safe.
    Timeout,
    /// Not signed in as the operator.
    Unauthorized,
    /// Not found.
    NotFound,
    /// The method is not allowed on this path.
    MethodNotAllowed,
    /// A defect. The cause is logged; the client sees only this code.
    Internal,
}

impl RefusalCode {
    /// The wire code.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MalformedBody => "malformed_body",
            Self::PayloadTooLarge => "payload_too_large",
            Self::UnsupportedMediaType => "unsupported_media_type",
            Self::ValidationFailed => "validation_failed",
            Self::Rejected => "rejected",
            Self::UnknownSite => "unknown_site",
            Self::RateLimited => "rate_limited",
            Self::SubmissionConflict => "submission_conflict",
            Self::Unavailable => "unavailable",
            Self::Overloaded => "overloaded",
            Self::Timeout => "timeout",
            Self::Unauthorized => "unauthorized",
            Self::NotFound => "not_found",
            Self::MethodNotAllowed => "method_not_allowed",
            Self::Internal => "internal",
        }
    }

    /// Whether resending the same submission later may succeed (the client should honour
    /// `Retry-After`).
    pub const fn is_retryable(self) -> bool {
        matches!(
            self,
            Self::RateLimited | Self::Unavailable | Self::Overloaded | Self::Timeout
        )
    }
}

#[cfg(test)]
mod tests {
    use super::RefusalCode::*;

    #[test]
    fn codes_are_snake_case_and_unique() {
        let all = [
            MalformedBody,
            PayloadTooLarge,
            UnsupportedMediaType,
            ValidationFailed,
            Rejected,
            UnknownSite,
            RateLimited,
            SubmissionConflict,
            Unavailable,
            Overloaded,
            Timeout,
            Unauthorized,
            NotFound,
            MethodNotAllowed,
            Internal,
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
    fn only_transient_refusals_are_retryable() {
        assert!(Unavailable.is_retryable());
        assert!(RateLimited.is_retryable());
        assert!(!ValidationFailed.is_retryable());
        assert!(!SubmissionConflict.is_retryable());
    }
}
