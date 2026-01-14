// SPDX-FileCopyrightText: © 2025 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::oneshot;
use url::Url;
use uuid::Uuid;

use crate::board::{Listener, Waiter};
use crate::error::Error;
use crate::job::Job;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct BridgeBoard {
    jobs: HashMap<Uuid, Job>,

    signal_url: Option<Url>,

    // do not serialise or clone the waiters
    #[serde(skip)]
    waiters: HashMap<Uuid, Vec<Listener>>,
}

impl Clone for BridgeBoard {
    /// Clone the board, but do not clone the waiters
    fn clone(&self) -> Self {
        Self {
            jobs: self.jobs.clone(),
            signal_url: self.signal_url.clone(),
            waiters: HashMap::new(),
        }
    }
}

impl BridgeBoard {
    pub fn new() -> Self {
        Self {
            jobs: HashMap::new(),
            signal_url: None,
            waiters: HashMap::new(),
        }
    }

    ///
    /// Return a list of all of the unfinished jobs on the board
    ///
    pub fn unfinished_jobs(&self) -> Vec<Job> {
        self.jobs
            .iter()
            .filter_map(|(_, job)| {
                if job.is_finished() {
                    None
                } else {
                    Some(job.clone())
                }
            })
            .collect()
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
    /// Add the passed job to our board - this will return
    /// a waiter that can be used to wait until the job
    /// is completed or errored
    ///
    /// It is an error to attempt to add a job that is already
    /// present on the board
    ///
    pub fn add(&mut self, job: &Job) -> Result<Waiter, Error> {
        match self.jobs.get_mut(&job.id()) {
            Some(j) => {
                return Err(Error::Duplicate(format!(
                    "Job already exists on board: {:?}",
                    j.id()
                )))
            }
            None => {
                // add the job to the board
                self.jobs.insert(job.id(), job.clone());
            }
        }

        // return a waiter that can be used to wait until the job
        // is completed or errored
        self.get_waiter(job)
    }

    ///
    /// Update the passed job on our board
    ///
    pub fn update(&mut self, job: &Job) {
        // check that we have this job on the board
        match self.jobs.get_mut(&job.id()) {
            Some(j) => {
                // only update if newer
                if job.version() > j.version() {
                    *j = job.clone();

                    // notify any listeners that the job has been updated
                    if job.is_finished() {
                        if let Some(listeners) = self.waiters.remove(&job.id()) {
                            for listener in listeners {
                                listener.notify(job.clone());
                            }
                        }
                    }
                }
            }
            None => {
                tracing::warn!("Job not found on board: {:?}", job.id());
            }
        }
    }

    ///
    /// Remove the passed job from our board
    /// If the job doesn't exist then we fail silently
    ///
    /// This returns whether or not the board has changed
    /// (i.e. whether the job was on the board)
    ///
    pub fn remove(&mut self, job: &Job) -> Result<bool, Error> {
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

        Ok(removed)
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
    }

    pub fn set_signal_url(&mut self, url: Url) {
        self.signal_url = Some(url);
    }

    pub fn signal_url(&self) -> Option<Url> {
        self.signal_url.clone()
    }
}
