// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent_core::Config;
use crate::board::{Board, Error as BoardError};
use crate::command::Command;
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
    Board(#[from] BoardError),

    #[error("{0}")]
    Paddington(#[from] PaddingtonError),

    #[error("{0}")]
    Serde(#[from] SerdeError),

    #[error("Unknown error")]
    Unknown,
}

impl From<Error> for paddington::Error {
    fn from(error: Error) -> paddington::Error {
        paddington::Error::Any(error.into())
    }
}
