// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::{Context, Result};
use reqwest::Client;
use std::time::Duration;
use url::Url;

use templemeads::agent;
use templemeads::agent::bridge::{process_args, run, Defaults};
use templemeads::async_runnable;
use templemeads::destination::{Destination, Destinations};
use templemeads::grammar::Instruction::{
    CreateProject, GetProject, GetProjectMapping, GetProjects, GetUsageReport, GetUsageReports,
    RemoveProject, SyncOfferings, UpdateProject,
};
use templemeads::job::{Envelope, Job};
use templemeads::server;
use templemeads::Error;

///
/// Main function for the bridge application
///
/// The purpose of this application is to bridge between the user portal
/// (e.g. Waldur) and OpenPortal.
///
/// It does this by providing a "Client" agent in OpenPortal that can be
/// used to make requests over the OpenPortal protocol.
///
/// It also provides a web API that can be called by the user portal to
/// submit and get information about those requests. This API is designed
/// to be called via, e.g. the openportal Python client.
///
#[tokio::main]
async fn main() -> Result<()> {
    // start tracing
    templemeads::config::initialise_tracing();

    // start system monitoring
    templemeads::spawn_system_monitor();

    // create the OpenPortal paddington defaults
    let defaults = Defaults::parse(
        Some("bridge".to_owned()),
        Some(
            dirs::config_local_dir()
                .unwrap_or(
                    ".".parse()
                        .expect("Could not parse fallback config directory."),
                )
                .join("openportal")
                .join("bridge-config.toml"),
        ),
        Some("ws://localhost:8044".to_owned()),
        Some("127.0.0.1".to_owned()),
        Some(8044),
        None,
        None,
        Some("http://localhost:3000".to_owned()),
        Some("127.0.0.1".to_owned()),
        Some(3000),
        Some("http://localhost/signal".to_owned()),
    );

    // now parse the command line arguments to get the service configuration
    let config = match process_args(&defaults).await? {
        Some(config) => config,
        None => {
            // Not running the service, so can safely exit
            return Ok(());
        }
    };

    let board = server::get_board().await?;

    if let Some(signal_url) = &config.bridge.signal_url {
        board.write().await.set_signal_url(signal_url.clone());
    }

    async_runnable! {
        ///
        /// Runnable function that will be called when a job is received
        /// by the agent
        ///
        pub async fn bridge_runner(envelope: Envelope) -> Result<Job, Error>
        {
            let job = envelope.job();

            // look for commands that come direct from the portal agent
            match agent::agent_type(&envelope.sender()).await {
                Some(agent::Type::Portal) => {
                    match job.instruction() {
                        SyncOfferings(offerings) => {
                            tracing::info!("Syncing offerings to: {:?}", offerings);
                            return job.completed(sync_offerings(&offerings).await?);
                        }
                        _ => {
                            return Err(Error::InvalidInstruction(
                                format!("Invalid instruction: {}. Only sync_offerings instructions can be sent from a portal agent to a bridge", job.instruction()),
                            ));
                        }
                    }
                }
                Some(_) => {}
                None => {}
            }

            // only virtual agents (either ourselves or other virtuals)
            // can submit instructions to the bridge. These virtual agents
            // are dynamically configured based on requests from the
            // portal agent
            if !agent::is_virtual(&envelope.sender()).await {
                return Err(Error::InvalidInstruction(
                    format!("Invalid instruction: {}. Only virtual agents can submit instructions to a bridge", job.instruction()),
                ));
            }

            match job.instruction() {
                CreateProject(project, details) => {
                    // create a new project in the cluster
                    tracing::debug!("Creating project {} with details {:?}", project, details);

                    let board = server::get_board().await?;

                    let waiter = board.write().await.add(&job)?;

                    // now signal the web-portal connected to the bridge
                    // that this job is ready to be processed
                    let signal_url = board.read().await.signal_url();

                    match signal_web_portal(&signal_url, &job).await {
                        Ok(_) => {},
                        Err(e) => {
                            // remove the job from the board as it will not be processed
                            board.write().await.remove(&job)?;
                            return job.errored(
                                &format!("Failed to signal web portal: {}", e),
                            );
                        }
                    }

                    let mut result = waiter.result().await?;

                    while !result.is_finished() {
                        // get a new waiter to wait for the job to finish
                        let waiter = board.write().await.get_waiter(&result)?;
                        result = waiter.result().await?;
                    }

                    job.copy_result_from(&result)
                }
                RemoveProject(project) => {
                    // remove the project from the cluster
                    tracing::debug!("Removing project {}", project);

                    let board = server::get_board().await?;

                    let waiter = board.write().await.add(&job)?;

                    // now signal the web-portal connected to the bridge
                    // that this job is ready to be processed
                    let signal_url = board.read().await.signal_url();

                    match signal_web_portal(&signal_url, &job).await {
                        Ok(_) => {},
                        Err(e) => {
                            // remove the job from the board as it will not be processed
                            board.write().await.remove(&job)?;
                            return job.errored(
                                &format!("Failed to signal web portal: {}", e),
                            );
                        }
                    }

                    let mut result = waiter.result().await?;

                    while !result.is_finished() {
                        // get a new waiter to wait for the job to finish
                        let waiter = board.write().await.get_waiter(&result)?;
                        result = waiter.result().await?;
                    }

                    job.copy_result_from(&result)
                }
                UpdateProject(project, details) => {
                    // update the project in the cluster
                    tracing::debug!("Updating project {} with details {:?}", project, details);

                    let board = server::get_board().await?;

                    let waiter = board.write().await.add(&job)?;

                    // now signal the web-portal connected to the bridge
                    // that this job is ready to be processed
                    let signal_url = board.read().await.signal_url();

                    match signal_web_portal(&signal_url, &job).await {
                        Ok(_) => {},
                        Err(e) => {
                            // remove the job from the board as it will not be processed
                            board.write().await.remove(&job)?;
                            return job.errored(
                                &format!("Failed to signal web portal: {}", e),
                            );
                        }
                    }

                    let mut result = waiter.result().await?;

                    while !result.is_finished() {
                        // get a new waiter to wait for the job to finish
                        let waiter = board.write().await.get_waiter(&result)?;
                        result = waiter.result().await?;
                    }

                    job.copy_result_from(&result)
                }
                GetProject(project) => {
                    // get the project from the cluster
                    tracing::debug!("Getting project {}", project);

                    let board = server::get_board().await?;

                    let waiter = board.write().await.add(&job)?;

                    // now signal the web-portal connected to the bridge
                    // that this job is ready to be processed
                    let signal_url = board.read().await.signal_url();

                    match signal_web_portal(&signal_url, &job).await {
                        Ok(_) => {},
                        Err(e) => {
                            // remove the job from the board as it will not be processed
                            board.write().await.remove(&job)?;
                            return job.errored(
                                &format!("Failed to signal web portal: {}", e),
                            );
                        }
                    }

                    let mut result = waiter.result().await?;

                    while !result.is_finished() {
                        // get a new waiter to wait for the job to finish
                        let waiter = board.write().await.get_waiter(&result)?;
                        result = waiter.result().await?;
                    }

                    job.copy_result_from(&result)
                }
                GetProjects(portal) => {
                    tracing::debug!("Getting projects for portal {}", portal);

                    let board = server::get_board().await?;

                    let waiter = board.write().await.add(&job)?;

                    // now signal the web-portal connected to the bridge
                    // that this job is ready to be processed
                    let signal_url = board.read().await.signal_url();

                    match signal_web_portal(&signal_url, &job).await {
                        Ok(_) => {},
                        Err(e) => {
                            // remove the job from the board as it will not be processed
                            board.write().await.remove(&job)?;
                            return job.errored(
                                &format!("Failed to signal web portal: {}", e),
                            );
                        }
                    }

                    let mut result = waiter.result().await?;

                    while !result.is_finished() {
                        // get a new waiter to wait for the job to finish
                        let waiter = board.write().await.get_waiter(&result)?;
                        result = waiter.result().await?;
                    }

                    job.copy_result_from(&result)
                }
                GetProjectMapping(project) => {
                    // get the project mapping from the cluster
                    tracing::debug!("Getting project mapping for {}", project);

                    let board = server::get_board().await?;

                    let waiter = board.write().await.add(&job)?;

                    // now signal the web-portal connected to the bridge
                    // that this job is ready to be processed
                    let signal_url = board.read().await.signal_url();

                    match signal_web_portal(&signal_url, &job).await {
                        Ok(_) => {},
                        Err(e) => {
                            // remove the job from the board as it will not be processed
                            board.write().await.remove(&job)?;
                            return job.errored(
                                &format!("Failed to signal web portal: {}", e),
                            );
                        }
                    }

                    let mut result = waiter.result().await?;

                    while !result.is_finished() {
                        // get a new waiter to wait for the job to finish
                        let waiter = board.write().await.get_waiter(&result)?;
                        result = waiter.result().await?;
                    }

                    job.copy_result_from(&result)
                }
                GetUsageReport(project, dates) => {
                    // get the usage report for the project from the cluster
                    tracing::debug!("Getting usage report for {} for dates {}", project, dates);

                    let board = server::get_board().await?;

                    let waiter = board.write().await.add(&job)?;

                    // now signal the web-portal connected to the bridge
                    // that this job is ready to be processed
                    let signal_url = board.read().await.signal_url();

                    match signal_web_portal(&signal_url, &job).await {
                        Ok(_) => {},
                        Err(e) => {
                            // remove the job from the board as it will not be processed
                            board.write().await.remove(&job)?;
                            return job.errored(
                                &format!("Failed to signal web portal: {}", e),
                            );
                        }
                    }

                    let mut result = waiter.result().await?;

                    while !result.is_finished() {
                        // get a new waiter to wait for the job to finish
                        let waiter = board.write().await.get_waiter(&result)?;
                        result = waiter.result().await?;
                    }

                    job.copy_result_from(&result)
                }
                GetUsageReports(portal, dates) => {
                    // get the usage reports for the portal from the cluster
                    tracing::debug!("Getting usage reports for {} for dates {}", portal, dates);

                    let board = server::get_board().await?;

                    let waiter = board.write().await.add(&job)?;

                    // now signal the web-portal connected to the bridge
                    // that this job is ready to be processed
                    let signal_url = board.read().await.signal_url();

                    match signal_web_portal(&signal_url, &job).await {
                        Ok(_) => {},
                        Err(e) => {
                            // remove the job from the board as it will not be processed
                            board.write().await.remove(&job)?;
                            return job.errored(
                                &format!("Failed to signal web portal: {}", e),
                            );
                        }
                    }

                    let mut result = waiter.result().await?;

                    while !result.is_finished() {
                        // get a new waiter to wait for the job to finish
                        let waiter = board.write().await.get_waiter(&result)?;
                        result = waiter.result().await?;
                    }

                    job.copy_result_from(&result)
                }
                _ => {
                    tracing::error!("Unknown instruction: {:?}", job.instruction());
                    Err(Error::UnknownInstruction(
                        format!("Unknown instruction: {:?}", job.instruction()).to_string(),
                    ))
                }
            }
        }
    }

    // run the Bridge agent
    run(config, bridge_runner).await?;

    Ok(())
}

