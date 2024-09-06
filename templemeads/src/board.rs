// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::command::Command;
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
    /// Register that the passed job has been received from our peer
    ///
    pub async fn received(&mut self, job: &Job) -> Result<(), Error> {
        self.jobs.insert(job.id(), job.clone());

        Ok(())
    }

    ///
    /// Add the passed job to our board and send it to our peer
    ///
    pub async fn add(&mut self, job: &Job) -> Result<(), Error> {
        self.jobs.insert(job.id(), job.clone());

        // send the job to our peer
        Command::put(job.clone()).send_to(&self.peer).await?;

        Ok(())
    }

    ///
    /// Update the job on our board - this will only update if
    /// the job is newer than the one we have
    ///
    pub async fn update(&mut self, job: &Job) -> Result<(), Error> {
        if let Some(j) = self.jobs.get_mut(&job.id()) {
            // only update if newer
            if job.version() > j.version() {
                *j = job.clone();
            }
        }

        Ok(())
    }
}

/// Errors

#[derive(Error, Debug)]
pub enum Error {
    #[error("{0}")]
    AnyError(#[from] AnyError),

    #[error("Unknown error")]
    Unknown,
}
