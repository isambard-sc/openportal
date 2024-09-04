// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent_core::Config;
use crate::board::{Board, Error as BoardError};
use crate::job::{Error as JobError, Job};
use anyhow::{Error as AnyError, Result};
use once_cell::sync::Lazy;
use paddington::{async_message_handler, Error as PaddingtonError, Message};
use serde_json::Error as SerdeError;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;

///
/// Shared state for the Bridge Agent.
///
struct State {
    board: Arc<RwLock<Board>>,
}

impl State {
    fn new() -> Self {
        Self {
            board: Arc::new(RwLock::new(Board::default())),
        }
    }
}

static STATE: Lazy<RwLock<State>> = Lazy::new(|| RwLock::new(State::new()));

async_message_handler! {
    ///
    /// Message handler for the Bridge Agent.
    ///
    pub async fn process_message(message: Message) -> Result<(), paddington::Error> {
        tracing::info!("Received message: {:?}", message);

        // try to parse the message as a Job
        let job: Job = match serde_json::from_str(&message.message) {
            Ok(job) => job,
            Err(e) => {
                tracing::error!("Could not parse message as Job: {:?}", e);
                return Ok(()); // ignore the message
            }
        };

        tracing::info!("Received job: {:?}", job);

        Ok(())
    }
}

///
/// Run the agent service
///
pub async fn run(config: Config) -> Result<(), AnyError> {
    // run the bridge OpenPortal agent
    paddington::set_handler(process_message).await?;
    paddington::run(config.service).await?;

    Ok(())
}

/// Errors

#[derive(Error, Debug)]
pub enum Error {
    #[error("{0}")]
    AnyError(#[from] AnyError),

    #[error("{0}")]
    Job(#[from] JobError),

    #[error("{0}")]
    Board(#[from] BoardError),

    #[error("{0}")]
    Paddington(#[from] PaddingtonError),

    #[error("{0}")]
    Serde(#[from] SerdeError),

    #[error("Unknown error")]
    Unknown,
}