fn should_allow_invalid_certs() -> bool {
    match std::env::var("OPENPORTAL_ALLOW_INVALID_SSL_CERTS") {
        Ok(value) => value.to_lowercase() == "true",
        Err(_) => false,
    }
}

const PORTAL_WAIT_TIME: u64 = 5;

///
/// Synchronise the offerings to the passed set
///
pub async fn sync_offerings(offerings: &Destinations) -> Result<Destinations, Error> {
    // get our portal agent so that we can find out the zone name for this connection
    let portal = match agent::portal(PORTAL_WAIT_TIME).await {
        Some(portal) => portal,
        None => {
            return Err(Error::Unknown(
                format!(
                    "Could not find portal agent after waiting {} seconds",
                    PORTAL_WAIT_TIME
                )
                .to_string(),
            ));
        }
    };

    let mut synched_offerings = Vec::<Destination>::new();

    // first, make sure that we have virtual agents for each
    // of the offerings
    for offering in offerings.iter() {
        if offering.agents().len() != 3 {
            tracing::error!(
                "Invalid offering: {}. Offerings must have exactly three agents",
                offering
            );
            continue;
        }

        // The offering's destination should be of the form
        // offering-name.local-portal.remote-portal
        if offering.agents()[1] != portal.name() {
            tracing::error!(
                "Invalid offering: {}. The second agent in the offering must be the portal ({})",
                offering,
                portal.name()
            );
            continue;
        }

        // the offering-name should be the name of the virtual resource,
        // while the zone should the same as the portal's zone.
        let peer = agent::Peer::new(offering.agents()[0].as_str(), portal.zone());

        if !agent::has_virtual(&peer).await {
            tracing::info!(
                "Registering virtual agent {} for offering {}",
                peer,
                offering.first()
            );

            agent::register_peer(&peer, &agent::Type::Virtual, "virtual", "virtual").await;
        }

        synched_offerings.push(offering.clone());
    }

    // now go through the virtual agents and remove any that
    // are not in the synched offerings
    let virtual_agents = agent::get_all(&agent::Type::Virtual).await;

    for virtual_agent in virtual_agents {
        // the agent should have teh same zone as the portal, and have
        // a name that matches the first offering name
        if !(virtual_agent.zone() == portal.zone()
            && synched_offerings
                .iter()
                .any(|offering| offering.first() == virtual_agent.name()))
        {
            tracing::info!(
                "Removing virtual agent {} in zone {}",
                virtual_agent.name(),
                virtual_agent.zone()
            );
            agent::remove(&virtual_agent).await;
        }
    }

    Ok(Destinations::new(&synched_offerings))
}

