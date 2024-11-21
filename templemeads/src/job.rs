// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent::Peer;
use crate::board::Waiter;
use crate::command::Command as ControlCommand;
use crate::destination::Destination;
use crate::error::Error;
use crate::grammar::Instruction;
use crate::state;

use anyhow::Result;
use chrono::serde::ts_seconds;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    recipient: String,
    sender: String,
    zone: String,
    job: Job,
}

impl Envelope {
    pub fn new(recipient: &str, sender: &str, zone: &str, job: &Job) -> Self {
        Self {
            recipient: recipient.to_owned(),
            sender: sender.to_owned(),
            zone: zone.to_owned(),
            job: job.clone(),
        }
    }

    pub fn recipient(&self) -> Peer {
        Peer::new(&self.recipient, &self.zone)
    }

    pub fn sender(&self) -> Peer {
        Peer::new(&self.sender, &self.zone)
    }

    pub fn job(&self) -> Job {
        self.job.clone()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Status {
    Created,
    Pending,
    Running,
    Complete,
    Error,
}

///
/// This is the internal representation of the parsed command. We don't
/// make this publicly visible as we don't want to confuse users with too
/// many "command" types.
///
#[derive(Clone, PartialEq)]
struct Command {
    destination: Destination,
    instruction: Instruction,
}

impl Command {
    pub fn parse(command: &str, check_portal: bool) -> Result<Self, Error> {
        // the format of commands is "destination command arguments..."
        let mut parts = command.split_whitespace();

        let destination = match Destination::parse(parts.next().unwrap_or("")) {
            Ok(d) => d,
            Err(e) => {
                return Err(Error::Parse(format!(
                    "Could not parse destination from command '{}': {}",
                    command, e
                )))
            }
        };

        let instruction = match Instruction::parse(&parts.collect::<Vec<&str>>().join(" ")) {
            Ok(i) => i,
            Err(e) => {
                return Err(Error::Parse(format!(
                    "Could not parse instruction from command '{}': {}",
                    command, e
                )))
            }
        };

        if check_portal {
            let user = match instruction.clone() {
                Instruction::AddUser(user) => Some(user),
                Instruction::RemoveUser(user) => Some(user),
                _ => None,
            };

            if let Some(user) = user {
                if user.portal() != destination.first() {
                    tracing::error!(
                    "Invalid command '{}'. Commands involving user '{}' can only be issued via the portal '{}', not '{}'.",
                    command, user, user.portal(), destination.first()
                );
                    return Err(Error::Parse(format!(
                    "Invalid command '{}'. Commands involving user '{}' can only be issued via the portal '{}', not '{}'.",
                    command, user, user.portal(), destination.first()
                )));
                }
            }
        }

        Ok(Self {
            destination,
            instruction,
        })
    }

    pub fn destination(&self) -> Destination {
        self.destination.clone()
    }

    pub fn instruction(&self) -> Instruction {
        self.instruction.clone()
    }
}

impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{} {}", self.destination, self.instruction)
    }
}

impl std::fmt::Display for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{} {}", self.destination, self.instruction,)
    }
}

// serialise via the string representation - this looks better
impl Serialize for Command {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::ser::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

// deserialise via the string representation - this looks better

impl<'de> Deserialize<'de> for Command {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match Command::parse(&s, false) {
            Ok(command) => Ok(command),
            Err(e) => Err(serde::de::Error::custom(e.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Job {
    id: Uuid,
    #[serde(with = "ts_seconds")]
    created: chrono::DateTime<Utc>,
    #[serde(with = "ts_seconds")]
    changed: chrono::DateTime<Utc>,
    version: u64,
    command: Command,
    state: Status,
    result: Option<String>,
    #[serde(skip)]
    board: Option<Peer>,
}

// implement display for Job
impl std::fmt::Display for Job {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{{{}}}: version={}, created={}, changed={}, state={:?}",
            self.command, self.version, self.created, self.changed, self.state,
        )
    }
}

impl Job {
    pub fn parse(command: &str, check_portal: bool) -> Result<Self, Error> {
        tracing::info!("Parsing command: {:?}", command);

        let now = Utc::now();

        Ok(Self {
            id: Uuid::new_v4(),
            created: now,
            changed: now,
            version: 1,
            command: Command::parse(command, check_portal)?,
            state: Status::Created,
            result: None,
            board: None,
        })
    }

