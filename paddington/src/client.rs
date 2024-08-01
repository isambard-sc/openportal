// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Error as AnyError;
use std::io::Error as IOError;
use thiserror::Error;
use tracing;

use crate::config::{PeerConfig, ServiceConfig};
use crate::connection::{Connection, ConnectionError};
use crate::crypto;

#[derive(Error, Debug)]
pub enum ClientError {
    #[error("{0}")]
    IOError(#[from] IOError),

    #[error("{0}")]
    AnyError(#[from] AnyError),

    #[error("{0}")]
    TungsteniteError(#[from] tokio_tungstenite::tungstenite::error::Error),

    #[error("{0}")]
    CryptoError(#[from] crypto::CryptoError),

    #[error("{0}")]
    ConnectionError(#[from] ConnectionError),

    #[error("{0}")]
    UnknownPeerError(String),

    #[error("Unknown config error")]
    Unknown,
}

pub async fn run_once(config: ServiceConfig, peer: PeerConfig) -> Result<(), ClientError> {
    let service_name = config.name.clone().unwrap_or_default();

    if service_name.is_empty() {
        return Err(ClientError::UnknownPeerError(
            "Cannot connect as service must have a name".to_string(),
        ));
    }

    let peer_name = peer.name().clone().unwrap_or_default();

    if peer_name.is_empty() {
        return Err(ClientError::UnknownPeerError(
            "Cannot connect as peer must have a name".to_string(),
        ));
    }

    // use a default message handler for now - in the future we could
    // choose this based on the identities of the sides of the connection
    let message_handler = |msg: &str| -> Result<(), anyhow::Error> {
        tracing::info!("Received message: {}", msg);
        Ok(())
    };

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
    connection.make_connection(&peer, message_handler).await?;

    Ok(())
}

pub async fn run(config: ServiceConfig, peer: PeerConfig) -> Result<(), ClientError> {
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
