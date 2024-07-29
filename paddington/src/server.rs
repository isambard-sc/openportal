// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::crypto::CryptoError;
use anyhow::Error as AnyError;
use std::io::Error as IOError;
use thiserror::Error;

use tokio::net::TcpListener;

use crate::config::ServiceConfig;
use crate::connection::Connection;

#[derive(Error, Debug)]
pub enum ServerError {
    #[error("{0}")]
    IOError(#[from] IOError),

    #[error("{0}")]
    AnyError(#[from] AnyError),

    #[error("{0}")]
    TungsteniteError(#[from] tokio_tungstenite::tungstenite::error::Error),

    #[error("{0}")]
    CryptoError(#[from] CryptoError),

    #[error("Unknown config error")]
    Unknown,
}

///
/// Run the server - this will execute the server and listen for incoming
/// connections indefinitely, until it is stopped.
///
/// # Arguments
///
/// * `config` - The configuration for the service.
///
/// # Returns
///
/// This function will return a ServerError if the server fails to start.
///
pub async fn run(config: ServiceConfig) -> Result<(), ServerError> {
    // Create the event loop and TCP listener we'll accept connections on.
    let listener = TcpListener::bind("127.0.0.1").await?;
    println!("Listening on: {}", listener.local_addr()?);

    // Let's spawn the handling of each connection in a separate task.
    loop {
        println!("Awaiting the next connection...");

        match listener.accept().await {
            Ok((stream, addr)) => {
                println!("New connection from: {}", addr);

                let connection = Connection::new(config.clone());

                // eventually could look up different handlers based on different
                // configs and addresses of clients - for now, we will do something basic
                let message_handler = |msg: &str| -> Result<(), AnyError> {
                    println!("Handler received message: {}", msg);
                    Ok(())
                };

                let result = tokio::spawn(connection.handle_connection(stream, message_handler));

                match result.await {
                    Ok(_) => {
                        println!("Spawn: Connection closed");
                    }
                    Err(e) => {
                        eprintln!("Error handling connection: {}", e);
                    }
                }
            }
            Err(e) => {
                eprintln!("Error accepting connection: {}", e);
            }
        }
    }
}