    pub fn id(&self) -> Uuid {
        self.id
    }

    pub fn destination(&self) -> Destination {
        self.command.destination()
    }

    pub fn instruction(&self) -> Instruction {
        self.command.instruction()
    }

    pub fn is_finished(&self) -> bool {
        self.state == Status::Complete || self.state == Status::Error
    }

    pub fn state(&self) -> Status {
        self.state.clone()
    }

    pub fn created(&self) -> chrono::DateTime<Utc> {
        self.created
    }

    pub fn changed(&self) -> chrono::DateTime<Utc> {
        self.changed
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn increment_version(&self) -> Self {
        Self {
            id: self.id,
            created: self.created,
            changed: Utc::now(),
            version: self.version + 1,
            command: self.command.clone(),
            state: self.state.clone(),
            result: self.result.clone(),
            board: self.board.clone(),
        }
    }

    pub fn assert_is_for_board(&self, agent: &Peer) -> Result<(), Error> {
        match &self.board {
            Some(b) => {
                if b == agent {
                    Ok(())
                } else {
                    Err(Error::InvalidBoard(
                        format!("Job {} is on board {}, not board {}", self.id, b, agent)
                            .to_owned(),
                    ))
                }
            }
            None => Err(Error::InvalidBoard(
                format!(
                    "Job {} is not on any board, so is not on board {}",
                    self.id, agent
                )
                .to_owned(),
            )),
        }
    }

    pub fn pending(&self) -> Result<Job, Error> {
        match self.state {
            Status::Created => Ok(Job {
                id: self.id,
                created: self.created,
                changed: Utc::now(),
                version: self.version + 1,
                command: self.command.clone(),
                state: Status::Pending,
                result: self.result.clone(),
                board: self.board.clone(),
            }),
            Status::Pending => Ok(self.clone()),
            _ => Err(Error::InvalidState(
                format!("Cannot set pending on job in state: {:?}", self.state).to_owned(),
            )),
        }
    }

    pub fn running(&self, progress: Option<String>) -> Result<Job, Error> {
        match self.state {
            Status::Pending | Status::Running => Ok(Job {
                id: self.id,
                created: self.created,
                changed: Utc::now(),
                version: self.version + 1,
                command: self.command.clone(),
                state: Status::Running,
                result: progress,
                board: self.board.clone(),
            }),
            _ => Err(Error::InvalidState(
                format!("Cannot set running on job in state: {:?}", self.state).to_owned(),
            )),
        }
    }

    pub fn completed<T>(&self, result: T) -> Result<Job, Error>
    where
        T: serde::Serialize,
    {
        match self.state {
            Status::Pending | Status::Running => Ok(Job {
                id: self.id,
                created: self.created,
                changed: Utc::now(),
                version: self.version + 1000, // make sure this is the newest version
                command: self.command.clone(),
                state: Status::Complete,
                result: Some(serde_json::to_string(&result)?),
                board: self.board.clone(),
            }),
            _ => Err(Error::InvalidState(
                format!("Cannot set complete on job in state: {:?}", self.state).to_owned(),
            )),
        }
    }

    pub fn errored(&self, message: &str) -> Result<Job, Error> {
        match self.state {
            Status::Pending | Status::Running => Ok(Job {
                id: self.id,
                created: self.created,
                changed: Utc::now(),
                version: self.version + 1000, // make sure this is the newest version
                command: self.command.clone(),
                state: Status::Error,
                result: Some(message.to_owned()),
                board: self.board.clone(),
            }),
            _ => Err(Error::InvalidState(
                format!("Cannot set error on job in state: {:?}", self.state).to_owned(),
            )),
        }
    }

    pub fn is_error(&self) -> bool {
        self.state == Status::Error
    }

    pub fn error_message(&self) -> Option<String> {
        match self.state {
            Status::Error => self.result.clone(),
            _ => None,
        }
    }

    pub fn progress_message(&self) -> Option<String> {
        match self.state {
            Status::Running => {
                if let Some(result) = &self.result {
                    Some(result.clone())
                } else {
                    Some("Running".to_owned())
                }
            }
            Status::Created => Some("Created".to_owned()),
            Status::Pending => Some("Pending".to_owned()),
            Status::Complete => Some("Complete".to_owned()),
            Status::Error => Some("Error".to_owned()),
        }
    }

    pub fn result<T>(&self) -> Result<Option<T>, Error>
    where
        T: serde::de::DeserializeOwned,
    {
        match self.state {
            Status::Created => Ok(None),
            Status::Pending => Ok(None),
            Status::Running => Ok(None),
            Status::Error => match &self.result {
                Some(result) => Err(Error::Run(result.clone())),
                None => Err(Error::InvalidState("Unknown error".to_owned())),
            },
            Status::Complete => match &self.result {
                Some(result) => Ok(Some(serde_json::from_str(result)?)),
                None => Err(Error::Unknown("No result available".to_owned())),
            },
        }
    }

    pub async fn execute(&self) -> Result<Job, Error> {
        match self.state() {
            Status::Pending => {
                tracing::info!("Running job.execute() for job: {:?}", self);
                self.errored(format!("No default runner for job: {:?}", self).as_str())
            }
            _ => Err(Error::InvalidState(
                format!("Cannot execute job in state: {:?}", self.state).to_owned(),
            )),
        }
    }

    pub async fn received(&self, peer: &Peer) -> Result<Job, Error> {
        if self.state == Status::Created {
            return Err(Error::InvalidState(
                format!("A created job should not have been received? {:?}", self).to_owned(),
            ));
        }

        let mut job = self.clone();

        // get a RwLock to the board from the shared state
        let board = match state::get(peer).await {
            Ok(b) => b.board().await,
            Err(e) => {
                tracing::error!(
                    "Error getting board for agent: {:?}. Is this agent known to us?",
                    e
                );
                return Err(e);
            }
        };

        // in a scope so we drop the lock asap
        {
            // get the mutable board from the Arc<RwLock> board - this is the
            // blocking operation
            let mut board = board.write().await;

            // add the job to the board - we need to set our board to the agent
            // first, so that the board can check it is correct
            job.board = Some(peer.clone());
            board.add(&job)?;
        }

        Ok(job)
    }

    pub async fn put(&self, peer: &Peer) -> Result<Job, Error> {
        // transition the job to pending, recording where it was sent
        let mut job = self.pending()?;

        // get a RwLock to the board from the shared state
        let board = match state::get(peer).await {
            Ok(b) => b.board().await,
            Err(e) => {
                tracing::error!(
                    "Error getting board for agent: {:?}. Is this agent known to us?",
                    e
                );
                return Err(e);
            }
        };

        // in a scope so we drop the lock asap
        {
            // get the mutable board from the Arc<RwLock> board - this is the
            // blocking operation
            let mut board = board.write().await;

            // add the job to the board - we need to set our board to the agent
            // first, so that the board can check it is correct
            job.board = Some(peer.clone());
            board.add(&job)?;
        }

        // now send it to the agent for processing
        match ControlCommand::put(&job).send_to(peer).await {
            Ok(_) => (),
            Err(e) => {
                // if we can't send the command, then we need to need to add
                // it to a queue for sending once the peer is back online
                tracing::error!("Error sending command to agent: {:?}", e);
                let mut board = board.write().await;
                board.queue(ControlCommand::put(&job));
            }
        }

        Ok(job)
    }

    pub async fn updated(&self) -> Result<Job, Error> {
        let agent = match self.board {
            Some(ref a) => a,
            None => {
                return Err(Error::InvalidBoard(
                    "Job has no board, so cannot be updated".to_owned(),
                ))
            }
        };

        // get a RwLock to the board from the shared state
        let board = match state::get(agent).await {
            Ok(b) => b.board().await,
            Err(e) => {
                tracing::error!(
                    "Error getting board for agent: {:?}. Is this agent known to us?",
                    e
                );
                return Err(e);
            }
        };

        // in a scope so we drop the lock asap
        {
            // get the mutable board from the Arc<RwLock> board - this is the
            // blocking operation
            let mut board = board.write().await;

            // add the job to the board - we need to set our board to the agent
            // first, so that the board can check it is correct
            board.add(self)?;
        }

        Ok(self.clone())
    }

    pub async fn update(&self, peer: &Peer) -> Result<Job, Error> {
        let mut job = self.clone();

        // get a RwLock to the board from the shared state
        let board = match state::get(peer).await {
            Ok(b) => b.board().await,
            Err(e) => {
                tracing::error!(
                    "Error getting board for agent: {:?}. Is this agent known to us?",
                    e
                );
                return Err(e);
            }
        };

        // in a scope so we drop the lock asap
        {
            // get the mutable board from the Arc<RwLock> board - this is the
            // blocking operation
            let mut board = board.write().await;

            // add the job to the board - we need to set our board to the agent
            // first, so that the board can check it is correct
            job.board = Some(peer.clone());
            board.add(&job)?;
        }

        // now send it to the agent for processing
        match ControlCommand::update(&job).send_to(peer).await {
            Ok(_) => (),
            Err(e) => {
                // if we can't send the command, then we need to need to add
                // it to a queue for sending once the peer is back online
                tracing::error!("Error sending command to agent: {:?}", e);
                let mut board = board.write().await;
                board.queue(ControlCommand::update(&job));
            }
        }

        Ok(job)
    }

    pub async fn deleted(&self, peer: &Peer) -> Result<Job, Error> {
        let mut job = self.clone();

        // get a RwLock to the board from the shared state
        let board = match state::get(peer).await {
            Ok(b) => b.board().await,
            Err(e) => {
                tracing::error!(
                    "Error getting board for agent: {:?}. Is this agent known to us?",
                    e
                );
                return Err(e);
            }
        };

        // in a scope so we drop the lock asap
        {
            // get the mutable board from the Arc<RwLock> board - this is the
            // blocking operation
            let mut board = board.write().await;

            // remove the job to the board
            job.board = Some(peer.clone());
            board.remove(&job)?;
            job.board = None;
        }

        Ok(job)
    }

    pub async fn delete(&self, peer: &Peer) -> Result<Job, Error> {
        let mut job = self.clone();

        // get a RwLock to the board from the shared state
        let board = match state::get(peer).await {
            Ok(b) => b.board().await,
            Err(e) => {
                tracing::error!(
                    "Error getting board for agent: {:?}. Is this agent known to us?",
                    e
                );
                return Err(e);
            }
        };

        // in a scope so we drop the lock asap
        {
            // get the mutable board from the Arc<RwLock> board - this is the
            // blocking operation
            let mut board = board.write().await;

            // remove the job from the board
            job.board = Some(peer.clone());
            board.remove(&job)?;
            job.board = None;
        }

        // now send it to the agent for processing
        match ControlCommand::delete(&job).send_to(peer).await {
            Ok(_) => (),
            Err(e) => {
                // if we can't send the command, then we need to need to add
                // it to a queue for sending once the peer is back online
                tracing::error!("Error sending command to agent: {:?}", e);
                let mut board = board.write().await;
                board.queue(ControlCommand::delete(&job));
            }
        }

        Ok(job)
    }

    pub async fn wait(&self) -> Result<Job, Error> {
        if self.is_finished() {
            return Ok(self.clone());
        }

        let agent = match self.board {
            Some(ref a) => a,
            None => {
                return Err(Error::InvalidBoard(
                    "Job has no board, so cannot waited upon".to_owned(),
                ))
            }
        };

        // get a RwLock to the board from the shared state
        let board = match state::get(agent).await {
            Ok(b) => b.board().await,
            Err(e) => {
                tracing::error!(
                    "Error getting board for agent: {:?}. Is this agent known to us?",
                    e
                );
                return Err(e);
            }
        };

        let waiter: Waiter;

        // in a scope so we drop the lock asap
        {
            // get the mutable board from the Arc<RwLock> board - this is the
            // blocking operation
            let mut board = board.write().await;

            // return a waiter for the job constructed from the board
            waiter = board.get_waiter(self)?;
        }

        // wait for the job to finish
        let result = waiter.result().await?;

        Ok(result)
    }
}

///
/// Function used to send all jobs that were queued for the specified peer
///
pub async fn send_queued(peer: &Peer) -> Result<(), Error> {
    // get a RwLock to the board from the shared state
    let board = match state::get(peer).await {
        Ok(b) => b.board().await,
        Err(e) => {
            tracing::error!(
                "Error getting board for agent: {:?}. Is this agent known to us?",
                e
            );
            return Err(e);
        }
    };

    // get all of the queued jobs
    let queued: Vec<ControlCommand>;

    // in a scope so we drop the lock asap
    {
        // get the mutable board from the Arc<RwLock> board - this is the
        // blocking operation
        let mut board = board.write().await;
        queued = board.take_queued();
    }

    // now send all of the queued jobs - if anything goes wrong, then
    // put the failed jobs back into the queue
    for job in queued {
        tracing::info!("Running queued job: {:?}", job);

        match job.send_to(peer).await {
            Ok(_) => (),
            Err(e) => {
                // if we can't send the command, then we need to need to add
                // it to a queue for sending once the peer is back online
                tracing::error!("Error sending queued command to agent: {:?}", e);
                let mut board = board.write().await;
                board.queue(job);
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_new() {
        #[allow(clippy::unwrap_used)]
        let command = Command::parse("portal.cluster add_user demo.proj.portal", true).unwrap();
        assert_eq!(command.destination().to_string(), "portal.cluster");
        assert_eq!(
            command.instruction().to_string(),
            "add_user demo.proj.portal"
        );
    }

    #[test]
    fn test_command_display() {
        #[allow(clippy::unwrap_used)]
        let command = Command::parse("portal.cluster add_user demo.proj.portal", true).unwrap();
        assert_eq!(
            command.to_string(),
            "portal.cluster add_user demo.proj.portal"
        );
    }

    #[test]
    fn test_job_new() {
        #[allow(clippy::unwrap_used)]
        let job = Job::parse("portal.cluster add_user demo.proj.portal", true).unwrap();
        assert_eq!(
            job.command.to_string(),
            "portal.cluster add_user demo.proj.portal"
        );
        assert_eq!(job.state, Status::Created);
        assert_eq!(job.result, None);
    }

    #[test]
    fn test_job_state() {
        #[allow(clippy::unwrap_used)]
        let mut job = Job::parse("portal.cluster add_user demo.proj.portal", true).unwrap();

        assert!(!job.is_finished());
        assert_eq!(job.state(), Status::Created);
        assert_eq!(job.created(), job.changed());
        assert_eq!(job.version(), 1);

        job = job.pending().unwrap_or(job);

        assert!(!job.is_finished());
        assert_eq!(job.state(), Status::Pending);
        assert!(job.changed() > job.created());
        assert_eq!(job.version(), 2);

        job = job.running(None).unwrap_or(job);

        assert!(!job.is_finished());
        assert_eq!(job.state(), Status::Running);
        assert!(job.changed() > job.created());
        assert_eq!(job.version(), 3);

        job = job.completed("done").unwrap_or(job);

        assert!(job.is_finished());
        assert_eq!(job.state(), Status::Complete);
        assert!(job.changed() > job.created());
        assert_eq!(job.version(), 1003);

        assert_eq!(
            job.result::<String>().unwrap_or_default(),
            Some("done".to_owned())
        );
    }

    #[test]
    fn test_job_error() {
        #[allow(clippy::unwrap_used)]
        let mut job = Job::parse("portal.cluster add_user demo.proj.portal", true).unwrap();

        assert!(!job.is_finished());
        assert_eq!(job.state(), Status::Created);
        assert_eq!(job.created(), job.changed());
        assert_eq!(job.version(), 1);

        job = job.pending().unwrap_or(job);

        assert!(!job.is_finished());
        assert_eq!(job.state(), Status::Pending);
        assert!(job.changed() > job.created());
        assert_eq!(job.version(), 2);

        job = job.running(None).unwrap_or(job);

        assert!(!job.is_finished());
        assert_eq!(job.state(), Status::Running);
        assert!(job.changed() > job.created());
        assert_eq!(job.version(), 3);

        job = job.errored("failed").unwrap_or(job);

        assert!(job.is_finished());
        assert_eq!(job.state(), Status::Error);
        assert!(job.changed() > job.created());
        assert_eq!(job.version(), 1003);

        assert_eq!(job.error_message(), Some("failed".to_owned()));

        match job.result::<String>() {
            Ok(_) => unreachable!("Should not have a result"),
            Err(e) => assert_eq!(e.to_string(), "failed"),
        }
    }
}
