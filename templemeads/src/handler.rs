// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent;
use crate::agent::Type as AgentType;
use crate::board::Error as BoardError;
use crate::command::Command;
use crate::control_message::process_control_message;
use crate::destination::Position;
use crate::job::{Error as JobError, Job};
use crate::state;
use anyhow::{Error as AnyError, Result};
use once_cell::sync::Lazy;
use paddington::message::Message;
use paddington::{async_message_handler, Error as PaddingtonError};
use serde_json::Error as SerdeError;
use thiserror::Error;
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
struct ServiceDetails {
    service: String,
    agent_type: AgentType,
}

impl Default for ServiceDetails {
    fn default() -> Self {
        ServiceDetails {
            service: String::new(),
            agent_type: agent::Type::Portal,
        }
    }
}

static SERVICE_DETAILS: Lazy<RwLock<ServiceDetails>> =
    Lazy::new(|| RwLock::new(ServiceDetails::default()));

pub async fn set_service_details(service: &str, agent_type: &agent::Type) -> Result<()> {
    agent::register(service, agent_type).await;
    let mut service_details = SERVICE_DETAILS.write().await;
    service_details.service = service.to_string();
    service_details.agent_type = agent_type.clone();

    Ok(())
}

async fn board_update(sender: &str, job: &Job) -> Result<(), Error> {
    let board = state::get(sender).await?.board().await;
    let mut board = board.write().await;
    board.update(job).await?;

    Ok(())
}

async fn board_received(sender: &str, job: &Job) -> Result<(), Error> {
    let board = state::get(sender).await?.board().await;
    let mut board = board.write().await;
    board.received(job).await?;

    Ok(())
}

///
/// This is the main function that processes a command sent via the OpenPortal system
/// This will either route the command to the right place, or if the command has reached
/// its destination it will take action
///
async fn process_command(recipient: &str, sender: &str, command: &Command) -> Result<(), Error> {
    tracing::info!(
        "Processing command: {:?} to {} from {}",
        command,
        recipient,
        sender
    );

    match command {
        Command::Register { agent } => {
            tracing::info!("Registering agent: {:?}", agent);
            agent::register(sender, agent).await;
        }
        Command::Update { job } => {
            // update the board with the updated job
            tracing::info!("Update job: {:?} to {} from {}", job, recipient, sender);

            match job.destination().position(recipient, sender) {
                Position::Upstream => {
                    board_update(sender, job).await?;

                    // if we are upstream, then we need to send the job to the next agent
                    if let Some(next_agent) = job.destination().next(recipient) {
                        Command::update(job.clone()).send_to(&next_agent).await?;
                    }
                }
                Position::Downstream => {
                    board_update(sender, job).await?;

                    // if we are downstream, then we need to send the job to the previous agent
                    if let Some(previous_agent) = job.destination().previous(recipient) {
                        Command::update(job.clone())
                            .send_to(&previous_agent)
                            .await?;
                    }
                }
                Position::Endpoint => {
                    // if we are at the endpoint, then there is nothing for us to do (we should have been the updated?)
                    tracing::warn!(
                        "Received update for endpoint job: {:?}. Ignoring the UPDATE",
                        job
                    );
                }
                Position::Error => {
                    tracing::error!(
                        "Error finding position for job {:?} when sent from {} to {}",
                        job,
                        sender,
                        recipient
                    );
                    return Err(Error::Delivery(format!(
                        "Error finding position for job {:?} when sent from {} to {}",
                        job, sender, recipient
                    )));
                }
            }
        }
        Command::Put { job } => {
            // update the board with the updated job
            tracing::info!("Put job: {:?} to {} from {}", job, recipient, sender);

            match job.destination().position(recipient, sender) {
                Position::Upstream => {
                    board_received(sender, job).await?;

                    // if we are upstream, then we need to send the job to the next agent
                    if let Some(next_agent) = job.destination().next(recipient) {
                        Command::put(job.clone()).send_to(&next_agent).await?;
                    }
                }
                Position::Downstream => {
                    board_received(sender, job).await?;

                    // if we are downstream, then we need to send the job to the previous agent
                    if let Some(previous_agent) = job.destination().previous(recipient) {
                        Command::put(job.clone()).send_to(&previous_agent).await?;
                    }
                }
                Position::Endpoint => {
                    // we are the endpoint, so we only need to receive the job into our board
                    board_received(sender, job).await?;
                }
                Position::Error => {
                    tracing::error!(
                        "Error finding position for job {:?} when sent from {} to {}",
                        job,
                        sender,
                        recipient
                    );
                    return Err(Error::Delivery(format!(
                        "Error finding position for job {:?} when sent from {} to {}",
                        job, sender, recipient
                    )));
                }
            }
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
        let service_info: ServiceDetails = SERVICE_DETAILS.read().await.to_owned();

        match message.is_control() {
            true => Ok(process_control_message(&service_info.agent_type, message.into()).await?),
            false => {
                let sender: String = message.sender().to_owned();
                let recipient: String = message.recipient().to_owned();
                let command: Command = message.into();

                if (recipient != service_info.service) {
                    return Err(Error::Delivery(format!("Recipient {} does not match service {}", recipient, service_info.service)).into());
                }

                Ok(process_command(&recipient, &sender, &command).await?)
            }
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
    Paddington(#[from] PaddingtonError),

    #[error("{0}")]
    Serde(#[from] SerdeError),

    #[error("{0}")]
    State(#[from] state::Error),

    #[error("{0}")]
    Board(#[from] BoardError),

    #[error("{0}")]
    Delivery(String),
}

impl From<Error> for paddington::Error {
    fn from(error: Error) -> paddington::Error {
        paddington::Error::Any(error.into())
    }
}
