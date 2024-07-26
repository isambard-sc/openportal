// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use paddington;
use std::path::absolute;
use tokio;

pub mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

fn version() -> &'static str {
    built_info::GIT_VERSION.unwrap_or(built_info::PKG_VERSION)
}

fn default_config_dir() -> std::path::PathBuf {
    dirs::config_local_dir()
        .unwrap_or(
            ".".parse()
                .expect("Could not parse fallback config directory."),
        )
        .join("openportal")
}

#[derive(Parser)]
#[command(version = version(), about, long_about = None)]
struct Args {
    #[arg(
        long,
        short='c',
        help=format!(
            "Path to the openportal config directory [default: {}]",
            &default_config_dir().display(),
        )
    )]
    config_dir: Option<std::path::PathBuf>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Adding and removing clients
    Client {
        /// Generate the SSH config snippet
        #[command(subcommand)]
        command: Option<ClientCommands>,
    },

    /// Initialise the Service
    Init {
        /// Initialise the service
        #[arg(long, short = 'n', help = "Name of the service to initialise")]
        service: Option<String>,

        #[arg(
            long,
            short = 'h',
            help = "Hostname of the service (e.g. https://localhost - protocol is optional)"
        )]
        host: Option<String>,

        #[arg(long, short = 'p', help = "Port number for the service")]
        port: Option<u16>,

        #[arg(long, short = 'f', help = "Force reinitialisation")]
        force: bool,
    },
}

#[derive(Subcommand)]
enum ClientCommands {
    /// Add a client to the service
    Add {
        #[arg(long)]
        client: String,
    },

    /// Remove a client from the service
    Remove {
        #[arg(long)]
        client: String,
    },
}

async fn async_main(config: paddington::config::ServiceConfig) {
    match paddington::client::run(config).await {
        Ok(_) => {}
        Err(e) => {
            eprintln!("Unable to start the client: {}", e);
            std::process::exit(1);
        }
    }
}

fn main() -> Result<()> {
    let args = Args::parse();

    let config_dir = absolute(match &args.config_dir {
        Some(f) => f.clone(),
        None => default_config_dir(),
    })?;

    // see if we need to initialise the config directory
    match &args.command {
        Some(Commands::Init {
            service,
            host,
            port,
            force,
        }) => {
            if config_dir.try_exists()? {
                if *force {
                    std::fs::remove_dir_all(&config_dir)
                        .context("Could not remove existing config directory.")?;
                } else {
                    anyhow::bail!(
                        "Config directory {} already exists.\nUse --force to reinitialise.",
                        config_dir.display()
                    );
                }
            }

            paddington::config::create(&config_dir, service, host, port)?;
        }
        _ => {}
    }

    let config = paddington::config::load(&config_dir)?;

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|e| {
            eprintln!("Error creating runtime: {}", e);
            std::process::exit(1);
        })
        .block_on(async_main(config));

    Ok(())
}
