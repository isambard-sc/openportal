// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Context;
use anyhow::Error as AnyError;
use thiserror::Error;

use tokio_tungstenite::tungstenite::protocol::Message;

use std::{
    collections::HashMap,
    io::Error as IOError,
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use futures_channel::mpsc::{unbounded, UnboundedSender};
use futures_util::{future, pin_mut, stream::TryStreamExt, StreamExt};

use tokio::net::{TcpListener, TcpStream};

use crate::config;
use crate::crypto::{CryptoError, EncryptedData};
use secrecy::ExposeSecret;

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

type Tx = UnboundedSender<Message>;
type PeerMap = Arc<Mutex<HashMap<SocketAddr, Tx>>>;

///
/// Internal function used to handle a connection from a client.
/// This function will handle the connection from a client
///
async fn handle_connection(
    peer_map: PeerMap,
    raw_stream: TcpStream,
    addr: SocketAddr,
    config: config::ServiceConfig,
) -> Result<(), ServerError> {
    println!("Incoming TCP connection from: {}", addr);

    let ws_stream = tokio_tungstenite::accept_async(raw_stream)
        .await
        .with_context(|| format!("Error accepting WebSocket connection from: {}", addr))?;

    println!("WebSocket connection established: {}", addr);

    // Insert the write part of this peer to the peer map.
    let (tx, rx) = unbounded();

    loop {
        match peer_map.lock() {
            Ok(mut peers) => {
                peers.insert(addr, tx.clone());
                break;
            }
            Err(_) => {
                continue;
            }
        }
    }

    let (outgoing, incoming) = ws_stream.split();

    let broadcast_incoming = incoming.try_for_each(|msg| {
        // If we can't parse the message, we'll just ignore it.
        let msg = msg.to_text().unwrap_or_else(|_| {
            println!("Error parsing message from {}", addr);
            ""
        });

        if msg.is_empty() {
            // ignore empty messages
            return future::ok(());
        }

        println!("Received a message from {}: {}", addr, msg);

        let obj: EncryptedData = serde_json::from_str::<EncryptedData>(msg).unwrap_or_else(|_| {
            println!("Error parsing message from {}", addr);
            EncryptedData {
                data: vec![],
                version: 0,
            }
        });

        if obj.data.is_empty() {
            // ignore empty messages
            return future::ok(());
        }

        println!("Encrypted message: {:?}", obj);

        let key = &config.key;

        let decrypted_message = key.expose_secret().decrypt(&obj).unwrap_or_else(|_| {
            println!("Error decrypting message from {}", addr);
            "".to_string()
        });

        println!("Decrypted message: {:?}", decrypted_message);

        loop {
            match peer_map.lock() {
                Ok(peers) => {
                    // We want to broadcast the message to everyone except ourselves.
                    let broadcast_recipients = peers
                        .iter()
                        .filter(|(peer_addr, _)| peer_addr != &&addr)
                        .map(|(_, ws_sink)| ws_sink);

                    for recp in broadcast_recipients {
                        match recp.unbounded_send(Message::text(msg)) {
                            Ok(_) => {}
                            Err(_) => {
                                println!("Error sending message");
                                continue;
                            }
                        }
                    }

                    break;
                }
                Err(_) => {
                    continue;
                }
            }
        }

        future::ok(())
    });

    let receive_from_others = rx.map(Ok).forward(outgoing);

    pin_mut!(broadcast_incoming, receive_from_others);
    future::select(broadcast_incoming, receive_from_others).await;

    println!("{} disconnected", &addr);

    loop {
        match peer_map.lock() {
            Ok(mut peers) => {
                peers.remove(&addr);
                break;
            }
            Err(_) => {
                continue;
            }
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
/// This function will return a ServerError if the server fails to start.
///
pub async fn run(config: config::ServiceConfig) -> Result<(), ServerError> {
    let addr: String = config.server.clone() + ":" + &config.port.to_string();

    let state = PeerMap::new(Mutex::new(HashMap::new()));

    // Create the event loop and TCP listener we'll accept connections on.
    let listener = TcpListener::bind(&addr).await?;
    println!("Listening on: {}", addr);

    // Let's spawn the handling of each connection in a separate task.
    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                tokio::spawn(handle_connection(
                    state.clone(),
                    stream,
                    addr,
                    config.clone(),
                ));
            }
            Err(e) => {
                eprintln!("Error accepting connection: {}", e);
            }
        }
    }
}
