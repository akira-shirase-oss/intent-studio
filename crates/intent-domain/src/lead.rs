//! The lead a site visitor submits, and the rules that make it one.
//!
//! "Parse, don't validate": [`LeadForm`] is the wire shape, with every field optional so a
//! missing field is reported as a named [`FieldError`] instead of a serde message that could echo
//! input. [`LeadForm::validate`] turns it into a [`ValidatedLead`], whose existence proves every
//! rule held. Neither type prints submitted values in `Debug` (D-029): logs carry lead ids, not
//! lead content.

use std::fmt;

use serde::{Deserialize, Deserializer, de};

use crate::limits::{
    EMAIL_MAX_CHARS, MESSAGE_MAX_CHARS, NAME_MAX_CHARS, PHONE_MAX_CHARS, PHONE_MAX_DIGITS,
    PHONE_MIN_DIGITS, SHORT_FIELD_MAX_CHARS, SUBMISSION_ID_MAX_CHARS, SUBMISSION_ID_MIN_CHARS,
};

/// The honeypot field's name. Real visitors never see it; a value in it marks a bot (D-019).
pub const HONEYPOT_FIELD: &str = "website";

/// A lead submission as it arrives, from JSON or from a no-script form post.
///
/// Unknown fields are refused, so the payload stays bounded and honest about what is stored.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeadForm {
    /// Generated in the page; makes a retried submit the same lead (D-019).
    pub submission_id: Option<String>,
    /// The visitor's name.
    pub name: Option<String>,
    /// An e-mail address; this or `phone` is required.
    pub email: Option<String>,
    /// A phone number; this or `email` is required.
    pub phone: Option<String>,
    /// What the visitor wants.
    pub message: Option<String>,
    /// The service type asked about (rule-based qualification later, D-019).
    pub service: Option<String>,
    /// The service area or postcode.
    pub area: Option<String>,
    /// The visitor agreed to be contacted under the site's privacy notice (D-031).
    #[serde(default, deserialize_with = "consent_value")]
    pub consent: Option<bool>,
    /// Which version of the privacy notice the form showed (D-044), e.g. `privacy-2026-11-16`.
    pub consent_notice: Option<String>,
    /// The honeypot ([`HONEYPOT_FIELD`]).
    pub website: Option<String>,
}

impl fmt::Debug for LeadForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Presence only: no submitted value is ever formatted.
        f.debug_struct("LeadForm")
            .field("submission_id", &self.submission_id.is_some())
            .field("name", &self.name.is_some())
            .field("email", &self.email.is_some())
            .field("phone", &self.phone.is_some())
            .field("message", &self.message.is_some())
            .field("service", &self.service.is_some())
            .field("area", &self.area.is_some())
            .field("consent", &self.consent)
            .field("consent_notice", &self.consent_notice.is_some())
            .field("website", &self.website.is_some())
            .finish()
    }
}

/// A field of the lead form, as named in error responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// `submission_id`
    SubmissionId,
    /// `name`
    Name,
    /// `email`
    Email,
    /// `phone`
    Phone,
    /// `message`
    Message,
    /// `service`
    Service,
    /// `area`
    Area,
    /// `consent`
    Consent,
    /// `consent_notice`
    ConsentNotice,
    /// `email` or `phone`: neither was given.
    Contact,
}

impl Field {
    /// The wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SubmissionId => "submission_id",
            Self::Name => "name",
            Self::Email => "email",
            Self::Phone => "phone",
            Self::Message => "message",
            Self::Service => "service",
            Self::Area => "area",
            Self::Consent => "consent",
            Self::ConsentNotice => "consent_notice",
            Self::Contact => "contact",
        }
    }
}

