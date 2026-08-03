// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! Shared validation for the string components that make up identifiers.
//!
//! This lives in `templemeads` rather than in a domain crate because both
//! layers need it: `templemeads::portal_identifier::PortalIdentifier` is
//! defined here, while the identifier types that *contain* a portal component
//! (`ProjectIdentifier`, `UserIdentifier`, and the mapping types) are defined
//! by each `Domain` - and a domain crate cannot be a dependency of
//! `templemeads`. Keeping one implementation here means an identifier
//! component is validated the same way whichever layer parses it.
//!
//! See `docs/specifications/security-review.md` (finding F5) for the original
//! allow-list, and `docs/specifications/security-review-2.md` (findings R14 and
//! R18) for the two gaps this module was created to close.

use crate::Error;

/// Maximum length of any single identifier component (username, project, or
/// portal). Generous enough for any real portal/project/user name while
/// bounding the size of everything derived from it downstream (Unix account
/// and group names, filesystem path components, Slurm account names, FreeIPA
/// cn/uid values).
pub const MAX_IDENTIFIER_COMPONENT_LEN: usize = 64;

/// Validate a single identifier component against a strict allow-list.
///
/// Identifier components flow, unescaped, into privileged operations: Unix
/// `useradd`/`groupadd` operands, filesystem paths, Slurm account names,
/// FreeIPA RPC parameters, and (for `op-cloudaccount`) state-file names.
/// Restricting them to `[A-Za-z0-9_-]`, forbidding a leading `-`, and capping
/// the length closes argument-injection (a leading-dash name read as a flag by
/// a spawned tool), path-traversal (a `/` or `.` in a name), and
/// resource-exhaustion vectors at the point identifiers enter the system. See
/// `docs/specifications/security-review.md` (finding F5).
pub fn validate_identifier_component(
    value: &str,
    field: &str,
    identifier: &str,
) -> Result<(), Error> {
    validate_component(value, field, identifier, false)
}

/// As [`validate_identifier_component`], but additionally permitting `.` in the
/// interior of the value - for a *mapping target* (a local Unix user or group
/// name, or a Slurm account name) rather than an identifier component.
///
/// Mapping targets legitimately contain `.` (a local account derived from
/// `user.project` is named `user.project`), which is why they originally got a
/// deny-list rather than this allow-list. That deny-list turned out to permit
/// whitespace, `,`, `=`, `%`, `?` and `#`, each of which matters because these
/// names are not only passed to spawned tools as operands but also interpolated
/// into space-delimited OpenPortal instruction strings (where a space shifts
/// every later argument), into `sacctmgr` `key=value` arguments (where a comma
/// is a list separator), and into Slurm REST URLs (where `?` starts a query).
/// See `docs/specifications/security-review-2.md` (finding R14).
pub fn validate_mapping_target(value: &str, field: &str, mapping: &str) -> Result<(), Error> {
    validate_component(value, field, mapping, true)?;

    // A leading or trailing `.` would make the name resolve to the current or
    // parent directory when used as a path component, and `..` anywhere is
    // never a legitimate account name.
    if value.starts_with('.') || value.ends_with('.') {
        return Err(Error::Parse(format!(
            "Invalid {} - cannot start or end with '.' '{}'",
            field, mapping
        )));
    }

    if value.contains("..") {
        return Err(Error::Parse(format!(
            "Invalid {} - cannot contain '..' '{}'",
            field, mapping
        )));
    }

    Ok(())
}

fn validate_component(
    value: &str,
    field: &str,
    identifier: &str,
    allow_period: bool,
) -> Result<(), Error> {
    if value.is_empty() {
        return Err(Error::Parse(format!(
            "Invalid identifier - {} cannot be empty '{}'",
            field, identifier
        )));
    }

    if value.len() > MAX_IDENTIFIER_COMPONENT_LEN {
        return Err(Error::Parse(format!(
            "Invalid identifier - {} is longer than {} characters '{}'",
            field, MAX_IDENTIFIER_COMPONENT_LEN, identifier
        )));
    }

    if value.starts_with('-') {
        return Err(Error::Parse(format!(
            "Invalid identifier - {} cannot start with '-' '{}'",
            field, identifier
        )));
    }

    if let Some(bad) = value.chars().find(|c| {
        !(c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || (allow_period && *c == '.'))
    }) {
        return Err(Error::Parse(format!(
            "Invalid identifier - {} contains an illegal character '{}' \
             (allowed: A-Z, a-z, 0-9, '_', '-'{}) '{}'",
            field,
            bad,
            if allow_period { ", '.'" } else { "" },
            identifier
        )));
    }

    Ok(())
}

