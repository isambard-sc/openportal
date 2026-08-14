// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! A minimal, concrete `Domain` used only by templemeads's own test suite.
//!
//! templemeads is meant to be domain-agnostic, so it cannot test `Job`,
//! `Board`, `Command`, etc. against any real command vocabulary - there
//! isn't one here. This stub echoes whatever string it's given straight
//! back out, which is enough to exercise the generic framework machinery
//! (parsing, routing, serialisation round-trips) on its own.
#![cfg(test)]

use crate::domain::Domain;
use crate::error::Error;
use crate::named::NamedType;
use crate::notification::Notification;
use crate::portal_identifier::PortalIdentifier;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TestInstruction(pub String);

impl NamedType for TestInstruction {
    fn type_name() -> String {
        "TestInstruction".to_string()
    }
}

impl std::fmt::Display for TestInstruction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) enum TestNotificationEvent {
    Echo(String),
    Forward(Box<Notification<TestDomain>>),
}

impl std::fmt::Display for TestNotificationEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Echo(s) => write!(f, "{}", s),
            Self::Forward(n) => write!(f, "forward [{}]", n),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TestDomain;

impl Domain for TestDomain {
    type Instruction = TestInstruction;
    type NotificationEvent = TestNotificationEvent;

    fn parse_instruction(s: &str) -> Result<Self::Instruction, Error> {
        Ok(TestInstruction(s.to_string()))
    }

    /// Treat the last dot-separated component of the last argument as the
    /// owning portal, mirroring how a real domain's identifiers are shaped
    /// (`user.project.portal`). Enough to exercise the framework's
    /// portal-ownership re-check - see `handler::check_portal_ownership` and
    /// `docs/specifications/security-review-2.md` (finding R34).
    fn owning_portal(instruction: &Self::Instruction) -> Option<PortalIdentifier> {
        let last = instruction.0.split_whitespace().last()?;
        let portal = last.rsplit('.').next()?;

        match portal == last {
            // no dot at all, so this instruction names no portal
            true => None,
            false => PortalIdentifier::parse(portal).ok(),
        }
    }

    fn parse_notification_event(s: &str) -> Result<Self::NotificationEvent, Error> {
        Ok(TestNotificationEvent::Echo(s.to_string()))
    }

    fn name() -> &'static str {
        "test-domain"
    }

    fn version() -> &'static str {
        "0.0.0"
    }

    fn wrap_forward(inner: Notification<Self>) -> Self::NotificationEvent {
        TestNotificationEvent::Forward(Box::new(inner))
    }
}