/// Why a field was refused. Stable codes: clients branch on them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldCode {
    /// The field is required and was missing or blank.
    Required,
    /// Longer than the field's limit.
    TooLong,
    /// Shorter than the field's minimum.
    TooShort,
    /// Contains control characters (or, for ids, characters outside the allowed set).
    InvalidCharacters,
    /// Not shaped like an e-mail address.
    InvalidEmail,
    /// Not shaped like a phone number.
    InvalidPhone,
    /// Consent was missing or false.
    ConsentRequired,
    /// Neither an e-mail address nor a phone number was given.
    ContactRequired,
}

impl FieldCode {
    /// The wire code.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Required => "required",
            Self::TooLong => "too_long",
            Self::TooShort => "too_short",
            Self::InvalidCharacters => "invalid_characters",
            Self::InvalidEmail => "invalid_email",
            Self::InvalidPhone => "invalid_phone",
            Self::ConsentRequired => "consent_required",
            Self::ContactRequired => "contact_required",
        }
    }
}

/// One refused field. Carries no submitted value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldError {
    /// Which field.
    pub field: Field,
    /// Why.
    pub code: FieldCode,
}

/// Why a submission is not a lead.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Rejection {
    /// The honeypot was filled.
    #[error("submission rejected")]
    Honeypot,
    /// One or more fields broke a rule; every broken field is listed.
    #[error("{} field(s) failed validation", .0.len())]
    Invalid(Vec<FieldError>),
}

/// A client-supplied submission id: 8 to 64 characters of `A-Z a-z 0-9 _ -`.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ClientSubmissionId(String);

impl ClientSubmissionId {
    /// Parses a client-supplied id.
    pub fn parse(raw: &str) -> Result<Self, FieldCode> {
        let count = raw.chars().count();
        if count < SUBMISSION_ID_MIN_CHARS {
            return Err(FieldCode::TooShort);
        }
        if count > SUBMISSION_ID_MAX_CHARS {
            return Err(FieldCode::TooLong);
        }
        if !raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(FieldCode::InvalidCharacters);
        }
        Ok(Self(raw.to_owned()))
    }

    /// The id as the client sent it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ClientSubmissionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Not personal data, but client-controlled: keep it out of debug output anyway.
        write!(f, "ClientSubmissionId(len={})", self.0.len())
    }
}

/// A lead whose every field passed validation. Fields are trimmed; blank optional fields are
/// `None`.
#[derive(Clone, PartialEq, Eq)]
pub struct ValidatedLead {
    submission_id: Option<ClientSubmissionId>,
    name: String,
    email: Option<String>,
    phone: Option<String>,
    message: String,
    service: Option<String>,
    area: Option<String>,
    consent_notice: String,
}

impl fmt::Debug for ValidatedLead {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ValidatedLead")
            .field("submission_id", &self.submission_id)
            .field("name_chars", &self.name.chars().count())
            .field("has_email", &self.email.is_some())
            .field("has_phone", &self.phone.is_some())
            .field("message_chars", &self.message.chars().count())
            .finish_non_exhaustive()
    }
}

impl ValidatedLead {
    /// The client-supplied submission id, if the form carried one.
    pub fn submission_id(&self) -> Option<&ClientSubmissionId> {
        self.submission_id.as_ref()
    }
    /// The visitor's name.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// The e-mail address, if given.
    pub fn email(&self) -> Option<&str> {
        self.email.as_deref()
    }
    /// The phone number, if given.
    pub fn phone(&self) -> Option<&str> {
        self.phone.as_deref()
    }
    /// The message (possibly empty).
    pub fn message(&self) -> &str {
        &self.message
    }
    /// The service type, if given.
    pub fn service(&self) -> Option<&str> {
        self.service.as_deref()
    }
    /// The service area, if given.
    pub fn area(&self) -> Option<&str> {
        self.area.as_deref()
    }
    /// The privacy-notice version the visitor consented under.
    pub fn consent_notice(&self) -> &str {
        &self.consent_notice
    }
}

