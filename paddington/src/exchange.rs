// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Error as AnyError;
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;
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
}

impl Default for Exchange {
    fn default() -> Self {
        Self::new()
    }
}

impl Exchange {
    pub fn new() -> Self {
        Self {
            connections: Arc::new(TokioMutex::new(HashMap::new())),
        }
    }

    pub async fn add_connection(&self, connection: Connection) -> Result<(), ExchangeError> {
        let name = connection.name().unwrap_or_default();

        if name.is_empty() {
            return Err(ExchangeError::UnnamedConnectionError(
                "Connection must have a name".to_string(),
            ));
        }

        let mut connections = self.connections.lock().await;
        connections.insert(name.clone(), connection);
        Ok(())
    }

    pub async fn send_message(&self, name: &str, message: &str) -> Result<(), ExchangeError> {
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
}
