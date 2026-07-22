// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::error::Error;
use crate::named::NamedType;
use crate::notification::Notification;
use crate::portal_identifier::PortalIdentifier;

use serde::{Deserialize, Serialize};

///
/// The compile-time choice of command vocabulary a set of OpenPortal agents
/// is built against. Everything templemeads moves around - `Job`, `Board`,
/// `Notification`, ... - is generic over one `L: Domain`, so a developer can
/// bring their own instruction/notification vocabulary (in its own crate)
/// and reuse paddington/templemeads without forking either. Two agents
/// built against different `Domain`s are not expected to interoperate: a
/// `Job<A>` and a `Job<B>` are different types with no conversion between
/// them, and that's intentional.
///
pub trait Domain: Clone + std::fmt::Debug + 'static {
    /// The command vocabulary a `Job` carries: what an agent is being asked
    /// to do, and with what arguments.
    type Instruction: Clone
        + PartialEq
        + std::fmt::Debug
        + std::fmt::Display
        + Serialize
        + for<'de> Deserialize<'de>
        + NamedType
        + Send
        + Sync
        + 'static;

    /// The fire-and-forget event vocabulary a `Notification` carries.
    type NotificationEvent: Clone
        + PartialEq
        + std::fmt::Debug
        + std::fmt::Display
        + Serialize
        + for<'de> Deserialize<'de>
        + Send
        + Sync
        + 'static;

    /// Parse an `Instruction` from the text that follows the destination in
    /// a command string (e.g. `"add_user alice.myproject.myportal"`).
    fn parse_instruction(s: &str) -> Result<Self::Instruction, Error>;

    /// Parse a `NotificationEvent` from the text that follows the event
    /// name in a notification string.
    fn parse_notification_event(s: &str) -> Result<Self::NotificationEvent, Error>;

    /// The portal that "owns" this instruction, if it has one - i.e. whose
    /// name a job's destination's first hop must match. `PortalIdentifier`
    /// lives in templemeads itself (it names a fixed position in the agent
    /// hierarchy, not domain vocabulary), so this is expressed in the real
    /// type, not a bare string. Default: no such policy - a new domain
    /// opts into this only if it needs it.
    fn owning_portal(_instruction: &Self::Instruction) -> Option<PortalIdentifier> {
        None
    }

    /// Wrap an inner `Notification` for southbound forwarding: used by a
    /// bridge agent to ask the portal to forward a notification, stripping
    /// the bridge from the path (analogous to `Job`'s `submit` instruction).
    /// Every domain must provide this so templemeads' bridge infrastructure
    /// (which cannot see any domain's concrete event vocabulary) can still
    /// construct this one infrastructure-level event generically. Unwrapping
    /// it back is a domain-level concern (e.g. the portal agent's own notify
    /// runner matches its concrete `NotificationEvent::Forward` variant
    /// directly), so templemeads has no need for the reverse operation.
    fn wrap_forward(inner: Notification<Self>) -> Self::NotificationEvent
    where
        Self: Sized;
}
