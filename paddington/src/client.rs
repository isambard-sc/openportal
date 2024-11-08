// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::config::{PeerConfig, ServiceConfig};
use crate::connection::Connection;
use crate::error::Error;
use crate::exchange;
use crate::healthcheck;

pub async fn run_once(config: ServiceConfig, peer: PeerConfig) -> Result<(), Error> {
    let service_name = config.name();

    if service_name.is_empty() {
        return Err(Error::UnknownPeer(
            "Cannot connect as service must have a name".to_string(),
        ));
    }

    let peer_name = peer.name();

    if peer_name.is_empty() {
        return Err(Error::UnknownPeer(
            "Cannot connect as peer must have a name".to_string(),
        ));
    }

    tracing::info!(
        "Initiating connection: {:?} <=> {:?}",
        service_name,
        peer_name
    );

    // create a connection object to make the connection - these are
    // mutable as they hold the state of the connection in this
    // throwaway client
    let mut connection = Connection::new(config.clone());

    // this will loop until the connection is closed
    connection.make_connection(&peer).await?;

    Ok(())
}

pub async fn run(config: ServiceConfig, peer: PeerConfig) -> Result<(), Error> {
    // set the name of the service in the exchange
    exchange::set_name(&config.name()).await?;

    if let Some(healthcheck_port) = config.healthcheck_port() {
        // spawn the health check server
        healthcheck::spawn(config.ip(), healthcheck_port).await?;
    }

    loop {
        match run_once(config.clone(), peer.clone()).await {
            Ok(_) => {
                tracing::info!("Client exited successfully.");
            }
            Err(e) => {
                tracing::error!("Client exited with error: {:?}", e);
            }
        }

        // sleep for a bit before trying again
        tracing::info!("Sleeping for 5 seconds before retrying the connection...");
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
    }
}