impl LeadForm {
    /// Applies every rule and reports every broken field, or the honeypot.
    ///
    /// The honeypot is checked first and alone: a filled honeypot says nothing about which other
    /// fields a bot got right, and reporting them would teach it.
    pub fn validate(self) -> Result<ValidatedLead, Rejection> {
        if self
            .website
            .as_deref()
            .is_some_and(|v| !v.trim().is_empty())
        {
            return Err(Rejection::Honeypot);
        }
        let mut errors = Vec::new();
        let mut check = |field: Field, result: Result<Option<String>, FieldCode>| match result {
            Ok(value) => value,
            Err(code) => {
                errors.push(FieldError { field, code });
                None
            }
        };

        let submission_id = check(
            Field::SubmissionId,
            optional(
                self.submission_id.as_deref(),
                SUBMISSION_ID_MAX_CHARS,
                false,
            ),
        );
        let name = check(
            Field::Name,
            required(self.name.as_deref(), NAME_MAX_CHARS, false),
        );
        let email = check(Field::Email, email(self.email.as_deref()));
        let phone = check(Field::Phone, phone(self.phone.as_deref()));
        let message = check(
            Field::Message,
            optional(self.message.as_deref(), MESSAGE_MAX_CHARS, true),
        );
        let service = check(
            Field::Service,
            optional(self.service.as_deref(), SHORT_FIELD_MAX_CHARS, false),
        );
        let area = check(
            Field::Area,
            optional(self.area.as_deref(), SHORT_FIELD_MAX_CHARS, false),
        );
        let consent_notice = check(
            Field::ConsentNotice,
            notice_version(self.consent_notice.as_deref()),
        );

        let submission_id = match submission_id.as_deref().map(ClientSubmissionId::parse) {
            None => None,
            Some(Ok(id)) => Some(id),
            Some(Err(code)) => {
                errors.push(FieldError {
                    field: Field::SubmissionId,
                    code,
                });
                None
            }
        };
        let contact_given = self.email.as_deref().is_some_and(|v| !v.trim().is_empty())
            || self.phone.as_deref().is_some_and(|v| !v.trim().is_empty());
        if !contact_given {
            errors.push(FieldError {
                field: Field::Contact,
                code: FieldCode::ContactRequired,
            });
        }
        if self.consent != Some(true) {
            errors.push(FieldError {
                field: Field::Consent,
                code: FieldCode::ConsentRequired,
            });
        }

        match (name, consent_notice) {
            (Some(name), Some(consent_notice)) if errors.is_empty() => Ok(ValidatedLead {
                submission_id,
                name,
                email,
                phone,
                message: message.unwrap_or_default(),
                service,
                area,
                consent_notice,
            }),
            _ => Err(Rejection::Invalid(errors)),
        }
    }
}

/// Trims; blank is `None`; enforces the length and the character rule.
fn optional(raw: Option<&str>, max: usize, multiline: bool) -> Result<Option<String>, FieldCode> {
    let Some(value) = raw.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if value.chars().count() > max {
        return Err(FieldCode::TooLong);
    }
    let allowed = |c: char| {
        !is_deceptive(c, multiline)
            && (!c.is_control() || (multiline && matches!(c, '\n' | '\r' | '\t')))
    };
    if !value.chars().all(allowed) {
        return Err(FieldCode::InvalidCharacters);
    }
    Ok(Some(value.to_owned()))
}

/// Bidirectional controls and invisible characters: they make text read differently from what
/// it is (a right-to-left override reverses what follows; zero-width characters make two
/// different names look the same). The zero-width joiner is allowed in the message only, where
/// emoji sequences use it.
pub(crate) fn is_deceptive(c: char, allow_joiner: bool) -> bool {
    matches!(
        c,
        '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'
            | '\u{200C}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
    ) || (c == '\u{200D}' && !allow_joiner)
}

fn required(raw: Option<&str>, max: usize, multiline: bool) -> Result<Option<String>, FieldCode> {
    match optional(raw, max, multiline)? {
        Some(value) => Ok(Some(value)),
        None => Err(FieldCode::Required),
    }
}

