// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::agent::Peer;
use crate::command::Command as ControlCommand;
use crate::error::Error;
use crate::job::Job;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum JobAddState {
    /// The job was added to the board
    Added,
    /// The job was already on the board, but it was updated
    Updated,
    /// The job was added to the board, but it is a duplicate of an existing job
    Duplicated,
    /// The job was not added because it was already on the board
    Unchanged,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct SyncState {
    jobs: Vec<Job>,
}

impl SyncState {
    pub fn jobs(&self) -> &Vec<Job> {
        &self.jobs
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Board {
    peer: Peer,
    jobs: HashMap<Uuid, Job>,

    // all of the queued commands that are waiting for the connection
    // to re-open, so that they can be sent
    queued_commands: Vec<ControlCommand>,

    // do not serialise or clone the waiters
    #[serde(skip)]
    waiters: HashMap<Uuid, Vec<Listener>>,

    // do not serialise the duplicates
    #[serde(skip)]
    duplicates: HashMap<Uuid, Vec<Uuid>>,
}

impl Clone for Board {
    /// Clone the board, but do not clone the waiters
    fn clone(&self) -> Self {
        Self {
            peer: self.peer.clone(),
            jobs: self.jobs.clone(),
            queued_commands: self.queued_commands.clone(),
            waiters: HashMap::new(),
            duplicates: self.duplicates.clone(),
        }
    }
}

impl Board {
    pub fn new(peer: &Peer) -> Self {
        Self {
            peer: peer.clone(),
            jobs: HashMap::new(),
            queued_commands: Vec::new(),
            waiters: HashMap::new(),
            duplicates: HashMap::new(),
        }
    }

    ///
    /// Return the sync state that can be used to synchronise this board
    /// with its copy on the peer
    ///
    pub fn sync_state(&self) -> SyncState {
        SyncState {
            jobs: self.jobs.values().cloned().collect(),
        }
    }

    ///
    /// Return a waiter that can be used to receive a job when
    /// it is either completed or errored. This will block until
    /// the passed job transitions into one of those states,
    /// and it will return the new version of the job
    ///
    pub fn get_waiter(&mut self, job: &Job) -> Result<Waiter, Error> {
        // check that we have this job on the board
        match self.jobs.get(&job.id()) {
            Some(j) => {
                // if the job is already in a terminal state then we can return
                if j.is_finished() {
                    return Ok(Waiter::finished(j.clone()));
                }
            }
            None => {
                // look through the queued commands...
                let mut found = false;

                for command in &self.queued_commands {
                    if let Some(j) = command.job() {
                        if j.id() == job.id() {
                            if j.is_finished() {
                                return Ok(Waiter::finished(j.clone()));
                            }

                            found = true;
                            break;
                        }
                    }
                }

                if !found {
                    return Err(Error::NotFound(format!("Job not found: {:?}", job.id())));
                }
            }
        }

        let (tx, rx) = oneshot::channel();

        // add the listener to the list of listeners
        match self.waiters.get_mut(&job.id()) {
            Some(listeners) => {
                listeners.push(Listener::new(tx));
            }
            None => {
                self.waiters.insert(job.id(), vec![Listener::new(tx)]);
            }
        }

        Ok(Waiter::pending(rx))
    }

    ///
    /// Add the passed job to our board - this will update the
    /// job if it already exists and the new job has a newer
    /// version
    ///
    /// The indicated board for the job must match the name of this board
    ///
    /// This returns the state change for the board, i.e.
    /// if the job was added, updated, duplicated, or unchanged.
    ///
    pub fn add(&mut self, job: &Job) -> Result<(Job, JobAddState), Error> {
        job.assert_is_for_board(&self.peer)?;

        let mut state = JobAddState::Unchanged;
        let mut job = job.clone();

        match self.jobs.get_mut(&job.id()) {
            Some(j) => {
                // only update if newer
                if job.version() > j.version() {
                    *j = job.clone();
                    state = JobAddState::Updated;
                }
                // else if the job is newer, then automatically create a new version
                else if job.changed() > j.changed() {
                    let newer_version = j.version();
                    *j = job.clone();

                    while j.version() <= newer_version {
                        *j = j.increment_version();
                    }

                    job = j.clone();
                    state = JobAddState::Updated;
                }
            }
            None => {
                // don't need to check if this has been queued, as
                // the next step would re-queue the job if there was
                // a problem, and job changes are idempotent (i.e.
                // it doesn't matter if this happens twice)
                self.jobs.insert(job.id(), job.clone());
                state = JobAddState::Added;
            }
        }

        // if this is a new job then check for any duplicates
        if state == JobAddState::Added && job.is_pending() {
            // do through all of the existing jobs to see if there are
            // any others that are pending and have the same destination
            // and command
            for (id, existing_job) in &self.jobs.clone() {
                if *id != job.id() && job.is_duplicate_of(existing_job) {
                    // change the status of our job to be a duplicate
                    let duplicate = match job.duplicate(existing_job) {
                        Ok(dup) => dup,
                        Err(e) => {
                            tracing::error!("Failed to create duplicate job: {}", e);
                            continue;
                        }
                    };

                    assert!(duplicate.is_duplicate());

                    // we now need to update this job to be a duplicate
                    self.jobs.insert(duplicate.id(), duplicate.clone());

                    // now record this as a duplicate for the original's ID
                    self.duplicates.entry(*id).or_default().push(duplicate.id());

                    tracing::info!(
                        "Number of duplicates for instruction {}: {}",
                        duplicate.instruction(),
                        self.duplicates.get(id).map_or(0, |v| v.len())
                    );

                    return Ok((duplicate, JobAddState::Duplicated));
                }
            }
        }

        // if we have any waiters for this job then notify them if the
        // job has been updated and it is in a finished state
        if (state == JobAddState::Added || state == JobAddState::Updated) && job.is_finished() {
            if let Some(listeners) = self.waiters.remove(&job.id()) {
                for listener in listeners {
                    listener.notify(job.clone());
                }
            }

            // if we have any duplicates for this job then we also
            // need to update those duplicates and notify their listeners
            if let Some(duplicate_ids) = self.duplicates.remove(&job.id()) {
                tracing::info!(
                    "Original finished: Updating {} duplicates for job {}",
                    duplicate_ids.len(),
                    job.id()
                );

                for duplicate_id in duplicate_ids {
                    if let Some(duplicate_job) = self.jobs.get_mut(&duplicate_id) {
                        // update the duplicate job to be finished
                        if !duplicate_job.is_finished() {
                            *duplicate_job = match duplicate_job.copy_result_from(&job) {
                                Ok(dup) => dup,
                                Err(e) => {
                                    tracing::error!(
                                        "Failed to copy result from job {}: {}",
                                        job.id(),
                                        e
                                    );
                                    match duplicate_job
                                        .errored("Failed to copy result from original job")
                                    {
                                        Ok(dup) => dup,
                                        Err(e) => {
                                            tracing::error!(
                                                "Failed to mark duplicate job as errored: {}",
                                                e
                                            );
                                            duplicate_job.clone()
                                        }
                                    }
                                }
                            };
                        }

                        // notify any listeners for the duplicate job
                        if let Some(listeners) = self.waiters.remove(&duplicate_id) {
                            for listener in listeners {
                                listener.notify(duplicate_job.clone());
                            }
                        }
                    }
                }
            }
        }

        Ok((job, state))
    }

    ///
    /// Remove the passed job from our board
    /// If the job doesn't exist then we fail silently
    ///
    /// This returns whether or not the board has changed
    /// (i.e. whether the job was on the board)
    ///
    pub fn remove(&mut self, job: &Job) -> Result<bool, Error> {
        job.assert_is_for_board(&self.peer)?;

        // if we have any waiters for this job then notify them with an error
        if let Some(listeners) = self.waiters.remove(&job.id()) {
            let mut notify_job = job.clone();

            if !notify_job.is_finished() {
                notify_job = notify_job.errored("Job removed from board")?;
            }

            for listener in listeners {
                listener.notify(notify_job.clone());
            }
        }

        let removed = self.jobs.remove(&job.id()).is_some();

        // we also need to wake up any waiters for this job and
        // remove any duplicates
        if let Some(listeners) = self.waiters.remove(&job.id()) {
            for listener in listeners {
                listener.notify(job.clone());
            }
        }

        if let Some(duplicate_ids) = self.duplicates.remove(&job.id()) {
            for duplicate_id in duplicate_ids {
                if let Some(mut duplicate_job) = self.jobs.remove(&duplicate_id) {
                    if !duplicate_job.is_finished() {
                        duplicate_job = match duplicate_job.copy_result_from(job) {
                            Ok(dup) => dup,
                            Err(e) => {
                                tracing::error!(
                                    "Failed to copy result from job {}: {}",
                                    job.id(),
                                    e
                                );
                                match duplicate_job
                                    .errored("Failed to copy result from original job")
                                {
                                    Ok(dup) => dup,
                                    Err(e) => {
                                        tracing::error!(
                                            "Failed to mark duplicate job as errored: {}",
                                            e
                                        );
                                        duplicate_job
                                    }
                                }
                            }
                        };
                    }

                    // notify any listeners for the duplicate job
                    if let Some(listeners) = self.waiters.remove(&duplicate_id) {
                        for listener in listeners {
                            listener.notify(duplicate_job.clone());
                        }
                    }
                }
            }
        }

        Ok(removed)
    }

    ///
    /// Get the job with the passed id
    /// If the job doesn't exist then we return an error
    ///
    pub fn get(&self, id: &Uuid) -> Result<Job, Error> {
        match self.jobs.get(id) {
            Some(j) => Ok(j.clone()),
            None => {
                // look through the queued jobs...
                for command in &self.queued_commands {
                    if let Some(job) = command.job() {
                        if &job.id() == id {
                            return Ok(job);
                        }
                    }
                }

                Err(Error::NotFound(format!("Job not found: {:?}", id)))
            }
        }
    }

    ///
    /// Add a job to the board that should be sent later, e.g.
    /// because the connection to the agent is currently unavailable
    ///
    pub fn queue(&mut self, command: ControlCommand) {
        tracing::info!("Queuing command: {:?}", command);

        // remove the job from the main board as it never made it
        // to the destination
        if let Some(job_id) = command.job_id() {
            self.jobs.remove(&job_id);
            self.queued_commands.push(command);
        } else {
            tracing::error!("Cannot queue command without a job id: {:?}", command);
        }
    }

    ///
    /// Take all of the queued commands - this removes the commands from this
    /// board and returns them as a list
    ///
    pub fn take_queued(&mut self) -> Vec<ControlCommand> {
        let mut queued_commands = Vec::new();
        std::mem::swap(&mut queued_commands, &mut self.queued_commands);
        queued_commands
    }

    ///
    /// Return whether or not this board would be changed by the
    /// passed job
    ///
    pub fn would_be_changed_by(&self, job: &Job) -> bool {
        if job.is_expired() {
            return false;
        }

        match self.jobs.get(&job.id()) {
            Some(j) => {
                // only update if newer
                job.version() > j.version()
            }
            None => true,
        }
    }

    ///
    /// Remove all expired jobs from the board
    ///
    pub fn remove_expired_jobs(&mut self) {
        let expired_jobs: Vec<Uuid> = self
            .jobs
            .iter()
            .filter_map(|(id, job)| {
                if job.is_expired() {
                    // make sure that the job is in an errored state
                    let job = match job.is_finished() {
                        true => job.clone(),
                        false => match job.errored("Job expired") {
                            Ok(j) => j,
                            Err(e) => {
                                tracing::error!("Failed to mark job as errored: {}", e);
                                job.clone()
                            }
                        },
                    };

                    // remove any listeners for this job
                    if let Some(listeners) = self.waiters.remove(id) {
                        for listener in listeners {
                            listener.notify(job.clone());
                        }
                    }

                    Some(*id)
                } else {
                    None
                }
            })
            .collect();

        for job_id in expired_jobs.iter() {
            let _ = self.jobs.remove(job_id);
        }

        // now remove any queued expired jobs
        self.queued_commands.retain(|command| {
            if let Some(job) = command.job() {
                if job.is_expired() {
                    tracing::debug!("Removing expired queued job {}", job);

                    // remove any listeners for this job
                    if let Some(listeners) = self.waiters.remove(&job.id()) {
                        for listener in listeners {
                            listener.notify(job.clone());
                        }
                    }

                    false
                } else {
                    true
                }
            } else {
                true
            }
        });
    }
}

///
/// A waiter that can be used to wait for when a job is finished,
/// or errored. This will block until the job transitions into one
/// of those states, and it will return the new version of the job
/// when it does
///
#[derive(Debug)]
pub enum Waiter {
    Pending(oneshot::Receiver<Job>),
    Finished(Box<Job>),
}

impl Waiter {
    pub fn pending(rx: oneshot::Receiver<Job>) -> Self {
        Waiter::Pending(rx)
    }

    pub fn finished(job: Job) -> Self {
        Waiter::Finished(Box::new(job))
    }

    pub async fn result(self) -> Result<Job, Error> {
        match self {
            Waiter::Pending(rx) => match rx.await {
                Ok(job) => Ok(job),
                Err(_) => Err(Error::Unknown("Failed to receive job".to_string())),
            },
            Waiter::Finished(job) => Ok(*job),
        }
    }
}

///
/// A listener that can be used to notify a waiter when a job
/// is finished, or errored
///
#[derive(Debug)]
pub struct Listener {
    tx: oneshot::Sender<Job>,
}

impl Listener {
    pub fn new(tx: oneshot::Sender<Job>) -> Self {
        Self { tx }
    }

    pub fn notify(self, job: Job) {
        let _ = self.tx.send(job);
    }
}
