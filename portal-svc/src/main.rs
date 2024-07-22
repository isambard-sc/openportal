// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use clap::{CommandFactory as _, Parser, Subcommand};
use paddington;
use tokio;

pub mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

fn version() -> &'static str {
    built_info::GIT_VERSION.unwrap_or(built_info::PKG_VERSION)
}

fn default_config_path() -> std::path::PathBuf {
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
        help=format!(
            "Path to the openportal config directory [default: {}]",
            &default_config_path().display(),
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

#[tokio::main]
async fn main() {
    let args = match Args::try_parse() {
        Ok(args) => args,
        Err(err) => {
            err.print();
            std::process::exit(64); // sysexit EX_USAGE
        }
    };

    let config = paddington::config::load().await.unwrap_or_else(|err| {
        panic!("Error loading config: {:?}", err);
    });

    println!("Loaded config: {:?}", config);

    paddington::server::run(config).await;
}
