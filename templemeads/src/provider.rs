// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent;
use crate::agent_core::Config;
use crate::board::Error as BoardError;
use crate::command::Command;
use crate::control_message::process_control_message;
use crate::state;
use anyhow::{Error as AnyError, Result};
use paddington::message::Message;
use paddington::{async_message_handler, Error as PaddingtonError};
use serde_json::Error as SerdeError;
use thiserror::Error;

async fn process_command(peer: &str, command: &Command) -> Result<(), Error> {
    match command {
        Command::Register { agent } => {
            tracing::info!("Registering agent: {:?}", agent);
            agent::register(peer, agent).await;
        }
        Command::Update { job } => {
            // update the board with the updated job
            tracing::info!("Update job: {:?}", job);

            let board = state::get(peer).await?.board().await;
            let mut board = board.write().await;
            board.update(job).await?;
        }
        Command::Put { job } => {
            // save the job in our board for the caller
            tracing::info!("Received job: {:?}", job);

            // find the platform for the job
        }
        _ => {}
    }

    Ok(())
}

async_message_handler! {
    ///
    /// Message handler for the Provider Agent.
    ///
    pub async fn process_message(message: Message) -> Result<(), paddington::Error> {
        match message.is_control() {
            true => Ok(process_control_message(&agent::Type::Provider, message.into()).await?),
            false => {
                let peer: String = message.peer.clone();
                let command: Command = message.into();

                tracing::info!("Received command: {:?} from {}", command, peer);

                Ok(process_command(&peer, &command).await?)
            }
        }
    }
}

///
/// Run the agent service
///
pub async fn run(config: Config) -> Result<(), AnyError> {
    // run the Provider OpenPortal agent
    paddington::set_handler(process_message).await?;
    paddington::run(config.service).await?;

    Ok(())
}

/// Errors

#[derive(Error, Debug)]
enum Error {
    #[error("{0}")]
    Any(#[from] AnyError),

    #[error("{0}")]
    Board(#[from] BoardError),

    #[error("{0}")]
    Paddington(#[from] PaddingtonError),

    #[error("{0}")]
    Serde(#[from] SerdeError),

    #[error("{0}")]
    State(#[from] state::Error),
}

impl From<Error> for paddington::Error {
    fn from(error: Error) -> paddington::Error {
        paddington::Error::Any(error.into())
    }
}
