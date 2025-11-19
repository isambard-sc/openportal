// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinSet;

use crate::command::Command;
use crate::connection::Connection;
use crate::connection::StandbyStatus;
use crate::error::Error;
use crate::message::Message;

///
/// Global flag indicating whether a soft restart is in progress
/// When true, new connections should be rejected
///
static SOFT_RESTART_IN_PROGRESS: Lazy<AtomicBool> = Lazy::new(|| AtomicBool::new(false));

///
/// Check if a soft restart is currently in progress
///
pub fn is_soft_restart_in_progress() -> bool {
    SOFT_RESTART_IN_PROGRESS.load(Ordering::Acquire)
}

///
/// RAII guard that sets the soft restart flag on creation and clears it on drop
/// This ensures the flag is always cleared even if the restart function panics
///
pub struct SoftRestartGuard;

impl SoftRestartGuard {
    pub fn new() -> Self {
        SOFT_RESTART_IN_PROGRESS.store(true, Ordering::Release);
        tracing::debug!("Soft restart guard acquired - blocking new connections");
        SoftRestartGuard
    }
}

impl Default for SoftRestartGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for SoftRestartGuard {
    fn drop(&mut self) {
        SOFT_RESTART_IN_PROGRESS.store(false, Ordering::Release);
        tracing::debug!("Soft restart guard released - accepting connections again");
    }
}

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

    // whether or not we are a secondary server
    is_secondary: bool,

    // a hash counting the number of standby connections
    // per peer
    standby_peers: HashMap<String, u32>,
}

impl Default for Exchange {
    fn default() -> Self {
        Self::new()
    }
}

async fn increment_standby_count(name: &str, zone: &str) {
    let key = get_key_from_str(name, zone);

    let mut exchange = match SINGLETON_EXCHANGE.write() {
        Ok(exchange) => exchange,
        Err(e) => {
            tracing::error!("Error getting write lock: {}", e);
            return;
        }
    };

    let count = exchange.standby_peers.entry(key).or_insert(0);
    *count += 1;
    tracing::debug!("Incremented standby count for {}@{}: {}", name, zone, count);
}

async fn decrement_standby_count(name: &str, zone: &str) {
    let key = get_key_from_str(name, zone);

    let mut exchange = match SINGLETON_EXCHANGE.write() {
        Ok(exchange) => exchange,
        Err(e) => {
            tracing::error!("Error getting write lock: {}", e);
            return;
        }
    };

    if let Some(count) = exchange.standby_peers.get(&key) {
        let new_count = count.saturating_sub(1);

        if new_count == 0 {
            exchange.standby_peers.remove(&key);
        } else {
            exchange.standby_peers.insert(key.clone(), new_count);
        }

        tracing::debug!(
            "Decremented standby count for {}@{}: {}",
            name,
            zone,
            new_count
        );
    } else {
        tracing::debug!("No standby count for {}@{}", name, zone);
    }
}

fn get_standby_count(exchange: &Exchange, name: &str, zone: &str) -> u32 {
    let key = get_key_from_str(name, zone);
    *exchange.standby_peers.get(&key).unwrap_or(&0)
}

#[derive(Default, Clone)]
pub struct StandbyWaiter {
    name: String,
    zone: String,
    dropped: bool,
}

impl StandbyWaiter {
    pub fn new(name: &str, zone: &str) -> Self {
        Self {
            name: name.to_string(),
            zone: zone.to_string(),
            dropped: false,
        }
    }

    async fn terminate(&self) {
        tracing::debug!("Terminating StandbyWaiter: {}@{}", self.name, self.zone);
        decrement_standby_count(&self.name, &self.zone).await;
        tracing::debug!("StandbyWaiter terminated: {}@{}", self.name, self.zone);
    }
}

impl Drop for StandbyWaiter {
    fn drop(&mut self) {
        tracing::debug!("Dropping StandbyWaiter: {}@{}", self.name, self.zone);

        if !self.dropped {
            let mut this = StandbyWaiter::default();
            std::mem::swap(self, &mut this);
            this.dropped = true;
            tokio::spawn(async move { this.terminate().await });
        }
    }
}

