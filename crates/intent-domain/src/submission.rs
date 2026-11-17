//! The idempotency key that makes a retried submit one lead (D-019, invariant 5).
//!
//! One logical lead per `(site, submission key)`. The key is the client's `submission_id` when
//! the page generated one, or, for a form without script, a key the server derives from the site,
//! the normalized content and a 10-minute window. Stored keys are namespaced (`c:` client,
//! `d:` derived), and `:` is outside the client charset, so the two kinds can never collide.
//!
//! The content [`Fingerprint`] is stored with the lead. A resubmission with the same key and the
//! same fingerprint is the same lead; the same key with a different fingerprint is a conflict
//! (a reused id, never silently merged).

use sha2::{Digest, Sha256};

use crate::{
    ids::SiteId,
    lead::{ClientSubmissionId, ValidatedLead},
    limits::DERIVED_KEY_WINDOW_SECS,
};

/// SHA-256 over the normalized lead content. Not secret; never printed with the content.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint([u8; 32]);

impl Fingerprint {
    /// The digest bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Rebuilds a fingerprint read back from storage.
    pub fn from_slice(bytes: &[u8]) -> Option<Self> {
        <[u8; 32]>::try_from(bytes).ok().map(Self)
    }
}

impl std::fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Fingerprint({}…)", hex::encode(&self.0[..4]))
    }
}

/// Fingerprints the normalized content: every field length-prefixed, absent fields marked, the
/// e-mail address compared case-insensitively. The client's submission id is not content.
pub fn fingerprint(lead: &ValidatedLead) -> Fingerprint {
    let mut hasher = Sha256::new();
    hasher.update(b"intent-lead-v1");
    let email = lead.email().map(str::to_lowercase);
    for field in [
        Some(lead.name()),
        email.as_deref(),
        lead.phone(),
        Some(lead.message()),
        lead.service(),
        lead.area(),
    ] {
        match field {
            Some(value) => {
                hasher.update([1]);
                hasher.update((value.len() as u64).to_be_bytes());
                hasher.update(value.as_bytes());
            }
            None => hasher.update([0]),
        }
    }
    Fingerprint(hasher.finalize().into())
}

/// The 10-minute window a received time falls in.
pub const fn window_index(received_unix_secs: i64) -> i64 {
    received_unix_secs.div_euclid(DERIVED_KEY_WINDOW_SECS)
}

/// A submission key as stored: `c:<client id>` or `d:<32 hex digits>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StoredKey(String);

impl StoredKey {
    /// The key for a client-supplied id.
    pub fn client(id: &ClientSubmissionId) -> Self {
        Self(format!("c:{}", id.as_str()))
    }

    /// The derived key for a form without script, in the given window.
    pub fn derived(site: SiteId, fingerprint: &Fingerprint, window: i64) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"intent-derived-key-v1");
        hasher.update(site.as_uuid().as_bytes());
        hasher.update(fingerprint.as_bytes());
        hasher.update(window.to_be_bytes());
        let digest: [u8; 32] = hasher.finalize().into();
        Self(format!("d:{}", hex::encode(&digest[..16])))
    }

    /// The stored text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether the server derived this key.
    pub fn is_derived(&self) -> bool {
        self.0.starts_with("d:")
    }

    /// The id to show the client: its own id, or the derived key.
    pub fn public(&self) -> &str {
        self.0.get(2..).unwrap_or_default()
    }
}

/// The keys a submission is looked up under.
///
/// A derived key also checks the previous window, so two identical no-script submits that straddle
/// a window boundary are still one lead; the store must accept the previous-window match only when
/// that lead arrived within [`DERIVED_KEY_WINDOW_SECS`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmissionKeys {
    /// The key a new lead is stored under.
    pub primary: StoredKey,
    /// For derived keys: the previous window's key.
    pub previous_window: Option<StoredKey>,
}

/// Chooses the submission keys for a validated lead received at `received_unix_secs`.
pub fn keys_for(
    site: SiteId,
    lead: &ValidatedLead,
    fingerprint: &Fingerprint,
    received_unix_secs: i64,
) -> SubmissionKeys {
    if let Some(id) = lead.submission_id() {
        return SubmissionKeys {
            primary: StoredKey::client(id),
            previous_window: None,
        };
    }
    let window = window_index(received_unix_secs);
    SubmissionKeys {
        primary: StoredKey::derived(site, fingerprint, window),
        previous_window: Some(StoredKey::derived(site, fingerprint, window - 1)),
    }
}

/// What a store reports when the primary key already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Replay {
    /// Same content: the same lead, acknowledged again.
    SameLead,
    /// Different content under the same key: refused, never merged.
    Conflict,
}

