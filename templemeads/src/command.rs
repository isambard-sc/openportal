// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent::Type as AgentType;
use crate::error::Error;
use crate::job::Job;

use anyhow::Result;
use paddington::message::Message;
use paddington::send as send_to_peer;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Command {
    Error { error: String },
    Put { job: Job },
    Update { job: Job },
    Delete { job: Job },
    Register { agent: AgentType },
}

impl Command {
    pub fn put(job: &Job) -> Self {
        Self::Put { job: job.clone() }
    }

    pub fn update(job: &Job) -> Self {
        Self::Update { job: job.clone() }
    }

    pub fn delete(job: &Job) -> Self {
        Self::Delete { job: job.clone() }
    }

    pub fn error(error: &str) -> Self {
        Self::Error {
            error: error.to_owned(),
        }
    }

    pub fn register(agent: &AgentType) -> Self {
        Self::Register {
            agent: agent.clone(),
        }
    }

    pub async fn send_to(&self, peer: &str) -> Result<(), Error> {
        Ok(send_to_peer(Message::new(peer, &serde_json::to_string(self)?)).await?)
    }
}

impl From<Message> for Command {
    fn from(m: Message) -> Self {
        serde_json::from_str(m.payload())
            .unwrap_or(Command::error(&format!("Could not parse command: {:?}", m)))
    }
}
