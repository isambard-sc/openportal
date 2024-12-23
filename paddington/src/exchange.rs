// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::RwLock;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinSet;

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
