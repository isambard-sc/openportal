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

    fn parse_notification_event(s: &str) -> Result<Self::NotificationEvent, Error> {
        Ok(TestNotificationEvent::Echo(s.to_string()))
    }

    fn wrap_forward(inner: Notification<Self>) -> Self::NotificationEvent {
        TestNotificationEvent::Forward(Box::new(inner))
    }
}
