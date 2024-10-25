// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use tokio::net::TcpListener;

use crate::config::ServiceConfig;
use crate::connection::Connection;
use crate::error::Error;
use crate::exchange;
use crate::healthcheck;

///
/// Internal function used to handle a single connection to the server.
/// This will enter an event loop to process messages from the client
///
async fn handle_connection(
    stream: tokio::net::TcpStream,
    config: ServiceConfig,
) -> Result<(), Error> {
    let mut connection = Connection::new(config);

    match connection.handle_connection(stream).await {
        Ok(_) => {
            tracing::info!("Connection handled successfully");
        }
        Err(e) => {
            tracing::error!("Error handling connection: {}", e);
        }
    }

    Ok(())
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
/// This function will return a Error if the server fails to start.
///
pub async fn run_once(config: ServiceConfig) -> Result<(), Error> {
    // Create the event loop and TCP listener we'll accept connections on.

    let addr = format!("{}:{}", config.ip(), config.port());

    let listener = TcpListener::bind(addr).await?;
    tracing::info!("Listening on: {}", listener.local_addr()?);

    // Let's spawn the handling of each connection in a separate task.
    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                tracing::info!("New connection from: {}", addr);

                // spawn a new task to handle the connection, and don't
                // wait for it to finish - the function will handle all
                // the processing and errors itself
                tokio::spawn(handle_connection(stream, config.clone()));
            }
            Err(e) => {
                tracing::error!("Error accepting connection: {:?}", e);
            }
        }
    }
}

pub async fn run(config: ServiceConfig) -> Result<(), Error> {
    // set the name of the service in the exchange
    exchange::set_name(&config.name()).await?;

    // spawn the healthcheck server if enabled
    if let Some(healthcheck_port) = config.healthcheck_port() {
        healthcheck::spawn(config.ip(), healthcheck_port).await?;
    }

    loop {
        let result = run_once(config.clone()).await;

        match result {
            Ok(_) => {
                tracing::info!("Server run completed successfully");
            }
            Err(e) => {
                tracing::error!("Error running server: {}", e);

                // sleep for a bit before retrying
                tracing::info!("Retrying in 5 seconds");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
    }
}
