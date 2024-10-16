// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;

use crate::config::ServiceConfig;
use crate::error::Error;
use crate::{client, server};

pub async fn run(config: ServiceConfig) -> Result<(), Error> {
    let mut server_handles = vec![];
    let mut client_handles = vec![];

    if !config.clients().is_empty() {
        let my_config = config.clone();
        server_handles.push(tokio::spawn(async move { server::run(my_config).await }));
    }

    for server in config.servers() {
        let my_config = config.clone();
        client_handles.push(tokio::spawn(async move {
            client::run(my_config.clone(), server.to_peer()).await
        }));
    }

    if server_handles.is_empty() && client_handles.is_empty() {
        tracing::warn!("No servers or clients to run.");
    }

    if !server_handles.is_empty() {
        tracing::info!("Number of expected clients: {}", config.clients().len());
    }

    if !client_handles.is_empty() {
        tracing::info!("Number of expected servers: {}", config.servers().len());
    }

    for handle in server_handles {
        let _ = handle.await?;
    }

    for handle in client_handles {
        let _ = handle.await?;
    }

    tracing::info!("All handles joined.");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ServiceConfig;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_run() -> Result<()> {
        // this tests that the service can be configured and will run
        // (it will exit immediately as there are no clients or servers)
        let config = ServiceConfig::parse("test_server", "http://localhost", "127.0.0.1", 5544)?;
        run(config).await?;

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_ping_pong() -> Result<()> {
        let mut primary = ServiceConfig::parse("primary", "http://localhost", "127.0.0.1", 5544)
            .unwrap_or_else(|e| {
                unreachable!("Cannot create service config: {}", e);
            });

        let mut secondary =
            ServiceConfig::parse("secondary", "http://localhost", "127.0.0.1", 5545)
                .unwrap_or_else(|e| {
                    unreachable!("Cannot create service config: {}", e);
                });

        // introduce the secondary to the primary
        let invite = primary
            .add_client(&secondary.name(), "127.0.0.1")
            .unwrap_or_else(|e| {
                unreachable!("Cannot add secondary to primary: {}", e);
            });

        // give the invitation to the secondary
        secondary.add_server(invite).unwrap_or_else(|e| {
            unreachable!("Cannot add primary to secondary: {}", e);
        });

        // run the primary
        let primary_handle = tokio::spawn(async move { run(primary).await });

        // run the secondary
        let secondary_handle = tokio::spawn(async move { run(secondary).await });

        // wait for the primary to finish
        let _ = primary_handle.await?;

        // wait for the secondary to finish
        let _ = secondary_handle.await?;

        Ok(())
    }
}
