// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Error as AnyError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::job::{Error as JobError, Job};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Board {
    peer: String,
    jobs: HashMap<Uuid, Job>,

    // do not serialise or clone the waiters
    #[serde(skip)]
    waiters: HashMap<Uuid, Vec<Listener>>,
}

impl Clone for Board {
    /// Clone the board, but do not clone the waiters
    fn clone(&self) -> Self {
        Self {
            peer: self.peer.clone(),
            jobs: self.jobs.clone(),
            waiters: HashMap::new(),
        }
    }
}

impl Board {
    pub fn new(peer: &str) -> Self {
        Self {
            peer: peer.to_string(),
            jobs: HashMap::new(),
            waiters: HashMap::new(),
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
                return Err(Error::NotFound(format!("Job not found: {:?}", job.id())));
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
    /// This returns the job (it may be updated to be on a new board)
    ///
    pub fn add(&mut self, job: &Job) -> Result<(), Error> {
        job.assert_is_for_board(&self.peer)?;

        let mut updated = false;

        match self.jobs.get_mut(&job.id()) {
            Some(j) => {
                // only update if newer
                if job.version() > j.version() {
                    *j = job.clone();
                    updated = true;
                }
            }
            None => {
                self.jobs.insert(job.id(), job.clone());
                updated = true;
            }
        }

        // if we have any waiters for this job then notify them if the
        // job has been updated and it is in a finished state
        if updated && job.is_finished() {
            if let Some(listeners) = self.waiters.remove(&job.id()) {
                for listener in listeners {
                    listener.notify(job.clone());
                }
            }
        }

        Ok(())
    }

    ///
    /// Remove the passed job from our board
    /// If the job doesn't exist then we fail silently
    /// This returns the removed job
    ///
    pub fn remove(&mut self, job: &Job) -> Result<(), Error> {
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

        self.jobs.remove(&job.id());

        Ok(())
    }

    ///
    /// Get the job with the passed id
    /// If the job doesn't exist then we return an error
    ///
    pub fn get(&self, id: &Uuid) -> Result<Job, Error> {
        match self.jobs.get(id) {
            Some(j) => Ok(j.clone()),
            None => Err(Error::NotFound(format!("Job not found: {:?}", id))),
        }
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
                Err(_) => Err(Error::Unknown),
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

/// Errors

#[derive(Error, Debug)]
pub enum Error {
    #[error("{0}")]
    Any(#[from] AnyError),

    #[error("{0}")]
    NotFound(String),

    #[error("Unknown error")]
    Unknown,
}

// automatically convert to JobError
impl From<Error> for JobError {
    fn from(e: Error) -> JobError {
        JobError::Any(e.into())
    }
}

// automatically convert JobError to BoardError
impl From<JobError> for Error {
    fn from(e: JobError) -> Error {
        Error::Any(e.into())
    }
}
