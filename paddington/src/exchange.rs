// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::RwLock;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinSet;

use crate::command::Command;
use crate::connection::Connection;
use crate::error::Error;
use crate::message::Message;

// We use the singleton pattern for the exchange data, as there can only
// be one in the program, and this will let us expose the exchange functions
// directly
static SINGLETON_EXCHANGE: Lazy<RwLock<Exchange>> = Lazy::new(|| RwLock::new(Exchange::new()));

#[macro_export]
macro_rules! async_message_handler {(
    $( #[$attr:meta] )* // includes doc strings
    $pub:vis
    async
    fn $fname:ident ( $($args:tt)* ) $(-> $Ret:ty)?
    {
        $($body:tt)*
    }
) => (
    $( #[$attr] )*
    #[allow(unused_parens)]
    $pub
    fn $fname ( $($args)* ) -> ::std::pin::Pin<::std::boxed::Box<
        dyn Send + ::std::future::Future<Output = ($($Ret)?)>
    >>
    {
        Box::pin(async move { $($body)* })
    }
)}

type AsyncMessageHandler = fn(
    Message,
) -> Pin<
    Box<
        dyn Future<Output = Result<(), Error>> // future API / pollable
            + Send, // required by non-single-threaded executors
    >,
>;

async_message_handler! {
    async fn default_message_handler(message: Message) -> Result<(), Error>
    {
        tracing::info!(
            "Default handler received: {}", message);

        Ok(())
    }
}

pub struct Exchange {
    name: String,
    connections: HashMap<String, Connection>,
    tx: UnboundedSender<Message>,
    // handler holds object that implements the MessageHandler trait
    handler: Option<AsyncMessageHandler>,

    // active watchdog checks - ensures that we don't flood the exchange
    // if a connection flaps
    watchdogs: Arc<Mutex<HashSet<String>>>,
}

impl Default for Exchange {
    fn default() -> Self {
        Self::new()
    }
}

async fn event_loop(mut rx: UnboundedReceiver<Message>) -> Result<(), Error> {
    let mut workers = JoinSet::new();

    static MAX_WORKERS: usize = 10;

    while let Some(mut message) = rx.recv().await {
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

        let (handler, name) = match SINGLETON_EXCHANGE.read() {
            Ok(exchange) => (exchange.handler, exchange.name.clone()),
            Err(e) => {
                return Err(Error::Poison(format!("Error getting read lock: {}", e)));
            }
        };

        let handler = handler.unwrap_or(default_message_handler);

        // it is only now that we know who is receiving the message
        message.set_recipient(&name);

        workers.spawn(async move {
            handler(message).await.unwrap_or_else(|e| {
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
            name: "".to_string(),
            connections: HashMap::new(),
            tx,
            handler: None,
            watchdogs: Arc::new(Mutex::new(HashSet::new())),
        }
    }
}

pub async fn set_name(name: &str) -> Result<(), Error> {
    let mut exchange = match SINGLETON_EXCHANGE.write() {
        Ok(exchange) => exchange,
        Err(e) => {
            return Err(Error::Poison(format!("Error getting write lock: {}", e)));
        }
    };

    exchange.name = name.to_string();

    Ok(())
}

pub async fn set_handler(handler: AsyncMessageHandler) -> Result<(), Error> {
    let mut exchange = match SINGLETON_EXCHANGE.write() {
        Ok(exchange) => exchange,
        Err(e) => {
            return Err(Error::Poison(format!("Error getting write lock: {}", e)));
        }
    };

    exchange.handler = Some(handler);

    Ok(())
}

fn get_recipient(message: &Message) -> String {
    format!("{}@{}", message.recipient(), message.zone())
}

fn get_key(connection: &Connection) -> String {
    format!("{}@{}", connection.name(), connection.zone())
}

pub async fn unregister(connection: &Connection) -> Result<(), Error> {
    let name = connection.name();

    if name.is_empty() {
        return Err(Error::UnnamedConnection(
            "Connection must have a name".to_string(),
        ));
    }

    let zone = connection.zone();

    if zone.is_empty() {
        return Err(Error::UnnamedConnection(
            "Connection must have a zone".to_string(),
        ));
    }

    let mut exchange = match SINGLETON_EXCHANGE.write() {
        Ok(exchange) => exchange,
        Err(e) => {
            return Err(Error::Poison(format!("Error getting write lock: {}", e)));
        }
    };

    let key = get_key(connection);

    if exchange.connections.contains_key(&key) {
        exchange.connections.remove(&key);
    }

    Ok(())
}

pub async fn register(connection: Connection) -> Result<(), Error> {
    let name = connection.name();
    let zone = connection.zone();

    if name.is_empty() {
        return Err(Error::UnnamedConnection(
            "Connection must have a name".to_string(),
        ));
    }

    if zone.is_empty() {
        return Err(Error::UnnamedConnection(
            "Connection must have a zone".to_string(),
        ));
    }

    let mut exchange = match SINGLETON_EXCHANGE.write() {
        Ok(exchange) => exchange,
        Err(e) => {
            return Err(Error::Poison(format!("Error getting write lock: {}", e)));
        }
    };

    let key = get_key(&connection);

    if exchange.connections.contains_key(&key) {
        return Err(Error::InvalidPeer(format!(
            "Connection {} already exists",
            key
        )));
    }

    exchange.connections.insert(key, connection);
    Ok(())
}

pub async fn send(message: Message) -> Result<(), Error> {
    let connection = match SINGLETON_EXCHANGE.read() {
        Ok(exchange) => exchange,
        Err(e) => {
            return Err(Error::Poison(format!("Error getting read lock: {}", e)));
        }
    }
    .connections
    .get(&get_recipient(&message))
    .cloned();

    if let Some(connection) = connection {
        connection.send_message(message.payload()).await?;
        Ok(())
    } else {
        Err(Error::UnnamedConnection(format!(
            "Connection {} not found",
            message.recipient()
        )))
    }
}

pub fn received(message: Message) -> Result<(), Error> {
    let exchange = match SINGLETON_EXCHANGE.read() {
        Ok(exchange) => exchange,
        Err(e) => {
            return Err(Error::Poison(format!("Error getting read lock: {}", e)));
        }
    };

    exchange.tx.send(message).map_err(|e| {
        tracing::error!("Error sending message: {}", e);
        Error::Send(format!("Error sending message: {}", e))
    })
}

pub async fn watchdog(peer: &str, zone: &str) -> Result<(), Error> {
    let name = format!("{}@{}", peer, zone);

    let connection = match SINGLETON_EXCHANGE.read() {
        Ok(exchange) => exchange,
        Err(e) => {
            return Err(Error::Poison(format!("Error getting read lock: {}", e)));
        }
    }
    .connections
    .get(&name)
    .cloned();

    if let Some(mut connection) = connection {
        tracing::debug!("Sending watchdog to {}", name);
        connection.watchdog().await?;
        tracing::debug!("Sent watchdog to {}", name);

        // make sure we are the only watchdog for this connection
        let watchdogs = match SINGLETON_EXCHANGE.read() {
            Ok(exchange) => exchange.watchdogs.clone(),
            Err(e) => {
                tracing::error!("Error getting read lock: {}", e);
                return Err(Error::Poison(format!("Error getting read lock: {}", e)));
            }
        };

        tracing::debug!("Checking watchdogs for {}", name);

        match watchdogs.lock() {
            Ok(mut watchdogs) => {
                if watchdogs.contains(&name) {
                    tracing::warn!("Watchdog already active for {} - skipping", name);
                    return Ok(());
                }

                watchdogs.insert(name.clone());
            }
            Err(e) => {
                return Err(Error::Poison(format!("Error getting lock: {}", e)));
            }
        };

        // wait 27 seconds and then send the watchdog message again
        tracing::debug!("Waiting for 27 seconds before sending watchdog again");
        tokio::time::sleep(tokio::time::Duration::from_secs(27)).await;
        tracing::debug!("Checking watchdogs for {} again...", name);

        // remove the entry for this connection - other's should be able to send
        match watchdogs.lock() {
            Ok(mut watchdogs) => {
                tracing::debug!("Removing watchdog for {}", name);
                watchdogs.remove(&name);
            }
            Err(e) => {
                tracing::error!("Error getting lock: {}", e);
                return Err(Error::Poison(format!("Error getting lock: {}", e)));
            }
        }

        tracing::debug!("Sending watchdog to {} again", name);
        match received(Command::watchdog(peer, zone).into()) {
            Ok(_) => {
                tracing::debug!("Sent watchdog to {} again", name);
            }
            Err(e) => {
                tracing::error!("Error sending watchdog message to {}: {}", name, e);
                match connection.disconnect().await {
                    Ok(_) => {}
                    Err(e) => {
                        tracing::error!("Error disconnecting {}: {}", name, e);
                    }
                }
            }
        }

        tracing::debug!("End of watchdog for {}", name);

        Ok(())
    } else {
        Err(Error::UnnamedConnection(format!(
            "Connection {}@{} not found",
            peer, zone
        )))
    }
}

pub async fn disconnect(peer: &str, zone: &str) -> Result<(), Error> {
    let connection = match SINGLETON_EXCHANGE.read() {
        Ok(exchange) => exchange,
        Err(e) => {
            return Err(Error::Poison(format!("Error getting read lock: {}", e)));
        }
    }
    .connections
    .get(&format!("{}@{}", peer, zone))
    .cloned();

    if let Some(mut connection) = connection {
        connection.disconnect().await?;
        Ok(())
    } else {
        Err(Error::UnnamedConnection(format!(
            "Connection {} not found",
            peer
        )))
    }
}
