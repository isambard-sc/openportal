// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent::Peer;
use crate::board::{JobAddState, SyncState, Waiter};
use crate::command::Command as ControlCommand;
use crate::destination::{Destination, Position};
use crate::domain::Domain;
use crate::error::Error;
use crate::joberror::JobError;
use crate::named::NamedType;
use crate::state;

use anyhow::Result;
use chrono::serde::ts_seconds;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fmt::Display;
use ts_rs::TS;
use uuid::Uuid;

/// Maximum lifetime of a Job, from creation to expiry.
///
/// `expires` is a wire field, and reaping expired Jobs is the only thing that
/// bounds a board's size, so a peer-chosen far-future value meant a Job was never
/// reaped. Real Jobs are created with a two-minute lifetime (`Job::new`) and
/// occasionally extended, so an hour is generous. See
/// `docs/specifications/security-review-2.md` (finding R31).
const MAX_JOB_LIFETIME: chrono::TimeDelta = chrono::TimeDelta::hours(1);

/// Maximum number of Jobs accepted in a single `Command::Sync` payload.
const MAX_SYNCED_JOBS: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(bound = "")]
pub struct Envelope<L: Domain> {
    recipient: String,
    sender: String,
    zone: String,
    job: Job<L>,
}

impl<L: Domain> Envelope<L> {
    pub fn new(recipient: &str, sender: &str, zone: &str, job: &Job<L>) -> Self {
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

    pub fn job(&self) -> Job<L> {
        self.job.clone()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum Status {
    Created,
    Pending,
    Running,
    Complete,
    Error,
    Duplicate,
}

impl Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Status::Created => write!(f, "created"),
            Status::Pending => write!(f, "pending"),
            Status::Running => write!(f, "running"),
            Status::Complete => write!(f, "complete"),
            Status::Error => write!(f, "error"),
            Status::Duplicate => write!(f, "duplicate"),
        }
    }
}

impl std::str::FromStr for Status {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "created" => Ok(Status::Created),
            "pending" => Ok(Status::Pending),
            "running" => Ok(Status::Running),
            "complete" => Ok(Status::Complete),
            "error" => Ok(Status::Error),
            "duplicate" => Ok(Status::Duplicate),
            _ => Err(Error::Parse(format!("Unknown status: {}", s))),
        }
    }
}

///
/// This is the internal representation of the parsed command. We don't
/// make this publicly visible as we don't want to confuse users with too
/// many "command" types.
///
#[derive(Clone, PartialEq)]
struct Command<L: Domain> {
    destination: Destination,
    instruction: L::Instruction,
}

impl<L: Domain> Command<L> {
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

        let instruction = match L::parse_instruction(&parts.collect::<Vec<&str>>().join(" ")) {
            Ok(i) => i,
            Err(e) => {
                return Err(Error::Parse(format!(
                    "Could not parse instruction from command '{}': {}",
                    command, e
                )))
            }
        };

        if check_portal {
            if let Some(portal) = L::owning_portal(&instruction) {
                if portal.portal() != destination.first() {
                    tracing::error!(
                    "Invalid command '{}'. Commands involving portal '{}' can only be issued via the portal '{}', not '{}'.",
                    command, portal, portal.portal(), destination.first()
                );
                    return Err(Error::Parse(format!(
                    "Invalid command '{}'. Commands involving portal '{}' can only be issued via the portal '{}', not '{}'.",
                    command, portal, portal.portal(), destination.first()
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

    pub fn instruction(&self) -> L::Instruction {
        self.instruction.clone()
    }
}

impl<L: Domain> std::fmt::Debug for Command<L> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{} {}", self.destination, self.instruction)
    }
}

impl<L: Domain> std::fmt::Display for Command<L> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{} {}", self.destination, self.instruction,)
    }
}

// serialise via the string representation - this looks better
impl<L: Domain> Serialize for Command<L> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::ser::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

// deserialise via the string representation - this looks better

impl<'de, L: Domain> Deserialize<'de> for Command<L> {
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
#[serde(bound = "")]
pub struct Job<L: Domain> {
    id: Uuid,
    #[serde(with = "ts_seconds")]
    created: chrono::DateTime<Utc>,
    #[serde(with = "ts_seconds")]
    changed: chrono::DateTime<Utc>,
    #[serde(with = "ts_seconds")]
    expires: chrono::DateTime<Utc>,
    version: u64,
    command: Command<L>,
    state: Status,
    result: Option<String>,
    result_type: Option<String>,
    #[serde(default)]
    forwarded_for: Option<Destination>,
    /// Why this job failed, structured. `result` still carries the same text
    /// this holds in `message`, so a peer that reads only that is unaffected;
    /// this adds the machine-readable `kind` beside it.
    ///
    /// `None` either because the job did not fail, or because the peer that
    /// failed it predates the field - `Register`'s `supports_structured_errors`
    /// says which, and `JobError::infer` reconstructs a kind from the text
    /// when it is the latter. See `docs/plans/structured-errors-design.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<JobError>,
    /// The `Domain::name()` that authored this Job's instruction, set once
    /// at `Job::parse()` and never touched again - including by any
    /// domain-oblivious router hop it passes through, which relays it as
    /// just another opaque field it doesn't need to understand. `None`
    /// only for a Job from a peer running templemeads from before this
    /// field existed.
    #[serde(default)]
    domain: Option<String>,
    /// The domain's version, alongside `domain`.
    #[serde(default)]
    domain_version: Option<String>,
    #[serde(skip)]
    board: Option<Peer>,
}

// implement display for Job
impl<L: Domain> std::fmt::Display for Job<L> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.state {
            Status::Created => write!(f, "{{{}: Created}}", self.command),
            Status::Pending => write!(f, "{{{}: Pending}}", self.command),
            Status::Running => write!(f, "{{{}: Running}}", self.command),
            Status::Complete => match self.result.clone() {
                Some(result) => write!(f, "{{{}: Complete - {}}}", self.command, result),
                None => write!(f, "{{{}: Complete}}", self.command),
            },
            Status::Error => match self.result.clone() {
                Some(result) => write!(f, "{{{}: Error - {}}}", self.command, result),
                None => write!(f, "{{{}: Unknown Error}}", self.command),
            },
            Status::Duplicate => write!(f, "{{{}: Duplicate of {:?}}}", self.command, self.result),
        }
    }
}

impl<L: Domain> Job<L> {
    pub fn parse(command: &str, check_portal: bool) -> Result<Self, Error> {
        tracing::debug!("Parsing command: {:?}", command);

        let now = Utc::now();

        Ok(Self {
            id: Uuid::new_v4(),
            created: now,
            changed: now,
            // settled on 2 minutes as this makes the interface with the
            // user portal more responsive - any task that takes longer
            // than 2 minutes can have its lifetime changed using the
            // set_lifetime method
            expires: now + chrono::Duration::minutes(2),
            version: 1,
            command: Command::parse(command, check_portal)?,
            state: Status::Created,
            result: None,
            result_type: None,
            error: None,
            forwarded_for: None,
            domain: Some(L::name().to_string()),
            domain_version: Some(L::version().to_string()),
            board: None,
        })
    }