///
/// Call 'get' on the passed signal URL, passing in the job ID
/// as the 'job_id' query parameter. Do nothing if the signal URL
/// is not set. Attempt to call this 5 times, then give up
///
pub async fn signal_web_portal(signal_url: &Option<Url>, job: &Job) -> Result<(), Error> {
    if let Some(url) = signal_url {
        let job_id = job.id().to_string();
        let mut attempts = 0;

        let client = Client::builder()
            .danger_accept_invalid_certs(should_allow_invalid_certs())
            .timeout(Duration::from_secs(60))
            .build()
            .with_context(|| {
                format!(
                    "Failed to build HTTP client for signaling web portal for job: {}",
                    job_id
                )
            })?;

        while attempts < 5 {
            attempts += 1;

            let response = match client
                .get(url.clone())
                .query(&[("job_id", job_id.as_str())])
                .send()
                .await
            {
                Ok(resp) => resp,
                Err(e) => {
                    tracing::warn!(
                        "Attempt {}: Failed to signal web portal for job: {}. Error: {}",
                        attempts,
                        job_id,
                        e
                    );
                    // Wait before retrying
                    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                    continue;
                }
            };

            if response.status().is_success() {
                tracing::info!("Successfully signaled web portal for job: {}", job_id);
                return Ok(());
            } else {
                tracing::warn!(
                    "Failed to signal web portal for job: {}. Status: {}",
                    job_id,
                    response.status()
                );
            }

            // Wait before retrying
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        }

        tracing::error!(
            "Failed to signal web portal after 5 re-attempts for job: {}",
            job_id
        );

        return Err(Error::Unknown(
            format!(
                "Failed to signal web portal after 5 re-attempts for job: {}",
                job_id
            )
            .to_string(),
        ));
    } else {
        tracing::warn!(
            "Signal URL is not set, skipping signaling web portal for job: {}",
            job.id()
        );
    }

    Ok(())
}
