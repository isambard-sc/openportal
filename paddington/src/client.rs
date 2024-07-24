// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Context;
use anyhow::Error as AnyError;
use thiserror::Error;

use futures::{SinkExt, StreamExt};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::http::response;
use tokio_tungstenite::tungstenite::protocol::Message;

use crate::config;

#[derive(Error, Debug)]
pub enum ClientError {
    #[error("{0}")]
    AnyError(#[from] AnyError),

    #[error("{0}")]
    TungsteniteError(#[from] tokio_tungstenite::tungstenite::error::Error),

    #[error("Unknown config error")]
    Unknown,
}

pub async fn run(config: config::ServiceConfig) -> Result<(), ClientError> {
    println!("Starting client {}", config.name);

    let url = "ws://localhost:8080/socket";

    let (mut socket, _) = connect_async(url)
        .await
        .with_context(|| format!("Error connecting to WebSocket at: {}", url))?;

    println!("Successfully connected to the WebSocket");

    for i in 0..10 {
        // create message
        let message = Message::from(format!("Message {}", i));

        // send message
        if let Err(e) = socket.send(message).await {
            eprintln!("Error sending message: {:?}", e);
        }

        // recieve response
        if let Some(Ok(response)) = socket.next().await {
            println!("{response}");
        }
    }

    Ok(())
}
