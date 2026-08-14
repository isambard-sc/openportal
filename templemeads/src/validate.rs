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

/// Maximum length of a whole email address, and of its local part.
///
/// RFC 5321 §4.5.3.1 caps the local part at 64 octets and the whole path at
/// 256; 254 is the longest address that can actually appear in a `MAIL FROM`
/// without exceeding that. These are separate from
/// [`MAX_IDENTIFIER_COMPONENT_LEN`] because an email address is not an
/// identifier component - it never becomes a Unix name or path segment (see
/// [`LocalUser`]), so the 64-character cap that bounds those does not apply.
pub const MAX_EMAIL_LEN: usize = 254;
pub const MAX_EMAIL_LOCAL_PART_LEN: usize = 64;

/// The "local user" half of a user mapping, which means two different things
/// depending on which layer produced the mapping.
///
/// An account agent (`op-freeipa`, `op-localaccount`) reports the *Unix account
/// name* it created. A portal reports the member's *email address*, because at
/// the portal level the email is the equivalent of a Unix username - there are
/// no Unix accounts there to name (see `get_users` in `portal/src/main.rs`).
/// Both travel in the same `local_user` position of the same wire format, so
/// the string alone cannot say which one it is.
///
/// That matters because a Unix-side `local_user` flows unescaped into
/// privileged operations - `useradd` operands, filesystem path components,
/// Slurm account names, FreeIPA RPC parameters - which is why
/// [`validate_mapping_target`] restricts it so tightly. An email address cannot
/// meet those rules (`@` alone is disqualifying, and real addresses also carry
/// `+`), so the two forms need different grammars.
///
/// Rather than widen the mapping-target rules - which would have relaxed the
/// charset for *every* consumer, including the ones spawning processes - the
/// distinction is made once, here, at the point the value is parsed. Each
/// variant is validated against its own grammar, and consumers then state which
/// they need:
///
/// * [`LocalUser::unix`] returns the name only when it really is a Unix
///   account, and errors otherwise. Anything that builds a path, an operand, or
///   an RPC parameter must go through it.
/// * [`LocalUser::as_str`] returns the raw string for display, serialisation,
///   and portal-facing reports, where either form is fine.
///
/// Because `local_user()` hands back a `&LocalUser` rather than a `&str`, a
/// consumer cannot reach the underlying string without choosing one of those,
/// so a portal-supplied email can never silently arrive where a Unix name was
/// assumed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LocalUser {
    /// A local Unix account name, validated by [`validate_mapping_target`].
    Unix(String),

    /// An email address, used where the mapping was produced by a portal.
    Email(String),
}

impl LocalUser {
    /// Parse a `local_user` value, choosing the variant by whether it contains
    /// an `@`. The two grammars are disjoint - [`validate_mapping_target`] does
    /// not permit `@` - so the discriminator cannot be ambiguous.
    pub fn parse(value: &str) -> Result<Self, Error> {
        let value = value.trim();

        match value.contains('@') {
            true => {
                validate_email(value, "local_user")?;
                Ok(Self::Email(value.to_string()))
            }
            false => {
                validate_mapping_target(value, "local_user", value)?;
                Ok(Self::Unix(value.to_string()))
            }
        }
    }

    /// The underlying string, whichever form it takes. Use for display,
    /// serialisation, and portal-facing reports - never to build a Unix name,
    /// path, or command operand (use [`LocalUser::unix`] for those).
    pub fn as_str(&self) -> &str {
        match self {
            Self::Unix(value) => value,
            Self::Email(value) => value,
        }
    }

    /// The Unix account name, or an error if this mapping carries an email
    /// address instead.
    ///
    /// This is the guard that lets the `Email` form exist at all: it is the
    /// only way to obtain a `&str` for a privileged operation, so a portal
    /// mapping that reached an account, filesystem, or scheduler agent fails
    /// loudly there rather than being spliced into a path or operand.
    pub fn unix(&self) -> Result<&str, Error> {
        match self {
            Self::Unix(value) => Ok(value),
            Self::Email(value) => Err(Error::Parse(format!(
                "'{}' is an email address, not a local Unix account name. A mapping \
                 produced by a portal cannot be used where a Unix account is required.",
                value
            ))),
        }
    }

