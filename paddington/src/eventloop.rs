// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;

use crate::config::ServiceConfig;
use crate::error::Error;
use crate::{client, server};

pub async fn run(config: ServiceConfig) -> Result<(), Error> {
    match rustls::crypto::ring::default_provider().install_default() {
        Ok(_) => {}
        Err(e) => {
            tracing::error!("Could not install default ring provider: {:?}", e);
            return Err(Error::NotExists(
                "Could not install default ring provider".to_owned(),
            ));
        }
    }

    let mut server_handles = vec![];
    let mut client_handles = vec![];

    tracing::info!(
        "Communication layer: {} version {}",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION")
    );

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
        let config = ServiceConfig::new(
            "test_server",
            "http://localhost",
            "127.0.0.1",
            &5544,
            &None,
            &None,
        )?;
        run(config).await?;

        Ok(())
    }
}