async fn event_loop(mut rx: UnboundedReceiver<Message>) -> Result<(), Error> {
    let mut workers = JoinSet::new();

    let mut last_logged_update = chrono::Utc::now();
    let mut last_logged_count: i64 = 0;

    while let Some(mut message) = rx.recv().await {
        // process and spawn a new task to handle the message first...
        let (handler, name) = match SINGLETON_EXCHANGE.read() {
            Ok(exchange) => (exchange.handler, exchange.name.clone()),
            Err(e) => {
                tracing::error!("Error getting read lock: {}", e);
                continue;
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

        // now take the opportunity to try to join any finished workers
        while let Some(result) = workers.try_join_next() {
            match result {
                Ok(()) => {}
                Err(e) => {
                    tracing::error!("Error processing message: {}", e);
                }
            }
        }

        if (last_logged_count - workers.len() as i64).abs() >= 10
            || last_logged_update
                .signed_duration_since(chrono::Utc::now())
                .num_seconds()
                >= 60
        {
            last_logged_count = workers.len() as i64;
            last_logged_update = chrono::Utc::now();
            tracing::info!("Number of workers: {}", workers.len());
        }

        if workers.len() > 1024 {
            tracing::warn!(
                "High number of workers: {}. Attempting to reduce...",
                workers.len()
            );

            let start_reaping = chrono::Utc::now();
            let mut last_update = start_reaping;

            while workers.len() > 768 {
                if let Some(result) = workers.try_join_next() {
                    match result {
                        Ok(()) => {}
                        Err(e) => {
                            tracing::error!("Error processing message: {}", e);
                        }
                    }
                } else {
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

                    // log a message every 10 seconds
                    if last_update
                        .signed_duration_since(chrono::Utc::now())
                        .num_seconds()
                        >= 10
                    {
                        let count = workers.len();
                        tracing::warn!(
                            "It has been {} seconds and there are still a high number of workers: {}. Attempting to reduce...",
                            start_reaping.signed_duration_since(last_update).num_seconds(),
                            count
                        );
                        last_update = chrono::Utc::now();
                    }
                }

                if start_reaping
                    .signed_duration_since(chrono::Utc::now())
                    .num_seconds()
                    >= 300
                {
                    tracing::error!(
                        "It has been {} seconds since the last log message and there are still a high number of workers: {}.",
                        start_reaping.signed_duration_since(last_logged_update).num_seconds(),
                        workers.len()
                    );
                    tracing::error!("Something has gone wrong, so we will now abort all tasks and restart event processing.");

                    workers.abort_all();
                    workers.detach_all();

                    tracing::error!(
                        "Aborted all tasks. Number of workers is now: {}",
                        workers.len()
                    );
                }
            }
        }
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
            is_secondary: false,
            standby_peers: HashMap::new(),
        }
    }
}

#[allow(dead_code)]
pub async fn set_is_primary() -> Result<(), Error> {
    let mut exchange = match SINGLETON_EXCHANGE.write() {
        Ok(exchange) => exchange,
        Err(e) => {
            return Err(Error::Poison(format!("Error getting write lock: {}", e)));
        }
    };

    exchange.is_secondary = false;

    Ok(())
}

#[allow(dead_code)]
pub async fn set_is_secondary() -> Result<(), Error> {
    let connections = match SINGLETON_EXCHANGE.write() {
        Ok(mut exchange) => {
            if exchange.is_secondary {
                None
            } else {
                exchange.is_secondary = true;
                Some(exchange.connections.clone())
            }
        }
        Err(e) => {
            return Err(Error::Poison(format!("Error getting write lock: {}", e)));
        }
    };

    // possible race conditions here, if we are quickly re-enabled
    // as the primary - but these will be sorted out when everything
    // reconnects

    if let Some(mut connections) = connections {
        // if we are changing the state from primary to secondary
        // we should disconnect all connections
        for connection in connections.values_mut() {
            if let Err(e) = connection.disconnect().await {
                tracing::error!("Error disconnecting connection: {}", e);
            }
        }
    }

    Ok(())
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

fn get_key_from_str(name: &str, zone: &str) -> String {
    format!("{}@{}", name, zone)
}

fn get_recipient(message: &Message) -> String {
    get_key_from_str(message.recipient(), message.zone())
}

fn get_key(connection: &Connection) -> String {
    get_key_from_str(&connection.name(), &connection.zone())
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

pub async fn get_standby_waiter(name: &str, zone: &str) -> Result<Arc<StandbyWaiter>, Error> {
    increment_standby_count(name, zone).await;
    Ok(Arc::new(StandbyWaiter::new(name, zone)))
}

pub async fn check_standby(name: &str, zone: &str) -> Result<StandbyStatus, Error> {
    let exchange = match SINGLETON_EXCHANGE.read() {
        Ok(exchange) => exchange,
        Err(e) => {
            return Err(Error::Poison(format!("Error getting read lock: {}", e)));
        }
    };

    // if the number of standby connections is greater than 16 then raise
    // an error to terminate the connection - this prevents a DoS attack
    if get_standby_count(&exchange, name, zone) > 16 {
        return Err(Error::TooManyStandbyConnections(format!(
            "Too many standby connections for {}@{}",
            name, zone
        )));
    }

    if exchange.is_secondary {
        return Ok(StandbyStatus::secondary_server());
    }

    if exchange
        .connections
        .contains_key(&get_key_from_str(name, zone))
    {
        // there is a connection with this name and zone, so any more
        // connections will become secondary
        Ok(StandbyStatus::secondary_client())
    } else {
        // there isn't, so this could be a primary connection
        // (subject to the race condition - it will only become primary
        //  if it wins the race)
        Ok(StandbyStatus::primary())
    }
}

async fn locked_register(connection: Connection) -> Result<bool, Error> {
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

    // go through and see if we have any standby connections that
    // are for keys that are alphabetically more than this one.
    // If so, then we need to disconnect them all, as this is a
    // standby-only agent
    let is_standby_only = exchange
        .standby_peers
        .iter()
        .any(|(k, v)| *v > 0 && k > &key);

    if !is_standby_only {
        exchange.connections.insert(key, connection);
    }

    Ok(is_standby_only)
}

pub async fn register(connection: Connection) -> Result<(), Error> {
    let is_standby_only = locked_register(connection.clone()).await?;

    if is_standby_only {
        let mut connections = match SINGLETON_EXCHANGE.read() {
            Ok(exchange) => exchange.connections.clone(),
            Err(e) => {
                return Err(Error::Poison(format!("Error getting read lock: {}", e)));
            }
        };

        // potential race condition here, but this will be resolved when
        // all of the peers disconnect and reconnect

        for connection in connections.values_mut() {
            match connection.disconnect().await {
                Ok(_) => {
                    tracing::debug!("Disconnected connection: {}", connection.name());
                }
                Err(e) => {
                    tracing::error!("Error disconnecting connection: {}", e);
                }
            }
        }

        // NOTE THAT WE DON'T YET REMOVE ANY SERVERS - THEY COULD STILL
        // BE LISTENING FOR CONNECTIONS. WE DO NEED TO WORK OUT HOW
        // TO HANDLE HA FOR SERVERS - THIS IS A WORK IN PROGRESS

        // PROBABLY THE BEST ROUTE IS TO HAVE A CONTROL MESSAGE WE SEND
        // OURSELVES THAT SWITCHES US OVER TO SECONDARY MODE - THIS WOULD
        // AVOID HAVING TO DISCONNECT EVERYTHING ABOVE - CHALLENGE IS
        // HOW TO SEND A MESSAGE TO SWITCH US BACK TO PRIMARY MODE

        return Err(Error::PeerIsSecondary(
            "This peer is fully secondary".to_string(),
        ));
    }

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
    let name = get_key_from_str(peer, zone);

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
            "Connection {} not found",
            name
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
    .get(&get_key_from_str(peer, zone))
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
