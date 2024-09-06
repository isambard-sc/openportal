// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent;
use crate::board::Error as BoardError;
use crate::command::Command;
use crate::control_message::process_control_message;
use crate::job::{Error as JobError, Job};
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
        _ => {}
    }

    Ok(())
}

async_message_handler! {
    ///
    /// Message handler for the Bridge Agent.
    ///
    pub async fn process_message(message: Message) -> Result<(), paddington::Error> {
        match message.is_control() {
            true => Ok(process_control_message(&agent::Type::Bridge, message.into()).await?),
            false => {
                let peer: String = message.peer.clone();
                let command: Command = message.into();

                tracing::info!("Received command: {:?} from {}", command, peer);

                Ok(process_command(&peer, &command).await?)
            }
        }
    }
}

pub async fn run(command: &str) -> Result<Job, Error> {
    let job = Job::new(command);

    // get the name of the portal agent
    if let Some(portal) = agent::portal().await {
        // get the (shared) board for the portal
        let board = state::get(&portal).await?.board().await;

        // get the mutable board from the Arc<RwLock> board - this is the
        // blocking operation
        let mut board = board.write().await;

        // add the job to the board - this will send it to the portal
        board.add(&job).await?;
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
    Paddington(#[from] PaddingtonError),

    #[error("{0}")]
    Serde(#[from] SerdeError),

    #[error("{0}")]
    State(#[from] state::Error),

    #[error("{0}")]
    Board(#[from] BoardError),
}

impl From<Error> for paddington::Error {
    fn from(error: Error) -> paddington::Error {
        paddington::Error::Any(error.into())
    }
}
