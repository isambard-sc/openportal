// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::error::Error;
use crate::named::NamedType;

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
    fn type_name() -> &'static str {
        "PortalIdentifier"
    }
}

impl NamedType for Vec<PortalIdentifier> {
    fn type_name() -> &'static str {
        "Vec<PortalIdentifier>"
    }
}

impl PortalIdentifier {
    pub fn parse(identifier: &str) -> Result<Self, Error> {
        let portal = identifier.trim();

        if portal.is_empty() {
            return Err(Error::Parse(format!(
                "Invalid PortalIdentifier - portal cannot be empty '{}'",
                identifier
            )));
        };

        if portal.contains(' ') || portal.contains('.') {
            return Err(Error::Parse(format!(
                "Invalid PortalIdentifier - portal cannot contain spaces or periods '{}'",
                identifier
            )));
        };

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
