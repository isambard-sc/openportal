// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};

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
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber)?;

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

///
/// This function creates an Instance Agent that represents a cluster
/// that is being directly connected to by the portal
///
async fn run_cluster(invitation: &Path) -> Result<(), Error> {
    Ok(())
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
    Ok(())
}