    /// The `Domain::name()` that authored this Job's instruction, if known -
    /// see the field doc comment on `Job` for what `None` means.
    pub fn domain(&self) -> Option<&str> {
        self.domain.as_deref()
    }

    /// The domain's version, alongside `domain()`.
    pub fn domain_version(&self) -> Option<&str> {
        self.domain_version.as_deref()
    }

    pub fn to_json(&self) -> Result<String, Error> {
        serde_json::to_string(self).map_err(Error::SerdeJson)
    }

    pub fn from_json(json: &str) -> Result<Self, Error> {
        serde_json::from_str(json).map_err(Error::SerdeJson)
    }

    pub fn id(&self) -> Uuid {
        self.id
    }

    pub fn destination(&self) -> Destination {
        self.command.destination()
    }

    pub fn instruction(&self) -> L::Instruction {
        self.command.instruction()
    }

    pub fn expires(&self) -> &chrono::DateTime<Utc> {
        &self.expires
    }

    ///
    /// Return this Job with its `expires` clamped to at most `MAX_JOB_LIFETIME`
    /// after `created`, and to at most that far in the future.
    ///
    /// `expires` arrives from the wire as whatever the sending peer wrote, and
    /// reaping is the only thing that bounds a board's size - so a Job claiming to
    /// expire in the year 3000 stayed on the board for the life of the process. A
    /// legitimate Job's lifetime is minutes. See
    /// `docs/specifications/security-review-2.md` (finding R31).
    ///
    pub fn clamp_expires(&self) -> Self {
        let now = Utc::now();

        // `checked_add_signed` rather than `+`: `created` is a wire field, so it
        // can sit near `DateTime::MAX` where the addition would panic - and with
        // `panic = "abort"` that is a remote process kill (cf. finding R25).
        let ceiling = match (
            self.created.checked_add_signed(MAX_JOB_LIFETIME),
            now.checked_add_signed(MAX_JOB_LIFETIME),
        ) {
            (Some(from_created), Some(from_now)) => std::cmp::min(from_created, from_now),
            // `created` is absurd; fall back to a ceiling relative to now only.
            (None, Some(from_now)) => from_now,
            // Only reachable if the clock itself is near DateTime::MAX.
            _ => now,
        };

        if self.expires <= ceiling {
            return self.clone();
        }

        tracing::warn!(
            "Job {} claims to expire at {}, which is beyond the {} minute maximum \
             lifetime - clamping it to {}.",
            self.id,
            self.expires,
            MAX_JOB_LIFETIME.num_minutes(),
            ceiling
        );

        let mut clamped = self.clone();
        clamped.expires = ceiling;
        clamped
    }

    pub fn set_lifetime(&self, lifetime: chrono::Duration) -> Self {
        Self {
            id: self.id,
            created: self.created,
            changed: self.changed,
            expires: self.created + lifetime,
            version: self.version,
            command: self.command.clone(),
            state: self.state.clone(),
            result: self.result.clone(),
            result_type: self.result_type.clone(),
            error: self.error.clone(),
            forwarded_for: self.forwarded_for.clone(),
            domain: self.domain.clone(),
            domain_version: self.domain_version.clone(),
            board: self.board.clone(),
        }
    }

    pub fn is_expired(&self) -> bool {
        self.expires < Utc::now()
    }

    pub fn is_pending(&self) -> bool {
        self.state == Status::Pending
    }

    pub fn is_finished(&self) -> bool {
        self.state == Status::Complete || self.state == Status::Error
    }

    pub fn is_duplicate(&self) -> bool {
        self.state == Status::Duplicate
    }

