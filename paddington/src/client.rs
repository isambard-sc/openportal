// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Context;
use anyhow::Error as AnyError;
use std::io::Error as IOError;
use thiserror::Error;

use crate::config::{PeerConfig, ServiceConfig};
use crate::connection::Connection;
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

    #[error("Unknown config error")]
    Unknown,
}

pub async fn run(config: ServiceConfig) -> Result<(), ClientError> {
    println!("Starting client {}", config.name);

    // create a fake peer
    let peer = PeerConfig::new(
        "client".to_string(),
        crypto::Key::generate(),
        "localhost".to_string(),
        8080,
    );

    let connection = Connection::new(config.clone());

    // use a default message handler for now - in the future we could
    // choose this based on the identities of the sides of the connection
    let message_handler = |msg: &str| -> Result<(), anyhow::Error> {
        println!("Received message: {}", msg);
        Ok(())
    };

    println!("Making the connection to the server");

    // connect to the server
    connection
        .make_connection(peer.clone(), message_handler)
        .await
        .with_context(|| {
            format!(
                "Error with the connection to the server at: {}:{}",
                peer.server, peer.port
            )
        })?;

    Ok(())
}
