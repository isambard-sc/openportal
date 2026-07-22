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

    fn owning_portal(instruction: &Self::Instruction) -> Option<PortalIdentifier> {
        grammar::owning_portal(instruction)
    }

    fn wrap_forward(inner: Notification<Self>) -> Self::NotificationEvent {
        NotificationEvent::Forward(Box::new(inner))
    }
}