    pub fn is_unix(&self) -> bool {
        matches!(self, Self::Unix(_))
    }

    pub fn is_email(&self) -> bool {
        matches!(self, Self::Email(_))
    }
}

impl std::fmt::Display for LocalUser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Validate an email address against a deliberately conservative grammar.
///
/// This is narrower than RFC 5321 allows: quoted local parts, and the
/// `!#$%&'*/=?^`{|}~` characters that are legal but effectively unused, are all
/// rejected. The permitted local-part set is `[A-Za-z0-9._+-]` and the domain
/// is a conventional hostname, which covers the addresses portals actually hold
/// while keeping the value free of characters that are meaningful to a shell,
/// a URL, a `key=value` argument list, or a space-delimited OpenPortal
/// instruction string (see `docs/specifications/security-review-2.md`, finding
/// R14 - the same reasoning that shaped [`validate_mapping_target`]).
///
/// Narrow is the right default here: an address that this rejects can still be
/// carried by the portal in `AwardDetails.members`, which is free-form; what it
/// cannot do is enter a `UserMapping`, where the value sits one accessor away
/// from privileged operations.
pub fn validate_email(value: &str, field: &str) -> Result<(), Error> {
    if value.is_empty() {
        return Err(Error::Parse(format!(
            "Invalid {} - email address cannot be empty",
            field
        )));
    }

    if value.len() > MAX_EMAIL_LEN {
        return Err(Error::Parse(format!(
            "Invalid {} - email address is longer than {} characters '{}'",
            field, MAX_EMAIL_LEN, value
        )));
    }

    // Exactly one `@`, so the split into local part and domain is unambiguous.
    let Some((local_part, domain)) = value.split_once('@') else {
        return Err(Error::Parse(format!(
            "Invalid {} - email address must contain an '@' '{}'",
            field, value
        )));
    };

    if domain.contains('@') {
        return Err(Error::Parse(format!(
            "Invalid {} - email address must contain exactly one '@' '{}'",
            field, value
        )));
    }

    validate_email_local_part(local_part, field, value)?;
    validate_email_domain(domain, field, value)
}

fn validate_email_local_part(local_part: &str, field: &str, value: &str) -> Result<(), Error> {
    if local_part.is_empty() {
        return Err(Error::Parse(format!(
            "Invalid {} - email address has an empty local part '{}'",
            field, value
        )));
    }

    if local_part.len() > MAX_EMAIL_LOCAL_PART_LEN {
        return Err(Error::Parse(format!(
            "Invalid {} - email local part is longer than {} characters '{}'",
            field, MAX_EMAIL_LOCAL_PART_LEN, value
        )));
    }

    // A leading `-` is rejected for the same reason as in an identifier
    // component: a value that starts with a dash can be read as a flag by a
    // spawned tool.
    if local_part.starts_with('-') {
        return Err(Error::Parse(format!(
            "Invalid {} - email local part cannot start with '-' '{}'",
            field, value
        )));
    }

    if let Some(bad) = local_part
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+')))
    {
        return Err(Error::Parse(format!(
            "Invalid {} - email local part contains an illegal character '{}' \
             (allowed: A-Z, a-z, 0-9, '.', '_', '-', '+') '{}'",
            field, bad, value
        )));
    }

    // The same path-shaped hazards as `validate_mapping_target` guards against:
    // even though an email never becomes a path component, keeping the rule
    // identical means one form cannot be used to smuggle a shape the other
    // rejects.
    if local_part.starts_with('.') || local_part.ends_with('.') {
        return Err(Error::Parse(format!(
            "Invalid {} - email local part cannot start or end with '.' '{}'",
            field, value
        )));
    }

    if local_part.contains("..") {
        return Err(Error::Parse(format!(
            "Invalid {} - email local part cannot contain '..' '{}'",
            field, value
        )));
    }

    Ok(())
}

