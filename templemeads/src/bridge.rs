// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent;
use crate::board::Error as BoardError;
use crate::job::{Error as JobError, Job};
use crate::state;
use anyhow::{Error as AnyError, Result};
use serde_json::Error as SerdeError;
use thiserror::Error;

pub async fn run(command: &str) -> Result<Job, Error> {
    tracing::info!("Received command: {}", command);
    let job = Job::new(command);

    // get the name of the portal agent
    if let Some(portal) = agent::portal().await {
        // get the (shared) board for the portal
        let board = match state::get(&portal).await {
            Ok(b) => b.board().await,
            Err(e) => {
                tracing::error!("Error getting board for portal: {:?}", e);
                return Err(Error::State(e));
            }
        };

        // get the mutable board from the Arc<RwLock> board - this is the
        // blocking operation
        let mut board = board.write().await;

        // add the job to the board - this will send it to the portal
        match board.add(&job).await {
            Ok(_) => (),
            Err(e) => {
                tracing::error!("Error adding job to board: {:?}", e);
                return Err(Error::Board(e));
            }
        }
    } else {
        tracing::error!("No portal agent found");
        return Err(Error::NoPortal(
            "Cannot submit the job because there is no portal agent".to_string(),
        ));
    }

    Ok(job)
}

/// Errors

#[derive(Error, Debug)]
pub enum Error {
    #[error("{0}")]
    Any(#[from] AnyError),

    #[error("{0}")]
    Job(#[from] JobError),

    #[error("{0}")]
    Serde(#[from] SerdeError),

    #[error("{0}")]
    State(#[from] state::Error),

    #[error("{0}")]
    Board(#[from] BoardError),

    #[error("{0}")]
    NoPortal(String),
}
