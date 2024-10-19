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

impl std::fmt::Display for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Command::Error { error } => write!(f, "Error: {}", error),
            Command::Put { job } => write!(f, "Put: {}", job),
            Command::Update { job } => write!(f, "Update: {}", job),
            Command::Delete { job } => write!(f, "Delete: {}", job),
            Command::Register { agent } => write!(f, "Register: {}", agent),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_display() {
        let job = Job::new("test");
        let command = Command::put(&job);
        assert_eq!(format!("{}", command), format!("Put: {}", job));
    }

    #[test]
    fn test_command_put() {
        let job = Job::new("test");
        let command = Command::put(&job);
        assert_eq!(command, Command::Put { job });
    }

    #[test]
    fn test_command_update() {
        let job = Job::new("test");
        let command = Command::update(&job);
        assert_eq!(command, Command::Update { job });
    }

    #[test]
    fn test_command_delete() {
        let job = Job::new("test");
        let command = Command::delete(&job);
        assert_eq!(command, Command::Delete { job });
    }

    #[test]
    fn test_command_error() {
        let error = "test error";
        let command = Command::error(error);
        assert_eq!(
            command,
            Command::Error {
                error: error.to_owned()
            }
        );
    }

    #[test]
    fn test_command_register() {
        let agent = AgentType::Portal;
        let command = Command::register(&agent);
        assert_eq!(command, Command::Register { agent });
    }
}
