// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Context;
use anyhow::Error as AnyError;

use futures::{SinkExt, StreamExt};
use futures_channel::mpsc::{unbounded, UnboundedSender};
use futures_util::{future, pin_mut, stream::TryStreamExt};
use secrecy::ExposeSecret;
use serde::{de::DeserializeOwned, Serialize};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::Mutex as TokioMutex;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::protocol::Message as TokioMessage;
use tungstenite::handshake::server::{
    ErrorResponse as HandshakeErrorResponse, Request as HandshakeRequest,
    Response as HandshakeResponse,
};

use crate::command::Command;
use crate::config::{ClientConfig, PeerConfig, ServiceConfig};
use crate::crypto::{Key, SecretKey};
use crate::error::Error;
use crate::exchange;
use crate::message::Message;

#[derive(Debug, Clone, PartialEq)]
enum ConnectionState {
    None,
    Connecting,
    Connected,
    Disconnected,
    Error,
}

#[derive(Debug, Clone)]
pub struct Connection {
    state: Arc<TokioMutex<ConnectionState>>,
    config: ServiceConfig,
    inner_key: Option<SecretKey>,
    outer_key: Option<SecretKey>,
    peer: Option<PeerConfig>,
    tx: Option<Arc<TokioMutex<UnboundedSender<TokioMessage>>>>,
}

fn envelope_message<T>(
    message: T,
    inner_key: &SecretKey,
    outer_key: &SecretKey,
) -> Result<TokioMessage, AnyError>
where
    T: Serialize,
{
    Ok(TokioMessage::text(
        outer_key
            .expose_secret()
            .encrypt(inner_key.expose_secret().encrypt(message)?)?,
    ))
}

fn deenvelope_message<T>(
    message: TokioMessage,
    inner_key: &SecretKey,
    outer_key: &SecretKey,
) -> Result<T, AnyError>
where
    T: DeserializeOwned,
{
    Ok(inner_key.expose_secret().decrypt::<T>(
        &outer_key
            .expose_secret()
            .decrypt::<String>(message.to_text()?)?,
    )?)
}

impl Connection {
    pub fn new(config: ServiceConfig) -> Self {
        Connection {
            state: Arc::new(TokioMutex::new(ConnectionState::None)),
            config,
            inner_key: None,
            outer_key: None,
            peer: None,
            tx: None,
        }
    }

    ///
    /// Return the name of the connection - this is the name of the peer
    /// that the connection is connected to.
    ///
    pub fn name(&self) -> Option<String> {
        self.peer.as_ref().unwrap_or(&PeerConfig::None).name()
    }

    ///
    /// Send a message to the peer on the other end of the connection.
    ///
    pub async fn send_message(&self, message: &str) -> Result<(), Error> {
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

        tx.send(envelope_message(message.to_string(), inner_key, outer_key)?)
            .await
            .with_context(|| "Error sending message to peer")?;

        Ok(())
    }

