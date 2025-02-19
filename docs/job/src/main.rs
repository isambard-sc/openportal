// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};

use paddington::config::ServiceConfig;
use paddington::invite::Invite;
use templemeads::agent;
use templemeads::async_runnable;
use templemeads::grammar::Instruction::{AddUser, RemoveUser};
use templemeads::job::{Envelope, Job};
use templemeads::Error;

mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

fn version() -> &'static str {
    built_info::GIT_VERSION.unwrap_or(built_info::PKG_VERSION)
}

// simple command line options - either this is the
// "server" or the "client"
#[derive(Parser)]
#[command(version = version(), about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Invitation file from the portal to the cluster
    Cluster {
        #[arg(long, short = 'c', help = "Path to the invitation file")]
        invitation: Option<std::path::PathBuf>,
    },

    /// Specify the port to listen on for the portal
    Portal {
        #[arg(
            long,
            short = 'u',
            help = "URL of the portal including port and route (e.g. http://localhost:8080)"
        )]
        url: Option<String>,

        #[arg(
            long,
            short = 'i',
            help = "IP address on which to listen for connections (e.g. 127.0.0.1)"
        )]
        ip: Option<String>,

        #[arg(
            long,
            short = 'p',
            help = "Port on which to listen for connections (e.g. 8042)"
        )]
        port: Option<u16>,

        #[arg(
            long,
            short = 'r',
            help = "IP range on which to accept connections from the cluster"
        )]
        range: Option<String>,

        #[arg(
            long,
            short = 'c',
            help = "Config file to which to write the invitation for the cluster"
        )]
        invitation: Option<std::path::PathBuf>,
    },
}

///
/// Simple main function - this will parse the command line arguments
/// and either call run_client or run_server depending on whether this
/// is the client or server
///
#[tokio::main]
async fn main() -> Result<()> {
    // start tracing
    templemeads::config::initialise_tracing();

    // parse the command line arguments
    let args: Args = Args::parse();

    match &args.command {
        Some(Commands::Cluster { invitation }) => {
            let invitation = match invitation {
                Some(invitation) => invitation,
                None => {
                    return Err(anyhow::anyhow!("No invitation file specified."));
                }
            };

            run_cluster(invitation).await?;
        }
        Some(Commands::Portal {
            url,
            ip,
            port,
            range,
            invitation,
        }) => {
            let url = match url {
                Some(url) => url.clone(),
                None => "http://localhost:6501".to_string(),
            };

            let ip = match ip {
                Some(ip) => ip.clone(),
                None => "127.0.0.1".to_string(),
            };

            let port = match port {
                Some(port) => port,
                None => &6501,
            };

            let range = match range {
                Some(range) => range.clone(),
                None => "0.0.0.0/0.0.0.0".to_string(),
            };

            let invitation = match invitation {
                Some(invitation) => invitation.clone(),
                None => PathBuf::from("invitation.toml"),
            };

            run_portal(&url, &ip, port, &range, &invitation).await?;
        }
        _ => {
            let _ = Args::command().print_help();
        }
    }

    Ok(())
}

async_runnable! {
    ///
    /// Runnable function that will be called when a job is received
    /// by the cluster agent
    ///
    pub async fn cluster_runner(envelope: Envelope) -> Result<Job, Error>
    {
        let mut job = envelope.job();

        match job.instruction() {
            AddUser(user) => {
                // add the user to the cluster
                tracing::info!("Adding {} to cluster", user);

                tracing::info!("Here we would implement the business logic to add the user to the cluster");

                job = job.completed("account created".to_string())?;
            }
            RemoveUser(user) => {
                // remove the user from the cluster
                tracing::info!("Removing {} from the cluster", user);

                tracing::info!("Here we would implement the business logic to remove the user from the cluster");

                if user.project() == "admin" {
                    job = job.errored(&format!("You are not allowed to remove the account for {:?}",
                                      user.username()))?;
                } else {
                    job = job.completed("account removed".to_string())?;
                }
            }
            _ => {
                tracing::error!("Unknown instruction: {:?}", job.instruction());
                return Err(Error::UnknownInstruction(
                    format!("Unknown instruction: {:?}", job.instruction()).to_string(),
                ));
            }
        }

        Ok(job)
    }
}

