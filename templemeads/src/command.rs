// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent::Type as AgentType;
use crate::agent::{self, Peer};
use crate::board::SyncState;
use crate::destination::Destination;
use crate::diagnostics::DiagnosticsReport;
use crate::error::Error;
use crate::health::HealthInfo;
use crate::job::Job;
use crate::virtual_agent::send as send_to_virtual;

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
    HealthCheck {
        /// Chain of agents that have already been visited in this health check cascade
        /// to prevent circular loops across zones
        #[serde(default)]
        visited: Vec<String>,
    },
    HealthResponse {
        health: Box<HealthInfo>,
    },
    Restart {
        /// Type of restart: "soft" (networking only), "hard" (terminate process), etc.
        restart_type: String,
        /// Dot-separated destination path (e.g., "brics.aip2.clusters")
        /// Empty string means restart self
        destination: String,
    },
    DiagnosticsRequest {
        /// Dot-separated destination path (e.g., "brics.aip2.clusters")
        /// Empty string means request from self
        destination: String,
    },
    DiagnosticsResponse {
        report: Box<DiagnosticsReport>,
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
            Command::HealthCheck { visited } => {
                write!(f, "HealthCheck (visited: {})", visited.len())
            }
            Command::HealthResponse { health } => write!(f, "HealthResponse: {}", health),
            Command::Restart {
                restart_type,
                destination,
            } => write!(
                f,
                "Restart: type={}, destination={}",
                restart_type, destination
            ),
            Command::DiagnosticsRequest { destination } => {
                write!(f, "DiagnosticsRequest: destination={}", destination)
            }
            Command::DiagnosticsResponse { report } => {
                write!(f, "DiagnosticsResponse: {}", report)
            }
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

    pub fn health_check() -> Self {
        Self::HealthCheck {
            visited: Vec::new(),
        }
    }

    pub fn health_check_with_visited(visited: Vec<String>) -> Self {
        Self::HealthCheck { visited }
    }

    pub fn health_response(health: HealthInfo) -> Self {
        Self::HealthResponse {
            health: Box::new(health),
        }
    }

    pub fn restart(restart_type: &str, destination: &str) -> Self {
        Self::Restart {
            restart_type: restart_type.to_owned(),
            destination: destination.to_owned(),
        }
    }

    pub fn diagnostics_request(destination: &str) -> Self {
        Self::DiagnosticsRequest {
            destination: destination.to_owned(),
        }
    }

    pub fn diagnostics_response(report: DiagnosticsReport) -> Self {
        Self::DiagnosticsResponse {
            report: Box::new(report),
        }
    }

    pub async fn send_to(&self, peer: &Peer) -> Result<(), Error> {
        // Check if sending to ourselves
        let my_name = agent::name().await;
        if peer.name() == my_name {
            tracing::debug!("Sending command to self - processing locally");
            // Process the command locally by injecting it into the received queue
            return self.received_from(peer);
        }

        if agent::is_virtual(peer).await {
            tracing::debug!("Sending command to virtual peer {} locally", peer);
            Ok(send_to_virtual(
                &self.destination(),
                Message::send_to(peer.name(), peer.zone(), &serde_json::to_string(self)?),
            )
            .await?)
        } else {
            Ok(send_to_peer(Message::send_to(
                peer.name(),
                peer.zone(),
                &serde_json::to_string(self)?,
            ))
            .await?)
        }
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
            Command::HealthCheck { visited: _ } => None,
            Command::HealthResponse { health: _ } => None,
            Command::Restart {
                restart_type: _,
                destination: _,
            } => None,
            Command::DiagnosticsRequest { destination: _ } => None,
            Command::DiagnosticsResponse { report: _ } => None,
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
            Command::HealthCheck { visited: _ } => None,
            Command::HealthResponse { health: _ } => None,
            Command::Restart {
                restart_type: _,
                destination: _,
            } => None,
            Command::DiagnosticsRequest { destination: _ } => None,
            Command::DiagnosticsResponse { report: _ } => None,
        }
    }

    pub fn destination(&self) -> Option<Destination> {
        match self {
            Command::Put { job } => Some(job.destination().to_owned()),
            Command::Update { job } => Some(job.destination().to_owned()),
            Command::Delete { job } => Some(job.destination().to_owned()),
            Command::Sync { state: _ } => None,
            Command::Register {
                agent: _,
                engine: _,
                version: _,
            } => None,
            Command::Error { error: _ } => None,
            Command::HealthCheck { visited: _ } => None,
            Command::HealthResponse { health: _ } => None,
            Command::Restart {
                restart_type: _,
                destination: _,
            } => None,
            Command::DiagnosticsRequest { destination: _ } => None,
            Command::DiagnosticsResponse { report: _ } => None,
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