    ///
    /// This function should be called by the handler of errors raised
    /// by the make_connection or handle_connection functions, when
    /// an error is detected. This sets the state of the connection
    /// to error (it will have been automatically closed)
    ///
    pub async fn set_error(&mut self) {
        let mut state = self.state.lock().await;
        *state = ConnectionState::Error;
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

        let mut state = self.state.lock().await;
        *state = ConnectionState::Disconnected;
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
        {
            let mut state = self.state.lock().await;
            if *state != ConnectionState::None {
                tracing::warn!("Already handling a connection - closing new connection.");
                return Err(Error::BusyLine(format!(
                    "Already handling a connection {:?} - closing new connection.",
                    state
                )));
            }
            *state = ConnectionState::Connecting;
        }

        // we now know we are the only ones handling a connection,
        // so it is safe to update the peer and keys

        // save the peer we are connecting to
        self.peer = Some(peer.clone());
        let peer_name = peer.name().unwrap_or_default();

        let url = server.get_websocket_url()?.to_string();

        tracing::info!("Connecting to WebSocket at: {} - initiating handshake", url);

        let socket = match connect_async(url.clone()).await {
            Ok((socket, _)) => socket,
            Err(e) => {
                tracing::warn!("Error connecting to WebSocket at: {} - {:?}", url, e);
                self.set_error().await;
                return Err(Error::Any(e.into()));
            }
        };

        // Split the WebSocket stream into incoming and outgoing parts
        let (mut outgoing, mut incoming) = socket.split();

        // the client generates the new session outer key, and sends this to the server
        // using the pre-shared client/server inner and outer keys
        let outer_key = Key::generate();

        let message =
            match envelope_message(outer_key.clone(), &server.inner_key(), &server.outer_key()) {
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

        // the server has generated a new session inner key, and has sent that
        // wrapped using the client/server inner key and the new session outer key
        let inner_key: SecretKey =
            match deenvelope_message(response, &server.inner_key(), &outer_key) {
                Ok(inner_key) => inner_key,
                Err(e) => {
                    tracing::warn!("Error de-enveloping message: {:?}", e);
                    self.set_error().await;
                    return Err(e.into());
                }
            };

        tracing::info!("Handshake complete!");

        // we can now save these keys as the new session keys for the connection
        self.inner_key = Some(inner_key.clone());
        self.outer_key = Some(outer_key.clone());

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
        {
            let mut state = self.state.lock().await;
            *state = ConnectionState::Connected;
        }

        // and now we can start the message handling loop - make sure to
        // handle the sending of messages to others
        let received_from_peer = incoming.try_for_each(|msg| {
            // we need to deenvelope the message
            let msg: String = match deenvelope_message(msg, &inner_key, &outer_key) {
                Ok(msg) => msg,
                Err(e) => {
                    tracing::warn!("Error de-enveloping message: {:?}", e);
                    return future::ok(());
                }
            };

            exchange::received(Message::new(&peer_name, &msg)).unwrap_or_else(|e| {
                tracing::warn!("Error handling message: {:?}", e);
            });

            future::ok(())
        });

        // handle messages that should be sent to the client (received locally
        // from other services that should be forwarded to the client via the
        // outgoing stream)
        let send_to_peer = rx.map(Ok).forward(outgoing);

        // now tell ourselves who has connected
        exchange::received(Command::connected(peer_name.clone()).into())
            .with_context(|| "Error triggering /connected control message")?;

        // finally, send a keepalive message to the peer - this will start
        // a ping-pong with the peer that should keep it open
        // (client sends, as the server should already be set up now)
        exchange::send(Message::keepalive(&peer_name))
            .await
            .with_context(|| "Error sending keepalive message to peer")?;

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
        {
            let mut state = self.state.lock().await;
            if *state != ConnectionState::None {
                tracing::warn!("Already handling a connection - closing new connection.");
                return Err(Error::BusyLine(format!(
                    "Already handling a connection {:?} - closing new connection.",
                    state
                )));
            }
            *state = ConnectionState::Connecting;
        }

        // we now know we are the only ones handling the connection,
        // and are safe to update the keys etc.

        let mut client_ip: std::net::IpAddr = stream
            .peer_addr()
            .with_context(|| "Error getting the peer address. Ensure the connection is open.")?
            .ip();

        let proxy_header = self.config.proxy_header();
        let mut proxy_client = None;

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

                match deenvelope_message::<SecretKey>(
                    message.clone(),
                    &client.inner_key(),
                    &client.outer_key(),
                ) {
                    Ok(_) => {
                        tracing::info!(
                            "Client {:?} authenticated for address: {}",
                            client.name().unwrap_or_default(),
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

        let peer_name = peer.name().unwrap_or_default();

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

        // the peer has sent us the new session outer key that should be used,
        // wrapped in the client/server inner and outer keys
        let outer_key =
            deenvelope_message::<SecretKey>(message, &peer.inner_key(), &peer.outer_key())
                .with_context(|| "Error de-enveloping message - closing connection.")?;

        // we will create a new session inner key and send it back to the
        // client, wrapped in the client/server inner key and session outer key
        let inner_key = Key::generate();

        let response = envelope_message(inner_key.clone(), &peer.inner_key(), &outer_key)
            .with_context(|| "Error enveloping message - closing connection.")?;

        outgoing
            .send(response)
            .await
            .with_context(|| "Error sending response to peer")?;

        tracing::info!("Handshake complete!");

        // create a new channel for sending messages
        let (tx, rx) = unbounded::<TokioMessage>();

        // save this with the connection
        self.tx = Some(Arc::new(TokioMutex::new(tx)));
        self.inner_key = Some(inner_key.clone());
        self.outer_key = Some(outer_key.clone());
        self.peer = Some(peer.to_peer().clone());
        {
            let mut state = self.state.lock().await;
            *state = ConnectionState::Connected;
        }

        // we've now completed the handshake and can use the two session
        // keys to trust and secure both ends of the connection - we can
        // register this connection - must unregister when we close
        exchange::register(self.clone())
            .await
            .with_context(|| "Error registering connection with exchange")?;

        // handle the sending of messages to others
        let received_from_peer = incoming.try_for_each(|msg| {
            // we need to deenvelope the message
            let msg: String = match deenvelope_message(msg, &inner_key, &outer_key) {
                Ok(msg) => msg,
                Err(e) => {
                    tracing::warn!("Error de-enveloping message: {:?}", e);
                    return future::ok(());
                }
            };

            exchange::received(Message::new(&peer_name, &msg)).unwrap_or_else(|e| {
                tracing::warn!("Error handling message: {:?}", e);
            });

            future::ok(())
        });

        // handle messages that should be sent to the client (received locally
        // from other services that should be forwarded to the client via the
        // outgoing stream)
        let send_to_peer = rx.map(Ok).forward(outgoing);

        // now tell ourselves who has connected
        exchange::received(Command::connected(peer_name.clone()).into())
            .with_context(|| "Error triggering /connected control message")?;

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

        let message = "Hello, world!";

        let envelope = envelope_message(message, &inner_key, &outer_key).unwrap_or_else(|e| {
            unreachable!("Error enveloping message: {:?}", e);
        });

        let deenvelope = deenvelope_message::<String>(envelope, &inner_key, &outer_key)
            .unwrap_or_else(|e| {
                unreachable!("Error de-enveloping message: {:?}", e);
            });

        assert_eq!(message, deenvelope);
    }
}