///
/// Whether TLS certificate verification should be disabled for outbound HTTPS
/// calls, per the `OPENPORTAL_ALLOW_INVALID_SSL_CERTS` environment variable.
///
/// This is a development escape hatch: `op-freeipa` and `op-bridge` both build
/// `reqwest` clients with `danger_accept_invalid_certs`, and turning it on
/// makes those connections trivially interceptable. It lives here so there is
/// exactly one implementation of the rule (and one test of it) rather than a
/// copy per agent that can drift into being more permissive.
///
/// Fails closed: only the literal string `true` (in any case) enables it.
///
pub fn allow_invalid_ssl_certs() -> bool {
    parse_allow_invalid_ssl_certs(std::env::var("OPENPORTAL_ALLOW_INVALID_SSL_CERTS").ok())
}

/// The pure part of [`allow_invalid_ssl_certs`], so the rule can be tested
/// without mutating process-global environment state.
pub fn parse_allow_invalid_ssl_certs(value: Option<String>) -> bool {
    match value {
        Some(value) => value.trim().to_lowercase() == "true",
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_disabling_tls_verification_requires_an_exact_opt_in() {
        // Fail closed: anything other than "true" leaves certificate
        // verification on. A typo'd or partially-set value must not silently
        // disable TLS checking for op-freeipa's and op-bridge's outbound calls.
        assert!(parse_allow_invalid_ssl_certs(Some("true".to_string())));
        assert!(parse_allow_invalid_ssl_certs(Some("TRUE".to_string())));
        assert!(parse_allow_invalid_ssl_certs(Some(" True \n".to_string())));

        for off in ["", " ", "false", "1", "yes", "on", "truthy", "0"] {
            assert!(
                !parse_allow_invalid_ssl_certs(Some(off.to_string())),
                "{:?} must not disable certificate verification",
                off
            );
        }

        assert!(!parse_allow_invalid_ssl_certs(None));
    }

    #[test]
    fn test_validate_identifier_component_allow_list() {
        assert!(validate_identifier_component("user", "f", "i").is_ok());
        assert!(validate_identifier_component("a-b_c1", "f", "i").is_ok());

        // rejected: empty, too long, leading dash, and anything outside the
        // allow-list - including the period that mapping targets may use.
        assert!(validate_identifier_component("", "f", "i").is_err());
        assert!(validate_identifier_component(&"x".repeat(65), "f", "i").is_err());
        assert!(validate_identifier_component("-x", "f", "i").is_err());
        assert!(validate_identifier_component("a.b", "f", "i").is_err());

        for bad in [
            "a/b",
            "a b",
            "a,b",
            "a=b",
            "a%b",
            "a?b",
            "a#b",
            "a;b",
            "a$b",
            "a*b",
            "a:b",
            "a\tb",
            "a\nb",
            "a\0b",
            "ⅼocalhost",
            "café",
        ] {
            assert!(
                validate_identifier_component(bad, "f", "i").is_err(),
                "{:?} must be rejected",
                bad
            );
        }
    }

    #[test]
    fn test_validate_mapping_target_permits_interior_period_only() {
        // The legitimate shape: a local account named after user.project.
        assert!(validate_mapping_target("bob.proj", "local_user", "m").is_ok());
        assert!(validate_mapping_target("portal.project", "local_group", "m").is_ok());
        assert!(validate_mapping_target("plain", "local_group", "m").is_ok());

        // The classes finding R14 was about: whitespace shifts arguments in a
        // space-delimited instruction, a comma is a Slurm list separator, and
        // `?`/`#`/`%`/`=` matter in a REST URL.
        for bad in [
            "grp evil",
            "grp\tevil",
            "a,b",
            "a=b",
            "a%2fb",
            "a?with_deleted=true",
            "a#b",
            "a*b",
            "a$b",
            "a/b",
            "-grp",
            ".grp",
            "grp.",
            "a..b",
            "",
        ] {
            assert!(
                validate_mapping_target(bad, "local_group", "m").is_err(),
                "{:?} must be rejected",
                bad
            );
        }

        assert!(validate_mapping_target(&"x".repeat(65), "local_group", "m").is_err());
    }
}
