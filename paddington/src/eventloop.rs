// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Error as AnyError;
use anyhow::Result;
use thiserror::Error;

use crate::args::{process_args, Defaults, Error as ArgsError, ProcessResult};
use crate::config::ConfigError;
use crate::{client, server};

pub async fn run(defaults: Defaults) -> Result<(), Error> {
    match process_args(&defaults).await? {
        ProcessResult::ServiceConfig(config) => {
            if config.is_null() {
                return Ok(());
            }

            let mut server_handles = vec![];
            let mut client_handles = vec![];

            if config.has_clients() {
                let my_config = config.clone();
                server_handles.push(tokio::spawn(async move { server::run(my_config).await }));
            }

            let servers = config.get_servers();

            for server in servers {
                let my_config = config.clone();
                client_handles.push(tokio::spawn(async move {
                    client::run(my_config.clone(), server.to_peer()).await
                }));
            }

            if server_handles.is_empty() && client_handles.is_empty() {
                tracing::warn!("No servers or clients to run.");
            }

            if !server_handles.is_empty() {
                tracing::info!("Number of expected clients: {}", config.num_clients());
            }

            if !client_handles.is_empty() {
                tracing::info!("Number of expected servers: {}", config.num_servers());
            }

            for handle in server_handles {
                let _ = handle.await?;
            }

            for handle in client_handles {
                let _ = handle.await?;
            }

            tracing::info!("All handles joined.");
        }
        ProcessResult::Invite(invite) => {
            // write the invite to a file
            let filename = invite.save()?;
            println!("Invite saved to {}", filename);
            println!(
                "You can load this into the client using the 'server --add {filename}' command."
            );
        }
        ProcessResult::Message(message) => {
            println!("{}", message);
        }
        ProcessResult::None => {
            // this is the exit condition
            return Ok(());
        }
    }

    Ok(())
}

/// Errors

#[derive(Error, Debug)]
pub enum Error {
    #[error("{0}")]
    Any(#[from] AnyError),

    #[error("{0}")]
    Args(#[from] ArgsError),

    #[error("{0}")]
    Config(#[from] ConfigError),

    #[error("{0}")]
    Join(#[from] tokio::task::JoinError),

    #[error("Unknown config error")]
    Unknown,
}
