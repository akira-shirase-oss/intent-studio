//! The notification text for a stored lead (D-019).
//!
//! Plain text. The subject carries no submitted value — only the operator-configured site name —
//! so nothing a visitor typed can reach a mail header. Submitted values appear only in the body,
//! already validated (no control characters outside the message's line breaks).

use crate::ids::LeadId;

/// What the notification is built from: a stored lead's fields.
#[derive(Clone, Copy)]
pub struct LeadContent<'a> {
    /// The visitor's name.
    pub name: &'a str,
    /// E-mail, if given.
    pub email: Option<&'a str>,
    /// Phone, if given.
    pub phone: Option<&'a str>,
    /// Service type, if given.
    pub service: Option<&'a str>,
    /// Service area, if given.
    pub area: Option<&'a str>,
    /// The message (possibly empty).
    pub message: &'a str,
}

/// A composed notification.
#[derive(Clone, PartialEq, Eq)]
pub struct NotificationText {
    /// The subject line: no submitted values.
    pub subject: String,
    /// The plain-text body.
    pub body: String,
}

impl std::fmt::Debug for NotificationText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotificationText")
            .field("subject_chars", &self.subject.chars().count())
            .field("body_chars", &self.body.chars().count())
            .finish()
    }
}

/// Longest site name used in a subject; longer names are cut at a character boundary.
const SUBJECT_SITE_MAX_CHARS: usize = 80;

/// Composes the notification for one lead.
pub fn compose(site_name: &str, lead: LeadId, content: LeadContent<'_>) -> NotificationText {
    let site: String = site_name
        .chars()
        .filter(|c| !c.is_control())
        .take(SUBJECT_SITE_MAX_CHARS)
        .collect();
    let mut body = format!("A new quote request arrived for {site}.\n\n");
    let mut line = |label: &str, value: Option<&str>| {
        if let Some(value) = value {
            body.push_str(label);
            body.push_str(": ");
            body.push_str(value);
            body.push('\n');
        }
    };
    line("Name", Some(content.name));
    line("Email", content.email);
    line("Phone", content.phone);
    line("Service", content.service);
    line("Area", content.area);
    if !content.message.is_empty() {
        body.push_str("\nMessage:\n");
        body.push_str(content.message);
        body.push('\n');
    }
    body.push_str("\nLead ");
    body.push_str(&lead.to_string());
    body.push_str(" is saved in the Intent Studio inbox.\n");
    NotificationText {
        subject: format!("New quote request: {site}"),
        body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lead() -> LeadId {
        "0192f4a0-7c1e-7b3a-9d2e-1f0a2b3c4d5e".parse().unwrap()
    }

    fn content() -> LeadContent<'static> {
        LeadContent {
            name: "Ada Lovelace",
            email: Some("ada@example.test"),
            phone: None,
            service: Some("boiler"),
            area: None,
            message: "Line one\nLine two",
        }
    }

    #[test]
    fn the_subject_carries_no_submitted_value() {
        let text = compose("Northside Plumbing", lead(), content());
        assert_eq!(text.subject, "New quote request: Northside Plumbing");
        for value in ["Ada", "ada@example.test", "boiler", "Line one"] {
            assert!(!text.subject.contains(value));
            assert!(text.body.contains(value));
        }
        assert!(!text.body.contains("Phone:"));
        assert!(text.body.contains(&lead().to_string()));
    }

    #[test]
    fn a_hostile_site_name_cannot_break_the_subject_line() {
        let text = compose("Evil\r\nBcc: x@y.test", lead(), content());
        assert!(!text.subject.contains('\r') && !text.subject.contains('\n'));
        let long = compose(&"s".repeat(500), lead(), content());
        assert_eq!(
            long.subject.chars().count(),
            "New quote request: ".len() + 80
        );
    }

    #[test]
    fn debug_prints_sizes_only() {
        let printed = format!("{:?}", compose("Site", lead(), content()));
        assert!(!printed.contains("Ada"));
    }
}