/// The privacy-notice version: 1 to 64 characters of `A-Z a-z 0-9 . _ : -`, required.
fn notice_version(raw: Option<&str>) -> Result<Option<String>, FieldCode> {
    let value = required(raw, crate::limits::CONSENT_NOTICE_MAX_CHARS, false)?;
    if value.as_deref().is_some_and(|v| {
        !v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
    }) {
        return Err(FieldCode::InvalidCharacters);
    }
    Ok(value)
}

/// A shape check, not deliverability: one `@`, a non-empty local part of at most 64
/// characters, and a dotted domain whose labels are non-empty and do not start or end with `-`.
pub(crate) fn email(raw: Option<&str>) -> Result<Option<String>, FieldCode> {
    let Some(value) = optional(raw, EMAIL_MAX_CHARS, false)? else {
        return Ok(None);
    };
    let shaped = value.split_once('@').is_some_and(|(local, domain)| {
        !local.is_empty()
            && local.chars().count() <= 64
            && !domain.contains('@')
            && domain.contains('.')
            && domain.split('.').all(|label| {
                !label.is_empty()
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label.chars().all(|c| c.is_alphanumeric() || c == '-')
            })
            && !local.chars().any(char::is_whitespace)
    });
    if shaped {
        Ok(Some(value))
    } else {
        Err(FieldCode::InvalidEmail)
    }
}

/// Digits with common formatting (`+` only first, spaces, `-`, `.`, parentheses); 6 to 15
/// digits.
pub(crate) fn phone(raw: Option<&str>) -> Result<Option<String>, FieldCode> {
    let Some(value) = optional(raw, PHONE_MAX_CHARS, false)? else {
        return Ok(None);
    };
    let digits = value.chars().filter(char::is_ascii_digit).count();
    let charset = value.char_indices().all(|(i, c)| {
        c.is_ascii_digit() || matches!(c, ' ' | '-' | '.' | '(' | ')') || (c == '+' && i == 0)
    });
    if charset && (PHONE_MIN_DIGITS..=PHONE_MAX_DIGITS).contains(&digits) {
        Ok(Some(value))
    } else {
        Err(FieldCode::InvalidPhone)
    }
}

