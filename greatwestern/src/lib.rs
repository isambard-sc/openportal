// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! `greatwestern` is the HPC/Waldur command vocabulary that rides on top of
//! `paddington` and `templemeads` - the reference [`Domain`] every built-in
//! OpenPortal agent (freeipa, slurm, filesystem, cluster, portal, bridge,
//! cloudaccount, localaccount, cloudportal, ...) is compiled against.
//!
//! Everything domain-specific that used to live inside `templemeads` - the
//! `Instruction` enum, `ProjectIdentifier`/`UserIdentifier`, usage/storage
//! reports, and the notification event vocabulary - lives here instead, so
//! that templemeads itself stays generic over any `Domain` a developer
//! wants to bring for a different kind of infrastructure entirely.

pub mod grammar;
mod job_bindings;
pub mod notification;
pub mod storage;
pub mod storagereport;
pub mod usagereport;

// Needed only so ts-rs's "uuid-impl" feature (which implements TS for
// uuid::Uuid, used transitively via chrono/serde derive on our report
// types) is enabled - nothing here calls into the uuid crate directly.
use uuid as _;

pub use grammar::Instruction;
pub use notification::NotificationEvent;

use templemeads::domain::Domain;
use templemeads::notification::Notification;
use templemeads::portal_identifier::PortalIdentifier;
use templemeads::Error;

/// The HPC/Waldur `Domain`: OpenPortal's original, built-in command
/// vocabulary. A zero-sized marker type - it only ever appears as a type
/// parameter (`Job<Hpc>`, `Board<Hpc>`, ...), never as a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hpc;

impl Domain for Hpc {
    type Instruction = Instruction;
    type NotificationEvent = NotificationEvent;

    fn parse_instruction(s: &str) -> Result<Self::Instruction, Error> {
        Instruction::parse(s)
    }

    fn parse_notification_event(s: &str) -> Result<Self::NotificationEvent, Error> {
        NotificationEvent::parse(s)
    }

    fn name() -> &'static str {
        "greatwestern"
    }

    fn version() -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn owning_portal(instruction: &Self::Instruction) -> Option<PortalIdentifier> {
        grammar::owning_portal(instruction)
    }

    fn assume_legacy_domain_version(engine_version: &str) -> Option<&'static str> {
        // Before the templemeads/greatwestern split, templemeads only ever
        // spoke this vocabulary - there was no separable "domain" at all, so
        // any templemeads peer at or below the last pre-split release
        // (0.32.2) was unambiguously speaking greatwestern 0.32.2, whatever
        // its own engine version happens to be. This is a historical fact
        // tied to this exact crate split, not a general compatibility guess
        // - it's why the threshold is hardcoded rather than derived from
        // `version()` above.
        match parse_simple_version(engine_version) {
            Some(v) if v <= (0, 32, 2) => Some("0.32.2"),
            _ => None,
        }
    }

    fn wrap_forward(inner: Notification<Self>) -> Self::NotificationEvent {
        NotificationEvent::Forward(Box::new(inner))
    }
}

/// Parses a plain `MAJOR.MINOR.PATCH` version string (no pre-release/build
/// metadata - this codebase has never used either). Returns `None` on any
/// other shape, so an unparseable version is treated as "not eligible for
/// the legacy assumption" rather than guessed at.
fn parse_simple_version(s: &str) -> Option<(u64, u64, u64)> {
    let mut parts = s.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    match parts.next() {
        None => Some((major, minor, patch)),
        Some(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_assume_legacy_domain_version() {
        assert_eq!(Hpc::assume_legacy_domain_version("0.32.2"), Some("0.32.2"));
        assert_eq!(Hpc::assume_legacy_domain_version("0.32.1"), Some("0.32.2"));
        assert_eq!(Hpc::assume_legacy_domain_version("0.10.0"), Some("0.32.2"));
        assert_eq!(Hpc::assume_legacy_domain_version("0.33.0"), None);
        assert_eq!(Hpc::assume_legacy_domain_version("1.0.0"), None);
        assert_eq!(Hpc::assume_legacy_domain_version("not-a-version"), None);
        assert_eq!(Hpc::assume_legacy_domain_version("0.32"), None);
        assert_eq!(Hpc::assume_legacy_domain_version("0.32.2.1"), None);
    }

    #[test]
    fn test_name_and_version() {
        assert_eq!(Hpc::name(), "greatwestern");
        assert!(!Hpc::version().is_empty());
    }
}