fn validate_email_domain(domain: &str, field: &str, value: &str) -> Result<(), Error> {
    if domain.is_empty() {
        return Err(Error::Parse(format!(
            "Invalid {} - email address has an empty domain '{}'",
            field, value
        )));
    }

    let labels: Vec<&str> = domain.split('.').collect();

    // A bare hostname is not a routable address, and accepting one would also
    // accept `user@localhost`-style values that mean different things on
    // different hosts.
    if labels.len() < 2 {
        return Err(Error::Parse(format!(
            "Invalid {} - email domain must have at least two labels '{}'",
            field, value
        )));
    }

    for label in labels {
        if label.is_empty() {
            return Err(Error::Parse(format!(
                "Invalid {} - email domain has an empty label '{}'",
                field, value
            )));
        }

        if label.len() > MAX_DOMAIN_LABEL_LEN {
            return Err(Error::Parse(format!(
                "Invalid {} - email domain label is longer than {} characters '{}'",
                field, MAX_DOMAIN_LABEL_LEN, value
            )));
        }

        if label.starts_with('-') || label.ends_with('-') {
            return Err(Error::Parse(format!(
                "Invalid {} - email domain label cannot start or end with '-' '{}'",
                field, value
            )));
        }

        if let Some(bad) = label
            .chars()
            .find(|c| !(c.is_ascii_alphanumeric() || *c == '-'))
        {
            return Err(Error::Parse(format!(
                "Invalid {} - email domain contains an illegal character '{}' \
                 (allowed: A-Z, a-z, 0-9, '-') '{}'",
                field, bad, value
            )));
        }
    }

    Ok(())
}