/// `consent` arrives as a JSON boolean, or as a string from a no-script form (`on` for a ticked
/// checkbox).
fn consent_value<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<bool>, D::Error> {
    struct ConsentVisitor;

    impl de::Visitor<'_> for ConsentVisitor {
        type Value = Option<bool>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a boolean, or one of on/true/yes/1/off/false/no/0")
        }

        fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
            Ok(Some(value))
        }

        fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
            match value {
                "on" | "true" | "yes" | "1" => Ok(Some(true)),
                "off" | "false" | "no" | "0" | "" => Ok(Some(false)),
                _ => Err(E::custom("consent must be a boolean")),
            }
        }

        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
    }

    deserializer.deserialize_any(ConsentVisitor)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form() -> LeadForm {
        LeadForm {
            submission_id: Some("sub_0123456789".into()),
            name: Some("  Ada Lovelace ".into()),
            email: Some("ada@example.test".into()),
            phone: None,
            message: Some("Please quote for a boiler service.\nThanks".into()),
            service: Some("boiler service".into()),
            area: Some("N1".into()),
            consent: Some(true),
            consent_notice: Some("privacy-2026-11-16".into()),
            website: None,
        }
    }

    fn codes(form: LeadForm) -> Vec<(Field, FieldCode)> {
        match form.validate() {
            Err(Rejection::Invalid(errors)) => {
                errors.into_iter().map(|e| (e.field, e.code)).collect()
            }
            other => panic!("expected field errors, got {other:?}"),
        }
    }

    #[test]
    fn a_complete_form_validates_and_is_trimmed() {
        let lead = form().validate().unwrap();
        assert_eq!(lead.name(), "Ada Lovelace");
        assert_eq!(lead.email(), Some("ada@example.test"));
        assert_eq!(lead.phone(), None);
        assert_eq!(lead.submission_id().unwrap().as_str(), "sub_0123456789");
    }

    #[test]
    fn a_filled_honeypot_is_rejected_before_anything_else() {
        let bot = LeadForm {
            website: Some("http://spam.example".into()),
            name: None,
            ..form()
        };
        assert_eq!(bot.validate(), Err(Rejection::Honeypot));
        // A blank honeypot is what a real browser sends.
        let human = LeadForm {
            website: Some("  ".into()),
            ..form()
        };
        assert!(human.validate().is_ok());
    }

    #[test]
    fn every_broken_field_is_reported_together() {
        let broken = LeadForm {
            submission_id: Some("short".into()),
            name: Some("   ".into()),
            email: None,
            phone: None,
            consent: Some(false),
            ..form()
        };
        assert_eq!(
            codes(broken),
            vec![
                (Field::Name, FieldCode::Required),
                (Field::SubmissionId, FieldCode::TooShort),
                (Field::Contact, FieldCode::ContactRequired),
                (Field::Consent, FieldCode::ConsentRequired),
            ]
        );
    }

    #[test]
    fn the_consent_notice_version_is_required_and_plain() {
        let lead = form().validate().unwrap();
        assert_eq!(lead.consent_notice(), "privacy-2026-11-16");
        assert_eq!(
            codes(LeadForm {
                consent_notice: None,
                ..form()
            }),
            vec![(Field::ConsentNotice, FieldCode::Required)]
        );
        assert_eq!(
            codes(LeadForm {
                consent_notice: Some("v1 <script>".into()),
                ..form()
            }),
            vec![(Field::ConsentNotice, FieldCode::InvalidCharacters)]
        );
        assert_eq!(
            codes(LeadForm {
                consent_notice: Some("v".repeat(65)),
                ..form()
            }),
            vec![(Field::ConsentNotice, FieldCode::TooLong)]
        );
    }

    #[test]
    fn consent_is_required_and_must_be_true() {
        assert_eq!(
            codes(LeadForm {
                consent: None,
                ..form()
            }),
            vec![(Field::Consent, FieldCode::ConsentRequired)]
        );
    }

    #[test]
    fn length_limits_hold_at_the_boundary() {
        let at_limit = LeadForm {
            name: Some("n".repeat(NAME_MAX_CHARS)),
            message: Some("é".repeat(MESSAGE_MAX_CHARS)),
            ..form()
        };
        assert!(at_limit.validate().is_ok());
        let over = LeadForm {
            name: Some("n".repeat(NAME_MAX_CHARS + 1)),
            message: Some("é".repeat(MESSAGE_MAX_CHARS + 1)),
            ..form()
        };
        assert_eq!(
            codes(over),
            vec![
                (Field::Name, FieldCode::TooLong),
                (Field::Message, FieldCode::TooLong)
            ]
        );
    }

    #[test]
    fn control_characters_are_refused_except_line_breaks_in_the_message() {
        assert_eq!(
            codes(LeadForm {
                name: Some("Ada\u{0}".into()),
                ..form()
            }),
            vec![(Field::Name, FieldCode::InvalidCharacters)]
        );
        assert_eq!(
            codes(LeadForm {
                service: Some("a\nb".into()),
                ..form()
            }),
            vec![(Field::Service, FieldCode::InvalidCharacters)]
        );
        assert!(
            LeadForm {
                message: Some("line one\r\nline two\tend".into()),
                ..form()
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn bidi_controls_and_invisible_characters_are_refused() {
        // A right-to-left override can make "evil.exe" read as "exe.live" in the inbox; zero-width
        // characters make two different names look the same.
        for (field, value) in [
            (Field::Name, "Ada\u{202E}ecalevoL"),
            (Field::Name, "Ada\u{200B}Lovelace"),
            (Field::Name, "\u{2066}Ada\u{2069}"),
            (Field::Name, "Ada\u{200D}"),
            (Field::Service, "boiler\u{FEFF}"),
            (Field::Area, "N1\u{200F}"),
            (Field::Message, "pay \u{202D}here"),
            (Field::Message, "hi\u{2060}there"),
        ] {
            let mut form = form();
            match field {
                Field::Name => form.name = Some(value.into()),
                Field::Service => form.service = Some(value.into()),
                Field::Area => form.area = Some(value.into()),
                _ => form.message = Some(value.into()),
            }
            assert_eq!(
                codes(form),
                vec![(field, FieldCode::InvalidCharacters)],
                "{value:?}"
            );
        }
        assert_eq!(
            email(Some("ada\u{200B}@example.test")),
            Err(FieldCode::InvalidCharacters)
        );
        // A joiner inside an emoji sequence is ordinary text in a message.
        let family = LeadForm {
            message: Some("for my family \u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}".into()),
            ..form()
        };
        assert!(family.validate().is_ok());
    }

    #[test]
    fn email_shapes() {
        for good in ["a@b.co", "first.last+tag@sub.example.test", "ü@例え.テスト"] {
            assert!(email(Some(good)).is_ok(), "{good}");
        }
        for bad in [
            "plain",
            "@example.test",
            "a@",
            "a@nodot",
            "a@b@c.test",
            "a b@c.test",
            "a@-x.test",
            "a@x..test",
        ] {
            assert_eq!(email(Some(bad)), Err(FieldCode::InvalidEmail), "{bad}");
        }
    }

    #[test]
    fn phone_shapes() {
        for good in ["+44 20 7946 0958", "(020) 7946-0958", "555.0100.12"] {
            assert!(phone(Some(good)).is_ok(), "{good}");
        }
        for bad in [
            "12345",
            "+1 555 0100 1234 5678",
            "0207 946 095x",
            "44+20794609",
        ] {
            assert_eq!(phone(Some(bad)), Err(FieldCode::InvalidPhone), "{bad}");
        }
    }

    #[test]
    fn a_phone_alone_is_enough_contact() {
        let lead = LeadForm {
            email: None,
            phone: Some("+44 20 7946 0958".into()),
            ..form()
        };
        assert!(lead.validate().is_ok());
    }

    #[test]
    fn submission_ids_allow_only_a_safe_charset() {
        assert!(ClientSubmissionId::parse("0192f4a0-7c1e-7b3a-9d2e-1f0a2b3c4d5e").is_ok());
        assert_eq!(
            ClientSubmissionId::parse("has:colon1"),
            Err(FieldCode::InvalidCharacters)
        );
        assert_eq!(
            ClientSubmissionId::parse(&"x".repeat(65)),
            Err(FieldCode::TooLong)
        );
    }

    #[test]
    fn unknown_fields_are_refused_on_the_wire() {
        let result = serde_json::from_str::<LeadForm>(r#"{"name":"a","extra":"b"}"#);
        assert!(result.is_err());
    }

    #[test]
    fn consent_parses_from_json_and_from_a_no_script_form() {
        let json: LeadForm = serde_json::from_str(r#"{"consent":true}"#).unwrap();
        assert_eq!(json.consent, Some(true));
        let form: LeadForm = serde_urlencoded::from_str("name=Ada&consent=on").unwrap();
        assert_eq!(form.consent, Some(true));
        let unticked: LeadForm = serde_urlencoded::from_str("name=Ada").unwrap();
        assert_eq!(unticked.consent, None);
        assert!(serde_json::from_str::<LeadForm>(r#"{"consent":"maybe"}"#).is_err());
    }

    #[test]
    fn debug_output_never_contains_submitted_values() {
        let raw = form();
        let printed = format!("{raw:?}");
        let lead = raw.validate().unwrap();
        let printed_lead = format!("{lead:?}");
        for value in ["Ada", "ada@example.test", "boiler", "N1", "sub_0123456789"] {
            assert!(!printed.contains(value), "{printed}");
            assert!(!printed_lead.contains(value), "{printed_lead}");
        }
    }
}
