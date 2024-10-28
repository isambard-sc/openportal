// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent;
use crate::agent::Type as AgentType;
use crate::command::Command;
use crate::control_message::process_control_message;
use crate::destination::Position;
use crate::error::Error;
use crate::job::Envelope;
use crate::runnable::{default_runner, AsyncRunnable};

use anyhow::Result;
use once_cell::sync::Lazy;
use paddington::async_message_handler;
use paddington::message::{Message, MessageType};
use std::boxed::Box;
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
struct ServiceDetails {
    service: String,
    agent_type: AgentType,
    runner: AsyncRunnable,
}

impl Default for ServiceDetails {
    fn default() -> Self {
        ServiceDetails {
            service: String::new(),
            agent_type: agent::Type::Portal,
            runner: default_runner,
        }
    }
}

static SERVICE_DETAILS: Lazy<RwLock<ServiceDetails>> =
    Lazy::new(|| RwLock::new(ServiceDetails::default()));

pub async fn set_service_details(
    service: &str,
    agent_type: &agent::Type,
    runner: Option<AsyncRunnable>,
) -> Result<()> {
    agent::register(service, agent_type).await;
    let mut service_details = SERVICE_DETAILS.write().await;
    service_details.service = service.to_string();
    service_details.agent_type = agent_type.clone();

    if let Some(runner) = runner {
        // only change this if a runner has been passed
        service_details.runner = runner;
    }

    Ok(())
}

///
/// This is the main function that processes a command sent via the OpenPortal system
/// This will either route the command to the right place, or if the command has reached
/// its destination it will take action
///
async fn process_command(
    recipient: &str,
    sender: &str,
    command: &Command,
    runner: &AsyncRunnable,
) -> Result<(), Error> {
    match command {
        Command::Register { agent } => {
            tracing::info!("Registering agent: {}", agent);
            agent::register(sender, agent).await;
        }
        Command::Update { job } => {
            tracing::info!("Update job: {} to {} from {}", job, recipient, sender);

            // update the sender's board with the received job
            let job = job.received(sender).await?;

            // now see if we need to send this to the next agent
            match job.destination().position(recipient, sender) {
                Position::Upstream => {
                    // if we are upstream, then the job is moving backwards so we need to
                    // send it to the previous agent
                    if let Some(agent) = job.destination().previous(recipient) {
                        let _ = job.update(&agent).await?;
                    }
                }
                Position::Downstream => {
                    // if we are downstream, then we continue to let the job
                    // flow downstream
                    if let Some(agent) = job.destination().next(recipient) {
                        let _ = job.update(&agent).await?;
                    }
                }
                Position::Destination => {
                    tracing::info!("Job has arrived at its destination: {}", job);
                }
                _ => {
                    tracing::warn!("Job {} is being updated, but is not moving?", job);
                }
            }
        }
        Command::Put { job } => {
            tracing::info!("Put job: {} to {} from {}", job, recipient, sender);

            // update the sender's board with the received job
            let job = job.received(sender).await?;

            match job.destination().position(recipient, sender) {
                Position::Downstream => {
                    // if we are downstream, then we continue to let the job
                    // flow downstream
                    if let Some(agent) = job.destination().next(recipient) {
                        let _ = job.put(&agent).await?;
                    }
                }
                Position::Destination => {
                    // we are the destination, so we need to take action
                    let job = match runner(Envelope::new(recipient, sender, &job)).await {
                        Ok(job) => job,
                        Err(e) => {
                            tracing::error!("Error running job: {}", e);
                            job.errored(&e.to_string())?
                        }
                    };

                    tracing::info!("Job has finished: {}", job);

                    // now the job has finished, update the sender's board
                    let _ = job.update(sender).await?;
                }
                _ => {
                    tracing::warn!("Job {} is being put, but is not moving?", job);
                }
            }
        }
        Command::Delete { job } => {
            tracing::info!("Delete job: {} to {} from {}", job, recipient, sender);

            // record that the sender has deleted the job
            let job = job.deleted(sender).await?;

            match job.destination().position(recipient, sender) {
                Position::Upstream => {
                    // if we are upstream, then the job is moving backwards so we need to
                    // send it to the previous agent
                    if let Some(agent) = job.destination().previous(recipient) {
                        let _ = job.delete(&agent).await?;
                    }
                }
                Position::Downstream => {
                    // if we are downstream, then we continue to let the job
                    // flow downstream
                    if let Some(agent) = job.destination().next(recipient) {
                        let _ = job.delete(&agent).await?;
                    }
                }
                _ => {
                    tracing::warn!("Job {} is being deleted, but is not moving?", job);
                }
            }
        }
        _ => {
            tracing::warn!("Command {} not recognised", command);
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

        match message.typ() {
            MessageType::Control => {
                process_control_message(&service_info.agent_type, message.into()).await?;

                Ok(())
            }
            MessageType::KeepAlive => {
                let sender: String = message.sender().to_owned();
                let recipient: String = message.recipient().to_owned();

                if (recipient != service_info.service) {
                    return Err(Error::Delivery(format!("Recipient {} does not match service {}", recipient, service_info.service)).into());
                }

                // wait 20 seconds and send a keep alive message back
                tokio::time::sleep(tokio::time::Duration::from_secs(20)).await;

                match paddington::send(Message::keepalive(&sender)).await {
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!("Error sending keepalive message to {} : {}", sender, e);
                    }
                }

                Ok(())
            }
            MessageType::Message => {
                let sender: String = message.sender().to_owned();
                let recipient: String = message.recipient().to_owned();
                let command: Command = message.into();

                if (recipient != service_info.service) {
                    return Err(Error::Delivery(format!("Recipient {} does not match service {}", recipient, service_info.service)).into());
                }

                process_command(&recipient, &sender, &command, &service_info.runner).await?;

                Ok(())
            }
        }
    }
}
