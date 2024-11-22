// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent::{Peer, Type as AgentType};
use crate::board::SyncState;
use crate::error::Error;
use crate::job::Job;

use anyhow::Result;
use paddington::message::Message;
use paddington::received as received_from_peer;
use paddington::send as send_to_peer;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Command {
    Error {
        error: String,
    },
    Put {
        job: Job,
    },
    Update {
        job: Job,
    },
    Delete {
        job: Job,
    },
    Register {
        agent: AgentType,
        engine: String,
        version: String,
    },
    Sync {
        state: SyncState,
    },
}

impl std::fmt::Display for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Command::Error { error } => write!(f, "Error: {}", error),
            Command::Put { job } => write!(f, "Put: {}", job),
            Command::Update { job } => write!(f, "Update: {}", job),
            Command::Delete { job } => write!(f, "Delete: {}", job),
            Command::Register {
                agent,
                engine,
                version,
            } => write!(
                f,
                "Register: {}, engine={} version={}",
                agent, engine, version
            ),
            Command::Sync { state: _ } => write!(f, "Sync: State"),
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

    pub fn register(agent: &AgentType, engine: &str, version: &str) -> Self {
        Self::Register {
            agent: agent.clone(),
            engine: engine.to_owned(),
            version: version.to_owned(),
        }
    }

    pub fn sync(state: &SyncState) -> Self {
        Self::Sync {
            state: state.clone(),
        }
    }

    pub async fn send_to(&self, peer: &Peer) -> Result<(), Error> {
        Ok(send_to_peer(Message::send_to(
            peer.name(),
            peer.zone(),
            &serde_json::to_string(self)?,
        ))
        .await?)
    }

    pub fn received_from(&self, peer: &Peer) -> Result<(), Error> {
        match received_from_peer(Message::received_from(
            peer.name(),
            peer.zone(),
            &serde_json::to_string(self)?,
        )) {
            Ok(_) => Ok(()),
            Err(e) => Err(Error::from(e)),
        }
    }

    pub fn job(&self) -> Option<Job> {
        match self {
            Command::Put { job } => Some(job.clone()),
            Command::Update { job } => Some(job.clone()),
            Command::Delete { job } => Some(job.clone()),
            Command::Sync { state: _ } => None,
            Command::Register {
                agent: _,
                engine: _,
                version: _,
            } => None,
            Command::Error { error: _ } => None,
        }
    }

    pub fn job_id(&self) -> Option<Uuid> {
        match self {
            Command::Put { job } => Some(job.id()),
            Command::Update { job } => Some(job.id()),
            Command::Delete { job } => Some(job.id()),
            Command::Sync { state: _ } => None,
            Command::Register {
                agent: _,
                engine: _,
                version: _,
            } => None,
            Command::Error { error: _ } => None,
        }
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
        #[allow(clippy::unwrap_used)]
        let job = Job::parse("a.b add_user person.group.a", true).unwrap();
        let command = Command::put(&job);
        assert_eq!(format!("{}", command), format!("Put: {}", job));
    }

    #[test]
    fn test_command_put() {
        #[allow(clippy::unwrap_used)]
        let job = Job::parse("a.b add_user person.group.a", true).unwrap();
        let command = Command::put(&job);
        assert_eq!(command, Command::Put { job });
    }

    #[test]
    fn test_command_update() {
        #[allow(clippy::unwrap_used)]
        let job = Job::parse("a.b add_user person.group.a", true).unwrap();
        let command = Command::update(&job);
        assert_eq!(command, Command::Update { job });
    }

    #[test]
    fn test_command_delete() {
        #[allow(clippy::unwrap_used)]
        let job = Job::parse("a.b add_user person.group.a", true).unwrap();
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
        let engine = "templemeads";
        let version = "0.0.10";
        let command = Command::register(&agent, engine, version);
        assert_eq!(
            command,
            Command::Register {
                agent,
                engine: engine.to_owned(),
                version: version.to_owned()
            }
        );
    }
}
