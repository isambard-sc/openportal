// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Context;
use anyhow::Error as AnyError;

use futures::{SinkExt, StreamExt};
use futures_channel::mpsc::{unbounded, UnboundedSender};
use futures_util::{future, pin_mut, stream::TryStreamExt};
use secrecy::ExposeSecret;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::env;
use std::fmt::Display;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::vec::Vec;
use tokio::net::TcpStream;
use tokio::sync::Mutex as TokioMutex;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::protocol::Message as TokioMessage;
use tungstenite::client::IntoClientRequest;
use tungstenite::handshake::server::{
    ErrorResponse as HandshakeErrorResponse, Request as HandshakeRequest,
    Response as HandshakeResponse,
};

use crate::command::Command;
use crate::config::{ClientConfig, PeerConfig, ServiceConfig};
use crate::crypto::{random_bytes, Key, Salt, SecretKey, KEY_SIZE};
use crate::error::Error;
use crate::exchange;
use crate::message::Message;

#[derive(Debug, Clone, PartialEq)]
enum ConnectionStatus {
    None,
    Connecting,
    Connected,
    Disconnected,
    Error,
}

#[derive(Debug, Clone)]
struct ConnectionState {
    status: ConnectionStatus,
    last_activity: chrono::DateTime<chrono::Utc>,
}

impl Default for ConnectionState {
    fn default() -> Self {
        ConnectionState {
            status: ConnectionStatus::None,
            last_activity: chrono::Utc::now(),
        }
    }
}

impl ConnectionState {
    fn set_error(&mut self) {
        self.status = ConnectionStatus::Error;
        self.last_activity = chrono::Utc::now();
    }

    fn set_disconnected(&mut self) {
        self.status = ConnectionStatus::Disconnected;
        self.last_activity = chrono::Utc::now();
    }

    fn set_connecting(&mut self) -> Result<(), Error> {
        if self.status == ConnectionStatus::None {
            self.status = ConnectionStatus::Connecting;
            self.last_activity = chrono::Utc::now();
            Ok(())
        } else {
            Err(Error::BusyLine(format!(
                "Connection is already {:?}",
                self.status
            )))
        }
    }

    fn set_connected(&mut self) -> Result<(), Error> {
        if self.status == ConnectionStatus::Connecting {
            self.status = ConnectionStatus::Connected;
            self.last_activity = chrono::Utc::now();
            Ok(())
        } else {
            Err(Error::BusyLine(format!(
                "Connection is not connecting - it is {:?}",
                self.status
            )))
        }
    }

    fn register_activity(&mut self) {
        self.last_activity = chrono::Utc::now();
    }
}

#[derive(Debug, Clone)]
pub struct Connection {
    state: Arc<StdMutex<ConnectionState>>,
    config: ServiceConfig,
    inner_key: Option<SecretKey>,
    outer_key: Option<SecretKey>,
    inner_key_salt: Option<Salt>,
    outer_key_salt: Option<Salt>,
    peer: Option<PeerConfig>,
    tx: Option<Arc<TokioMutex<UnboundedSender<TokioMessage>>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PeerDetails {
    name: String,
    zone: String,
    version: u32,
}

impl Display for PeerDetails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.name, self.zone)
    }
}

