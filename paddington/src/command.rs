// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::message::Message;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Command {
    Error {
        error: String,
    },
    Connected {
        agent: String,
        zone: String,
        engine: String,
        version: String,
    },
    Watchdog {
        agent: String,
        zone: String,
    },
    Disconnect {
        agent: String,
        zone: String,
    },
    Disconnected {
        agent: String,
        zone: String,
    },
}

impl Command {
    pub fn connected(agent: &str, zone: &str, engine: &str, version: &str) -> Self {
        Self::Connected {
            agent: agent.to_owned(),
            zone: zone.to_owned(),
            engine: engine.to_owned(),
            version: version.to_owned(),
        }
    }

    pub fn disconnect(agent: &str, zone: &str) -> Self {
        Self::Disconnect {
            agent: agent.to_owned(),
            zone: zone.to_owned(),
        }
    }

    pub fn disconnected(agent: &str, zone: &str) -> Self {
        Self::Disconnected {
            agent: agent.to_owned(),
            zone: zone.to_owned(),
        }
    }

    pub fn watchdog(agent: &str, zone: &str) -> Self {
        Self::Watchdog {
            agent: agent.to_owned(),
            zone: zone.to_owned(),
        }
    }

    pub fn error(error: String) -> Self {
        Self::Error { error }
    }
}

impl From<Message> for Command {
    fn from(m: Message) -> Self {
        match m.is_control() {
            true => serde_json::from_str(m.payload()).unwrap_or(Command::error(format!(
                "Could not parse command: {}",
                m.payload()
            ))),
            false => Command::error(format!("Invalid control message: {}", m.payload())),
        }
    }
}

impl From<Command> for Message {
    fn from(c: Command) -> Self {
        Message::control(&serde_json::to_string(&c).unwrap_or_default())
    }
}
