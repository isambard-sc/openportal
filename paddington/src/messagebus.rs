// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Error as AnyError;
use thiserror::Error;

use crate::connection::{Connection, ConnectionError};

#[derive(Error, Debug)]
pub enum MessageBusError {
    #[error("{0}")]
    AnyError(#[from] AnyError),

    #[error("{0}")]
    ConnectionError(#[from] ConnectionError),

    #[error("Unknown error")]
    Unknown,
}

#[derive(Error, Debug)]
struct MessageBus {
    connections: Arc<Mutex<HashMap<String, Connection>>>,
}

impl MessageBus {
    pub fn new() -> Self {
        Self {
            connections: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn add_connection(&self, connection: Connection) -> Result<(), MessageBusError> {
        let mut connections = self.connections.lock().await;
        connections.insert(connection.id.clone(), connection);
        Ok(())
    }

    pub async fn remove_connection(&self, id: &str) -> Result<(), MessageBusError> {
        let mut connections = self.connections.lock().await;
        connections.remove(id);
        Ok(())
    }

    pub async fn get_connection(&self, id: &str) -> Result<Connection, MessageBusError> {
        let connections = self.connections.lock().await;
        connections.get(id).cloned().ok_or(MessageBusError::Unknown)
    }

    pub async fn send_message(&self, id: &str, message: &str) -> Result<(), MessageBusError> {
        let connection = self.get_connection(id).await?;
        connection.send_message(message).await?;
        Ok(())
    }
}
