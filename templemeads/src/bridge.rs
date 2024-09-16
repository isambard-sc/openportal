// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent;
use crate::error::Error;
use crate::job::Job;
use crate::state;

use anyhow::Result;
use uuid::Uuid;

pub async fn status(job: &Uuid) -> Result<Job, Error> {
    tracing::info!("Received status request for job: {}", job);

    match agent::portal().await {
        Some(portal) => {
            // get the (shared) board for the portal
            let board = match state::get(&portal).await {
                Ok(b) => b.board().await,
                Err(e) => {
                    tracing::error!("Error getting board for portal: {:?}", e);
                    return Err(Error::State(e.to_string()));
                }
            };

            // get the board from the Arc<RwLock> board - this is the
            // blocking operation
            let board = board.read().await;

            // get the job from the board
            Ok(board.get(job)?.clone())
        }
        None => {
            tracing::error!("No portal agent found");
            Err(Error::NoPortal(
                "Cannot get the job status because there is no portal agent".to_string(),
            ))
        }
    }
}

pub async fn run(command: &str) -> Result<Job, Error> {
    tracing::info!("Received command: {}", command);

    match agent::portal().await {
        Some(portal) => Ok(Job::parse(command)?.put(&portal).await?),
        None => {
            tracing::error!("No portal agent found");
            Err(Error::NoPortal(
                "Cannot run the job because there is no portal agent".to_string(),
            ))
        }
    }
}
