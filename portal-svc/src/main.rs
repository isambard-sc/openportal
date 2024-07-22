// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::{Context, Result};
use clap::{CommandFactory as _, Parser, Subcommand};
use paddington;
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

    #[arg(
        long,
        help=format!(
            "Initialise the config directory using the passed name of the service",
        )
    )]
    init: Option<String>,

    #[arg(
        long,
        help=format!(
            "Whether or not to force reinitialisation of the config directory (removes the original!)",
        )
    )]
    force: bool,

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

fn main() -> Result<()> {
    let args = match Args::try_parse() {
        Ok(args) => args,
        Err(err) => {
            err.print();
            std::process::exit(64); // sysexit EX_USAGE
        }
    };

    // see if we need to initialise the config directory
    if let Some(service_name) = &args.init {
        let config_dir = match &args.config_dir {
            Some(f) => f.clone(),
            None => default_config_dir(),
        };

        if config_dir.exists() {
            if (args.force) {
                std::fs::remove_dir_all(&config_dir)
                    .context("Could not remove existing config directory.")?;
            } else {
                anyhow::bail!("Config directory already exists. Use --force to reinitialise.");
            }
        }

        paddington::config::create(&config_dir, service_name)
            .context("Could not create config directory.")?;
    }

    // Load settings from the config file
    let config_dir = match &args.config_dir {
        Some(f) => match f.try_exists() {
            Ok(true) => shellexpand::path::tilde(f),
            Ok(false) => anyhow::bail!(format!("Config directory `{}` not found.", &f.display())),
            Err(err) => return Err(err).context("Could not determine if config directory exists."),
        },
        None => default_config_dir().into(),
    };

    println!("Using config directory: {}", config_dir.display());

    let config = paddington::config::load(&config_dir).unwrap_or_else(|err| {
        panic!("Error loading config: {:?}", err);
    });

    println!("Loaded config: {:?}", config);

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            paddington::server::run(config).await;
        });

    Ok(())
}
