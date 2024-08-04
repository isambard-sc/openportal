// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Error as AnyError;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::RwLock;
use thiserror::Error;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinSet;

use crate::connection::{Connection, ConnectionError};

#[derive(Error, Debug)]
pub enum ExchangeError {
    #[error("{0}")]
    AnyError(#[from] AnyError),

    #[error("{0}")]
    ConnectionError(#[from] ConnectionError),

    #[error("{0}")]
    PoisonError(String),

    #[error("{0}")]
    SendError(String),

    #[error("{0}")]
    UnnamedConnectionError(String),

    #[error("Unknown error")]
    Unknown,
}

#[derive(Debug, Clone)]
pub struct ExchangeMessage {
    from: String,
    message: String,
}

// We use the singleton pattern for the exchange data, as there can only
// be one in the program, and this will let us expose the exchange functions
// directly
static SINGLETON_EXCHANGE: Lazy<RwLock<Exchange>> = Lazy::new(|| RwLock::new(Exchange::new()));

#[derive(Debug)]
pub struct Exchange {
    connections: HashMap<String, Connection>,
    tx: UnboundedSender<ExchangeMessage>,
}

impl Default for Exchange {
    fn default() -> Self {
        Self::new()
    }
}

async fn process_message(message: ExchangeMessage) -> Result<(), ExchangeError> {
    tracing::info!(
        "Received message: {} from: {}",
        message.message,
        message.from
    );

    let from = message.from.clone();

    if from == "portal" {
        send(&from, "Hello from the provider")
            .await
            .unwrap_or_else(|e| {
                tracing::error!("Error sending message: {}", e);
            });
    } else if from == "provider" {
        send(&from, "Hello from the portal")
            .await
            .unwrap_or_else(|e| {
                tracing::error!("Error sending message: {}", e);
            });
    }

    Ok(())
}

async fn event_loop(mut rx: UnboundedReceiver<ExchangeMessage>) -> Result<(), ExchangeError> {
    let mut workers = JoinSet::new();

    static MAX_WORKERS: usize = 10;

    while let Some(message) = rx.recv().await {
        // make sure we don't exceed the requested number of workers
        if workers.len() >= MAX_WORKERS {
            let result = workers.join_next().await;

            match result {
                Some(Ok(())) => {}
                Some(Err(e)) => {
                    tracing::error!("Error processing message: {}", e);
                }
                None => {
                    tracing::error!("Error processing message: None");
                }
            }
        }

        workers.spawn(async move {
            process_message(message).await.unwrap_or_else(|e| {
                tracing::error!("Error processing message: {}", e);
            });
        });
    }

    Ok(())
}

impl Exchange {
    pub fn new() -> Self {
        let (tx, rx) = unbounded_channel();

        tokio::spawn(event_loop(rx));

        Self {
            connections: HashMap::new(),
            tx,
        }
    }
}

pub fn create_default_workqueue() {}

pub async fn unregister(connection: &Connection) -> Result<(), ExchangeError> {
    let name = connection.name().unwrap_or_default();

    if name.is_empty() {
        return Err(ExchangeError::UnnamedConnectionError(
            "Connection must have a name".to_string(),
        ));
    }

    let mut exchange = match SINGLETON_EXCHANGE.write() {
        Ok(exchange) => exchange,
        Err(e) => {
            return Err(ExchangeError::PoisonError(format!(
                "Error getting write lock: {}",
                e
            )));
        }
    };

    let key = name.clone();

    if exchange.connections.contains_key(&key) {
        exchange.connections.remove(&key);
        Ok(())
    } else {
        Err(ExchangeError::UnnamedConnectionError(format!(
            "Connection {} not found",
            name
        )))
    }
}

pub async fn register(connection: Connection) -> Result<(), ExchangeError> {
    let name = connection.name().unwrap_or_default();

    if name.is_empty() {
        return Err(ExchangeError::UnnamedConnectionError(
            "Connection must have a name".to_string(),
        ));
    }

    let mut exchange = match SINGLETON_EXCHANGE.write() {
        Ok(exchange) => exchange,
        Err(e) => {
            return Err(ExchangeError::PoisonError(format!(
                "Error getting write lock: {}",
                e
            )));
        }
    };

    let key = name.clone();

    if exchange.connections.contains_key(&key) {
        return Err(ExchangeError::UnnamedConnectionError(format!(
            "Connection {} already exists",
            name
        )));
    }

    exchange.connections.insert(key, connection);
    Ok(())
}

pub async fn send(name: &str, message: &str) -> Result<(), ExchangeError> {
    let connection = match SINGLETON_EXCHANGE.read() {
        Ok(exchange) => exchange,
        Err(e) => {
            return Err(ExchangeError::PoisonError(format!(
                "Error getting read lock: {}",
                e
            )));
        }
    }
    .connections
    .get(name)
    .cloned();

    if let Some(connection) = connection {
        connection.send_message(message).await?;
        Ok(())
    } else {
        Err(ExchangeError::UnnamedConnectionError(format!(
            "Connection {} not found",
            name
        )))
    }
}

pub fn received(from: &str, message: &str) -> Result<(), ExchangeError> {
    tracing::info!("Posting message: {}", message);

    let exchange = match SINGLETON_EXCHANGE.read() {
        Ok(exchange) => exchange,
        Err(e) => {
            return Err(ExchangeError::PoisonError(format!(
                "Error getting read lock: {}",
                e
            )));
        }
    };

    let message = ExchangeMessage {
        from: from.to_string(),
        message: message.to_string(),
    };

    exchange.tx.send(message).map_err(|e| {
        tracing::error!("Error sending message: {}", e);
        ExchangeError::SendError(format!("Error sending message: {}", e))
    })
}
