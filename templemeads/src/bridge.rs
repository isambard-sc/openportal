// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent;
use crate::board::Error as BoardError;
use crate::command::Command;
use crate::job::{Error as JobError, Job};
use crate::state;
use anyhow::{Error as AnyError, Result};
use serde_json::Error as SerdeError;
use thiserror::Error;
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
                    return Err(Error::State(e));
                }
            };

            // get the board from the Arc<RwLock> board - this is the
            // blocking operation
            let board = board.read().await;

            // get the job from the board
            Ok(board.get(job).await?.clone())
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
    let job = Job::new(command);

    match agent::portal().await {
        Some(portal) => {
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
            {
                let mut board = board.write().await;

                // add the job to the board
                match board.add(&job).await {
                    Ok(_) => (),
                    Err(e) => {
                        tracing::error!("Error adding job to board: {:?}", e);
                        return Err(Error::Board(e));
                    }
                }
            }

            // now send it to the portal
            Command::put(&job).send_to(&portal).await?;

            Ok(job)
        }
        None => {
            tracing::error!("No portal agent found");
            Err(Error::NoPortal(
                "Cannot run the job because there is no portal agent".to_string(),
            ))
        }
    }
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
