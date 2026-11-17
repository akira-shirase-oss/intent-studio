//! Identifier newtypes, so a site id can never be passed where a lead id is expected.
//!
//! Generation (UUID v7) happens at the edges, where a clock and randomness exist; the domain only
//! carries and parses them.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! id_type {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Wraps an existing UUID.
            pub const fn from_uuid(id: Uuid) -> Self {
                Self(id)
            }

            /// The underlying UUID.
            pub const fn as_uuid(&self) -> Uuid {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s).map(Self)
            }
        }
    };
}

id_type!(
    /// One client business (D-015).
    TenantId
);
id_type!(
    /// One site of a tenant; leads are submitted to a site.
    SiteId
);
id_type!(
    /// One logical lead.
    LeadId
);
id_type!(
    /// One notification outbox row; also the provider's idempotency key (D-020).
    OutboxId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_through_text_and_json() {
        let text = "0192f4a0-7c1e-7b3a-9d2e-1f0a2b3c4d5e";
        let site: SiteId = text.parse().unwrap();
        assert_eq!(site.to_string(), text);
        assert_eq!(serde_json::to_string(&site).unwrap(), format!("\"{text}\""));
        assert_eq!(format!("{site:?}"), format!("SiteId({text})"));
    }

    #[test]
    fn a_malformed_id_is_refused() {
        assert!("not-a-uuid".parse::<LeadId>().is_err());
        assert!("".parse::<SiteId>().is_err());
    }
}
