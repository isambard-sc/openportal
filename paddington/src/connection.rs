// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::crypto::CryptoError;
use anyhow::Context;
use anyhow::Error as AnyError;
use thiserror::Error;

use futures::{SinkExt, StreamExt};
use futures_channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures_util::{future, pin_mut, stream::TryStreamExt};
use secrecy::ExposeSecret;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::protocol::Message;

use std::sync::{Arc, Mutex};

use crate::config::{PeerConfig, ServiceConfig};
use crate::crypto::{EncryptedData, Key, SecretKey};

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

fn envelope_message<T>(
    message: T,
    inner_key: &SecretKey,
    outer_key: &SecretKey,
) -> Result<Message, AnyError>
where
    T: Serialize,
{
    let message = inner_key
        .expose_secret()
        .encrypt(message)
        .with_context(|| "Error encrypting message with the inner key.")?;
    let message = outer_key
        .expose_secret()
        .encrypt(message)
        .with_context(|| "Error encrypting message with the outer key.")?;
    Ok(Message::text(
        serde_json::to_string(&message).with_context(|| "Error serialising message to JSON.")?,
    ))
}

fn deenvelope_message<T>(
    message: Message,
    inner_key: &SecretKey,
    outer_key: &SecretKey,
) -> Result<T, AnyError>
where
    T: DeserializeOwned,
{
    println!("De-enveloping message: {:?}", message);
    let message: EncryptedData = serde_json::from_str::<EncryptedData>(
        message
            .to_text()
            .with_context(|| "Error converting message to text.")?,
    )
    .with_context(|| "Error deserialising message from JSON.")?;

    println!("Outer key {:?}", message);
    let message = outer_key
        .expose_secret()
        .decrypt::<EncryptedData>(&message)
        .with_context(|| "Error decrypting message with the outer key.")?;

    println!("Inner key {:?}", message);
    let message = inner_key
        .expose_secret()
        .decrypt::<T>(&message)
        .with_context(|| "Error decrypting message with the inner key.")?;

    println!("Done de-enveloping message");
    Ok(message)
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

        let message = envelope_message(Key::generate(), &self.config.key, &peer.key)?;

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
                            "Already handling a connection {} - closing new connection.",
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

        let addr: std::net::SocketAddr = stream
            .peer_addr()
            .with_context(|| "Error getting the peer address. Ensure the connection is open.")?;

        println!("Accepted connection from peer: {}", addr);

        let ws_stream = tokio_tungstenite::accept_async(stream)
            .await
            .with_context(|| {
                format!(
                    "Error accepting WebSocket connection from: {}. Closing connection.",
                    addr
                )
            })?;

        // Split the WebSocket stream into incoming and outgoing parts
        let (mut outgoing, mut incoming) = ws_stream.split();

        // do the handshake with the client - the client should have sent an initial message
        // with the peer information
        let message = incoming
            .next()
            .await
            .ok_or_else(|| {
                eprintln!("No peer information received - closing connection.");
                ConnectionError::InvalidPeer(
                    "No peer information received - closing connection.".to_string(),
                )
            })?
            .unwrap_or_else(|_| Message::text(""));

        if message.is_empty() {
            eprintln!("No peer information received - closing connection.");
            return Err(ConnectionError::InvalidPeer(
                "No peer information received - closing connection.".to_string(),
            ));
        }

        println!("Received message: {:?}", message);

        // de-envelope the message
        let peer_session_key =
            deenvelope_message::<SecretKey>(message, &self.config.key, &self.config.key)
                .with_context(|| "Error de-enveloping message - closing connection.")?;

        // now check that the peer is correct and we are not already handling
        // another connection

        println!("Sending session key");

        // create our own session key and send this to the client
        let session_key = Key::generate();

        let response = envelope_message(session_key, &peer_session_key, &self.config.key)
            .with_context(|| "Error enveloping message - closing connection.")?;

        outgoing
            .send(response)
            .await
            .with_context(|| "Error sending response to peer")?;

        println!("Handshake complete!");

        // we've now completed the handshake and can use the two session
        // keys to trust and secure both ends of the connection

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