///
/// This function creates an Instance Agent that represents a cluster
/// that is being directly connected to by the portal
///
async fn run_cluster(invitation: &Path) -> Result<(), Error> {
    // load the invitation from the file
    let invite: Invite = Invite::load(invitation)?;

    // create the paddington service for the cluster agent
    // - note that the url, ip and port aren't used, as this
    // agent won't be listening for any connecting clients
    let mut service: ServiceConfig = ServiceConfig::new(
        "cluster",
        "http://localhost:6502",
        "127.0.0.1",
        &6502,
        &None,
        &None,
    )?;

    // now give the invitation to connect to the server to the client
    service.add_server(invite)?;

    // now create the config for this agent - this combines
    // the paddington service configuration with the Agent::Type
    // for the agent
    let config = agent::custom::Config::new(service, agent::Type::Instance);

    // now start the agent, passing in the message handler for the agent
    // We will start this in a background task, so that we can close the
    // program after a few seconds
    tokio::spawn(async move {
        agent::custom::run(config, cluster_runner)
            .await
            .unwrap_or_else(|e| {
                tracing::error!("Error running cluster: {}", e);
            });
    });

    // wait for a few seconds before exiting
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    std::process::exit(0);
}

async_runnable! {
    ///
    /// Runnable function that will be called when a job is received
    /// by the portal agent
    ///
    pub async fn portal_runner(envelope: Envelope) -> Result<Job, Error>
    {
        let job = envelope.job();

        tracing::error!("Unknown instruction: {:?}", job.instruction());

        return Err(Error::UnknownInstruction(
            format!("Unknown instruction: {:?}", job.instruction()).to_string(),
        ));
    }
}

///
/// This function creates a Portal Agent that represents a simple
/// portal that sends a job to a cluster
///
async fn run_portal(
    url: &str,
    ip: &str,
    port: &u16,
    range: &str,
    invitation: &Path,
) -> Result<(), Error> {
    // create a paddington service configuration for the portal agent
    let mut service = ServiceConfig::new("portal", url, ip, port, &None, &None)?;

    // add the cluster to the portal, returning an invitation
    let invite = service.add_client("cluster", range, &None)?;

    // save the invitation to the requested file
    invite.save(invitation)?;

    // now create the config for this agent - this combines
    // the paddington service configuration with the Agent::Type
    // for the agent
    let config = agent::custom::Config::new(service, agent::Type::Portal);

    // now start the agent, passing in the message handler for the agent
    // Do this in a background task, so that we can send jobs to the cluster
    // here - normally jobs will come from the bridge
    tokio::spawn(async move {
        agent::custom::run(config, portal_runner)
            .await
            .unwrap_or_else(|e| {
                tracing::error!("Error running portal: {}", e);
            });
    });

    // wait until the cluster has connected...
    let mut clusters = agent::get_all(&agent::Type::Instance).await;

    while clusters.is_empty() {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        clusters = agent::get_all(&agent::Type::Instance).await;
    }

    let cluster = clusters.pop().unwrap_or_else(|| {
        tracing::error!("No cluster connected to the portal");
        std::process::exit(1);
    });

    // create a job to add a user to the cluster
    let mut job = Job::parse("portal.cluster add_user fred.proj.portal", true)?;

    // put this job to the cluster
    job = job.put(&cluster).await?;

    // get the result - note that calling 'result' on its own would
    // just look to see if the result exists now. To actually wait
    // for the result to arrive we need to use the 'wait' function,
    // await on that, and then call 'result'
    let result: Option<String> = job.wait().await?.result()?;

    tracing::info!("Result: {:?}", result);

    // create a job to remove a user from the cluster
    let mut job = Job::parse("portal.cluster remove_user fred.proj.portal", true)?;

    // put this job to the cluster
    job = job.put(&cluster).await?;

    // get the result
    let result: Option<String> = job.wait().await?.result()?;

    tracing::info!("Result: {:?}", result);

    // try to remove a user who should not be removed
    let mut job = Job::parse("portal.cluster remove_user jane.admin.portal", true)?;

    // put this job to the cluster
    job = job.put(&cluster).await?;

    // get the result - this should exit with an error
    let result: Option<String> = job.wait().await?.result()?;

    tracing::info!("Result: {:?}", result);

    Ok(())
}
