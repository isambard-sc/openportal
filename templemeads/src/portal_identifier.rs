// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::error::Error;
use crate::named::NamedType;
use crate::validate::validate_identifier_component;

use serde::{Deserialize, Serialize};

///
/// A portal identifier - this is just a string with no spaces or periods.
///
/// This lives in templemeads, not in a domain-specific grammar crate,
/// because it names a fixed position in the agent hierarchy
/// (`agent::Type::Portal`) and is the type the framework's own trust
/// boundary - "who may submit a job targeting this scope" - is expressed
/// in, independent of whatever domain vocabulary a `Job`'s instruction
/// carries.
///
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PortalIdentifier {
    portal: String,
}

impl NamedType for PortalIdentifier {
    fn type_name() -> String {
        "PortalIdentifier".to_string()
    }
}

impl PortalIdentifier {
    pub fn parse(identifier: &str) -> Result<Self, Error> {
        let portal = identifier.trim();

        // Validated against the same allow-list every other identifier
        // component uses. This previously checked only for emptiness, spaces
        // and periods, which made `PortalIdentifier` the one identifier type
        // that F5's charset hardening never reached - despite being
        // deserialised straight off the wire as the sole argument of
        // `GetProjects`/`GetAwards`/`GetUsageReports`/`GetStorageReports`, and
        // despite `from_validated` below documenting the invariant as already
        // established. See
        // `docs/specifications/security-review-2.md` (finding R18).
        validate_identifier_component(portal, "portal", identifier)?;

        Ok(Self {
            portal: portal.to_string(),
        })
    }

    /// Construct directly from a string component that a domain crate has
    /// already validated as part of parsing its own identifier type (e.g.
    /// the portal segment of a `project.portal`-shaped identifier). Does
    /// not re-validate - this exists so domain crates (which cannot see
    /// this struct's private field) can still build a `PortalIdentifier`
    /// out of an identifier they already own.
    pub fn from_validated(portal: String) -> Self {
        Self { portal }
    }

    pub fn portal(&self) -> String {
        self.portal.clone()
    }
}

impl std::fmt::Display for PortalIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.portal)
    }
}

/// Serialize and Deserialize via the string representation
impl Serialize for PortalIdentifier {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PortalIdentifier {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_portal_identifier_applies_the_identifier_allow_list() {
        // Regression test for finding R18. This type was the one identifier
        // that F5's charset hardening never reached: it checked only for
        // emptiness, spaces and periods, while being deserialised straight off
        // the wire as the sole argument of `GetProjects`/`GetAwards`/
        // `GetUsageReports`/`GetStorageReports`.
        assert!(PortalIdentifier::parse("brics").is_ok());
        assert!(PortalIdentifier::parse("a-b_c1").is_ok());
        assert!(PortalIdentifier::parse("  brics  ").is_ok());

        for bad in [
            "",
            "  ",
            "../../etc",
            "-rf",
            "a/b",
            "a b",
            "a.b",
            "a,b",
            "a;b",
            "a$b",
            "a\0b",
            "café",
        ] {
            assert!(
                PortalIdentifier::parse(bad).is_err(),
                "{:?} must be rejected",
                bad
            );
        }

        // ...and the length cap applies, so a 1 MB portal name cannot be used
        // as an amplification vector.
        assert!(PortalIdentifier::parse(&"x".repeat(65)).is_err());
        assert!(PortalIdentifier::parse(&"x".repeat(64)).is_ok());
    }

    #[test]
    fn test_portal_identifier_deserialize_routes_through_parse() {
        // The wire path must be validated, not just the CLI path.
        let good: Result<PortalIdentifier, _> = serde_json::from_str(r#""brics""#);
        assert!(good.is_ok());

        for bad in [r#""../../etc""#, r#""-rf""#, r#""a b""#, r#""""#] {
            let result: Result<PortalIdentifier, _> = serde_json::from_str(bad);
            assert!(result.is_err(), "{} must be rejected on deserialize", bad);
        }
    }
}