/// Decides a replay by comparing fingerprints.
pub fn replay(existing: &Fingerprint, incoming: &Fingerprint) -> Replay {
    if existing == incoming {
        Replay::SameLead
    } else {
        Replay::Conflict
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lead::LeadForm;

    fn site() -> SiteId {
        "0192f4a0-7c1e-7b3a-9d2e-1f0a2b3c4d5e".parse().unwrap()
    }

    fn lead(submission_id: Option<&str>, email: &str) -> ValidatedLead {
        LeadForm {
            submission_id: submission_id.map(Into::into),
            name: Some("Ada".into()),
            email: Some(email.into()),
            message: Some("hello".into()),
            consent: Some(true),
            consent_notice: Some("privacy-v1".into()),
            ..LeadForm::default()
        }
        .validate()
        .unwrap()
    }

    #[test]
    fn the_same_content_has_the_same_fingerprint_and_case_of_email_does_not_matter() {
        let a = fingerprint(&lead(None, "Ada@Example.test"));
        let b = fingerprint(&lead(Some("other-id-123"), "ada@example.test"));
        assert_eq!(a, b);
        assert_ne!(a, fingerprint(&lead(None, "bob@example.test")));
    }

    #[test]
    fn field_boundaries_cannot_be_shifted_to_forge_a_fingerprint() {
        let one = LeadForm {
            name: Some("ab".into()),
            email: Some("x@y.test".into()),
            service: Some("c".into()),
            consent: Some(true),
            consent_notice: Some("privacy-v1".into()),
            ..LeadForm::default()
        };
        let two = LeadForm {
            name: Some("a".into()),
            email: Some("x@y.test".into()),
            service: Some("bc".into()),
            consent: Some(true),
            consent_notice: Some("privacy-v1".into()),
            ..LeadForm::default()
        };
        assert_ne!(
            fingerprint(&one.validate().unwrap()),
            fingerprint(&two.validate().unwrap())
        );
    }

    #[test]
    fn a_client_id_is_the_key_and_has_no_previous_window() {
        let lead = lead(Some("sub_0123456789"), "a@b.test");
        let keys = keys_for(site(), &lead, &fingerprint(&lead), 1_000);
        assert_eq!(keys.primary.as_str(), "c:sub_0123456789");
        assert_eq!(keys.primary.public(), "sub_0123456789");
        assert!(!keys.primary.is_derived());
        assert_eq!(keys.previous_window, None);
    }

    #[test]
    fn derived_keys_are_stable_within_a_window_and_change_across_windows() {
        let lead = lead(None, "a@b.test");
        let fp = fingerprint(&lead);
        let start = 1_800_000_000 - 1_800_000_000 % DERIVED_KEY_WINDOW_SECS;
        let early = keys_for(site(), &lead, &fp, start);
        let late = keys_for(site(), &lead, &fp, start + DERIVED_KEY_WINDOW_SECS - 1);
        let next = keys_for(site(), &lead, &fp, start + DERIVED_KEY_WINDOW_SECS);
        assert_eq!(early.primary, late.primary);
        assert_ne!(early.primary, next.primary);
        // A submit just after the boundary still finds the one just before it.
        assert_eq!(next.previous_window.as_ref(), Some(&early.primary));
        assert!(early.primary.is_derived());
        assert_eq!(early.primary.as_str().len(), 2 + 32);
    }

    #[test]
    fn derived_keys_differ_per_site() {
        let lead = lead(None, "a@b.test");
        let fp = fingerprint(&lead);
        let other: SiteId = "0192f4a0-7c1e-7b3a-9d2e-1f0a2b3c4d5f".parse().unwrap();
        assert_ne!(
            keys_for(site(), &lead, &fp, 0).primary,
            keys_for(other, &lead, &fp, 0).primary
        );
    }

    #[test]
    fn window_index_floors_negative_times_too() {
        assert_eq!(window_index(0), 0);
        assert_eq!(window_index(599), 0);
        assert_eq!(window_index(600), 1);
        assert_eq!(window_index(-1), -1);
    }

    #[test]
    fn replay_compares_content() {
        let a = fingerprint(&lead(None, "a@b.test"));
        let b = fingerprint(&lead(None, "b@b.test"));
        assert_eq!(replay(&a, &a), Replay::SameLead);
        assert_eq!(replay(&a, &b), Replay::Conflict);
        assert_eq!(Fingerprint::from_slice(a.as_bytes()), Some(a));
        assert_eq!(Fingerprint::from_slice(&[0; 31]), None);
    }
}
