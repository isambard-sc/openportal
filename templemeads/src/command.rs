// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent::{self, Peer, Type as AgentType};
use crate::board::SyncState;
use crate::destination::Destination;
use crate::error::Error;
use crate::grammar::NamedType;
use crate::job::Job;
use crate::virtual_agent::send as send_to_virtual;

use anyhow::Result;
use chrono::{DateTime, Utc};
use paddington::message::Message;
use paddington::received as received_from_peer;
use paddington::send as send_to_peer;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Health information for an agent
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HealthInfo {
    /// Agent name
    pub name: String,
    /// Agent type
    pub agent_type: AgentType,
    /// Whether agent is connected
    pub connected: bool,
    /// Number of active jobs on this agent's boards
    pub active_jobs: usize,
    /// Number of pending jobs
    pub pending_jobs: usize,
    /// Number of running jobs
    pub running_jobs: usize,
    /// Number of completed jobs (on boards)
    pub completed_jobs: usize,
    /// Number of duplicate jobs
    pub duplicate_jobs: usize,
    /// Time when agent started
    pub start_time: DateTime<Utc>,
    /// Current time on agent
    pub current_time: DateTime<Utc>,
    /// Uptime in seconds
    pub uptime_seconds: i64,
    /// Engine name (e.g., "templemeads")
    pub engine: String,
    /// Engine version
    pub version: String,
    /// Time when this health response was received/cached
    pub last_updated: DateTime<Utc>,
    /// Nested health information from downstream peers
    #[serde(default)]
    pub peers: HashMap<String, Box<HealthInfo>>,
}

impl HealthInfo {
    pub fn new(
        name: &str,
        agent_type: AgentType,
        connected: bool,
        start_time: DateTime<Utc>,
        engine: &str,
        version: &str,
    ) -> Self {
        let current_time = Utc::now();
        let uptime_seconds = current_time.signed_duration_since(start_time).num_seconds();

        Self {
            name: name.to_owned(),
            agent_type,
            connected,
            active_jobs: 0,
            pending_jobs: 0,
            running_jobs: 0,
            completed_jobs: 0,
            duplicate_jobs: 0,
            start_time,
            current_time,
            uptime_seconds,
            engine: engine.to_owned(),
            version: version.to_owned(),
            last_updated: current_time,
            peers: HashMap::new(),
        }
    }
}

impl NamedType for HealthInfo {
    fn type_name() -> &'static str {
        "HealthInfo"
    }
}

impl std::fmt::Display for HealthInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{} ({}) - {} - uptime: {}s, jobs: {} active ({} pending, {} running, {} completed, {} duplicates)",
            self.name,
            self.agent_type,
            if self.connected { "connected" } else { "disconnected" },
            self.uptime_seconds,
            self.active_jobs,
            self.pending_jobs,
            self.running_jobs,
            self.completed_jobs,
            self.duplicate_jobs
        )
    }
}

impl HealthInfo {
    pub fn add_peer_health(&mut self, peer_health: HealthInfo) {
        self.peers
            .insert(peer_health.name.clone(), Box::new(peer_health));
    }

    pub fn get(&self, peer_name: &str) -> Option<HealthInfo> {
        self.peers.get(peer_name).map(|h| *h.clone())
    }

    pub fn keys(&self) -> Vec<String> {
        self.peers.keys().cloned().collect()
    }
}

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
    HealthCheck,
    HealthResponse {
        health: HealthInfo,
    },
    Restart,
    RestartAck {
        agent: String,
        message: String,
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
            Command::HealthCheck => write!(f, "HealthCheck"),
            Command::HealthResponse { health } => write!(f, "HealthResponse: {}", health),
            Command::Restart => write!(f, "Restart"),
            Command::RestartAck { agent, message } => {
                write!(f, "RestartAck: {} - {}", agent, message)
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
        Self::HealthCheck
    }

    pub fn health_response(health: HealthInfo) -> Self {
        Self::HealthResponse { health }
    }

    pub fn restart() -> Self {
        Self::Restart
    }

    pub fn restart_ack(agent: &str, message: &str) -> Self {
        Self::RestartAck {
            agent: agent.to_owned(),
            message: message.to_owned(),
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
            Command::HealthCheck => None,
            Command::HealthResponse { health: _ } => None,
            Command::Restart => None,
            Command::RestartAck {
                agent: _,
                message: _,
            } => None,
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
            Command::HealthCheck => None,
            Command::HealthResponse { health: _ } => None,
            Command::Restart => None,
            Command::RestartAck {
                agent: _,
                message: _,
            } => None,
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
            Command::HealthCheck => None,
            Command::HealthResponse { health: _ } => None,
            Command::Restart => None,
            Command::RestartAck {
                agent: _,
                message: _,
            } => None,
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