    pub fn is_running(&self) -> bool {
        self.state == Status::Running
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

    pub fn forwarded_for(&self) -> Option<Destination> {
        self.forwarded_for.clone()
    }

    pub fn with_forwarded_for(self, dest: Destination) -> Self {
        Self {
            forwarded_for: Some(dest),
            ..self
        }
    }

    /// A copy of this Job with `version` set to `version`, and `changed`
    /// stamped now - the bulk equivalent of calling `increment_version()`
    /// repeatedly, without the intermediate clones. See
    /// `docs/specifications/security-review-2.md` (finding R6).
    pub fn with_version(&self, version: u64) -> Self {
        Self {
            version,
            changed: Utc::now(),
            ..self.clone()
        }
    }

    pub fn increment_version(&self) -> Self {
        Self {
            id: self.id,
            created: self.created,
            changed: Utc::now(),
            expires: self.expires,
            // Saturating: `version` is a wire field, and the release profile
            // sets no `overflow-checks`, so `self.version + 1` at `u64::MAX`
            // wrapped silently to zero. See
            // `docs/specifications/security-review-2.md` (finding R6).
            version: self.version.saturating_add(1),
            command: self.command.clone(),
            state: self.state.clone(),
            result: self.result.clone(),
            result_type: self.result_type.clone(),
            error: self.error.clone(),
            forwarded_for: self.forwarded_for.clone(),
            domain: self.domain.clone(),
            domain_version: self.domain_version.clone(),
            board: self.board.clone(),
        }
    }

    pub fn assert_is_for_board(&self, agent: &Peer) -> Result<(), Error> {
        if self.is_expired() {
            return Err(Error::Expired(
                format!("Job {} has expired", self.id).to_owned(),
            ));
        }

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

    pub fn assert_is_not_expired(&self) -> Result<(), Error> {
        if self.is_expired() {
            Err(Error::Expired(
                format!("Job {} has expired", self.id).to_owned(),
            ))
        } else {
            Ok(())
        }
    }

    pub fn pending(&self) -> Result<Job<L>, Error> {
        match self.state {
            Status::Created => Ok(Job {
                id: self.id,
                created: self.created,
                changed: Utc::now(),
                expires: self.expires,
                version: self.version + 1,
                command: self.command.clone(),
                state: Status::Pending,
                result: self.result.clone(),
                result_type: self.result_type.clone(),
                error: self.error.clone(),
                forwarded_for: self.forwarded_for.clone(),
                domain: self.domain.clone(),
                domain_version: self.domain_version.clone(),
                board: self.board.clone(),
            }),
            Status::Pending => Ok(self.clone()),
            _ => Err(Error::InvalidState(
                format!("Cannot set pending on job in state: {:?}", self.state).to_owned(),
            )),
        }
    }

    pub fn is_duplicate_of(&self, job: &Job<L>) -> bool {
        self.command.destination().last() == job.command.destination().last()
            && self.command.instruction() == job.command.instruction()
            && job.is_pending()
            && !job.is_expired()
            && self.is_pending()
    }

    pub fn duplicate(&self, job: &Job<L>) -> Result<Job<L>, Error> {
        if !self.is_duplicate_of(job) {
            return Err(Error::InvalidState(
                format!("Job {} is not a duplicate of job {}", self, job).to_owned(),
            ));
        }

        tracing::debug!(
            "Setting job {} as a duplicate of job {}. Repeated command: {}",
            self.id,
            job.id,
            job.command
        );

        match self.state {
            Status::Pending => Ok(Job {
                id: self.id,
                created: self.created,
                changed: Utc::now(),
                expires: self.expires,
                version: self.version + 1,
                command: job.command.clone(),
                state: Status::Duplicate,
                result: job.id.to_string().into(),
                result_type: None,
                error: None,
                forwarded_for: self.forwarded_for.clone(),
                domain: self.domain.clone(),
                domain_version: self.domain_version.clone(),
                board: self.board.clone(),
            }),
            _ => Err(Error::InvalidState(
                format!("Cannot set duplicate on job in state: {:?}", self.state).to_owned(),
            )),
        }
    }

    pub fn running(&self, progress: Option<String>) -> Result<Job<L>, Error> {
        match self.state {
            Status::Pending | Status::Running => Ok(Job {
                id: self.id,
                created: self.created,
                changed: Utc::now(),
                expires: self.expires,
                version: self.version + 1,
                command: self.command.clone(),
                state: Status::Running,
                result: progress,
                result_type: None,
                error: None,
                forwarded_for: self.forwarded_for.clone(),
                domain: self.domain.clone(),
                domain_version: self.domain_version.clone(),
                board: self.board.clone(),
            }),
            _ => Err(Error::InvalidState(
                format!("Cannot set running on job in state: {:?}", self.state).to_owned(),
            )),
        }
    }

    pub fn copy_result_from(&self, other: &Job<L>) -> Result<Job<L>, Error> {
        // check other has finished and is error or completed
        if !other.is_finished() {
            return Err(Error::InvalidState(
                format!("Cannot copy result from job in state: {:?}", other.state).to_owned(),
            ));
        }

        match self.state {
            Status::Duplicate | Status::Pending | Status::Running => Ok(Job {
                id: self.id,
                created: self.created,
                changed: Utc::now(),
                expires: self.expires,
                version: self.version + 1000,
                command: self.command.clone(),
                state: other.state.clone(),
                result: other.result.clone(),
                result_type: other.result_type.clone(),
                error: other.error.clone(),
                forwarded_for: self.forwarded_for.clone(),
                domain: self.domain.clone(),
                domain_version: self.domain_version.clone(),
                board: self.board.clone(),
            }),
            _ => Err(Error::InvalidState(
                format!("Cannot copy result from job in state: {:?}", self.state).to_owned(),
            )),
        }
    }

    pub fn completed_none(&self) -> Result<Job<L>, Error> {
        match self.state {
            Status::Pending | Status::Running => Ok(Job {
                id: self.id,
                created: self.created,
                changed: Utc::now(),
                expires: self.expires,
                version: self.version + 1000, // make sure this is the newest version
                command: self.command.clone(),
                state: Status::Complete,
                result: None,
                result_type: Some("None".to_string()),
                error: None,
                forwarded_for: self.forwarded_for.clone(),
                domain: self.domain.clone(),
                domain_version: self.domain_version.clone(),
                board: self.board.clone(),
            }),
            _ => Err(Error::InvalidState(
                format!("Cannot set complete on job in state: {:?}", self.state).to_owned(),
            )),
        }
    }

    pub fn completed<T>(&self, result: T) -> Result<Job<L>, Error>
    where
        T: serde::Serialize,
        T: NamedType,
    {
        match self.state {
            Status::Pending | Status::Running => Ok(Job {
                id: self.id,
                created: self.created,
                changed: Utc::now(),
                expires: self.expires,
                version: self.version + 1000, // make sure this is the newest version
                command: self.command.clone(),
                state: Status::Complete,
                result: Some(serde_json::to_string(&result)?),
                result_type: Some(T::type_name()),
                error: None,
                forwarded_for: self.forwarded_for.clone(),
                domain: self.domain.clone(),
                domain_version: self.domain_version.clone(),
                board: self.board.clone(),
            }),
            _ => Err(Error::InvalidState(
                format!("Cannot set complete on job in state: {:?}", self.state).to_owned(),
            )),
        }
    }

    /// Mark this job as failed, with a kind inferred from `message`.
    ///
    /// Every existing caller keeps working and acquires a structured kind for
    /// free: the domain is asked to classify the message first, and the
    /// transport's own sentinels are recognised otherwise. Use
    /// [`Self::errored_with`] when the kind is already known - which is always
    /// better, since inference is a reading of prose and this is not.
    pub fn errored(&self, message: &str) -> Result<Job<L>, Error> {
        let domain_kind = L::error_kind_for(JobError::unwrap_message(message));

        self.errored_with(JobError::infer(message, domain_kind))
    }

    /// Mark this job as failed with an explicit [`JobError`].
    ///
    /// The error's `message` is also written to `result`, exactly where the
    /// failure text has always been, so a peer that has never heard of
    /// structured errors reads the same string it read before.
    pub fn errored_with(&self, error: JobError) -> Result<Job<L>, Error> {
        match self.state {
            Status::Duplicate | Status::Pending | Status::Running => Ok(Job {
                id: self.id,
                created: self.created,
                changed: Utc::now(),
                expires: self.expires,
                version: self.version + 1000, // make sure this is the newest version
                command: self.command.clone(),
                state: Status::Error,
                result: Some(error.message().to_owned()),
                result_type: Some("Error".to_string()),
                error: Some(error),
                forwarded_for: self.forwarded_for.clone(),
                domain: self.domain.clone(),
                domain_version: self.domain_version.clone(),
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

    /// Why this job failed, structured.
    ///
    /// `None` on a job that did not fail. Also `None` on one failed by a peer
    /// that predates the field - use [`Self::error_or_infer`] to get the best
    /// available answer in that case.
    pub fn error(&self) -> Option<&JobError> {
        match self.state {
            Status::Error => self.error.as_ref(),
            _ => None,
        }
    }

    /// Why this job failed, falling back to inference for an older peer.
    ///
    /// Prefer this over [`Self::error`] anywhere the answer drives a decision:
    /// it gives the same answer for a modern peer, and the best available one
    /// for a peer that only sent prose.
    pub fn error_or_infer(&self) -> Option<JobError> {
        match self.error() {
            Some(error) => Some(error.clone()),
            None => self.error_message().map(|message| {
                let domain_kind = L::error_kind_for(JobError::unwrap_message(&message));
                JobError::infer(&message, domain_kind)
            }),
        }
    }

    /// Drop the `origin` from this job's error, if it has one.
    ///
    /// `origin` names an agent inside the network. The bridge applies this to
    /// every job it serves, so a connected portal never learns internal
    /// topology from a failure - see [`JobError::redact_origin`].
    pub fn redact_error_origin(&mut self) {
        if let Some(error) = self.error.as_mut() {
            error.redact_origin();
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
            Status::Duplicate => Some("Pending".to_owned()),
            Status::Error => Some("Error".to_owned()),
        }
    }

    pub fn result_json(&self) -> Result<String, Error> {
        match self.state {
            Status::Created => Ok("null".to_string()),
            Status::Pending => Ok("null".to_string()),
            Status::Duplicate => Ok("null".to_string()),
            Status::Running => Ok("null".to_string()),
            Status::Error => match &self.result {
                Some(result) => Err(Error::Run(result.clone())),
                None => Err(Error::InvalidState("Unknown error".to_owned())),
            },
            Status::Complete => match &self.result {
                Some(result) => Ok(result.clone()),
                None => Ok("{}".to_string()),
            },
        }
    }

    pub fn result_type(&self) -> Result<String, Error> {
        match self.state {
            Status::Created => Ok("None".to_string()),
            Status::Pending => Ok("None".to_string()),
            Status::Duplicate => Ok("None".to_string()),
            Status::Running => Ok("None".to_string()),
            Status::Error => match &self.result_type {
                Some(t) => Ok(t.clone()),
                None => Ok("Error".to_string()),
            },
            Status::Complete => match &self.result_type {
                Some(t) => Ok(t.clone()),
                None => Ok("None".to_string()),
            },
        }
    }

    pub fn result<T>(&self) -> Result<Option<T>, Error>
    where
        T: serde::de::DeserializeOwned,
    {
        match self.state {
            Status::Created => Ok(None),
            Status::Pending => Ok(None),
            Status::Duplicate => Ok(None),
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

    pub fn result_none(&self) -> Result<(), Error> {
        match self.state {
            Status::Created => Ok(()),
            Status::Pending => Ok(()),
            Status::Duplicate => Ok(()),
            Status::Running => Ok(()),
            Status::Error => match &self.result {
                Some(result) => Err(Error::Run(result.clone())),
                None => Err(Error::InvalidState("Unknown error".to_owned())),
            },
            Status::Complete => match self.result_type() {
                Ok(t) => {
                    if t == "None" {
                        Ok(())
                    } else {
                        Err(Error::InvalidState(
                            "Result type is not None for completed job".to_owned(),
                        ))
                    }
                }
                Err(e) => Err(e),
            },
        }
    }

    pub async fn execute(&self) -> Result<Job<L>, Error> {
        self.assert_is_not_expired()?;

        match self.state() {
            Status::Pending => {
                tracing::debug!("Running job.execute() for job: {:?}", self);
                self.errored(format!("No default runner for job: {:?}", self).as_str())
            }
            _ => Err(Error::InvalidState(
                format!("Cannot execute job in state: {:?}", self.state).to_owned(),
            )),
        }
    }

    pub async fn received(&self, peer: &Peer) -> Result<Job<L>, Error> {
        if self.state == Status::Created {
            return Err(Error::InvalidState(
                format!("A created job should not have been received? {:?}", self).to_owned(),
            ));
        }

        self.assert_is_not_expired()?;

        let mut job = self.clone();

        // get a RwLock to the board from the shared state
        let board = match state::get::<L>(peer).await {
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

            (job, _) = board.add(&job)?;
        }

        Ok(job)
    }

    pub async fn put(&self, peer: &Peer) -> Result<Job<L>, Error> {
        tracing::debug!("Put {} : {}", self.destination(), self.instruction());

        self.assert_is_not_expired()?;

        // transition the job to pending, recording where it was sent
        let mut job = self.pending()?;

        // get a RwLock to the board from the shared state
        let board = match state::get::<L>(peer).await {
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

            let state;

            (job, state) = board.add(&job)?;

            if state == JobAddState::Unchanged || state == JobAddState::Duplicated {
                // The board already contains this version of the job,
                // or a job which is a duplicate of this one.
                // There is no need to send to the peer
                // (the job has already been sent)
                if job.is_duplicate() {
                    tracing::info!("Not sending duplicate job: {}", job.instruction());
                }

                return Ok(job);
            }
        }

        // now send it to the agent for processing
        match ControlCommand::put(&job).send_to(peer).await {
            Ok(_) => (),
            Err(e) => {
                // if we can't send the command, then we need to need to add
                // it to a queue for sending once the peer is back online
                tracing::warn!("Error sending command to agent: {:?}", e);
                let mut board = board.write().await;
                board.queue(ControlCommand::put(&job));
            }
        }

        Ok(job)
    }

    pub async fn updated(&self) -> Result<Job<L>, Error> {
        self.assert_is_not_expired()?;

        let agent = match self.board {
            Some(ref a) => a,
            None => {
                return Err(Error::InvalidBoard(
                    "Job has no board, so cannot be updated".to_owned(),
                ))
            }
        };

        // get a RwLock to the board from the shared state
        let board = match state::get::<L>(agent).await {
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

            let (job, state) = board.add(self)?;

            // add the job to the board - we need to set our board to the agent
            // first, so that the board can check it is correct
            if state != JobAddState::Unchanged {
                // The board already contains this version of the job
                // There is no change, so no need to send to the peer
                // (the job has already been sent)
                return Ok(job);
            }
        }

        Ok(self.clone())
    }

    pub async fn update(&self, peer: &Peer) -> Result<Job<L>, Error> {
        self.assert_is_not_expired()?;

        let mut job = self.clone();

        // get a RwLock to the board from the shared state
        let board = match state::get::<L>(peer).await {
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

            let state;

            (job, state) = board.add(&job)?;

            if state == JobAddState::Unchanged {
                // The board already contains this version of the job
                // There is no change, so no need to send to the peer
                // (the job has already been sent)
                return Ok(job);
            }
        }

        // now send it to the agent for processing
        match ControlCommand::update(&job).send_to(peer).await {
            Ok(_) => (),
            Err(e) => {
                // if we can't send the command, then we need to need to add
                // it to a queue for sending once the peer is back online
                tracing::debug!("Error sending command to agent: {:?}", e);
                let mut board = board.write().await;
                board.queue(ControlCommand::update(&job));
            }
        }

        Ok(job)
    }

    /// Update a job that was processed by a virtual agent
    ///
    /// This handles the special case where a virtual agent (virtual_peer) processes a job
    /// on behalf of a hosting agent (hosting_peer). It:
    /// 1. Updates the hosting agent's board directly
    /// 2. Sends the Update message to the upstream agent (previous to the hosting agent)
    ///
    /// # Arguments
    /// * `virtual_peer` - The virtual agent that processed the job (e.g., isambard-ai)
    /// * `hosting_peer` - The agent hosting the virtual agent (e.g., waldur)
    pub async fn virtual_update(
        &self,
        virtual_peer: &Peer,
        hosting_peer: &Peer,
    ) -> Result<Job<L>, Error> {
        self.assert_is_not_expired()?;

        let mut job = self.clone();

        // First, update the hosting agent's board directly (since it "processed" the job)
        let board = match state::get::<L>(hosting_peer).await {
            Ok(b) => b.board().await,
            Err(e) => {
                tracing::error!(
                    "Error getting board for hosting agent: {:?}. Is this agent known to us?",
                    e
                );
                return Err(e);
            }
        };

        {
            let mut board = board.write().await;
            job.board = Some(hosting_peer.clone());
            match board.add(&job) {
                Ok((_, _)) => {}
                Err(e) => {
                    tracing::error!(
                        "Error updating hosting agent {} board with job {}: {:?}",
                        hosting_peer,
                        job,
                        e
                    );
                }
            };
        }

        // Now update the virtual agent's board
        let board = match state::get::<L>(virtual_peer).await {
            Ok(b) => b.board().await,
            Err(e) => {
                tracing::error!(
                    "Error getting board for virtual agent: {:?}. Is this agent known to us?",
                    e
                );
                return Err(e);
            }
        };

        {
            let mut board = board.write().await;
            job.board = Some(virtual_peer.clone());
            match board.add(&job) {
                Ok((_, _)) => {}
                Err(e) => {
                    tracing::error!(
                        "Error updating virtual agent {} board with job {}: {:?}",
                        virtual_peer,
                        job,
                        e
                    );
                }
            };
        }

        // Now determine where to send the Update: to the agent upstream of the hosting agent
        // Find the previous agent before the hosting agent in the destination chain
        if let Some(upstream_agent) = self.destination().previous(hosting_peer.name()) {
            let upstream_peer = Peer::new(&upstream_agent, hosting_peer.zone());

            tracing::debug!(
                "Virtual agent {} (hosted by {}) sending update to upstream agent {}",
                virtual_peer,
                hosting_peer,
                upstream_peer
            );

            // now update the upstream agent's board with the updated job
            let board = match state::get::<L>(&upstream_peer).await {
                Ok(b) => b.board().await,
                Err(e) => {
                    tracing::error!(
                        "Error getting board for upstream agent: {:?}. Is this agent known to us?",
                        e
                    );
                    return Err(e);
                }
            };

            {
                let mut board = board.write().await;
                job.board = Some(upstream_peer.clone());

                match board.add(&job) {
                    Ok((_, _)) => {}
                    Err(e) => {
                        tracing::error!(
                            "Error updating upstream agent {} board with job {}: {:?}",
                            upstream_peer,
                            job,
                            e
                        );
                    }
                };
            }

            // Send the update to the upstream agent
            // The message should appear to come from the hosting agent
            match ControlCommand::update(&job).send_to(&upstream_peer).await {
                Ok(_) => (),
                Err(e) => {
                    tracing::debug!("Error sending command to upstream agent: {:?}", e);
                    let mut board = board.write().await;
                    board.queue(ControlCommand::update(&job));
                }
            }
        } else {
            // No upstream agent - the hosting agent is at the start of the chain
            // This means the job is complete
            tracing::error!(
                "Virtual agent {} (hosted by {}) has no upstream agent - job complete",
                virtual_peer,
                hosting_peer
            );
        }

        Ok(job)
    }

    pub async fn deleted(&self, peer: &Peer) -> Result<Job<L>, Error> {
        self.assert_is_not_expired()?;

        let mut job = self.clone();

        // get a RwLock to the board from the shared state
        let board = match state::get::<L>(peer).await {
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
            let changed = board.remove(&job)?;
            job.board = None;

            if !changed {
                // The board already contains this version of the job
                // There is no change, so no need to send to the peer
                // (the job has already been sent)
                return Ok(job);
            }
        }

        Ok(job)
    }

    pub async fn delete(&self, peer: &Peer) -> Result<Job<L>, Error> {
        self.assert_is_not_expired()?;

        let mut job = self.clone();

        // get a RwLock to the board from the shared state
        let board = match state::get::<L>(peer).await {
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
            let changed = board.remove(&job)?;
            job.board = None;

            if !changed {
                // The board already contains this version of the job
                // There is no change, so no need to send to the peer
                // (the job has already been sent)
                return Ok(job);
            }
        }

        // now send it to the agent for processing
        match ControlCommand::delete(&job).send_to(peer).await {
            Ok(_) => (),
            Err(e) => {
                // if we can't send the command, then we need to need to add
                // it to a queue for sending once the peer is back online
                tracing::debug!("Error sending command to agent: {:?}", e);
                let mut board = board.write().await;
                board.queue(ControlCommand::delete(&job));
            }
        }

        Ok(job)
    }

    async fn _wait(&self) -> Result<Job<L>, Error> {
        if self.is_finished() || self.is_expired() {
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
        let board = match state::get::<L>(agent).await {
            Ok(b) => b.board().await,
            Err(e) => {
                tracing::error!(
                    "Error getting board for agent: {:?}. Is this agent known to us?",
                    e
                );
                return Err(e);
            }
        };

        let waiter: Waiter<L>;

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

    pub async fn wait(&self) -> Result<Job<L>, Error> {
        let mut job = self._wait().await?;

        // if the job is still running, then we need to wait for it to finish
        let mut rewaits = 0;

        while !job.is_finished() {
            // wait for the job to finish
            tracing::warn!("Wait returned even if the job is not finished: {:?}", job);
            rewaits += 1;

            if rewaits > 10 {
                tracing::error!("Job is still not finished after 10 waits: {:?}", job);
                return Err(Error::InvalidState(
                    "Job is still not finished after 10 waits".to_owned(),
                ));
            }

            job = job._wait().await?;
        }

        Ok(job)
    }

    pub async fn try_wait(&self, timeout_ms: u64) -> Result<Option<Job<L>>, Error> {
        if self.is_finished() || self.is_expired() {
            return Ok(Some(self.clone()));
        } else if timeout_ms == 0 {
            return Ok(Some(self.wait().await?));
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
        let board = match state::get::<L>(agent).await {
            Ok(b) => b.board().await,
            Err(e) => {
                tracing::error!(
                    "Error getting board for agent: {:?}. Is this agent known to us?",
                    e
                );
                return Err(e);
            }
        };

        let waiter: Waiter<L>;

        // in a scope so we drop the lock asap
        {
            // get the mutable board from the Arc<RwLock> board - this is the
            // blocking operation
            let mut board = board.write().await;

            // return a waiter for the job constructed from the board
            waiter = board.get_waiter(self)?;
        }

        // wait for the job to finish
        waiter.try_result(timeout_ms).await
    }
}

///
/// Function used to sync the board with the specified peer.
/// We will send our board, while the peer should also send its
/// board. From the two exchanges we should recover our true
/// shared state
///
pub async fn sync_board<L: Domain>(peer: &Peer) -> Result<(), Error> {
    // get a RwLock to the board from the shared state
    let board = match state::get::<L>(peer).await {
        Ok(b) => b.board().await,
        Err(e) => {
            tracing::error!(
                "Error getting board for agent: {:?}. Is this agent known to us?",
                e
            );
            return Err(e);
        }
    };

    // get the board sync state
    let sync_state = board.read().await.sync_state();

    // now send this to the peer
    match ControlCommand::sync(&sync_state).send_to(peer).await {
        Ok(_) => (),
        Err(e) => {
            tracing::error!("Error sending sync command to agent: {:?}", e);
            return Err(e);
        }
    }

    Ok(())
}

///
/// Function used to process the sync message received from the specified
/// peer
///
pub async fn sync_from_peer<L: Domain>(
    recipient: &str,
    peer: &Peer,
    sync: &SyncState<L>,
) -> Result<(), Error> {
    tracing::debug!("Syncing state from peer {}", peer);

    let jobs = sync.jobs();

    if jobs.is_empty() {
        tracing::debug!("No jobs to sync from peer {}", peer);
        return Ok(());
    }

    // `Command::Sync` carries a peer-supplied `Vec<Job>` that is re-injected into
    // the inbound channel, so an oversized one is both a large allocation and a
    // large amount of downstream work. A real sync carries a board's worth of live
    // Jobs. See `docs/specifications/security-review-2.md` (finding R31).
    if jobs.len() > MAX_SYNCED_JOBS {
        tracing::warn!(
            "Refusing to sync {} jobs from peer {} - the limit is {}.",
            jobs.len(),
            peer,
            MAX_SYNCED_JOBS
        );

        return Err(Error::Unavailable(format!(
            "Peer {} tried to sync {} jobs, which is more than the {} allowed",
            peer,
            jobs.len(),
            MAX_SYNCED_JOBS
        )));
    }

    let mut update_jobs = Vec::new();
    let mut put_jobs = Vec::new();

    let mut num_synced = 0;

    // loop over all of the jobs in the sync state and process them
    {
        // get a RwLock to the board from the shared state
        let board = match state::get::<L>(peer).await {
            Ok(b) => b.board().await,
            Err(e) => {
                tracing::error!(
                    "Error getting board for agent: {:?}. Is this agent known to us?",
                    e
                );
                return Err(e);
            }
        };

        let board = board.read().await;

        // loop through each job and see if we have them already in the board?
        for job in jobs {
            if board.would_be_changed_by(job) {
                match job.state() {
                    Status::Complete => {
                        // we don't need to run this again, so just update
                        update_jobs.push(job);
                    }
                    Status::Error => {
                        // we don't need to run this again, so just update
                        update_jobs.push(job);
                    }
                    _ => match job.destination().position(recipient, peer.name()) {
                        Position::Upstream => {
                            // sending the results back up to the putter
                            update_jobs.push(job);
                        }
                        Position::Downstream => {
                            // putting the job down to the destination
                            put_jobs.push(job);
                        }
                        Position::Destination => {
                            // we are the destination, so re-run the job
                            put_jobs.push(job);
                        }
                        _ => {
                            tracing::error!("Job has got into an errored position: {:?}", job);
                            tracing::error!("Ignoring this job during the state update");
                        }
                    },
                }
            } else {
                tracing::debug!("Already have job: {} on the board", job);
            }
        }
    }

    // ok - we now have all of the put and updates - send all the
    // updates first, then the puts
    for job in update_jobs {
        if !job.is_expired() {
            tracing::debug!("Updating job: {}", job);
            num_synced += 1;

            match ControlCommand::update(job).received_from(peer) {
                Ok(_) => (),
                Err(e) => {
                    tracing::error!("Error sending update command to agent: {:?}", e);
                    tracing::error!("Ignoring this job during the state update");
                }
            }
        }
    }

    for job in put_jobs {
        if !job.is_expired() {
            tracing::debug!("Putting job: {}", job);
            num_synced += 1;

            match ControlCommand::put(job).received_from(peer) {
                Ok(_) => (),
                Err(e) => {
                    tracing::error!("Error sending put command to agent: {:?}", e);
                    tracing::error!("Ignoring this job during the state update");
                }
            }
        }
    }

    match num_synced {
        0 => tracing::info!("No jobs synced from peer {}", peer),
        1 => tracing::info!("1 job synced from peer {}", peer),
        _ => tracing::info!("{} jobs synced from peer {}", num_synced, peer),
    }

    Ok(())
}

///
/// Function used to send all jobs that were queued for the specified peer
///
pub async fn send_queued<L: Domain>(peer: &Peer) -> Result<(), Error> {
    // get a RwLock to the board from the shared state
    let board = match state::get::<L>(peer).await {
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
    let queued: Vec<ControlCommand<L>>;

    // in a scope so we drop the lock asap
    {
        // get the mutable board from the Arc<RwLock> board - this is the
        // blocking operation
        let mut board = board.write().await;
        queued = board.take_queued();
    }

    // now send all of the queued jobs - if anything goes wrong,
    // the job will automatically put itself back on the queue
    for command in queued {
        tracing::debug!("Running queued command: {:?}", command);

        match command {
            ControlCommand::Put { job } => {
                job.put(peer).await?;
            }
            ControlCommand::Update { job } => {
                job.update(peer).await?;
            }
            ControlCommand::Delete { job } => {
                job.delete(peer).await?;
            }
            _ => {
                tracing::error!("Unknown command: {:?}", command);
            }
        }
    }

    Ok(())
}

///
/// Assert that the job with the specified expiry time has not expired
///
pub fn assert_not_expired(expiry: &chrono::DateTime<Utc>) -> Result<(), Error> {
    if Utc::now() > *expiry {
        return Err(Error::Expired(
            format!("Job expired at: {}", expiry).to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::test_domain::TestDomain;

    type Command = super::Command<TestDomain>;
    type Job = super::Job<TestDomain>;

    #[test]
    fn test_a_peer_supplied_expiry_is_clamped() {
        // `expires` is a wire field, and reaping expired Jobs is the only thing that
        // bounds a board's size - so a Job claiming to expire in the year 3000 sat
        // on the board for the life of the process. See finding R31.
        let job = Job::parse("portal.cluster add_user demo.proj.portal", true).unwrap();

        // A normal Job is untouched.
        assert_eq!(job.clamp_expires().expires(), job.expires());

        // A far-future expiry is pulled back to the ceiling.
        let far_future = job.set_lifetime(chrono::TimeDelta::days(365 * 1000));
        let clamped = far_future.clamp_expires();

        assert!(
            clamped.expires() < far_future.expires(),
            "a millennium-long lifetime must be clamped"
        );
        assert!(
            *clamped.expires() <= Utc::now() + MAX_JOB_LIFETIME + chrono::TimeDelta::seconds(5),
            "clamped expiry {} is still beyond the ceiling",
            clamped.expires()
        );

        // Clamping never *extends* a short lifetime, and is idempotent.
        let short = job.set_lifetime(chrono::TimeDelta::seconds(30));
        assert_eq!(short.clamp_expires().expires(), short.expires());
        assert_eq!(
            clamped.clamp_expires().expires(),
            clamped.expires(),
            "clamping twice must be a no-op"
        );
    }

    #[test]
    fn test_board_add_rejects_an_implausible_version() {
        // Regression test for finding R6, part 1. `version` is a wire field
        // with no validation; a value anywhere near this is a bug or an
        // attacker, not a real Job.
        use crate::agent::Peer;
        use crate::board::Board;

        let peer = Peer::new("cluster", "default");
        let mut board = Board::<TestDomain>::new(&peer);

        let mut job = Job::parse("portal.cluster add_user demo.proj.portal", true)
            .unwrap_or_else(|e| unreachable!("job: {:?}", e));
        job.board = Some(peer.clone());

        // A normal version is accepted...
        job.version = 3;
        assert!(board.add(&job).is_ok());

        // ...and an implausible one is refused rather than acted on.
        job.version = (1u64 << 60) + 1;
        assert!(board.add(&job).is_err());

        job.version = u64::MAX;
        assert!(board.add(&job).is_err());
    }

    #[test]
    fn test_board_add_supersedes_a_version_without_looping() {
        // Regression test for finding R6, part 2. This branch used to
        // `increment_version()` in a loop until it passed the stored version -
        // deep-cloning the Job each time, synchronously, while holding the
        // board's write lock. A stored version of 2^40 therefore drove ~10^12
        // clones; `u64::MAX` never terminated at all, because the increment
        // wrapped in release builds. It must now jump straight past.
        use crate::agent::Peer;
        use crate::board::Board;

        let peer = Peer::new("cluster", "default");
        let mut board = Board::<TestDomain>::new(&peer);

        let mut stored = Job::parse("portal.cluster add_user demo.proj.portal", true)
            .unwrap_or_else(|e| unreachable!("job: {:?}", e));
        stored.board = Some(peer.clone());
        stored.version = 1 << 40;
        stored.changed = Utc::now();

        assert!(board.add(&stored).is_ok());

        // Same id, *lower* version but a newer `changed` - the branch that
        // previously looped. It must return promptly with a version above the
        // one it superseded.
        let mut newer = stored.clone();
        newer.version = 0;
        newer.changed = stored.changed + chrono::Duration::seconds(10);

        let (result, _) = board
            .add(&newer)
            .unwrap_or_else(|e| unreachable!("add: {:?}", e));

        assert_eq!(result.version(), (1u64 << 40) + 1);
    }

    #[test]
    fn test_increment_version_saturates() {
        // The release profile sets no `overflow-checks`, so `version + 1` at
        // `u64::MAX` used to wrap silently to zero (finding R6).
        let mut job = Job::parse("portal.cluster add_user demo.proj.portal", true)
            .unwrap_or_else(|e| unreachable!("job: {:?}", e));

        job.version = u64::MAX;
        assert_eq!(job.increment_version().version(), u64::MAX);

        job.version = 7;
        assert_eq!(job.increment_version().version(), 8);
        assert_eq!(job.with_version(100).version(), 100);
    }

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

        job = job.completed("done".to_string()).unwrap_or(job);

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
    /// The whole point of the compatibility story: a job failed by a peer that
    /// has never heard of structured errors still yields a usable kind.
    #[test]
    fn a_failure_from_an_older_peer_still_yields_a_kind() {
        let job = Job::parse("portal.cluster add_user demo.proj.portal", true)
            .unwrap_or_else(|e| unreachable!("parse: {:?}", e))
            .pending()
            .unwrap_or_else(|e| unreachable!("pending: {:?}", e));
        let failed = job
            .errored("RuntimeError{no such project}")
            .unwrap_or_else(|e| unreachable!("errored: {:?}", e));

        // Simulate the wire form an older peer sends: prose, no `error` field.
        let mut json: serde_json::Value =
            serde_json::to_value(&failed).unwrap_or_else(|e| unreachable!("serialise: {:?}", e));
        json.as_object_mut()
            .unwrap_or_else(|| unreachable!("job must be an object"))
            .remove("error");

        let legacy: Job = serde_json::from_value(json)
            .unwrap_or_else(|e| unreachable!("a job with no error field must parse: {:?}", e));

        // Nothing structured arrived...
        assert!(legacy.error().is_none());
        // ...but the message is intact, and a kind is recoverable from it.
        assert_eq!(
            legacy.error_message().as_deref(),
            Some("RuntimeError{no such project}")
        );
        assert_eq!(
            legacy.error_or_infer().map(|e| e.kind().to_owned()),
            Some(crate::joberror::kind::RUN.to_owned())
        );
    }

    /// A structured error must not change the prose. This is what keeps an old
    /// peer - which reads only `result` - working unchanged.
    #[test]
    fn the_prose_is_unchanged_by_carrying_a_kind() {
        let job = Job::parse("portal.cluster add_user demo.proj.portal", true)
            .unwrap_or_else(|e| unreachable!("parse: {:?}", e))
            .pending()
            .unwrap_or_else(|e| unreachable!("pending: {:?}", e));

        let message = "ManagedProjectPendingError: awaiting approval";
        let failed = job
            .errored_with(JobError::new("award_pending", message))
            .unwrap_or_else(|e| unreachable!("errored: {:?}", e));

        assert_eq!(failed.error_message().as_deref(), Some(message));
        assert_eq!(failed.error().map(|e| e.kind()), Some("award_pending"));

        // ...and it survives a round trip through the wire.
        let json =
            serde_json::to_string(&failed).unwrap_or_else(|e| unreachable!("serialise: {:?}", e));
        let back: Job =
            serde_json::from_str(&json).unwrap_or_else(|e| unreachable!("parse: {:?}", e));

        assert_eq!(back.error_message().as_deref(), Some(message));
        assert_eq!(back.error().map(|e| e.kind()), Some("award_pending"));
    }

    /// A kind this build has never seen must survive being relayed. A router
    /// hop carries a domain's vocabulary without understanding it.
    #[test]
    fn an_unknown_kind_relays_intact() {
        let job = Job::parse("portal.cluster add_user demo.proj.portal", true)
            .unwrap_or_else(|e| unreachable!("parse: {:?}", e))
            .pending()
            .unwrap_or_else(|e| unreachable!("pending: {:?}", e));
        let failed = job
            .errored_with(JobError::new("some_future_kind", "a thing happened"))
            .unwrap_or_else(|e| unreachable!("errored: {:?}", e));

        let json =
            serde_json::to_string(&failed).unwrap_or_else(|e| unreachable!("serialise: {:?}", e));
        let back: Job =
            serde_json::from_str(&json).unwrap_or_else(|e| unreachable!("parse: {:?}", e));

        assert_eq!(back.error().map(|e| e.kind()), Some("some_future_kind"));
    }

    /// `origin` is diagnostic-only and must not reach anything outside the
    /// agent network - the bridge redacts every job it serves.
    #[test]
    fn redacting_the_origin_keeps_the_kind_and_message() {
        let job = Job::parse("portal.cluster add_user demo.proj.portal", true)
            .unwrap_or_else(|e| unreachable!("parse: {:?}", e))
            .pending()
            .unwrap_or_else(|e| unreachable!("pending: {:?}", e));
        let mut failed = job
            .errored_with(
                JobError::new("award_rejected", "no").with_origin("portal.clusters.shared"),
            )
            .unwrap_or_else(|e| unreachable!("errored: {:?}", e));

        assert_eq!(
            failed.error().and_then(|e| e.origin()),
            Some("portal.clusters.shared")
        );

        failed.redact_error_origin();

        assert_eq!(failed.error().and_then(|e| e.origin()), None);
        assert_eq!(failed.error().map(|e| e.kind()), Some("award_rejected"));
        assert_eq!(failed.error_message().as_deref(), Some("no"));
    }

    /// A job that did not fail has no error, however it is asked.
    #[test]
    fn a_job_that_did_not_fail_has_no_error() {
        let job = Job::parse("portal.cluster add_user demo.proj.portal", true)
            .unwrap_or_else(|e| unreachable!("parse: {:?}", e));

        assert!(job.error().is_none());
        assert!(job.error_or_infer().is_none());
    }
}
