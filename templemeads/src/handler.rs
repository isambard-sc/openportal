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
use chrono::{DateTime, Utc};
use once_cell::sync::Lazy;
use paddington::async_message_handler;
use paddington::message::{Message, MessageType};
use std::boxed::Box;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
struct ServiceDetails {
    service: String,
    agent_type: AgentType,
    runner: AsyncRunnable,
    keepalives: Arc<Mutex<HashSet<String>>>,
}

impl Default for ServiceDetails {
    fn default() -> Self {
        ServiceDetails {
            service: String::new(),
            agent_type: agent::Type::Portal,
            runner: default_runner,
            keepalives: Arc::new(Mutex::new(HashSet::new())),
        }
    }
}

static SERVICE_DETAILS: Lazy<RwLock<ServiceDetails>> =
    Lazy::new(|| RwLock::new(ServiceDetails::default()));

///
/// Global cache of health responses from agents
/// Maps agent_name -> (HealthInfo, timestamp_received)
///
static HEALTH_CACHE: Lazy<RwLock<HashMap<String, (crate::command::HealthInfo, DateTime<Utc>)>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

///
/// Store a health response in the global cache
///
pub async fn cache_health_response(health: crate::command::HealthInfo) {
    let agent_name = health.agent.clone();
    let timestamp = Utc::now();

    let mut cache = HEALTH_CACHE.write().await;
    cache.insert(agent_name.clone(), (health, timestamp));

    tracing::debug!("Cached health response for agent: {}", agent_name);
}

///
/// Get all cached health responses
/// Returns a HashMap of agent_name -> (HealthInfo, timestamp)
///
pub async fn get_cached_health() -> HashMap<String, (crate::command::HealthInfo, DateTime<Utc>)> {
    HEALTH_CACHE.read().await.clone()
}

pub async fn set_my_service_details(
    service: &str,
    agent_type: &agent::Type,
    runner: Option<AsyncRunnable>,
) -> Result<()> {
    let engine = env!("CARGO_PKG_NAME");
    let version = env!("CARGO_PKG_VERSION");

    tracing::info!(
        "Agent layer: {} version {}",
        engine,
        version
    );

    agent::register_self(service, agent_type, engine, version).await;
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

            tracing::debug!("Update job: {} to {} from {}", job, recipient, peer,);

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
                    tracing::debug!("Updated job has arrived at its destination: {}", job);
                }
                Position::Error => {
                    tracing::error!("Job has got into an errored position: {}", job);
                }
            }
        }
        Command::Put { job } => {
            let peer = Peer::new(sender, zone);

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

            if job.is_duplicate() {
                tracing::info!("Job is a duplicate: {}", job);

                // the existing job is being processed. We now need to wait
                // for that to finish - when it does, our new job will
                // be updated with the result
                while !job.is_finished() {
                    job = job.wait().await?;

                    if !job.is_finished() {
                        tracing::warn!("Still waiting for duplicate job to finish: {}", job);
                        job.assert_is_not_expired()?;
                    }
                }
            } else {
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
                                tracing::warn!(
                                    "Not rerunning job that has already completed: {}",
                                    job
                                );
                            }
                            Status::Error => {
                                tracing::warn!(
                                    "Not rerunning job that has already errored: {}",
                                    job
                                );
                            }
                            _ => {
                                tracing::info!(
                                    "Execute {} : {}",
                                    job.destination(),
                                    job.instruction()
                                );

                                job = match runner(Envelope::new(recipient, sender, zone, &job))
                                    .await
                                {
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
                        tracing::error!(
                            "Recipient: {}, Sender: {}, Destination: {}",
                            recipient,
                            sender,
                            job.destination()
                        );
                        job = job.errored("Job has got into an errored position")?;
                    }
                    _ => {
                        tracing::warn!("Job {} is being put, but is not moving?", job);
                        job = job.errored("Job has got into an unknown position")?;
                    }
                }
            }

            tracing::debug!("Job has finished: {}", job);

            // now the job has finished, update the sender's board
            let peer = Peer::new(sender, zone);
            agent::wait_for(&peer, 30).await?;

            let _ = job.update(&peer).await?;
        }
        Command::Delete { job } => {
            let peer = Peer::new(sender, zone);

            tracing::warn!("Delete job: {} to {} from {}", job, recipient, peer,);

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
        Command::HealthCheck => {
            tracing::info!("Received health check request from {}", sender);

            // Collect health information
            let agent_name = agent::name().await;
            let agent_type = agent::my_agent_type().await;
            let start_time = agent::start_time().await;
            let engine = agent::engine().await;
            let version = agent::version().await;

            let mut health = crate::command::HealthInfo::new(
                &agent_name,
                agent_type,
                true, // connected (since we're responding)
                start_time,
                &engine,
                &version,
            );

            // Get aggregated job stats from all boards
            let (active, pending, running, completed, duplicates) =
                crate::state::aggregate_job_stats().await;

            health.active_jobs = active;
            health.pending_jobs = pending;
            health.running_jobs = running;
            health.completed_jobs = completed;
            health.duplicate_jobs = duplicates;

            tracing::info!("Health check: {}", health);

            // Send health response back to sender
            let response = Command::health_response(health);
            response.send_to(&Peer::new(sender, zone)).await?;
        }
        Command::HealthResponse { health } => {
            tracing::info!("Received health response: {}", health);
            // Cache the health response for later retrieval
            cache_health_response(health.clone()).await;
        }
        Command::Restart => {
            tracing::warn!("Received restart command from {}", sender);

            let agent_name = agent::name().await;
            let ack = Command::restart_ack(
                &agent_name,
                "Restart acknowledged - agent will terminate and rely on supervisor to restart",
            );

            // Send acknowledgment before terminating
            if let Err(e) = ack.send_to(&Peer::new(sender, zone)).await {
                tracing::error!("Failed to send restart acknowledgment: {}", e);
            }

            // Give time for the ack to be sent
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            // Exit the process - supervisor should restart it
            tracing::warn!("Terminating process for restart...");
            std::process::exit(0);
        }
        Command::RestartAck { agent, message } => {
            tracing::info!("Restart acknowledged by {}: {}", agent, message);
        }
        _ => {
            tracing::warn!("Command {} not recognised", command);
        }
    }

    Ok(())
}

