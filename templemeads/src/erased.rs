// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! A `Domain` that understands nothing and forwards everything - for
//! routing-only agents (e.g. `provider`) that sit between leaf agents
//! speaking real, possibly-different `Domain`s.
//!
//! See `docs/plans/archive/multi-domain-routing-design.md` for the full design and
//! rationale.

use crate::domain::Domain;
use crate::error::Error;
use crate::notification::Notification;

use serde::{Deserialize, Serialize};
use std::fmt;

///
/// The raw text of an instruction this agent doesn't understand and never
/// needs to - captured verbatim so it can be forwarded unchanged.
///
/// `Command<L>` (the private struct behind `Job::command`) serialises via
/// `Display`/`parse` to a single string, so a `RawInstruction` that never
/// fails to parse and reproduces exactly what it was given round-trips
/// byte-for-byte identically to whatever the originating leaf agent sent.
///
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawInstruction(String);

impl fmt::Display for RawInstruction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

///
/// The raw JSON shape of a notification event this agent doesn't
/// understand, plus the one structured case every `Domain` must support
/// (see [`Domain::wrap_forward`]).
///
/// Unlike `Instruction`, `NotificationEvent` has no custom string
/// serialisation - it serialises via an ordinary derive, so the wire shape
/// is a structured JSON object (one key per variant), not a display
/// string. `#[serde(untagged)]` tries `Forward` first (matches only JSON
/// that happens to have exactly a `Notification`'s shape), falling
/// through to `Raw` - a `serde_json::Value` - for everything else, which
/// always succeeds and preserves whatever JSON shape the real `Domain`'s
/// event serialised as.
///
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RawNotificationEvent {
    Forward(Box<Notification<Erased>>),
    Raw(serde_json::Value),
}

impl fmt::Display for RawNotificationEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Forward(n) => write!(f, "forward [{}]", n),
            Self::Raw(v) => write!(f, "{}", v),
        }
    }
}

///
/// A `Domain` that understands nothing and forwards everything - for
/// routing-only agents that sit between leaf agents speaking real,
/// possibly-different `Domain`s. A zero-sized marker type - it only ever
/// appears as a type parameter (`Job<Erased>`, `Board<Erased>`, ...), never
/// as a value.
///
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Erased;

impl Domain for Erased {
    type Instruction = RawInstruction;
    type NotificationEvent = RawNotificationEvent;

    fn parse_instruction(s: &str) -> Result<Self::Instruction, Error> {
        Ok(RawInstruction(s.to_string()))
    }

    fn parse_notification_event(s: &str) -> Result<Self::NotificationEvent, Error> {
        // Only reachable via `Notification::parse` (a text command, e.g. a
        // bridge's `POST /notify`) - never via ordinary wire deserialisation,
        // since `Notification<L>` deserialises `event` directly through
        // `L::NotificationEvent`'s own `Deserialize` impl, not through this
        // function. Wraps the text as a JSON string value; never produces
        // `Forward`, matching every other `Domain`'s convention that
        // `Forward` is infrastructure-only and not parseable from text.
        Ok(RawNotificationEvent::Raw(serde_json::Value::String(
            s.to_string(),
        )))
    }

    fn name() -> &'static str {
        "erased"
    }

    fn version() -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    // owning_portal: default `None` - a router can't evaluate a
    // domain-specific ownership policy it doesn't understand. This is safe:
    // incoming `Command<L>` deserialisation always calls
    // `Command::parse(&s, false)` (check_portal = false) regardless of `L` -
    // only a *portal* agent parsing a fresh, human/bridge-supplied command
    // string with check_portal = true relies on this, and a portal is a
    // leaf role, never `Erased`.

    fn assume_legacy_domain_version(_engine_version: &str) -> Option<&'static str> {
        None // `Erased` has no pre-split history to claim
    }

    fn wrap_forward(inner: Notification<Self>) -> Self::NotificationEvent {
        RawNotificationEvent::Forward(Box::new(inner))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;
    use crate::destination::Destination;

    #[test]
    fn test_raw_instruction_roundtrip() {
        for s in [
            "add_user alice.myproject.myportal",
            "",
            "get_offerings",
            "not a real instruction but shouldn't matter",
        ] {
            let parsed = Erased::parse_instruction(s).expect("never fails");
            assert_eq!(parsed.to_string(), s);
        }
    }

    #[test]
    fn test_raw_notification_event_roundtrip_against_real_json() {
        // Simulates what a router actually receives on the wire: JSON
        // produced by a real Domain's NotificationEvent, not a synthetic
        // value. Uses a hand-written literal matching greatwestern's shape
        // rather than a live dependency on greatwestern (templemeads must
        // not depend on any domain crate).
        for json in [
            r#"{"UserAdded":"chris.project.brics"}"#,
            r#"{"ProjectChanged":"myproject.brics"}"#,
            r#""a_bare_string_event_body_should_still_work""#,
        ] {
            let event: RawNotificationEvent =
                serde_json::from_str(json).expect("Raw(Value) accepts any JSON");
            let reserialised = serde_json::to_string(&event).expect("serialises back out");
            assert_eq!(reserialised, json);
            assert!(matches!(event, RawNotificationEvent::Raw(_)));
        }
    }

    #[test]
    fn test_forward_disambiguation() {
        let inner = Notification::<Erased>::new(
            Destination::parse("a.b").expect("valid destination"),
            RawNotificationEvent::Raw(serde_json::Value::String("user_added x.y.z".to_string())),
        );
        let event = Erased::wrap_forward(inner.clone());
        assert!(matches!(event, RawNotificationEvent::Forward(_)));

        let json = serde_json::to_string(&event).expect("serialises");
        let roundtripped: RawNotificationEvent =
            serde_json::from_str(&json).expect("deserialises back as Forward");
        assert!(matches!(roundtripped, RawNotificationEvent::Forward(_)));
    }

    #[test]
    fn test_name_and_version() {
        assert_eq!(Erased::name(), "erased");
        assert!(!Erased::version().is_empty());
    }

    #[test]
    fn test_assume_legacy_domain_version_always_none() {
        assert_eq!(Erased::assume_legacy_domain_version("0.1.0"), None);
        assert_eq!(Erased::assume_legacy_domain_version("0.32.2"), None);
    }
}
