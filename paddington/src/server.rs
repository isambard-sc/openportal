// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use once_cell::sync::Lazy;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::config::ServiceConfig;
use crate::connection::Connection;
use crate::error::Error;
use crate::exchange;
use crate::healthcheck;

/// Maximum number of inbound connections that may be in the *unauthenticated*
/// (pre-handshake-completion) state at once. A legitimate deployment has at
/// most a few dozen agents, so this generous cap never affects real peers while
/// bounding a connection flood: an attacker would have to both originate from
/// an allow-listed source (see `ServiceConfig::may_attempt_connection`) *and*
/// hold this many half-open handshakes simultaneously to deny service. The
/// permit is released as soon as a connection authenticates, so long-lived
/// authenticated peers never occupy the pool. See
/// docs/specifications/security-review.md (finding F11).
const MAX_UNAUTHENTICATED_CONNECTIONS: usize = 2048;

/// Process-wide pool of unauthenticated-connection slots (see
/// `MAX_UNAUTHENTICATED_CONNECTIONS`).
static UNAUTHENTICATED_CONNECTION_SLOTS: Lazy<Arc<Semaphore>> =
    Lazy::new(|| Arc::new(Semaphore::new(MAX_UNAUTHENTICATED_CONNECTIONS)));

///
/// Internal function used to handle a single connection to the server.
/// This will enter an event loop to process messages from the client.
///
/// `permit` is the unauthenticated-connection slot acquired in the accept
/// loop; it is passed through to `Connection::handle_connection`, which
/// releases it the moment the peer authenticates (finding F11).
///
async fn handle_connection(
    stream: tokio::net::TcpStream,
    config: ServiceConfig,
    permit: OwnedSemaphorePermit,
) -> Result<(), Error> {
    let mut connection = Connection::new(config);

    match connection.handle_connection(stream, permit).await {
        Ok(_) => {
            tracing::debug!("Connection closed after successful handling");
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
    //
    // Built as a typed `SocketAddr` rather than a formatted string -
    // `Display` for an IPv6 `IpAddr` doesn't add the `[...]` brackets the
    // string socket-address syntax requires, so `format!("{}:{}", ...)`
    // would silently produce an unparseable address for an IPv6 `ip`. See
    // `docs/plans/ipv6-support-design.md` §4.1; mirrors the same pattern
    // already used in `healthcheck.rs`.
    let addr = SocketAddr::new(config.ip(), config.port());

    let listener = TcpListener::bind(addr).await?;
    tracing::info!("Listening on: {}", listener.local_addr()?);

    // Let's spawn the handling of each connection in a separate task.
    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                // Fail-fast (finding F11): drop connections from source
                // addresses that could never authenticate - anything not
                // matching a configured client IP or the trusted-proxy range -
                // before doing any WebSocket-upgrade or cryptographic work.
                // This is the cheapest possible rejection and is what keeps a
                // flood from unexpected addresses from costing anything.
                if !config.may_attempt_connection(&addr.ip()) {
                    tracing::warn!(
                        "Rejecting connection from unexpected address {} \
                         (not an allow-listed client IP or trusted proxy)",
                        addr
                    );
                    drop(stream);
                    continue;
                }

                // Bound the number of concurrent unauthenticated connections
                // (finding F11). `try_acquire_owned` never blocks the accept
                // loop; if the pool is exhausted we drop the new connection
                // rather than let a flood of half-open handshakes exhaust
                // tasks/sockets/memory. The permit is released as soon as the
                // connection authenticates.
                let permit = match Arc::clone(&UNAUTHENTICATED_CONNECTION_SLOTS).try_acquire_owned()
                {
                    Ok(permit) => permit,
                    Err(_) => {
                        tracing::warn!(
                            "Refusing connection from {}: too many unauthenticated \
                                 connections in progress (limit {})",
                            addr,
                            MAX_UNAUTHENTICATED_CONNECTIONS
                        );
                        drop(stream);
                        continue;
                    }
                };

                tracing::info!("New connection from: {}", addr);

                // spawn a new task to handle the connection, and don't
                // wait for it to finish - the function will handle all
                // the processing and errors itself
                tokio::spawn(handle_connection(stream, config.clone(), permit));
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

    // mark this agent as having server connections, which disables
    // the HA standby-only logic (only applicable to client-only agents)
    exchange::set_is_server();

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
