// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Context;
use anyhow::Error as AnyError;
use secrecy::ExposeSecret;
use thiserror::Error;

use futures::{SinkExt, StreamExt};
use serde_json::json;
use tokio_serde::formats::*;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::protocol::Message;

use crate::config;
use crate::crypto;

#[derive(Error, Debug)]
pub enum ClientError {
    #[error("{0}")]
    AnyError(#[from] AnyError),

    #[error("{0}")]
    TungsteniteError(#[from] tokio_tungstenite::tungstenite::error::Error),

    #[error("{0}")]
    CryptoError(#[from] crypto::CryptoError),

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

    let job = "Hello, World!".to_string();

    let key = config.key;

    let encrypted_message = key.expose_secret().encrypt(job)?;

    let json_data = serde_json::to_string(&encrypted_message).with_context(|| {
        "Failed to serialise the data to JSON. Ensure that the data is serialisable by serde."
    })?;

    let message: Message = Message::text(json_data);

    if let Err(r) = socket.send(message).await {
        eprintln!("Error sending message: {:?}", r);
    }

    // recieve response
    if let Some(Ok(response)) = socket.next().await {
        println!("{response}");
    }

    /*
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
    */

    Ok(())
}
