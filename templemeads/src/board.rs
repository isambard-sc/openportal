// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Error as AnyError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;
use uuid::Uuid;

use crate::job::Job;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Board {
    peer: String,
    jobs: HashMap<Uuid, Job>,
}

impl Board {
    pub fn new(peer: &str) -> Self {
        Self {
            peer: peer.to_string(),
            jobs: HashMap::new(),
        }
    }

    ///
    /// Add the passed job to our board - this will update the
    /// job if it already exists and the new job has a newer
    /// version
    ///
    pub async fn add(&mut self, job: &Job) -> Result<(), Error> {
        match self.jobs.get_mut(&job.id()) {
            Some(j) => {
                // only update if newer
                if job.version() > j.version() {
                    *j = job.clone();
                }
            }
            None => {
                self.jobs.insert(job.id(), job.clone());
            }
        }

        Ok(())
    }

    ///
    /// Remove the passed job from our board
    /// If the job doesn't exist then we fail silently
    ///
    pub async fn remove(&mut self, job: &Job) -> Result<(), Error> {
        self.jobs.remove(&job.id());
        Ok(())
    }

    ///
    /// Get the job with the passed id
    /// If the job doesn't exist then we return an error
    ///
    pub async fn get(&self, id: &Uuid) -> Result<Job, Error> {
        match self.jobs.get(id) {
            Some(j) => Ok(j.clone()),
            None => Err(Error::NotFound(format!("Job not found: {:?}", id))),
        }
    }
}

/// Errors

#[derive(Error, Debug)]
pub enum Error {
    #[error("{0}")]
    AnyError(#[from] AnyError),

    #[error("{0}")]
    NotFound(String),

    #[error("Unknown error")]
    Unknown,
}
