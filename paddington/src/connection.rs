// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::crypto::CryptoError;
use anyhow::Context;
use anyhow::Error as AnyError;
use thiserror::Error;

use futures::{SinkExt, StreamExt};
use futures_channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures_util::{future, pin_mut, stream::TryStreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::protocol::Message;

use std::sync::{Arc, Mutex};

use crate::config::{PeerConfig, ServiceConfig};

#[derive(Error, Debug)]
pub enum ConnectionError {
    #[error("{0}")]
    IOError(#[from] std::io::Error),

    #[error("{0}")]
    AnyError(#[from] AnyError),

    #[error("{0}")]
    SerdeError(#[from] serde_json::Error),

    #[error("{0}")]
    CryptoError(#[from] CryptoError),

    #[error("Invalid peer configuration: {0}")]
    InvalidPeer(String),

    #[error("Busy line: {0}")]
    BusyLine(String),

    #[error("Unknown config error")]
    Unknown,
}

type Tx = UnboundedSender<Message>;
type Rx = UnboundedReceiver<Message>;

#[derive(Debug)]
pub struct Connection {
    pub config: ServiceConfig,
    pub peer: Arc<Mutex<PeerConfig>>,

    tx: Arc<Mutex<Tx>>,
    rx: Rx,
}

impl Connection {
    pub fn new(config: ServiceConfig) -> Self {
        let (tx, rx) = unbounded::<Message>();

        Connection {
            config,
            peer: Arc::new(Mutex::new(PeerConfig::create_default())),
            tx: Arc::new(Mutex::new(tx.clone())),
            rx,
        }
    }

    pub async fn make_connection(
        self,
        peer: PeerConfig,
        message_handler: fn(&str) -> Result<(), anyhow::Error>,
    ) -> Result<(), ConnectionError> {
        println!("make_connection");

        // Check that we don't already have a connection, if we do,
        // we'll just return an error. This will also store the peer
        // so the peer info can be used later
        loop {
            match self.peer.lock() {
                Ok(mut self_peer) => {
                    if !self_peer.is_null() {
                        return Err(ConnectionError::BusyLine(format!(
                            "Already handling a connection to {}",
                            self_peer
                        )));
                    }

                    self_peer.clone_from(&peer);
                    break;
                }
                Err(_e) => {
                    // try again
                    continue;
                }
            }
        }

        let url = format!("ws://{}:{}/socket", peer.server, peer.port);

        println!("Connecting to WebSocket at: {}", url);

        let (mut socket, _) = connect_async(url.clone())
            .await
            .with_context(|| format!("Error connecting to WebSocket at: {}", url))?;

        println!("Successfully connected to the WebSocket");

        // do the handshake - send a message
        let message = Message::text("Hello World!");

        println!("Sending message: {:?}", message);
        if let Err(r) = socket.send(message).await {
            return Err(ConnectionError::AnyError(r.into()));
        }

        println!("Receiving message...");

        // receive the response
        let response = socket.next().await.with_context(|| {
            "Error receiving response from peer. Ensure the peer is valid and the connection is open."
        })?;

        println!("Received response: {:?}", response);

        Ok(())
    }

    pub async fn handle_connection(
        self,
        stream: TcpStream,
        message_handler: fn(&str) -> Result<(), anyhow::Error>,
    ) -> Result<(), ConnectionError> {
        // check that we aren't handling another connection
        loop {
            match self.peer.lock() {
                Ok(self_peer) => {
                    if !self_peer.is_null() {
                        return Err(ConnectionError::BusyLine(format!(
                            "Already handling a connection {}",
                            self_peer
                        )));
                    }
                    break;
                }
                Err(_e) => {
                    // try again
                    continue;
                }
            }
        }

        let addr: std::net::SocketAddr = stream.peer_addr().unwrap_or_else(|e| {
            eprint!("Error getting peer address: {}", e);
            std::net::SocketAddr::from(([0, 0, 0, 0], 0))
        });

        println!("Accepted connection from peer: {}", addr);

        let ws_stream = tokio_tungstenite::accept_async(stream)
            .await
            .with_context(|| format!("Error accepting WebSocket connection from: {}", addr))?;

        // Split the WebSocket stream into incoming and outgoing parts
        let (mut outgoing, mut incoming) = ws_stream.split();

        // do the handshake with the client - the client should have sent an initial message
        // with the peer information
        let peer_info = incoming
            .next()
            .await
            .ok_or_else(|| {
                ConnectionError::InvalidPeer("No peer information received".to_string())
            })?
            .unwrap_or_else(|_| Message::text(""));

        print!("Received peer information: {:?}", peer_info);

        // now check that the peer is correct and we are not already handling
        // another connection

        // now respond to the handshake
        let response = Message::text("Hello to you!");

        outgoing
            .send(response)
            .await
            .with_context(|| "Error sending response to peer")?;

        // handle the sending of messages to others
        let send_to_others = incoming.try_for_each(|msg| {
            // If we can't parse the message, we'll just ignore it.
            let msg = msg.to_text().unwrap_or_else(|_| {
                eprintln!("Error parsing message: {:?}", msg);
                ""
            });

            println!("Received message: {}", msg);

            message_handler(msg).unwrap_or_else(|e| {
                eprintln!("Error handling message: {:?}", e);
            });

            future::ok(())
        });

        // handle messages that should be sent to the client (received locally
        // from other services that should be forwarded to the client via the
        // outgoing stream)
        let receive_from_others = self.rx.map(Ok).forward(outgoing);

        pin_mut!(send_to_others, receive_from_others);
        future::select(send_to_others, receive_from_others).await;

        println!("{} disconnected", &addr);

        Ok(())
    }
}
