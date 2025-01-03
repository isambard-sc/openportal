// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent;
use crate::agent::{Peer, Type as AgentType};
use crate::command::Command;
use crate::control_message::process_control_message;
use crate::destination::Position;
use crate::error::Error;
use crate::job::{sync_from_peer, Envelope, Status};
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

pub async fn set_my_service_details(
    service: &str,
    agent_type: &agent::Type,
    runner: Option<AsyncRunnable>,
) -> Result<()> {
    tracing::info!(
        "Agent layer: {} version {}",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION")
    );

    agent::register_self(service, agent_type).await;
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
    zone: &str,
    command: &Command,
    runner: &AsyncRunnable,
) -> Result<(), Error> {
    match command {
        Command::Register {
            agent,
            engine,
            version,
        } => {
            tracing::info!(
                "Registering agent: {}, engine={} version={}",
                agent,
                engine,
                version
            );
            agent::register_peer(&Peer::new(sender, zone), agent, engine, version).await;
        }
        Command::Update { job } => {
            let peer = Peer::new(sender, zone);

            tracing::info!("Update job: {} to {} from {}", job, recipient, peer,);

            // update the sender's board with the received job
            let job = job.received(&peer).await?;

            // now see if we need to send this to the next agent
            match job.destination().position(recipient, sender) {
                Position::Upstream => {
                    // if we are upstream, then the job is moving backwards so we need to
                    // send it to the previous agent
                    if let Some(agent) = job.destination().previous(recipient) {
                        let peer = Peer::new(&agent, zone);
                        agent::wait_for(&peer, 30).await?;
                        job.update(&peer).await?;
                    }
                }
                Position::Downstream => {
                    // if we are downstream, then we continue to let the job
                    // flow downstream
                    if let Some(agent) = job.destination().next(recipient) {
                        let peer = Peer::new(&agent, zone);
                        agent::wait_for(&peer, 30).await?;
                        job.update(&peer).await?;
                    }
                }
                Position::Destination => {
                    tracing::info!("Updated job has arrived at its destination: {}", job);
                }
                Position::Error => {
                    tracing::error!("Job has got into an errored position: {}", job);
                }
            }
        }
        Command::Put { job } => {
            let peer = Peer::new(sender, zone);

            tracing::info!("Put job: {} to {} from {}", job, recipient, peer,);

            // update the sender's board with the received job
            let mut job = match job.received(&peer).await {
                Ok(job) => job,
                Err(e) => {
                    tracing::error!("Error receiving job: {}", e);
                    job.errored(&e.to_string())?;
                    let _ = job.update(&Peer::new(sender, zone)).await?;
                    return Ok(());
                }
            };

            match job.destination().position(recipient, sender) {
                Position::Downstream => {
                    // if we are downstream, then we continue to let the job
                    // flow downstream
                    if let Some(agent) = job.destination().next(recipient) {
                        let peer = Peer::new(&agent, zone);
                        agent::wait_for(&peer, 30).await?;

                        job = match job.put(&peer).await {
                            Ok(job) => job,
                            Err(e) => {
                                tracing::error!("Error putting job: {}", e);
                                job.errored(&e.to_string())?
                            }
                        }
                    }
                }
                Position::Destination => {
                    // we are the destination, so we need to take action
                    match job.state() {
                        Status::Complete => {
                            tracing::info!("Not rerunning job that has already completed: {}", job);
                        }
                        Status::Error => {
                            tracing::error!("Not rerunning job that has already errored: {}", job);
                        }
                        _ => {
                            job = match runner(Envelope::new(recipient, sender, zone, &job)).await {
                                Ok(job) => job,
                                Err(e) => {
                                    tracing::error!("Error running job: {}", e);
                                    job.errored(&e.to_string())?
                                }
                            };
                        }
                    }
                }
                Position::Error => {
                    tracing::error!("Job has got into an errored position: {}", job);
                    job = job.errored("Job has got into an errored position")?;
                }
                _ => {
                    tracing::warn!("Job {} is being put, but is not moving?", job);
                    job = job.errored("Job has got into an unknown position")?;
                }
            }

            tracing::info!("Job has finished: {}", job);

            // now the job has finished, update the sender's board
            let peer = Peer::new(sender, zone);
            agent::wait_for(&peer, 30).await?;

            let _ = job.update(&peer).await?;
        }
        Command::Delete { job } => {
            let peer = Peer::new(sender, zone);

            tracing::info!("Delete job: {} to {} from {}", job, recipient, peer,);

            // record that the sender has deleted the job
            let job = job.deleted(&peer).await?;

            match job.destination().position(recipient, sender) {
                Position::Upstream => {
                    // if we are upstream, then the job is moving backwards so we need to
                    // send it to the previous agent
                    if let Some(agent) = job.destination().previous(recipient) {
                        let peer = Peer::new(&agent, zone);
                        agent::wait_for(&peer, 30).await?;
                        job.delete(&peer).await?;
                    }
                }
                Position::Downstream => {
                    // if we are downstream, then we continue to let the job
                    // flow downstream
                    if let Some(agent) = job.destination().next(recipient) {
                        let peer = Peer::new(&agent, zone);
                        agent::wait_for(&peer, 30).await?;
                        job.delete(&peer).await?;
                    }
                }
                Position::Error => {
                    tracing::error!("Job has got into an errored position: {}", job);
                }
                _ => {
                    tracing::warn!("Job {} is being deleted, but is not moving?", job);
                }
            }
        }
        Command::Sync { state } => {
            let peer = Peer::new(sender, zone);
            sync_from_peer(recipient, &peer, state).await?;
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
                let zone: String = message.zone().to_owned();

                if (recipient != service_info.service) {
                    return Err(Error::Delivery(format!("Recipient {} does not match service {}", recipient, service_info.service)).into());
                }

                // wait 20 seconds and send a keep alive message back
                tokio::time::sleep(tokio::time::Duration::from_secs(20)).await;

                match paddington::send(Message::keepalive(&sender, &zone)).await {
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!("Error sending keepalive message to {} in zone {}: {}", sender, zone, e);
                    }
                }

                Ok(())
            }
            MessageType::Message => {
                let sender: String = message.sender().to_owned();
                let recipient: String = message.recipient().to_owned();
                let zone: String = message.zone().to_owned();
                let command: Command = message.into();

                if (recipient != service_info.service) {
                    return Err(Error::Delivery(format!("Recipient {} does not match service {}", recipient, service_info.service)).into());
                }

                process_command(&recipient, &sender, &zone, &command, &service_info.runner).await?;

                Ok(())
            }
        }
    }
}
