// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::job::Job;
use anyhow::Error;
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
}

impl Command {
    pub fn put(job: Job) -> Self {
        Self::Put { job }
    }

    pub fn update(job: Job) -> Self {
        Self::Update { job }
    }

    pub fn delete(job: Job) -> Self {
        Self::Delete { job }
    }

    pub fn error(error: String) -> Self {
        Self::Error { error }
    }

    pub async fn send_to(&self, peer: &str) -> Result<(), Error> {
        send_to_peer(Message::new(peer, &serde_json::to_string(self)?)).await;

        Ok(())
    }
}

impl From<Message> for Command {
    fn from(m: Message) -> Self {
        serde_json::from_str(&m.payload)
            .unwrap_or(Command::error(format!("Could not parse command: {:?}", m)))
    }
}
