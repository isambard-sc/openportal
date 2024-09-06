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

async fn add_job(agent: &str, job: &Job) -> Result<(), Error> {
    let board = state::get(agent).await?.board().await;
    let mut board = board.write().await;
    board.add(job).await?;

    Ok(())
}

async fn delete_job(agent: &str, job: &Job) -> Result<(), Error> {
    let board = state::get(agent).await?.board().await;
    let mut board = board.write().await;
    board.remove(job).await?;

    Ok(())
}

///
/// This is the main function that processes a command sent via the OpenPortal system
/// This will either route the command to the right place, or if the command has reached
/// its destination it will take action
///
async fn process_command(recipient: &str, sender: &str, command: &Command) -> Result<(), Error> {
    match command {
        Command::Register { agent } => {
            tracing::info!("Registering agent: {:?}", agent);
            agent::register(sender, agent).await;
        }
        Command::Update { job } => {
            tracing::info!("Update job: {:?} to {} from {}", job, recipient, sender);

            // update the sender's board with the updated job
            add_job(sender, job).await?;

            // now see if we need to send this to the next agent
            match job.destination().position(recipient, sender) {
                Position::Upstream => {
                    // if we are upstream, then the job is moving backwards so we need to
                    // send it to the previous agent
                    if let Some(agent) = job.destination().previous(recipient) {
                        add_job(&agent, job).await?;
                        Command::update(job).send_to(&agent).await?;
                    }
                }
                Position::Downstream => {
                    // if we are downstream, then we continue to let the job
                    // flow downstream
                    if let Some(agent) = job.destination().next(recipient) {
                        add_job(&agent, job).await?;
                        Command::update(job).send_to(&agent).await?;
                    }
                }
                _ => {
                    tracing::warn!("Job {:?} is being updated, but is not moving?", job);
                }
            }
        }
        Command::Put { job } => {
            tracing::info!("Put job: {:?} to {} from {}", job, recipient, sender);

            // update the sender's board with the updated job
            add_job(sender, job).await?;

            match job.destination().position(recipient, sender) {
                Position::Downstream => {
                    // if we are downstream, then we continue to let the job
                    // flow downstream
                    if let Some(agent) = job.destination().next(recipient) {
                        add_job(&agent, job).await?;
                        Command::put(job).send_to(&agent).await?;
                    }
                }
                Position::Destination => {
                    // we are the destination, so we need to take action
                    let job = job.execute().await?;

                    tracing::info!("Job has finished: {:?}", job);

                    // now the job has finished, update our board
                    add_job(sender, &job).await?;

                    // and now send this back to the sender
                    Command::update(&job).send_to(sender).await?;
                }
                _ => {
                    tracing::warn!("Job {:?} is being put, but is not moving?", job);
                }
            }
        }
        Command::Delete { job } => {
            tracing::info!("Delete job: {:?} to {} from {}", job, recipient, sender);

            // remove the job from the sender's board
            delete_job(sender, job).await?;

            match job.destination().position(recipient, sender) {
                Position::Upstream => {
                    // if we are upstream, then the job is moving backwards so we need to
                    // send it to the previous agent
                    if let Some(agent) = job.destination().previous(recipient) {
                        delete_job(&agent, job).await?;
                        Command::delete(job).send_to(&agent).await?;
                    }
                }
                Position::Downstream => {
                    // if we are downstream, then we continue to let the job
                    // flow downstream
                    if let Some(agent) = job.destination().next(recipient) {
                        delete_job(&agent, job).await?;
                        Command::delete(job).send_to(&agent).await?;
                    }
                }
                _ => {
                    tracing::warn!("Job {:?} is being deleted, but is not moving?", job);
                }
            }
        }
        _ => {
            tracing::warn!("Command {:?} not recognised", command);
        }
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
