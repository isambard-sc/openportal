// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent::Type as AgentType;
use crate::agent::{self, Peer};
use crate::board::SyncState;
use crate::destination::Destination;
use crate::diagnostics::DiagnosticsReport;
use crate::domain::Domain;
use crate::error::Error;
use crate::health::HealthInfo;
use crate::job::Job;
use crate::notification::Notification;
use crate::virtual_agent::send as send_to_virtual;

use anyhow::Result;
use paddington::message::Message;
use paddington::received as received_from_peer;
use paddington::send as send_to_peer;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(bound = "")]
pub enum Command<L: Domain> {
    Error {
        error: String,
    },
    Put {
        job: Job<L>,
    },
    Update {
        job: Job<L>,
    },
    Delete {
        job: Job<L>,
    },
    Register {
        agent: AgentType,
        engine: String,
        version: String,
        /// The sender's `Domain` name (e.g. `"greatwestern"`), if it sent
        /// one. Absent (`None`) on messages from a peer running templemeads
        /// <= 0.32.2, from before this field existed - see
        /// `Domain::assume_legacy_domain_version`.
        #[serde(default)]
        domain: Option<String>,
        /// The sender's `Domain` version, alongside `domain` above.
        #[serde(default)]
        domain_version: Option<String>,
    },
    Sync {
        state: SyncState<L>,
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
    Notify {
        notification: Notification<L>,
    },
}

impl<L: Domain> std::fmt::Display for Command<L> {
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
                domain,
                domain_version,
            } => write!(
                f,
                "Register: {}, engine={} version={} domain={} domain_version={}",
                agent,
                engine,
                version,
                domain.as_deref().unwrap_or("unknown"),
                domain_version.as_deref().unwrap_or("unknown")
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
            Command::Notify { notification } => write!(f, "Notify: {}", notification),
        }
    }
}

impl<L: Domain> Command<L> {
    pub fn put(job: &Job<L>) -> Self {
        Self::Put { job: job.clone() }
    }

    pub fn update(job: &Job<L>) -> Self {
        Self::Update { job: job.clone() }
    }

    pub fn delete(job: &Job<L>) -> Self {
        Self::Delete { job: job.clone() }
    }

    pub fn error(error: &str) -> Self {
        Self::Error {
            error: error.to_owned(),
        }
    }

    pub fn register(
        agent: &AgentType,
        engine: &str,
        version: &str,
        domain: &str,
        domain_version: &str,
    ) -> Self {
        Self::Register {
            agent: agent.clone(),
            engine: engine.to_owned(),
            version: version.to_owned(),
            domain: Some(domain.to_owned()),
            domain_version: Some(domain_version.to_owned()),
        }
    }

    pub fn sync(state: &SyncState<L>) -> Self {
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

    pub fn notify(notification: &Notification<L>) -> Self {
        Self::Notify {
            notification: notification.clone(),
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
            Ok(send_to_virtual::<L>(
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

    pub fn job(&self) -> Option<Job<L>> {
        match self {
            Command::Put { job } => Some(job.clone()),
            Command::Update { job } => Some(job.clone()),
            Command::Delete { job } => Some(job.clone()),
            Command::Sync { state: _ } => None,
            Command::Register {
                agent: _,
                engine: _,
                version: _,
                domain: _,
                domain_version: _,
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
            Command::Notify { notification: _ } => None,
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
                domain: _,
                domain_version: _,
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
            Command::Notify { notification: _ } => None,
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
                domain: _,
                domain_version: _,
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
            Command::Notify { notification } => Some(notification.destination().clone()),
        }
    }
}

impl<L: Domain> From<Message> for Command<L> {
    fn from(m: Message) -> Self {
        serde_json::from_str(m.payload())
            .unwrap_or(Command::error(&format!("Could not parse command: {:?}", m)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_domain::TestDomain;

    type Command = super::Command<TestDomain>;

    // Tests that exercise put/update/delete/display against a real parsed
    // Job (e.g. "a.b add_user person.group.a") need a concrete Domain, so
    // they live alongside the domain crate's own grammar tests instead of
    // here - templemeads itself has no concrete Instruction to parse.

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
        let domain = "test-domain";
        let domain_version = "0.0.0";
        let command = Command::register(&agent, engine, version, domain, domain_version);
        assert_eq!(
            command,
            Command::Register {
                agent,
                engine: engine.to_owned(),
                version: version.to_owned(),
                domain: Some(domain.to_owned()),
                domain_version: Some(domain_version.to_owned()),
            }
        );
    }

    /// A `Register` from a peer running templemeads <= 0.32.2 (before this
    /// field existed) has no `domain`/`domain_version` keys in its JSON at
    /// all. `#[serde(default)]` must let this still deserialize, defaulting
    /// both to `None`, rather than failing to parse.
    #[test]
    fn test_command_register_deserialize_without_domain_fields() {
        let json = r#"{"Register":{"agent":"Portal","engine":"templemeads","version":"0.32.2"}}"#;
        #[allow(clippy::expect_used)]
        let command: Command = serde_json::from_str(json).expect("legacy Register should parse");
        assert_eq!(
            command,
            Command::Register {
                agent: AgentType::Portal,
                engine: "templemeads".to_owned(),
                version: "0.32.2".to_owned(),
                domain: None,
                domain_version: None,
            }
        );
    }
}