/// Maximum length of a single DNS label, per RFC 1035 §2.3.4.
const MAX_DOMAIN_LABEL_LEN: usize = 63;

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
    let allowed =
        parse_allow_invalid_ssl_certs(std::env::var("OPENPORTAL_ALLOW_INVALID_SSL_CERTS").ok());

    // Announce it **once** per process, not per call.
    //
    // This is read every time an HTTPS client is built - so on every FreeIPA call -
    // and logging each time would bury the rest of the log. `Once` gives the operator
    // one clear statement of the process's TLS posture at the point it first matters.
    //
    // Note this is a legitimate operator decision, not a misconfiguration: FreeIPA is
    // commonly deployed with a local-only CA, and whether to trust such a certificate
    // is the operator's call - TLS is outside OpenPortal's control. It is stated
    // plainly rather than warned about because it was previously silent, and an
    // operator who set it in a development shell had no way to see it was still set.
    // See `docs/specifications/security-review-2.md` (finding R33).
    if allowed {
        static ANNOUNCED: std::sync::Once = std::sync::Once::new();

        ANNOUNCED.call_once(|| {
            tracing::info!(
                "OPENPORTAL_ALLOW_INVALID_SSL_CERTS is set, so TLS certificate \
                 verification is disabled for this process's outbound HTTPS calls. \
                 This is expected if your FreeIPA servers use a local-only CA. Note it \
                 applies process-wide, including the connection carrying the FreeIPA \
                 bind password, so the network path to those servers must be trusted."
            );
        });
    }

    allowed
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

    #[test]
    fn test_local_user_picks_its_variant_from_the_at_sign() {
        // No `@` - a Unix account name, held to the mapping-target rules.
        let unix = LocalUser::parse("bob.proj").expect("bob.proj must parse");
        assert_eq!(unix, LocalUser::Unix("bob.proj".to_string()));
        assert!(unix.is_unix());
        assert_eq!(unix.unix().expect("a Unix name must be usable"), "bob.proj");

        // With an `@` - an email, which no Unix consumer may use.
        let email = LocalUser::parse("alice@example.com").expect("an address must parse");
        assert_eq!(email, LocalUser::Email("alice@example.com".to_string()));
        assert!(email.is_email());
        assert_eq!(email.as_str(), "alice@example.com");

        // The whole point of the enum: the string is reachable for display,
        // but not as a Unix account name.
        let err = email
            .unix()
            .expect_err("an email must not be usable as a Unix account name");
        assert!(
            err.to_string().contains("not a local Unix account name"),
            "unexpected error: {}",
            err
        );

        // Surrounding whitespace is trimmed, not rejected, matching
        // `UserMapping::parse`'s treatment of the other components.
        assert_eq!(
            LocalUser::parse("  alice@example.com  ").expect("must parse"),
            LocalUser::Email("alice@example.com".to_string())
        );
    }

    #[test]
    fn test_the_email_form_does_not_widen_the_unix_form() {
        // Characters the email grammar allows must still be rejected in a Unix
        // account name - the two grammars are separate, not layered.
        for bad in ["alice+hpc", "a@b"] {
            assert!(
                validate_mapping_target(bad, "local_user", bad).is_err(),
                "{:?} must not be a valid Unix mapping target",
                bad
            );
        }

        // ...and `LocalUser` routes each to the grammar that judges it: `+`
        // without an `@` is not an address, so it is judged as a Unix name and
        // rejected.
        assert!(LocalUser::parse("alice+hpc").is_err());
    }

    #[test]
    fn test_validate_email_accepts_real_addresses() {
        for good in [
            "alice@example.com",
            "alice.smith@example.com",
            "alice+hpc@example.com",
            "alice_smith@bristol.ac.uk",
            "a-b@sub.domain.example.com",
            "123@example.com",
            "Alice.Smith@Example.COM",
        ] {
            assert!(
                validate_email(good, "local_user").is_ok(),
                "{:?} must be accepted",
                good
            );
        }
    }

    #[test]
    fn test_validate_email_rejects_dangerous_and_malformed_addresses() {
        for bad in [
            // The R14 classes: whitespace shifts arguments in a space-delimited
            // instruction, `,` is a Slurm list separator, `?`/`#`/`%`/`=` matter
            // in a REST URL, and the rest are shell- or path-significant.
            "alice evil@example.com",
            "alice\tevil@example.com",
            "alice\n@example.com",
            "a,b@example.com",
            "a=b@example.com",
            "a%2f@example.com",
            "a?x=1@example.com",
            "a#b@example.com",
            "a/b@example.com",
            "a;b@example.com",
            "a$b@example.com",
            "a*b@example.com",
            "a:b@example.com",
            "a\0b@example.com",
            "a!b@example.com",
            "a'b@example.com",
            "\"alice\"@example.com",
            "café@example.com",
            // Structural problems.
            "alice@@example.com",
            "alice@example@com.org",
            "@example.com",
            "alice@",
            "alice@localhost",
            "alice@.com",
            "alice@example..com",
            "alice@-example.com",
            "alice@example-.com",
            "alice@exa_mple.com",
            ".alice@example.com",
            "alice.@example.com",
            "a..b@example.com",
            "-alice@example.com",
            "",
        ] {
            assert!(
                validate_email(bad, "local_user").is_err(),
                "{:?} must be rejected",
                bad
            );
            // And nothing rejected by the grammar may sneak in via `LocalUser`.
            assert!(
                LocalUser::parse(bad).is_err(),
                "{:?} must not parse as a LocalUser",
                bad
            );
        }

        // Length caps.
        assert!(validate_email(&format!("{}@example.com", "x".repeat(65)), "f").is_err());
        assert!(validate_email(&format!("alice@{}.com", "x".repeat(64)), "f").is_err());
        assert!(
            validate_email(&format!("{}@{}.com", "x".repeat(64), "y".repeat(200)), "f").is_err()
        );
    }
}
