// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::board::{Board, Error as BoardError};
use crate::command::Command;
use crate::job::{Error as JobError, Job};
use anyhow::{Error as AnyError, Result};
use once_cell::sync::Lazy;
use paddington::command::Command as ControlCommand;
use paddington::message::Message;
use paddington::{async_message_handler, Error as PaddingtonError};
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

pub async fn process_control_message(command: ControlCommand) -> Result<(), Error> {
    match command {
        ControlCommand::Connected { agent } => {
            tracing::info!("Connected to agent: {}", agent);
        }
        ControlCommand::Disconnected { agent } => {
            tracing::info!("Disconnected from agent: {}", agent);
        }
        ControlCommand::Error { error } => {
            tracing::error!("Received error: {}", error);
        }
    }

    Ok(())
}

async_message_handler! {
    ///
    /// Message handler for the Bridge Agent.
    ///
    pub async fn process_message(message: Message) -> Result<(), paddington::Error> {
        tracing::info!("Received message: {:?}", message);

        match message.is_control() {
            true => Ok(process_control_message(message.into()).await?),
            false => {
                let peer: String = message.peer.clone();
                let command: Command = message.into();

                tracing::info!("Received command: {:?} from {}", command, peer);

                Ok(())
            }
        }
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
    Command::put(job.clone()).send_to("portal").await?;

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

    #[error("{0}")]
    InvalidControlMessage(String),

    #[error("Unknown error")]
    Unknown,
}

impl From<Error> for paddington::Error {
    fn from(error: Error) -> paddington::Error {
        paddington::Error::Any(error.into())
    }
}
