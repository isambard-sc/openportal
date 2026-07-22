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

/// Proves the `Erased` domain-oblivious router design
/// (`docs/plans/multi-domain-routing-design.md`) actually round-trips real
/// `greatwestern` Jobs/Notifications unchanged. Lives here (not in
/// `templemeads`) because it needs a real, concrete `Domain` to relay -
/// `templemeads` itself must never depend on `greatwestern`.
#[cfg(test)]
mod erased_routing_tests {
    use crate::Hpc;
    use templemeads::erased::Erased;
    use templemeads::job::Job;
    use templemeads::notification::Notification;

    #[test]
    fn test_job_roundtrips_through_erased_router() {
        #[allow(clippy::expect_used)]
        let job = Job::<Hpc>::parse("portal.cluster add_user alice.myproject.myportal", false)
            .expect("valid instruction");
        #[allow(clippy::expect_used)]
        let original_json = serde_json::to_string(&job).expect("serialises");

        // A domain-oblivious router receives this over the wire...
        #[allow(clippy::expect_used)]
        let relayed: Job<Erased> =
            serde_json::from_str(&original_json).expect("Erased must accept any Hpc job");
        #[allow(clippy::expect_used)]
        let relayed_json = serde_json::to_string(&relayed).expect("serialises");

        assert_eq!(relayed_json, original_json);
    }

    #[test]
    fn test_job_domain_provenance_survives_erased_hop() {
        #[allow(clippy::expect_used)]
        let job = Job::<Hpc>::parse("portal.cluster add_user alice.myproject.myportal", false)
            .expect("valid instruction");
        assert_eq!(job.domain(), Some("greatwestern"));

        #[allow(clippy::expect_used)]
        let json = serde_json::to_string(&job).expect("serialises");
        #[allow(clippy::expect_used)]
        let relayed: Job<Erased> = serde_json::from_str(&json).expect("deserialises");

        // The router's own domain is "erased", but the job's own tag - set
        // once at the true origin - is untouched by relaying through it.
        assert_eq!(relayed.domain(), Some("greatwestern"));
        assert_eq!(relayed.domain_version(), job.domain_version());
    }

    #[test]
    fn test_job_survives_multiple_erased_hops() {
        #[allow(clippy::expect_used)]
        let job =
            Job::<Hpc>::parse("portal.cluster get_offerings", false).expect("valid instruction");
        #[allow(clippy::expect_used)]
        let json = serde_json::to_string(&job).expect("serialises");

        #[allow(clippy::expect_used)]
        let hop1: Job<Erased> = serde_json::from_str(&json).expect("hop 1 deserialises");
        #[allow(clippy::expect_used)]
        let hop1_json = serde_json::to_string(&hop1).expect("hop 1 re-serialises");
        #[allow(clippy::expect_used)]
        let hop2: Job<Erased> = serde_json::from_str(&hop1_json).expect("hop 2 deserialises");
        #[allow(clippy::expect_used)]
        let hop2_json = serde_json::to_string(&hop2).expect("hop 2 re-serialises");

        assert_eq!(hop2_json, json);
        assert_eq!(hop2.domain(), Some("greatwestern"));
    }

    #[test]
    fn test_notification_roundtrips_through_erased_router() {
        #[allow(clippy::expect_used)]
        let notification =
            Notification::<Hpc>::parse("portal.clusters.shared user_added chris.project.brics")
                .expect("valid notification");
        #[allow(clippy::expect_used)]
        let original_json = serde_json::to_string(&notification).expect("serialises");

        #[allow(clippy::expect_used)]
        let relayed: Notification<Erased> =
            serde_json::from_str(&original_json).expect("Erased must accept any Hpc notification");
        #[allow(clippy::expect_used)]
        let relayed_json = serde_json::to_string(&relayed).expect("serialises");

        assert_eq!(relayed_json, original_json);
        assert_eq!(relayed.domain(), Some("greatwestern"));
    }
}