impl PeerDetails {
    fn new(name: &str, zone: &str) -> Self {
        // everything is currently version 1
        PeerDetails {
            name: name.to_string(),
            zone: zone.to_string(),
            version: 1,
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn zone(&self) -> &str {
        &self.zone
    }

    fn version(&self) -> u32 {
        self.version
    }
}

fn envelope_message<T>(
    message: T,
    inner_key: &SecretKey,
    outer_key: &SecretKey,
    inner_key_salt: &Salt,
    outer_key_salt: &Salt,
) -> Result<TokioMessage, AnyError>
where
    T: Serialize,
{
    // we will now generate per-message keys, using a
    // random additional info and the salts for this connection
    let inner_info = random_bytes(KEY_SIZE)?;
    let outer_info = random_bytes(KEY_SIZE)?;

    let inner_key = inner_key
        .expose_secret()
        .derive(inner_key_salt, Some(&inner_info))?;
    let outer_key = outer_key
        .expose_secret()
        .derive(outer_key_salt, Some(&outer_info))?;

    Ok(TokioMessage::text(
        hex::encode(&inner_info)
            + &hex::encode(&outer_info)
            + &outer_key
                .expose_secret()
                .encrypt(inner_key.expose_secret().encrypt(message)?)?,
    ))
}

fn deenvelope_message<T>(
    message: TokioMessage,
    inner_key: &SecretKey,
    outer_key: &SecretKey,
    inner_key_salt: &Salt,
    outer_key_salt: &Salt,
) -> Result<T, AnyError>
where
    T: DeserializeOwned,
{
    let message = message.to_text()?;

    if message.len() < 4 * KEY_SIZE + 2 {
        tracing::warn!("Message too short to de-envelop: {}", message.len());
        return Err(Error::Incompatible("Message too short to de-envelop".to_string()).into());
    }

    // the hex-encoded string is 2 times the number of bytes
    let inner_info = hex::decode(&message[0..(2 * KEY_SIZE)])?;
    let outer_info = hex::decode(&message[(2 * KEY_SIZE)..(4 * KEY_SIZE)])?;

    let inner_key = inner_key
        .expose_secret()
        .derive(inner_key_salt, Some(&inner_info))?;

    let outer_key = outer_key
        .expose_secret()
        .derive(outer_key_salt, Some(&outer_info))?;

    Ok(inner_key.expose_secret().decrypt::<T>(
        &outer_key
            .expose_secret()
            .decrypt::<String>(&message[(4 * KEY_SIZE)..])?,
    )?)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Handshake {
    session_key: SecretKey,
    engine: String,
    version: String,
}

impl Connection {
    pub fn new(config: ServiceConfig) -> Self {
        Connection {
            state: Arc::new(StdMutex::new(ConnectionState::default())),
            config,
            inner_key: None,
            outer_key: None,
            inner_key_salt: None,
            outer_key_salt: None,
            peer: None,
            tx: None,
        }
    }

    ///
    /// Return the name of the connection - this is the name of the peer
    /// that the connection is connected to.
    ///
    pub fn name(&self) -> String {
        self.peer.as_ref().unwrap_or(&PeerConfig::None).name()
    }

    ///
    /// Return the zone of the connection - both sides of the connection
    /// must agree on the same zone
    ///
    pub fn zone(&self) -> String {
        self.peer.as_ref().unwrap_or(&PeerConfig::None).zone()
    }

    ///
    /// Close the connection
    ///
    pub async fn disconnect(&mut self) -> Result<(), Error> {
        if let Some(tx) = self.tx.as_ref() {
            tracing::warn!(
                "Disconnecting connection to peer: {}@{}",
                self.name(),
                self.zone()
            );
            let mut tx = tx.lock().await;
            tx.close()
                .await
                .with_context(|| "Error closing connection")?;
        }

        Ok(())
    }

    ///
    /// Watchdog check the connection is still active
    ///
    pub async fn watchdog(&mut self) -> Result<(), Error> {
        let last_activity = match self.state.lock() {
            Ok(state) => state.last_activity,
            Err(e) => {
                tracing::warn!("Error getting last activity: {:?}", e);
                // return ok as we will check again
                return Ok(());
            }
        };

        if last_activity < chrono::Utc::now() - chrono::Duration::seconds(300) {
            tracing::warn!(
                "*WATCHDOG* Connection to peer: {}@{} has not been active for over 300 seconds - disconnecting",
                self.name(),
                self.zone()
            );

            match self.disconnect().await {
                Ok(_) => (),
                Err(e) => {
                    // only log this, as we are already in a watchdog
                    tracing::warn!("Error disconnecting connection: {:?}", e);
                }
            }
        }

        Ok(())
    }

    ///
    /// Send a message to the peer on the other end of the connection.
    ///
    pub async fn send_message(&self, message: &str) -> Result<(), Error> {
        if message.is_empty() {
            tracing::warn!("Empty message - not sending");
            return Ok(());
        }

        let tx = self.tx.as_ref().ok_or_else(|| {
            tracing::warn!("No connection to send message to!");
            Error::InvalidPeer("No connection to send message to!".to_string())
        })?;

        let mut tx = tx.lock().await;

        let inner_key = self.inner_key.as_ref().ok_or_else(|| {
            tracing::warn!("No inner key to send message with!");
            Error::InvalidPeer("No inner key to send message with!".to_string())
        })?;

        let outer_key = self.outer_key.as_ref().ok_or_else(|| {
            tracing::warn!("No outer key to send message with!");
            Error::InvalidPeer("No outer key to send message with!".to_string())
        })?;

        let inner_key_salt = self.inner_key_salt.as_ref().ok_or_else(|| {
            tracing::warn!("No inner key salt to send message with!");
            Error::InvalidPeer("No inner key salt to send message with!".to_string())
        })?;

        let outer_key_salt = self.outer_key_salt.as_ref().ok_or_else(|| {
            tracing::warn!("No outer key salt to send message with!");
            Error::InvalidPeer("No outer key salt to send message with!".to_string())
        })?;

        tx.send(envelope_message(
            message.to_string(),
            inner_key,
            outer_key,
            inner_key_salt,
            outer_key_salt,
        )?)
        .await
        .with_context(|| "Error sending message to peer")?;

        // record the last time we successfully sent a message
        match self.state.lock() {
            Ok(mut state) => {
                state.register_activity();
            }
            Err(e) => {
                tracing::warn!("Error registering activity: {:?}", e);
            }
        }

        Ok(())
    }

    ///
    /// This function should be called by the handler of errors raised
    /// by the make_connection or handle_connection functions, when
    /// an error is detected. This sets the state of the connection
    /// to error (it will have been automatically closed)
    ///
    pub async fn set_error(&mut self) {
        self.closed_connection().await;

        match self.state.lock() {
            Ok(mut state) => {
                state.set_error();
            }
            Err(e) => {
                tracing::warn!("Error setting connection state to error: {:?}", e);
            }
        }

        self.tx = None;
        self.inner_key = None;
        self.outer_key = None;
        self.peer = None;
    }

    ///
    /// Internal function called to indicate that the connection has
    /// been correctly closed.
    ///
    async fn closed_connection(&mut self) {
        exchange::unregister(self)
            .await
            .with_context(|| {
                "Error unregistering connection with exchange. Ensure the connection is open."
            })
            .unwrap_or_else(|e| {
                tracing::error!("Error unregistering connection with exchange: {:?}", e);
            });

        match self.state.lock() {
            Ok(mut state) => {
                state.set_disconnected();
            }
            Err(e) => {
                tracing::warn!("Error setting connection state to disconnected: {:?}", e);
            }
        }
        self.tx = None;
        self.inner_key = None;
        self.outer_key = None;
        self.peer = None;
    }

    ///
    /// Call this function to initiate a client connection to the passed
    /// peer. This will initiate the connection and then enter an event
    /// loop to handle the sending and receiving of messages.
    ///
    pub async fn make_connection(&mut self, peer: &PeerConfig) -> Result<(), Error> {
        // first, check that the peer is a server
        let server = match peer {
            PeerConfig::Server(server) => server,
            _ => {
                tracing::warn!("Peer '{}' must be a server to make a connection.", peer);
                return Err(Error::InvalidPeer(
                    "Peer must be a server to make a connection.".to_string(),
                ));
            }
        };

        // now check that we aren't already handling a connection
        match self.state.lock() {
            Ok(mut state) => {
                state.set_connecting()?;
            }
            Err(e) => {
                tracing::warn!("Error setting connection state to connecting: {:?}", e);
                return Err(Error::BusyLine(
                    "Error setting connection state to connecting.".to_string(),
                ));
            }
        }

        // we now know we are the only ones handling a connection,
        // so it is safe to update the peer and keys

        // save the peer we are connecting to
        self.peer = Some(peer.clone());
        let peer_name = peer.name();
        let peer_zone = peer.zone();

        // create two salts for the connection
        let inner_key_salt = Salt::generate()?;
        let outer_key_salt = Salt::generate()?;

        let url = server.get_websocket_url()?.to_string();

        tracing::info!("Connecting to WebSocket at: {} - initiating handshake", url);

        // add the salts to the headers, xor'd with the server's keys
        // (just to keep them more secret)
        let mut request = url
            .clone()
            .into_client_request()
            .with_context(|| format!("Error creating client request for WebSocket at: {}", url))?;

        request.headers_mut().insert(
            "openportal-inner-salt",
            inner_key_salt
                .xor(server.outer_key().expose_secret())
                .to_string()
                .parse()
                .with_context(|| {
                    format!("Error parsing inner key salt for WebSocket at: {}", url)
                })?,
        );

        request.headers_mut().insert(
            "openportal-outer-salt",
            outer_key_salt
                .xor(server.inner_key().expose_secret())
                .to_string()
                .parse()
                .with_context(|| {
                    format!("Error parsing outer key salt for WebSocket at: {}", url)
                })?,
        );

        let socket = match connect_async(request).await {
            Ok((socket, _)) => socket,
            Err(e) => {
                tracing::warn!("Error connecting to WebSocket at: {} - {:?}", url, e);
                self.set_error().await;
                return Err(Error::Any(e.into()));
            }
        };

        // Split the WebSocket stream into incoming and outgoing parts
        let (mut outgoing, mut incoming) = socket.split();

        // the client generates a handshake that contains the new session outer key,
        // the name of its comms engine and version, and sends this to the server
        // using the pre-shared client/server inner and outer keys
        let outer_key = Key::generate();

        let handshake = Handshake {
            session_key: outer_key.clone(),
            engine: env!("CARGO_PKG_NAME").to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        };

        let message = match envelope_message(
            handshake,
            &server.inner_key(),
            &server.outer_key(),
            &inner_key_salt,
            &outer_key_salt,
        ) {
            Ok(message) => message,
            Err(e) => {
                tracing::warn!("Error enveloping message: {:?}", e);
                self.set_error().await;
                return Err(e.into());
            }
        };

        if let Err(r) = outgoing.send(message).await {
            self.set_error().await;
            return Err(Error::Any(r.into()));
        }

        // receive the response
        let response = match incoming.next().await {
            Some(response) => response,
            None => {
                tracing::warn!("Error receiving response from peer. Ensure the peer is valid and the connection is open.");
                self.set_error().await;
                return Err(Error::InvalidPeer(
                    "Error receiving response from peer. Ensure the peer is valid and the connection is open.".to_string(),
                ));
            }
        };

        let response = match response {
            Ok(response) => response,
            Err(e) => {
                tracing::warn!("Error receiving response from peer: {:?}", e);
                self.set_error().await;
                return Err(Error::Any(e.into()));
            }
        };

        // the server has generated a new session inner key, and has put this into
        // a handshake with its comms engine and version, and sent that
        // wrapped using the client/server inner key and the new session outer key
        let handshake: Handshake = match deenvelope_message(
            response,
            &server.inner_key(),
            &outer_key,
            &inner_key_salt,
            &outer_key_salt,
        ) {
            Ok(inner_key) => inner_key,
            Err(e) => {
                tracing::warn!("Error de-enveloping message: {:?}", e);
                self.set_error().await;
                return Err(e.into());
            }
        };

        let inner_key = handshake.session_key.clone();

        // the final step is for the client to send the server its PeerDetails,
        // and for the server to respond. These should match up with
        // what we expect
        let peer_details = PeerDetails::new(&self.config.name(), &peer_zone);

        let message = match envelope_message(
            peer_details,
            &inner_key,
            &outer_key,
            &inner_key_salt,
            &outer_key_salt,
        ) {
            Ok(message) => message,
            Err(e) => {
                tracing::warn!("Error enveloping message: {:?}", e);
                self.set_error().await;
                return Err(e.into());
            }
        };

        if let Err(r) = outgoing.send(message).await {
            self.set_error().await;
            return Err(Error::Any(r.into()));
        }

        // receive the response
        let response = match incoming.next().await {
            Some(response) => response,
            None => {
                tracing::warn!("Error receiving response from peer. Ensure the peer is valid and the connection is open.");
                self.set_error().await;
                return Err(Error::InvalidPeer(
                    "Error receiving response from peer. Ensure the peer is valid and the connection is open.".to_string(),
                ));
            }
        };

        let response = match response {
            Ok(response) => response,
            Err(e) => {
                tracing::warn!("Error receiving response from peer: {:?}", e);
                self.set_error().await;
                return Err(Error::Any(e.into()));
            }
        };

        // the response should be the server's peer details, which should match what we expect
        let peer_details: PeerDetails = match deenvelope_message(
            response,
            &inner_key,
            &outer_key,
            &inner_key_salt,
            &outer_key_salt,
        ) {
            Ok(peer_details) => peer_details,
            Err(e) => {
                tracing::warn!("Error de-enveloping message: {:?}", e);
                self.set_error().await;
                return Err(e.into());
            }
        };

        tracing::info!(
            "Connecting to peer {}, comms engine {} version {}",
            peer_details,
            handshake.engine,
            handshake.version
        );

        // eventually we could check the engine and version here,
        // and do different things based on compatibility, but not for now

        if peer_details.name() != peer_name {
            tracing::warn!(
                "Peer name does not match expected name: {} != {}",
                peer_details.name(),
                peer_name
            );
            self.set_error().await;
            return Err(Error::InvalidPeer(
                "Peer name does not match expected name.".to_string(),
            ));
        }

        if peer_details.zone() != peer_zone {
            tracing::warn!(
                "Peer zone does not match expected zone: {} != {}",
                peer_details.zone(),
                peer_zone,
            );
            self.set_error().await;
            return Err(Error::InvalidPeer(
                "Peer zone does not match expected zone.".to_string(),
            ));
        }

        if peer_details.version() != 1 {
            tracing::warn!(
                "Peer version does not match expected version: {} != 1",
                peer_details.version()
            );
            self.set_error().await;
            return Err(Error::InvalidPeer(
                "Peer version does not match expected version.".to_string(),
            ));
        }

        tracing::info!("Handshake complete!");

        // we can now save these keys as the new session keys for the connection
        self.inner_key = Some(inner_key.clone());
        self.outer_key = Some(outer_key.clone());
        self.inner_key_salt = Some(inner_key_salt.clone());
        self.outer_key_salt = Some(outer_key_salt.clone());

        // finally, we need to create a new channel for sending messages
        let (tx, rx) = unbounded::<TokioMessage>();

        // save this with the connection
        self.tx = Some(Arc::new(TokioMutex::new(tx)));

        // and we can register this connection - need to unregister when disconnected
        match exchange::register(self.clone()).await {
            Ok(_) => (),
            Err(e) => {
                tracing::warn!("Error registering connection with exchange: {:?}", e);
                self.set_error().await;
                return Err(Error::Any(e.into()));
            }
        };

        // we have now connected :-)
        match self.state.lock() {
            Ok(mut state) => {
                state.set_connected()?;
            }
            Err(e) => {
                tracing::warn!("Error setting connection state to connected: {:?}", e);
                return Err(Error::BusyLine(
                    "Error setting connection state to connected.".to_string(),
                ));
            }
        }

        // and now we can start the message handling loop - make sure to
        // handle the sending of messages to others
        let received_from_peer = incoming.try_for_each(|msg| {
            if msg.is_empty() {
                // this may happen, e.g. if the connection is closed
                // This can be safely ignored
                return future::ok(());
            }

            // we need to deenvelope the message
            let msg: String = match deenvelope_message(
                msg,
                &inner_key,
                &outer_key,
                &inner_key_salt,
                &outer_key_salt,
            ) {
                Ok(msg) => msg,
                Err(e) => {
                    tracing::warn!("Error de-enveloping message: {:?}", e);
                    return future::ok(());
                }
            };

            exchange::received(Message::received_from(&peer_name, &peer_zone, &msg))
                .unwrap_or_else(|e| {
                    tracing::warn!("Error handling message: {:?}", e);
                });

            // record the last time we successfully received a message
            match self.state.lock() {
                Ok(mut state) => {
                    state.register_activity();
                }
                Err(e) => {
                    tracing::warn!("Error registering activity: {:?}", e);
                }
            }

            future::ok(())
        });

        // handle messages that should be sent to the client (received locally
        // from other services that should be forwarded to the client via the
        // outgoing stream)
        let send_to_peer = rx.map(Ok).forward(outgoing);

        // now tell ourselves who has connected
        match exchange::received(
            Command::connected(
                &peer_name,
                &peer_zone,
                &handshake.engine,
                &handshake.version,
            )
            .into(),
        ) {
            Ok(_) => (),
            Err(e) => {
                tracing::warn!("Error triggering /connected control message: {:?}", e);
                self.set_error().await;
                return Err(Error::Any(e.into()));
            }
        }

        // now tell ourselves that we have received a watchdog message
        // from this peer - this will start a periodic watchdog check
        exchange::received(Command::watchdog(&peer_name, &peer_zone).into())
            .with_context(|| "Error triggering /watchdog control message")?;

        // finally, send a keepalive message to the peer - this will start
        // a ping-pong with the peer that should keep it open
        // (client sends, as the server should already be set up now)
        match exchange::send(Message::keepalive(&peer_name, &peer_zone)).await {
            Ok(_) => (),
            Err(e) => {
                tracing::warn!("Error sending keepalive message: {:?}", e);
                self.set_error().await;
                return Err(Error::Any(e.into()));
            }
        }

        pin_mut!(received_from_peer, send_to_peer);
        future::select(received_from_peer, send_to_peer).await;

        // we've exited, meaning that this connection is now closed
        self.closed_connection().await;

        Ok(())
    }

    ///
    /// Call this function to handle a new connection made from a client.
    /// This function will handle the handshake and then enter an event
    /// loop to handle the sending and receiving of messages.
    ///
    pub async fn handle_connection(&mut self, stream: TcpStream) -> Result<(), Error> {
        let service_name = self.config.name();

        if service_name.is_empty() {
            tracing::warn!("Service must have a name to handle a connection.");
            return Err(Error::InvalidPeer(
                "Service must have a name to handle a connection.".to_string(),
            ));
        }

        // check we aren't handling another connection
        match self.state.lock() {
            Ok(mut state) => {
                state.set_connecting()?;
            }
            Err(e) => {
                tracing::warn!("Error setting connection state to connecting: {:?}", e);
                return Err(Error::BusyLine(
                    "Error setting connection state to connecting.".to_string(),
                ));
            }
        }

        // we now know we are the only ones handling the connection,
        // and are safe to update the keys etc.

        let mut client_ip: std::net::IpAddr = stream
            .peer_addr()
            .with_context(|| "Error getting the peer address. Ensure the connection is open.")?
            .ip();

        let proxy_header = self.config.proxy_header();
        let mut proxy_client = None;

        let mut inner_key_salt: String = String::new();
        let mut outer_key_salt: String = String::new();

        let process_headers = |request: &HandshakeRequest,
                               response: HandshakeResponse|
         -> Result<HandshakeResponse, HandshakeErrorResponse> {
            if let Some(proxy_header) = proxy_header {
                if let Some(value) = request
                    .headers()
                    .get(proxy_header)
                    .and_then(|value| value.to_str().ok())
                {
                    proxy_client = Some(value.to_string());
                }
            }

            inner_key_salt = request
                .headers()
                .get("openportal-inner-salt")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();

            outer_key_salt = request
                .headers()
                .get("openportal-outer-salt")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();

            Ok(response)
        };

        let ws_stream = tokio_tungstenite::accept_hdr_async(stream, process_headers)
            .await
            .with_context(|| {
                format!(
                    "Error accepting WebSocket connection from: {}. Closing connection.",
                    client_ip
                )
            })?;

        let inner_key_salt: Salt = inner_key_salt
            .parse()
            .with_context(|| "Error parsing inner key salt")?;

        let outer_key_salt: Salt = outer_key_salt
            .parse()
            .with_context(|| "Error parsing outer key salt")?;

        if let Some(proxy_client) = proxy_client {
            tracing::info!("Proxy client: {:?}", proxy_client);
            client_ip = proxy_client
                .parse()
                .with_context(|| "Error parsing proxy client address")?;
        }

        // this doesn't need to be mutable any more
        let client_ip = client_ip;

        tracing::info!("Accepted connection from peer: {}", client_ip);

        let clients: Vec<ClientConfig> = self
            .config
            .clients()
            .iter()
            .filter(|client| client.matches(client_ip))
            .cloned()
            .collect();

        if clients.is_empty() {
            tracing::warn!("No matching peer found for address: {}", client_ip);
            return Err(Error::InvalidPeer(
                "No matching peer found for address.".to_string(),
            ));
        }

        // Split the WebSocket stream into incoming and outgoing parts
        let (mut outgoing, mut incoming) = ws_stream.split();

        // do the handshake with the client - the client should have sent an initial message
        // with the peer information
        let message = incoming
            .next()
            .await
            .ok_or_else(|| {
                tracing::warn!("No peer information received - closing connection.");
                Error::InvalidPeer("No peer information received - closing connection.".to_string())
            })?
            .unwrap_or_else(|_| TokioMessage::text(""));

        if message.is_empty() {
            tracing::warn!("No peer information received - closing connection.");
            return Err(Error::InvalidPeer(
                "No peer information received - closing connection.".to_string(),
            ));
        }

        // find a client that can de-envelope the message - this is the
        // client that we will be connecting to
        let clients: Vec<ClientConfig> = clients
            .iter()
            .filter(|client| {
                // note, could use
                // deenvelope_message::<SecretKey>(message.clone(), &client.inner_key, &client.outer_key).is_ok()
                // but then we would lose tracing messages - these are very helpful
                // to debug issues

                match deenvelope_message::<Handshake>(
                    message.clone(),
                    &client.inner_key(),
                    &client.outer_key(),
                    &inner_key_salt.xor(client.outer_key().expose_secret()),
                    &outer_key_salt.xor(client.inner_key().expose_secret()),
                ) {
                    Ok(_) => {
                        tracing::info!(
                            "Client {:?} authenticated for address: {}",
                            client.name(),
                            client_ip
                        );
                        true
                    }
                    Err(_) => false,
                }
            })
            .cloned()
            .collect();

        if clients.is_empty() {
            tracing::warn!(
                "No matching peer could authenticate for address: {}",
                client_ip
            );
            return Err(Error::InvalidPeer(
                "No matching peer could authenticate for address.".to_string(),
            ));
        }

        if clients.len() > 1 {
            tracing::warn!(
                "Multiple matching peers found for address: {} - \
                    {:?}. Ignoring all but the first...",
                client_ip,
                clients
            );
        }

        let peer = clients[0].clone();

        let peer_name = peer.name();
        let peer_zone = peer.zone();

        if peer_name.is_empty() {
            tracing::warn!("Peer must have a name to handle a connection.");
            return Err(Error::InvalidPeer(
                "Peer must have a name to handle a connection.".to_string(),
            ));
        }

        tracing::info!(
            "Initiating connection: {:?} <=> {:?}",
            service_name,
            peer_name
        );

        // we have found the right client to xor the salts
        let inner_key_salt = inner_key_salt.xor(peer.outer_key().expose_secret());
        let outer_key_salt = outer_key_salt.xor(peer.inner_key().expose_secret());

        // the peer has sent us the new session outer key that should be used,
        // wrapped in the client/server inner and outer keys
        let handshake = deenvelope_message::<Handshake>(
            message,
            &peer.inner_key(),
            &peer.outer_key(),
            &inner_key_salt,
            &outer_key_salt,
        )
        .with_context(|| "Error de-enveloping message - closing connection.")?;

        let outer_key = handshake.session_key.clone();

        let peer_engine = handshake.engine;
        let peer_version = handshake.version;

        // we will create a new session inner key and send it back to the
        // client, wrapped in the client/server inner key and session outer key
        let inner_key = Key::generate();

        let handshake = Handshake {
            session_key: inner_key.clone(),
            engine: env!("CARGO_PKG_NAME").to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        };

        let response = envelope_message(
            handshake,
            &peer.inner_key(),
            &outer_key,
            &inner_key_salt,
            &outer_key_salt,
        )
        .with_context(|| "Error enveloping message - closing connection.")?;

        outgoing
            .send(response)
            .await
            .with_context(|| "Error sending response to peer")?;

        // the peer will now send us its PeerDetails
        let message = incoming.next().await.ok_or_else(|| {
            tracing::warn!("No peer information received - closing connection.");
            Error::InvalidPeer("No peer information received - closing connection.".to_string())
        })??;

        let peer_details = deenvelope_message::<PeerDetails>(
            message,
            &inner_key,
            &outer_key,
            &inner_key_salt,
            &outer_key_salt,
        )
        .with_context(|| "Error de-enveloping message - closing connection.")?;

        tracing::info!(
            "Connected to peer {}, using engine {}, version {}",
            peer_details,
            peer_engine,
            peer_version
        );

        if peer_details.name() != peer_name {
            tracing::warn!(
                "Peer name does not match expected name: {} != {}",
                peer_details.name(),
                peer_name
            );
            return Err(Error::InvalidPeer(
                "Peer name does not match expected name - closing connection.".to_string(),
            ));
        }

        if peer_details.zone() != peer_zone {
            tracing::warn!(
                "Peer zone does not match expected zone: {} != {}",
                peer_details.zone(),
                peer.zone()
            );
            return Err(Error::InvalidPeer(
                "Peer zone does not match expected zone - closing connection.".to_string(),
            ));
        }

        if peer_details.version() != 1 {
            tracing::warn!(
                "Peer version does not match expected version: {} != 1",
                peer_details.version()
            );
            return Err(Error::InvalidPeer(
                "Peer version does not match expected version - closing connection.".to_string(),
            ));
        }

        // now send back our PeerDetials
        let peer_details = PeerDetails::new(&service_name, &peer_zone);

        let message = envelope_message(
            peer_details,
            &inner_key,
            &outer_key,
            &inner_key_salt,
            &outer_key_salt,
        )?;

        outgoing
            .send(message)
            .await
            .with_context(|| "Error sending response to peer - closing connection")?;

        tracing::info!("Handshake complete!");

        // create a new channel for sending messages
        let (tx, rx) = unbounded::<TokioMessage>();

        // save this with the connection
        self.tx = Some(Arc::new(TokioMutex::new(tx)));
        self.inner_key = Some(inner_key.clone());
        self.outer_key = Some(outer_key.clone());
        self.inner_key_salt = Some(inner_key_salt.clone());
        self.outer_key_salt = Some(outer_key_salt.clone());
        self.peer = Some(peer.to_peer().clone());

        match self.state.lock() {
            Ok(mut state) => {
                state.set_connected()?;
            }
            Err(e) => {
                tracing::warn!("Error setting connection state to connected: {:?}", e);
                return Err(Error::BusyLine(
                    "Error setting connection state to connected.".to_string(),
                ));
            }
        }

        // we've now completed the handshake and can use the two session
        // keys to trust and secure both ends of the connection - we can
        // register this connection - must unregister when we close
        match exchange::register(self.clone()).await {
            Ok(_) => (),
            Err(e) => {
                tracing::warn!("Error registering connection with exchange: {:?}", e);
                return Err(Error::Any(e.into()));
            }
        }

        // handle the sending of messages to others
        let received_from_peer = incoming.try_for_each(|msg| {
            // we need to deenvelope the message
            let msg: String = match deenvelope_message(
                msg,
                &inner_key,
                &outer_key,
                &inner_key_salt,
                &outer_key_salt,
            ) {
                Ok(msg) => msg,
                Err(e) => {
                    tracing::warn!("Error de-enveloping message: {:?}", e);
                    return future::ok(());
                }
            };

            exchange::received(Message::received_from(&peer_name, &peer_zone, &msg))
                .unwrap_or_else(|e| {
                    tracing::warn!("Error handling message: {:?}", e);
                });

            future::ok(())
        });

        // handle messages that should be sent to the client (received locally
        // from other services that should be forwarded to the client via the
        // outgoing stream)
        let send_to_peer = rx.map(Ok).forward(outgoing);

        // now tell ourselves who has connected
        exchange::received(
            Command::connected(&peer_name, &peer_zone, &peer_engine, &peer_version).into(),
        )
        .with_context(|| "Error triggering /connected control message")?;

        // now tell ourselves that we have received a watchdog message
        // from this peer - this will start a periodic watchdog check
        exchange::received(Command::watchdog(&peer_name, &peer_zone).into())
            .with_context(|| "Error triggering /watchdog control message")?;

        pin_mut!(received_from_peer, send_to_peer);
        future::select(received_from_peer, send_to_peer).await;

        tracing::info!("{} disconnected", &client_ip);

        // we've exited, meaning that this connection is now closed
        self.closed_connection().await;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enveloping() {
        let inner_key = Key::generate();
        let outer_key = Key::generate();
        #[allow(clippy::unwrap_used)]
        let inner_key_salt = Salt::generate().unwrap();
        #[allow(clippy::unwrap_used)]
        let outer_key_salt = Salt::generate().unwrap();

        let message = "Hello, world!";

        let envelope = envelope_message(
            message,
            &inner_key,
            &outer_key,
            &inner_key_salt,
            &outer_key_salt,
        )
        .unwrap_or_else(|e| {
            unreachable!("Error enveloping message: {:?}", e);
        });

        let deenvelope = deenvelope_message::<String>(
            envelope,
            &inner_key,
            &outer_key,
            &inner_key_salt,
            &outer_key_salt,
        )
        .unwrap_or_else(|e| {
            unreachable!("Error de-enveloping message: {:?}", e);
        });

        assert_eq!(message, deenvelope);
    }
}
