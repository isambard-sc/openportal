// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::crypto::CryptoError;
use anyhow::Context;
use anyhow::Error as AnyError;
use thiserror::Error;

use futures_channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures_util::{future, pin_mut, stream::TryStreamExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::protocol::Message;

use std::sync::{Arc, Mutex};

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

    #[error("Unknown config error")]
    Unknown,
}

type Tx = UnboundedSender<Message>;
type Rx = UnboundedReceiver<Message>;

#[derive(Debug)]
pub struct Connection {
    pub name: String,

    tx: Arc<Mutex<Tx>>,
    rx: Rx,
}

impl Connection {
    pub fn new(name: String) -> Self {
        let (tx, rx) = unbounded::<Message>();

        Connection {
            name,
            tx: Arc::new(Mutex::new(tx)),
            rx,
        }
    }

    pub async fn handle_connection(self, stream: TcpStream) -> Result<(), ConnectionError> {
        let addr: std::net::SocketAddr = stream.peer_addr().unwrap_or_else(|e| {
            eprint!("Error getting peer address: {}", e);
            std::net::SocketAddr::from(([0, 0, 0, 0], 0))
        });

        println!("Accepted connection from peer: {}", addr);

        let ws_stream = tokio_tungstenite::accept_async(stream)
            .await
            .with_context(|| format!("Error accepting WebSocket connection from: {}", addr))?;

        // Split the WebSocket stream into incoming and outgoing parts
        let (outgoing, incoming) = ws_stream.split();

        // handle the sending of messages to others
        let send_to_others = incoming.try_for_each(|msg| {
            // If we can't parse the message, we'll just ignore it.
            let msg = msg.to_text().unwrap_or_else(|_| {
                eprintln!("Error parsing message: {:?}", msg);
                ""
            });

            println!("Received message: {}", msg);

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
