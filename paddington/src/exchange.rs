// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Context;
use anyhow::Error as AnyError;
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::Mutex as TokioMutex;

use crate::connection::{Connection, ConnectionError};

#[derive(Error, Debug)]
pub enum ExchangeError {
    #[error("{0}")]
    AnyError(#[from] AnyError),

    #[error("{0}")]
    ConnectionError(#[from] ConnectionError),

    #[error("{0}")]
    UnnamedConnectionError(String),

    #[error("Unknown error")]
    Unknown,
}

#[derive(Debug, Clone)]
pub struct Exchange {
    connections: Arc<TokioMutex<HashMap<String, Connection>>>,
    tx: Option<Arc<UnboundedSender<String>>>,
}

impl Default for Exchange {
    fn default() -> Self {
        Self::new()
    }
}

async fn run_work_queue(mut rx: UnboundedReceiver<String>) {
    tracing::info!("Starting work queue");

    while let Some(message) = rx.recv().await {
        tracing::info!("Received message: {}", message);
    }
}

impl Exchange {
    pub fn new() -> Self {
        let (tx, rx) = unbounded_channel::<String>();

        tokio::spawn(run_work_queue(rx));

        Self {
            connections: Arc::new(TokioMutex::new(HashMap::new())),
            tx: Some(Arc::new(tx)),
        }
    }

    pub async fn register(&self, connection: Connection) -> Result<(), ExchangeError> {
        let name = connection.name().unwrap_or_default();

        if name.is_empty() {
            return Err(ExchangeError::UnnamedConnectionError(
                "Connection must have a name".to_string(),
            ));
        }

        let mut connections = self.connections.lock().await;

        let key = name.clone();

        if connections.contains_key(&key) {
            return Err(ExchangeError::UnnamedConnectionError(format!(
                "Connection {} already exists",
                name
            )));
        }

        connections.insert(key, connection);
        Ok(())
    }

    pub async fn send(&self, name: &str, message: &str) -> Result<(), ExchangeError> {
        let connection = self.connections.lock().await.get(name).cloned();

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

    pub fn post(&self, message: &str) -> Result<(), ExchangeError> {
        tracing::info!("Posting message: {}", message);

        let tx = self.tx.as_ref().ok_or_else(|| {
            tracing::warn!("No work queue to send message to!");
            ConnectionError::InvalidPeer("No work queue to send message to!".to_string())
        })?;

        tx.send(message.to_string())
            .with_context(|| "Error sending job to work queue")?;

        Ok(())
    }
}
