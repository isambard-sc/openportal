// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::board::{Board, Error as BoardError};
use crate::job::{Error as JobError, Job};
use anyhow::{Error as AnyError, Result};
use once_cell::sync::Lazy;
use paddington::{async_message_handler, send, Error as PaddingtonError, Message};
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

        Ok(())
    }
}

pub async fn run(command: &str) -> Result<Job, Error> {
    let job = Job::new(command.to_string());

    // get the Arc<RwLock> board from the STATE - without holding
    // the lock too long or blocking other threads
    let board = STATE.read().await.board.clone();

    // get the mutable board from the Arc<RwLock> board - this is the
    // blocking operation
    let mut board = board.write().await;
    board.add_job(job.clone()).await?;

    // release the lock
    drop(board);

    // send the job to the agent
    send("portal", &serde_json::to_string(&job)?).await?;

    Ok(job)
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
