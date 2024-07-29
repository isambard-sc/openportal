// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Context;
use anyhow::Error as AnyError;
use clap::{Parser, Subcommand};
use std::path::absolute;
use thiserror::Error;
use url::Url;

use crate::config;

#[derive(Error, Debug)]
pub enum ArgsError {
    #[error("{0}")]
    IOError(#[from] std::io::Error),

    #[error("{0}")]
    ConfigError(#[from] config::ConfigError),

    #[error("{0}")]
    AnyError(#[from] AnyError),

    #[error("{0}")]
    ServiceNameError(String),

    #[error("Unknown arguments error")]
    Unknown,
}

pub mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

fn version() -> &'static str {
    built_info::GIT_VERSION.unwrap_or(built_info::PKG_VERSION)
}

fn default_config_file() -> std::path::PathBuf {
    dirs::config_local_dir()
        .unwrap_or(
            ".".parse()
                .expect("Could not parse fallback config directory."),
        )
        .join("openportal/service.toml")
}

#[derive(Parser)]
#[command(version = version(), about, long_about = None)]
struct Args {
    #[arg(
        long,
        short='c',
        help=format!(
            "Path to the openportal config file [default: {}]",
            &default_config_file().display(),
        )
    )]
    config_file: Option<std::path::PathBuf>,

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
            short = 'u',
            help = "URL of the service (e.g. https://localhost:8080)"
        )]
        url: Option<Url>,

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

pub async fn process_args(
    default_service_name: Option<String>,
) -> Result<config::ServiceConfig, ArgsError> {
    let args = Args::parse();

    let config_file = absolute(match &args.config_file {
        Some(f) => f.clone(),
        None => default_config_file(),
    })?;

    // see if we need to initialise the config directory
    match &args.command {
        Some(Commands::Init {
            service,
            url,
            force,
        }) => {
            if config_file.try_exists()? {
                if *force {
                    std::fs::remove_file(&config_file)
                        .context("Could not remove existing config file.")?;
                } else {
                    return Err(ArgsError::Unknown);
                }
            }

            let service_name = match service {
                Some(name) => name.clone(),
                None => match default_service_name {
                    Some(name) => name,
                    None => "".to_string(),
                },
            };

            if service_name.is_empty() {
                return Err(ArgsError::ServiceNameError(
                    "No service name provided.".to_string(),
                ));
            }

            config::create(&config_file, &service_name, url)?;
        }
        Some(Commands::Client { command }) => match command {
            Some(ClientCommands::Add { client }) => {
                println!("Adding client: {}", client);
            }
            Some(ClientCommands::Remove { client }) => {
                println!("Removing client: {}", client);
            }
            None => {
                return Err(ArgsError::Unknown);
            }
        },
        _ => {}
    }

    let config = config::load(&config_file)?;

    Ok(config)
}