async_message_handler! {
    ///
    /// Message handler for most templemeads agents
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

                // check that we are the only one sending keepalives to this peer
                let name = format!("{}@{}", sender, zone);
                tracing::debug!("Keepalive message from {}", name);

                match service_info.keepalives.lock() {
                    Ok(mut keepalives) => {
                        if keepalives.contains(&name) {
                            tracing::warn!("Duplicate keepalive message from {} in zone {} - skipping", sender, zone);
                            return Ok(());
                        }

                        keepalives.insert(name.clone());
                    }
                    Err(e) => {
                        tracing::warn!("Error locking keepalives: {}", e);
                        return Ok(());
                    }
                }

                // wait 23 seconds and send a keep alive message back
                tracing::debug!("Keepalive sleeping for 23 seconds from {}", name);
                tokio::time::sleep(tokio::time::Duration::from_secs(23)).await;
                tracing::debug!("Keepalive reawakened from {}", name);

                match service_info.keepalives.lock() {
                    Ok(mut keepalives) => {
                        keepalives.remove(&name);
                    }
                    Err(e) => {
                        tracing::error!("Error locking keepalives: {}", e);
                        return Ok(());
                    }
                }

                tracing::debug!("Sending keepalive message to {} again", name);
                match paddington::send(Message::keepalive(&sender, &zone)).await {
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!("Error sending keepalive message to {} in zone {}: {}. Disconnecting peer.", sender, zone, e);
                        paddington::disconnect(&sender, &zone).await?;
                    }
                }

                tracing::debug!("End of keepalive for {}", name);

                Ok(())
            }
            MessageType::Message => {
                let sender: String = message.sender().to_owned();
                let recipient: String = message.recipient().to_owned();
                let zone: String = message.zone().to_owned();
                let command: Command = message.into();

                if (recipient != service_info.service) {
                    // check to see if this is a virtual agent
                    if !agent::is_virtual(&Peer::new(&recipient, &zone)).await {
                        return Err(Error::Delivery(format!("Recipient {} does not match service {}", recipient, service_info.service)).into());
                    }
                }

                process_command(&recipient, &sender, &zone, &command, &service_info.runner).await?;

                Ok(())
            }
        }
    }
}
