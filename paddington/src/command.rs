// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::message::Message;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Command {
    Error { error: String },
    Connected { agent: String },
    Disconnected { agent: String },
}

impl Command {
    pub fn connected(agent: String) -> Self {
        Self::Connected { agent }
    }

    pub fn disconnected(agent: String) -> Self {
        Self::Disconnected { agent }
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
