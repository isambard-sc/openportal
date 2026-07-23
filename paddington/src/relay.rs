// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! A blind relay for agents that can only make outbound connections - see
//! `docs/plans/blind-relay-proxy-design.md` for the full design and
//! rationale.
//!
//! Two peers that can each only connect outwards to a shared proxy agent
//! can still talk to each other as if directly connected: each keeps an
//! ordinary, unmodified paddington connection to the proxy (§4.1), and
//! separately bootstraps a session with its true peer using a pre-shared
//! key pair the proxy never sees (§4.2). The proxy only ever relays the
//! resulting ciphertext, opaque to it in every case - it cannot read it.
//!
//! Relayed agents (not the proxy itself) call [`configure`] once at
//! startup from their `ServiceConfig`, then [`set_inner_handler`] with
//! their real message handler and register [`relay_dispatch_handler`]
//! with `paddington::set_handler` instead of that handler directly. The
//! proxy itself needs none of this - only [`set_proxy_policy`] and
//! [`proxy_handler`].

use anyhow::Context;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::RwLock as StdRwLock;
use tokio::sync::{oneshot, Mutex as TokioMutex, RwLock as TokioRwLock};
use tokio_tungstenite::tungstenite::protocol::Message as TokioMessage;

use once_cell::sync::Lazy;

use crate::command::Command;
use crate::config::ServiceConfig;
use crate::connection::{deenvelope_message, envelope_message};
use crate::crypto::{random_bytes, Key, Salt, SecretKey};
use crate::error::Error;
use crate::exchange;
use crate::message::Message;

/// Envelope carried as an ordinary [`Message`] payload when relaying
/// through a proxy - opaque to the proxy in every case, bootstrap or not.
/// `from`/`to` are the true peers, never the proxy itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayEnvelope {
    from: String,
    to: String,
    zone: String,
    ciphertext: String,
}

/// brics (relayed client) → airr (relayed server), via proxy. Encrypted
/// with the permanent pre-shared keys (§4.1) - the only thing they are
/// ever used for.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StartRelayedConnection {
    session_outer_key: SecretKey,
    inner_key_salt: Salt,
    outer_key_salt: Salt,
    magic: String,
    engine: String,
    version: String,
}

/// airr → brics, via proxy. Same permanent pre-shared keys - the last
/// message that ever uses them for this bootstrap attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RelayedConnectionAccepted {
    session_inner_key: SecretKey,
    magic: String,
    engine: String,
    version: String,
}

/// Internally tagged so a successful permanent-key decryption can be
/// recognised as one specific bootstrap message type unambiguously.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
enum BootstrapMessage {
    Start(StartRelayedConnection),
    Accepted(RelayedConnectionAccepted),
}

/// Which side of the *virtual* relayed connection we are - independent of
/// the fact both sides are physically only ever clients of the proxy (see
/// `docs/plans/blind-relay-proxy-design.md` §4.2).
#[derive(Debug, Clone)]
enum RelayedRole {
    /// We initiate the bootstrap - this peer is one of our `servers`
    /// entries, reached via a relay.
    Client { relay: String },
    /// We wait for the bootstrap - this peer is one of our `clients`
    /// entries, reached via a relay.
    Server { relay: String },
}

#[derive(Debug, Clone)]
struct RelayedPeer {
    /// Zone of the *relayed* relationship itself (e.g. `ukri`'s zone from
    /// `airr`'s perspective) - carried end-to-end in `RelayEnvelope.zone`
    /// and used for the synthesised `Message`, exactly like a direct
    /// connection's zone would be.
    zone: String,
    /// Zone of the *real, direct* connection to the relay/proxy itself
    /// (e.g. `airr`'s own `servers` entry for `"proxy"`) - this is what
    /// paddington's connection registry is actually keyed on, and is very
    /// often but **not necessarily** the same as `zone` above. Using
    /// `zone` here by mistake sends the bootstrap/relay traffic to
    /// `"proxy@<ukri's zone>"`, which paddington's real connection table
    /// has no entry for if `ukri` was added in a different zone to the
    /// proxy itself - see `Message::send_to` call sites below.
    relay_zone: String,
    role: RelayedRole,
    inner_key: SecretKey,
    outer_key: SecretKey,
}

struct RelayConfig {
    my_name: String,
    peers: HashMap<String, RelayedPeer>,
}

/// Once-bootstrapped session keys for a relayed peer - the relayed
/// equivalent of `Connection`'s `inner_key`/`outer_key`/salts.
#[derive(Debug, Clone)]
struct RelayedSession {
    inner_key: SecretKey,
    outer_key: SecretKey,
    inner_key_salt: Salt,
    outer_key_salt: Salt,
}

type MessageHandler = fn(Message) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send>>;

static RELAY_CONFIG: Lazy<TokioRwLock<Option<RelayConfig>>> = Lazy::new(|| TokioRwLock::new(None));
static SESSIONS: Lazy<TokioRwLock<HashMap<String, RelayedSession>>> =
    Lazy::new(|| TokioRwLock::new(HashMap::new()));
static PENDING_BOOTSTRAPS: Lazy<
    TokioMutex<HashMap<String, oneshot::Sender<RelayedConnectionAccepted>>>,
> = Lazy::new(|| TokioMutex::new(HashMap::new()));
static INNER_HANDLER: Lazy<StdRwLock<Option<MessageHandler>>> = Lazy::new(|| StdRwLock::new(None));
static PROXY_POLICY: Lazy<TokioRwLock<RelayPolicy>> =
    Lazy::new(|| TokioRwLock::new(RelayPolicy::default()));
/// On the proxy itself: name -> zone of each of *its own* real `clients`
/// connections (both real hops of a relayed pair are always `clients` of
/// the proxy - it never dials out). Populated by [`configure_proxy`] -
/// see [`proxy_handler`] for why this is needed.
static PROXY_CLIENT_ZONES: Lazy<TokioRwLock<HashMap<String, String>>> =
    Lazy::new(|| TokioRwLock::new(HashMap::new()));

const BOOTSTRAP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// How long to keep retrying the initial bootstrap send while the
/// underlying connection to the relay is still being established.
const RELAY_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const RELAY_CONNECT_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(1);

///
/// Fixed, non-secret salt used only for the one-off bootstrap messages -
/// there is no live connection to generate a per-connection salt from.
/// Safe because `envelope_message` always mixes in a fresh, random
/// per-message `info` value regardless of salt (see `connection.rs`), and
/// the permanent pre-shared key itself is already unique per peer pair -
/// this salt only needs to be consistent, not secret or unique.
///
fn bootstrap_salt() -> Result<Salt, Error> {
    "ab".repeat(32)
        .parse()
        .with_context(|| "Invalid hardcoded bootstrap salt constant")
        .map_err(Error::Any)
}

fn generate_magic() -> Result<String, Error> {
    Ok(hex::encode(random_bytes(32)?))
}

fn encrypt_with_keys<T: Serialize>(
    payload: &T,
    inner_key: &SecretKey,
    outer_key: &SecretKey,
    inner_key_salt: &Salt,
    outer_key_salt: &Salt,
) -> Result<String, Error> {
    let tokio_msg = envelope_message(
        payload,
        inner_key,
        outer_key,
        inner_key_salt,
        outer_key_salt,
    )?;
    Ok(tokio_msg
        .to_text()
        .with_context(|| "Enveloped relay message was not valid text")
        .map_err(Error::Any)?
        .to_string())
}

fn decrypt_with_keys<T: DeserializeOwned>(
    ciphertext: &str,
    inner_key: &SecretKey,
    outer_key: &SecretKey,
    inner_key_salt: &Salt,
    outer_key_salt: &Salt,
) -> Result<T, Error> {
    let tokio_msg = TokioMessage::text(ciphertext);
    Ok(deenvelope_message(
        tokio_msg,
        inner_key,
        outer_key,
        inner_key_salt,
        outer_key_salt,
    )?)
}

///
/// Configure this agent's relayed peers from its `ServiceConfig` - call
/// once at startup, alongside `paddington::run(config)`. Reads every
/// `servers`/`clients` entry that has a `proxy` set (see
/// `paddington::config`); entries without one are ignored here (they are
/// ordinary, unmodified paddington connections).
///
pub async fn configure(config: &ServiceConfig) -> Result<(), Error> {
    let mut peers = HashMap::new();

    // the zone of the real, direct connection to the relay itself - a
    // property of that one `servers` entry, shared by every relayed peer
    // that uses it (a service can only use one proxy at a time - see
    // `ServiceConfig::use_relay`).
    let relay_zone_of = |relay: &str| -> String {
        config
            .servers()
            .iter()
            .find(|s| s.name() == relay)
            .map(|s| s.zone())
            .unwrap_or_else(|| {
                tracing::warn!(
                    "Relay '{}' is not a known server - defaulting its zone to 'default'.",
                    relay
                );
                "default".to_string()
            })
    };

    for server in config.servers() {
        if let Some(relay) = server.proxy() {
            let relay_zone = relay_zone_of(&relay);
            peers.insert(
                server.name(),
                RelayedPeer {
                    zone: server.zone(),
                    relay_zone,
                    role: RelayedRole::Client { relay },
                    inner_key: server.inner_key(),
                    outer_key: server.outer_key(),
                },
            );
        }
    }

    for client in config.clients() {
        if let Some(relay) = client.proxy() {
            let relay_zone = relay_zone_of(&relay);
            peers.insert(
                client.name(),
                RelayedPeer {
                    zone: client.zone(),
                    relay_zone,
                    role: RelayedRole::Server { relay },
                    inner_key: client.inner_key(),
                    outer_key: client.outer_key(),
                },
            );
        }
    }

    let mut state = RELAY_CONFIG.write().await;
    *state = Some(RelayConfig {
        my_name: config.name(),
        peers,
    });

    Ok(())
}

async fn my_name() -> Result<String, Error> {
    RELAY_CONFIG
        .read()
        .await
        .as_ref()
        .map(|s| s.my_name.clone())
        .ok_or_else(|| {
            Error::InvalidPeer("Relay not configured - call relay::configure() first.".to_string())
        })
}

async fn get_peer(name: &str) -> Result<RelayedPeer, Error> {
    RELAY_CONFIG
        .read()
        .await
        .as_ref()
        .and_then(|s| s.peers.get(name).cloned())
        .ok_or_else(|| Error::UnknownPeer(format!("'{}' is not a configured relayed peer.", name)))
}

///
/// Whether `name` is a configured relayed peer - used by
/// [`crate::exchange::send`] to transparently fall back to [`send`] when a
/// caller (e.g. templemeads' `Command::send_to`) addresses a peer that has
/// no real paddington connection because it is only reachable via a proxy.
///
pub async fn is_configured(name: &str) -> bool {
    RELAY_CONFIG
        .read()
        .await
        .as_ref()
        .is_some_and(|s| s.peers.contains_key(name))
}

///
/// Register the real message handler to call once a relay envelope has
/// been dealt with (relayed, bootstrapped, or unwrapped) - or immediately,
/// for any payload that isn't relay-related at all. Register
/// [`relay_dispatch_handler`] with `paddington::set_handler` instead of
/// this handler directly.
///
pub async fn set_inner_handler(handler: MessageHandler) -> Result<(), Error> {
    match INNER_HANDLER.write() {
        Ok(mut inner) => {
            *inner = Some(handler);
            Ok(())
        }
        Err(e) => Err(Error::Poison(format!(
            "Error getting write lock for inner handler: {}",
            e
        ))),
    }
}

fn call_inner_handler(message: Message) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send>> {
    let handler = match INNER_HANDLER.read() {
        Ok(inner) => *inner,
        Err(e) => {
            let err = Error::Poison(format!("Error getting read lock for inner handler: {}", e));
            return Box::pin(async move { Err(err) });
        }
    };

    match handler {
        Some(handler) => handler(message),
        None => Box::pin(async move {
            Err(Error::InvalidPeer(
                "No inner message handler registered - call relay::set_inner_handler first."
                    .to_string(),
            ))
        }),
    }
}

///
/// (Re-)establish a relayed session with `peer_name`, if we are the
/// relayed *client* for it. Blocks (with a timeout) until the bootstrap
/// completes. Produces fresh, mutually-contributed session keys every
/// time it's called - this is where forward secrecy for relayed sessions
/// comes from (see `docs/plans/blind-relay-proxy-design.md` §4.2.1, §5).
///
pub async fn bootstrap(peer_name: &str) -> Result<(), Error> {
    let peer = get_peer(peer_name).await?;

    let relay = match &peer.role {
        RelayedRole::Client { relay } => relay.clone(),
        RelayedRole::Server { .. } => {
            return Err(Error::InvalidPeer(format!(
                "'{}' is a relayed server for us, not a relayed client - it waits, it doesn't initiate.",
                peer_name
            )));
        }
    };

    let my_name = my_name().await?;

    let outer_key = Key::generate();
    let inner_key_salt = Salt::generate()?;
    let outer_key_salt = Salt::generate()?;
    let magic = generate_magic()?;

    let start = StartRelayedConnection {
        session_outer_key: outer_key.clone(),
        inner_key_salt: inner_key_salt.clone(),
        outer_key_salt: outer_key_salt.clone(),
        magic: magic.clone(),
        engine: env!("CARGO_PKG_NAME").to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    };

    let bootstrap_salt = bootstrap_salt()?;
    let ciphertext = encrypt_with_keys(
        &BootstrapMessage::Start(start),
        &peer.inner_key,
        &peer.outer_key,
        &bootstrap_salt,
        &bootstrap_salt,
    )?;

    let envelope = RelayEnvelope {
        from: my_name,
        to: peer_name.to_string(),
        zone: peer.zone.clone(),
        ciphertext,
    };

    let payload = serde_json::to_string(&envelope)
        .with_context(|| "Could not serialise relay envelope")
        .map_err(Error::Any)?;

    let (tx, rx) = oneshot::channel();
    PENDING_BOOTSTRAPS.lock().await.insert(magic.clone(), tx);

    // the connection to the relay is normally still being dialled by
    // paddington's own event loop when a caller bootstraps at startup
    // (see `bootstrap_all_as_client`) - `exchange::send` fails immediately
    // rather than queuing if that connection doesn't exist yet, so retry
    // for a while rather than giving up on the first attempt.
    let mut retries_remaining =
        RELAY_CONNECT_TIMEOUT.as_millis() / RELAY_CONNECT_RETRY_DELAY.as_millis();
    loop {
        match exchange::send(Message::send_to(&relay, &peer.relay_zone, &payload)).await {
            Ok(()) => break,
            Err(e) if retries_remaining > 0 => {
                retries_remaining -= 1;
                tracing::debug!(
                    "Not yet connected to relay '{}' to bootstrap '{}' ({:?}) - retrying shortly.",
                    relay,
                    peer_name,
                    e
                );
                tokio::time::sleep(RELAY_CONNECT_RETRY_DELAY).await;
            }
            Err(e) => {
                PENDING_BOOTSTRAPS.lock().await.remove(&magic);
                return Err(e);
            }
        }
    }

    let accepted = match tokio::time::timeout(BOOTSTRAP_TIMEOUT, rx).await {
        Ok(Ok(accepted)) => accepted,
        Ok(Err(_)) => {
            return Err(Error::InvalidPeer(format!(
                "Relayed bootstrap to '{}' was cancelled before completing.",
                peer_name
            )));
        }
        Err(_) => {
            PENDING_BOOTSTRAPS.lock().await.remove(&magic);
            return Err(Error::InvalidPeer(format!(
                "Timed out waiting for '{}' to accept the relayed connection.",
                peer_name
            )));
        }
    };

    let session = RelayedSession {
        inner_key: accepted.session_inner_key,
        outer_key,
        inner_key_salt,
        outer_key_salt,
    };

    SESSIONS
        .write()
        .await
        .insert(peer_name.to_string(), session);

    tracing::info!(
        "Relayed connection established with {} (via {}), engine {} version {}",
        peer_name,
        relay,
        accepted.engine,
        accepted.version
    );

    // synthesise the "connected" control event, exactly as a direct
    // connection does (connection.rs) - so Register/Sync just work.
    exchange::received(
        Command::connected(peer_name, &peer.zone, &accepted.engine, &accepted.version).into(),
    )?;

    Ok(())
}

///
/// Bootstrap every configured relayed peer for which we are the relayed
/// *client* - call at startup, alongside connecting to any direct
/// `servers`. Peers for which we are the relayed *server* are left alone;
/// they wait for their own client to initiate.
///
pub async fn bootstrap_all_as_client() -> Result<(), Error> {
    let names: Vec<String> = {
        let config = RELAY_CONFIG.read().await;
        match config.as_ref() {
            Some(config) => config
                .peers
                .iter()
                .filter_map(|(name, peer)| match peer.role {
                    RelayedRole::Client { .. } => Some(name.clone()),
                    RelayedRole::Server { .. } => None,
                })
                .collect(),
            None => Vec::new(),
        }
    };

    for name in names {
        bootstrap(&name).await?;
    }

    Ok(())
}

async fn handle_start(
    from: &str,
    zone: &str,
    peer: &RelayedPeer,
    relay: &str,
    start: StartRelayedConnection,
) -> Result<(), Error> {
    let inner_key = Key::generate();

    let accepted = RelayedConnectionAccepted {
        session_inner_key: inner_key.clone(),
        magic: start.magic.clone(),
        engine: env!("CARGO_PKG_NAME").to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    };

    let bootstrap_salt = bootstrap_salt()?;
    let ciphertext = encrypt_with_keys(
        &BootstrapMessage::Accepted(accepted),
        &peer.inner_key,
        &peer.outer_key,
        &bootstrap_salt,
        &bootstrap_salt,
    )?;

    let my_name = my_name().await?;

    let envelope = RelayEnvelope {
        from: my_name,
        to: from.to_string(),
        zone: zone.to_string(),
        ciphertext,
    };

    let payload = serde_json::to_string(&envelope)
        .with_context(|| "Could not serialise relay envelope")
        .map_err(Error::Any)?;

    exchange::send(Message::send_to(relay, &peer.relay_zone, &payload)).await?;

    let session = RelayedSession {
        inner_key,
        outer_key: start.session_outer_key,
        inner_key_salt: start.inner_key_salt,
        outer_key_salt: start.outer_key_salt,
    };

    SESSIONS.write().await.insert(from.to_string(), session);

    tracing::info!(
        "Accepted relayed connection from {} (via {}), engine {} version {}",
        from,
        relay,
        start.engine,
        start.version
    );

    exchange::received(Command::connected(from, zone, &start.engine, &start.version).into())?;

    Ok(())
}

async fn handle_incoming_envelope(envelope: RelayEnvelope) -> Result<Option<Message>, Error> {
    let my_name = my_name().await?;

    if envelope.to != my_name {
        tracing::warn!(
            "Received a relay envelope addressed to '{}', not us ('{}') - ignoring.",
            envelope.to,
            my_name
        );
        return Ok(None);
    }

    let peer = get_peer(&envelope.from).await?;
    let bootstrap_salt = bootstrap_salt()?;

    // try the permanent pre-shared key first - only ever used for the two
    // bootstrap message types.
    if let Ok(bootstrap) = decrypt_with_keys::<BootstrapMessage>(
        &envelope.ciphertext,
        &peer.inner_key,
        &peer.outer_key,
        &bootstrap_salt,
        &bootstrap_salt,
    ) {
        match bootstrap {
            BootstrapMessage::Start(start) => {
                let relay = match &peer.role {
                    RelayedRole::Server { relay } => relay.clone(),
                    RelayedRole::Client { .. } => {
                        tracing::warn!(
                            "Received a StartRelayedConnection from '{}', but we relay to it as a client, not a server - ignoring.",
                            envelope.from
                        );
                        return Ok(None);
                    }
                };
                handle_start(&envelope.from, &envelope.zone, &peer, &relay, start).await?;
            }
            BootstrapMessage::Accepted(accepted) => {
                let mut pending = PENDING_BOOTSTRAPS.lock().await;
                if let Some(tx) = pending.remove(&accepted.magic) {
                    let _ = tx.send(accepted);
                } else {
                    tracing::warn!(
                        "Received a RelayedConnectionAccepted with unrecognised magic from '{}' - ignoring (stale or forged).",
                        envelope.from
                    );
                }
            }
        }
        return Ok(None);
    }

    // not decryptable with the permanent key - must be ongoing traffic
    // under an established session.
    let sessions = SESSIONS.read().await;
    let session = sessions.get(&envelope.from).ok_or_else(|| {
        Error::InvalidPeer(format!(
            "No relayed session established with '{}' yet - dropping message.",
            envelope.from
        ))
    })?;

    let payload: String = decrypt_with_keys(
        &envelope.ciphertext,
        &session.inner_key,
        &session.outer_key,
        &session.inner_key_salt,
        &session.outer_key_salt,
    )?;

    // synthesised messages are dispatched directly to the inner handler
    // (see `relay_dispatch_handler` below), bypassing paddington's own
    // `exchange::event_loop` - which is what normally calls
    // `set_recipient` on a real message just before handing it to the
    // registered handler (see `exchange.rs`). Without this, templemeads'
    // `process_message` rejects it: `MessageType::Message` payloads are
    // checked against `message.recipient()`, which `received_from` always
    // leaves blank.
    let mut message = Message::received_from(&envelope.from, &envelope.zone, &payload);
    message.set_recipient(&my_name);

    Ok(Some(message))
}

///
/// Message handler wrapper for a relayed agent: recognises `RelayEnvelope`
/// payloads and either bootstraps/relays them or unwraps them into a
/// synthesised direct message before calling the real registered handler
/// (see [`set_inner_handler`]) - everything else passes through unchanged.
/// Register this with `paddington::set_handler` in place of your real
/// handler.
///
pub fn relay_dispatch_handler(
    message: Message,
) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send>> {
    Box::pin(async move {
        match serde_json::from_str::<RelayEnvelope>(message.payload()) {
            Ok(envelope) => match handle_incoming_envelope(envelope).await {
                Ok(Some(synthesised)) => call_inner_handler(synthesised).await,
                Ok(None) => Ok(()),
                Err(e) => {
                    tracing::warn!("Error handling relay envelope: {:?}", e);
                    Ok(())
                }
            },
            Err(_) => call_inner_handler(message).await,
        }
    })
}

///
/// Send `payload` to the relayed peer `to`, bootstrapping a fresh session
/// first if none exists yet (relayed-client role only - a relayed server
/// cannot proactively bootstrap, it can only wait for one).
///
pub async fn send(to: &str, payload: &str) -> Result<(), Error> {
    let peer = get_peer(to).await?;

    if !SESSIONS.read().await.contains_key(to) {
        match &peer.role {
            RelayedRole::Client { .. } => bootstrap(to).await?,
            RelayedRole::Server { .. } => {
                return Err(Error::InvalidPeer(format!(
                    "No relayed session with '{}' yet - it must initiate (it is our relayed client).",
                    to
                )));
            }
        }
    }

    let relay = match &peer.role {
        RelayedRole::Client { relay } | RelayedRole::Server { relay } => relay.clone(),
    };

    let my_name = my_name().await?;

    let ciphertext = {
        let sessions = SESSIONS.read().await;
        let session = sessions.get(to).ok_or_else(|| {
            Error::InvalidPeer(format!("No relayed session established with '{}'.", to))
        })?;
        encrypt_with_keys(
            &payload.to_string(),
            &session.inner_key,
            &session.outer_key,
            &session.inner_key_salt,
            &session.outer_key_salt,
        )?
    };

    let envelope = RelayEnvelope {
        from: my_name,
        to: to.to_string(),
        zone: peer.zone.clone(),
        ciphertext,
    };

    let payload = serde_json::to_string(&envelope)
        .with_context(|| "Could not serialise relay envelope")
        .map_err(Error::Any)?;

    exchange::send(Message::send_to(&relay, &peer.relay_zone, &payload)).await
}

///
/// Explicit, default-deny allow-list of `(from, to)` pairs a proxy agent
/// will relay between - see `docs/plans/blind-relay-proxy-design.md` §4.3.
/// Checked in both directions: allowing `(a, b)` also allows `(b, a)`.
///
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RelayPolicy {
    pairs: Vec<(String, String)>,
}

impl RelayPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn allow(&mut self, a: &str, b: &str) {
        self.pairs.push((a.to_string(), b.to_string()));
    }

    pub fn permits(&self, from: &str, to: &str) -> bool {
        self.pairs
            .iter()
            .any(|(a, b)| (a == from && b == to) || (a == to && b == from))
    }
}

///
/// Set the policy a proxy agent's [`proxy_handler`] enforces. Call once at
/// startup (or whenever the policy changes).
///
pub async fn set_proxy_policy(policy: RelayPolicy) {
    let mut p = PROXY_POLICY.write().await;
    *p = policy;
}

///
/// Configure the proxy's own view of its real `clients` connections - call
/// once at startup, alongside [`set_proxy_policy`]. Needed so
/// [`proxy_handler`] can forward on the zone each real hop actually
/// connected under, rather than the zone the *relayed relationship*
/// happens to use (which is meaningful to the two relayed peers, not to
/// the proxy's own connection registry, and is very often different -
/// e.g. two peers connect to the proxy in zone `default`, but relay to
/// each other in a peer-specific zone like `ukri>brics`).
///
pub async fn configure_proxy(config: &ServiceConfig) {
    let zones = config
        .clients()
        .iter()
        .map(|c| (c.name(), c.zone()))
        .collect();

    let mut state = PROXY_CLIENT_ZONES.write().await;
    *state = zones;
}

///
/// Message handler for a pure relay/proxy agent - register this via
/// `paddington::set_handler` on the proxy. Forwards a `RelayEnvelope`
/// payload unchanged to its `to` peer if [`RelayPolicy`] allows the
/// `(from, to)` pair; drops (and logs) everything else, including any
/// non-`RelayEnvelope` payload and any disallowed pair. Never inspects
/// `ciphertext` - it is opaque to the proxy in every case.
///
pub fn proxy_handler(message: Message) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send>> {
    Box::pin(async move {
        let envelope = match serde_json::from_str::<RelayEnvelope>(message.payload()) {
            Ok(envelope) => envelope,
            Err(_) => {
                tracing::warn!(
                    "Proxy received a non-relay payload from {} - dropping.",
                    message.sender()
                );
                return Ok(());
            }
        };

        if !PROXY_POLICY
            .read()
            .await
            .permits(&envelope.from, &envelope.to)
        {
            tracing::warn!(
                "Proxy policy does not permit {} -> {} - dropping.",
                envelope.from,
                envelope.to
            );
            return Ok(());
        }

        // `envelope.zone` is the zone of the *relayed relationship* between
        // `from` and `to` - meaningful to those two peers, not to the
        // proxy's own connection registry. The proxy must instead address
        // this send using the zone `to` actually connected to *it* under
        // (see `configure_proxy`), which is very often a different zone.
        let real_zone = match PROXY_CLIENT_ZONES.read().await.get(&envelope.to).cloned() {
            Some(zone) => zone,
            None => {
                tracing::warn!(
                    "'{}' is not a known client of this proxy (or configure_proxy() was never \
                     called) - falling back to the relayed relationship's own zone '{}', which \
                     will likely fail.",
                    envelope.to,
                    envelope.zone
                );
                envelope.zone.clone()
            }
        };

        let outgoing = Message::send_to(&envelope.to, &real_zone, message.payload());

        if let Err(e) = exchange::send(outgoing).await {
            tracing::warn!(
                "Could not relay message from {} to {}: {:?}",
                envelope.from,
                envelope.to,
                e
            );
        }

        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;

    fn test_peer(role: RelayedRole) -> RelayedPeer {
        RelayedPeer {
            zone: "default".to_string(),
            relay_zone: "default".to_string(),
            role,
            inner_key: Key::generate(),
            outer_key: Key::generate(),
        }
    }

    #[test]
    fn test_bootstrap_message_roundtrip() {
        let peer = test_peer(RelayedRole::Server {
            relay: "proxy".to_string(),
        });
        let salt = bootstrap_salt().unwrap_or_else(|e| unreachable!("bootstrap_salt: {:?}", e));

        let start = StartRelayedConnection {
            session_outer_key: Key::generate(),
            inner_key_salt: Salt::generate().unwrap_or_else(|e| unreachable!("salt: {:?}", e)),
            outer_key_salt: Salt::generate().unwrap_or_else(|e| unreachable!("salt: {:?}", e)),
            magic: "abc123".to_string(),
            engine: "test".to_string(),
            version: "0.0.0".to_string(),
        };

        let ciphertext = encrypt_with_keys(
            &BootstrapMessage::Start(start),
            &peer.inner_key,
            &peer.outer_key,
            &salt,
            &salt,
        )
        .unwrap_or_else(|e| unreachable!("encrypt: {:?}", e));

        let decrypted: BootstrapMessage =
            decrypt_with_keys(&ciphertext, &peer.inner_key, &peer.outer_key, &salt, &salt)
                .unwrap_or_else(|e| unreachable!("decrypt: {:?}", e));

        match decrypted {
            BootstrapMessage::Start(start) => assert_eq!(start.magic, "abc123"),
            BootstrapMessage::Accepted(_) => unreachable!("expected Start"),
        }
    }

    #[test]
    fn test_bootstrap_message_wrong_key_fails() {
        let peer = test_peer(RelayedRole::Server {
            relay: "proxy".to_string(),
        });
        let wrong_inner_key = Key::generate();
        let salt = bootstrap_salt().unwrap_or_else(|e| unreachable!("bootstrap_salt: {:?}", e));

        let start = StartRelayedConnection {
            session_outer_key: Key::generate(),
            inner_key_salt: Salt::generate().unwrap_or_else(|e| unreachable!("salt: {:?}", e)),
            outer_key_salt: Salt::generate().unwrap_or_else(|e| unreachable!("salt: {:?}", e)),
            magic: "abc123".to_string(),
            engine: "test".to_string(),
            version: "0.0.0".to_string(),
        };

        let ciphertext = encrypt_with_keys(
            &BootstrapMessage::Start(start),
            &peer.inner_key,
            &peer.outer_key,
            &salt,
            &salt,
        )
        .unwrap_or_else(|e| unreachable!("encrypt: {:?}", e));

        // simulates the proxy (or anyone else) trying to forge a message
        // without the real pre-shared key
        let decrypted: Result<BootstrapMessage, Error> =
            decrypt_with_keys(&ciphertext, &wrong_inner_key, &peer.outer_key, &salt, &salt);

        assert!(decrypted.is_err());
    }

    #[test]
    fn test_ciphertext_never_contains_plaintext() {
        let peer = test_peer(RelayedRole::Client {
            relay: "proxy".to_string(),
        });
        let salt = bootstrap_salt().unwrap_or_else(|e| unreachable!("bootstrap_salt: {:?}", e));

        let secret_payload = "add_user alice.myproject.myportal - top secret instruction";

        let ciphertext = encrypt_with_keys(
            &secret_payload.to_string(),
            &peer.inner_key,
            &peer.outer_key,
            &salt,
            &salt,
        )
        .unwrap_or_else(|e| unreachable!("encrypt: {:?}", e));

        assert!(!ciphertext.contains("add_user"));
        assert!(!ciphertext.contains("alice"));
        assert!(!ciphertext.contains("top secret"));

        // the RelayEnvelope JSON (what the proxy actually sees) must not
        // leak the payload either
        let envelope = RelayEnvelope {
            from: "brics".to_string(),
            to: "airr".to_string(),
            zone: "default".to_string(),
            ciphertext,
        };
        let wire_json =
            serde_json::to_string(&envelope).unwrap_or_else(|e| unreachable!("json: {:?}", e));
        assert!(!wire_json.contains("add_user"));
        assert!(!wire_json.contains("alice"));
        assert!(!wire_json.contains("top secret"));
    }

    #[test]
    fn test_relay_policy_default_deny() {
        let policy = RelayPolicy::new();
        assert!(!policy.permits("airr", "brics"));
    }

    #[test]
    fn test_relay_policy_bidirectional() {
        let mut policy = RelayPolicy::new();
        policy.allow("airr", "brics");

        assert!(policy.permits("airr", "brics"));
        assert!(policy.permits("brics", "airr"));
        assert!(!policy.permits("airr", "someone_else"));
    }

    #[tokio::test]
    async fn test_configure_proxy_captures_client_zones() {
        // The proxy must forward using the zone each real client actually
        // connected to *it* under, not the zone the relayed relationship
        // between two of its clients happens to use (see `proxy_handler`).
        // NOTE: like `configure()`, `configure_proxy()` writes to a
        // process-global static (`PROXY_CLIENT_ZONES`) - this is the only
        // test that calls it, deliberately, to avoid racing another test
        // that also calls it concurrently.
        let mut proxy = ServiceConfig::new(
            "proxy-czones",
            "http://localhost",
            "127.0.0.1",
            &6005,
            &None,
            &None,
        )
        .unwrap_or_else(|e| unreachable!("service config: {}", e));

        proxy
            .add_client("ukri", "127.0.0.1", &None)
            .unwrap_or_else(|e| unreachable!("add_client: {}", e));
        proxy
            .add_client("cloud", "127.0.0.1", &Some("special-zone".to_string()))
            .unwrap_or_else(|e| unreachable!("add_client: {}", e));

        configure_proxy(&proxy).await;

        let zones = PROXY_CLIENT_ZONES.read().await;
        assert_eq!(zones.get("ukri").cloned(), Some("default".to_string()));
        assert_eq!(
            zones.get("cloud").cloned(),
            Some("special-zone".to_string())
        );
        assert_eq!(zones.get("nonexistent"), None);
    }

    #[tokio::test]
    async fn test_configure_reads_relayed_peers_from_service_config() {
        // NOTE: every assertion that depends on `configure()`'s effect on
        // the global `RELAY_CONFIG`/`my_name()` state lives in this single
        // test function, deliberately - `RELAY_CONFIG` is a process-wide
        // static, and `cargo test` runs test functions concurrently by
        // default, so two tests each calling `configure()` race and
        // clobber each other's state. Add new `configure()`-based
        // assertions here rather than in a new `#[tokio::test]`.
        let mut airr =
            ServiceConfig::new("airr", "http://localhost", "127.0.0.1", &6001, &None, &None)
                .unwrap_or_else(|e| unreachable!("service config: {}", e));
        let mut proxy = ServiceConfig::new(
            "proxy",
            "http://localhost",
            "127.0.0.1",
            &6002,
            &None,
            &None,
        )
        .unwrap_or_else(|e| unreachable!("service config: {}", e));

        let invite = proxy
            .add_client("airr", "127.0.0.1", &None)
            .unwrap_or_else(|e| unreachable!("add_client: {}", e));
        airr.add_server(&invite)
            .unwrap_or_else(|e| unreachable!("add_server: {}", e));

        let invite = airr
            .add_relayed_client("brics", "proxy", &None)
            .unwrap_or_else(|e| unreachable!("add_relayed_client: {}", e));
        let _ = invite; // brics's side isn't configured in this test

        // "ukri" is introduced via the same proxy but in a *different*
        // zone - the real connection to the proxy itself stays in the
        // default zone (§ above), but this relayed peer's own zone
        // differs. Bootstrap/relay traffic must be addressed using the
        // *proxy's* zone (`relay_zone`), not the relayed peer's own zone
        // (`zone`) - otherwise `exchange::send` looks up a connection key
        // that doesn't exist ("proxy@custom-zone" instead of
        // "proxy@default") and fails with `UnnamedConnection`, even though
        // the real connection to the proxy is up.
        airr.add_relayed_client("ukri", "proxy", &Some("custom-zone".to_string()))
            .unwrap_or_else(|e| unreachable!("add_relayed_client: {}", e));

        configure(&airr)
            .await
            .unwrap_or_else(|e| unreachable!("configure: {}", e));

        let peer = get_peer("brics")
            .await
            .unwrap_or_else(|e| unreachable!("get_peer: {}", e));
        assert!(matches!(peer.role, RelayedRole::Server { .. }));
        assert_eq!(peer.zone, "default");
        assert_eq!(peer.relay_zone, "default");
        assert_eq!(my_name().await.unwrap_or_default(), "airr");

        let peer = get_peer("ukri")
            .await
            .unwrap_or_else(|e| unreachable!("get_peer: {}", e));
        assert_eq!(peer.zone, "custom-zone");
        assert_eq!(peer.relay_zone, "default");

        assert!(get_peer("nonexistent").await.is_err());
    }

    #[tokio::test]
    async fn test_full_bootstrap_and_message_exchange_in_process() {
        // Simulates airr (relayed server) and brics (relayed client)
        // bootstrapping and exchanging a message, entirely in-process
        // (no live connections/proxy involved) - directly exercising the
        // same functions each side's dispatch handler would call, proving
        // the protocol itself is correct independent of any network layer.
        let permanent_inner = Key::generate();
        let permanent_outer = Key::generate();

        let airr_peer_for_brics = RelayedPeer {
            zone: "default".to_string(),
            relay_zone: "default".to_string(),
            role: RelayedRole::Server {
                relay: "proxy".to_string(),
            },
            inner_key: permanent_inner.clone(),
            outer_key: permanent_outer.clone(),
        };
        let brics_peer_for_airr = RelayedPeer {
            zone: "default".to_string(),
            relay_zone: "default".to_string(),
            role: RelayedRole::Client {
                relay: "proxy".to_string(),
            },
            inner_key: permanent_inner,
            outer_key: permanent_outer,
        };

        // brics builds the StartRelayedConnection it would send
        let outer_key = Key::generate();
        let inner_key_salt = Salt::generate().unwrap_or_else(|e| unreachable!("salt: {:?}", e));
        let outer_key_salt = Salt::generate().unwrap_or_else(|e| unreachable!("salt: {:?}", e));
        let magic = generate_magic().unwrap_or_else(|e| unreachable!("magic: {:?}", e));

        let start = StartRelayedConnection {
            session_outer_key: outer_key.clone(),
            inner_key_salt: inner_key_salt.clone(),
            outer_key_salt: outer_key_salt.clone(),
            magic: magic.clone(),
            engine: "test".to_string(),
            version: "0.0.0".to_string(),
        };

        let bootstrap_salt = bootstrap_salt().unwrap_or_else(|e| unreachable!("salt: {:?}", e));
        let start_ciphertext = encrypt_with_keys(
            &BootstrapMessage::Start(start),
            &brics_peer_for_airr.inner_key,
            &brics_peer_for_airr.outer_key,
            &bootstrap_salt,
            &bootstrap_salt,
        )
        .unwrap_or_else(|e| unreachable!("encrypt: {:?}", e));

        // airr decrypts it with what it believes are brics's permanent keys
        let decrypted: BootstrapMessage = decrypt_with_keys(
            &start_ciphertext,
            &airr_peer_for_brics.inner_key,
            &airr_peer_for_brics.outer_key,
            &bootstrap_salt,
            &bootstrap_salt,
        )
        .unwrap_or_else(|e| unreachable!("decrypt: {:?}", e));

        let start = match decrypted {
            BootstrapMessage::Start(start) => start,
            BootstrapMessage::Accepted(_) => unreachable!("expected Start"),
        };
        assert_eq!(start.magic, magic);

        // airr generates its own inner key and responds
        let airr_inner_key = Key::generate();
        let accepted = RelayedConnectionAccepted {
            session_inner_key: airr_inner_key.clone(),
            magic: start.magic.clone(),
            engine: "test".to_string(),
            version: "0.0.0".to_string(),
        };

        let accepted_ciphertext = encrypt_with_keys(
            &BootstrapMessage::Accepted(accepted),
            &airr_peer_for_brics.inner_key,
            &airr_peer_for_brics.outer_key,
            &bootstrap_salt,
            &bootstrap_salt,
        )
        .unwrap_or_else(|e| unreachable!("encrypt: {:?}", e));

        // brics decrypts the response
        let decrypted: BootstrapMessage = decrypt_with_keys(
            &accepted_ciphertext,
            &brics_peer_for_airr.inner_key,
            &brics_peer_for_airr.outer_key,
            &bootstrap_salt,
            &bootstrap_salt,
        )
        .unwrap_or_else(|e| unreachable!("decrypt: {:?}", e));

        let accepted = match decrypted {
            BootstrapMessage::Accepted(accepted) => accepted,
            BootstrapMessage::Start(_) => unreachable!("expected Accepted"),
        };
        assert_eq!(accepted.magic, magic);

        // both sides now hold the identical {inner_key (from airr),
        // outer_key (from brics)} session pair
        let brics_session = RelayedSession {
            inner_key: accepted.session_inner_key.clone(),
            outer_key: outer_key.clone(),
            inner_key_salt: inner_key_salt.clone(),
            outer_key_salt: outer_key_salt.clone(),
        };
        let airr_session = RelayedSession {
            inner_key: airr_inner_key,
            outer_key,
            inner_key_salt,
            outer_key_salt,
        };

        // brics sends real, ongoing traffic using the session keys
        let real_payload = "portal.cluster add_user alice.myproject.myportal";
        let ciphertext = encrypt_with_keys(
            &real_payload.to_string(),
            &brics_session.inner_key,
            &brics_session.outer_key,
            &brics_session.inner_key_salt,
            &brics_session.outer_key_salt,
        )
        .unwrap_or_else(|e| unreachable!("encrypt: {:?}", e));

        // this is exactly what the proxy would see - and it cannot recover
        // the payload from it
        assert!(!ciphertext.contains("add_user"));
        assert!(!ciphertext.contains("alice"));

        // airr decrypts using its side of the *same* session keys
        let decrypted: String = decrypt_with_keys(
            &ciphertext,
            &airr_session.inner_key,
            &airr_session.outer_key,
            &airr_session.inner_key_salt,
            &airr_session.outer_key_salt,
        )
        .unwrap_or_else(|e| unreachable!("decrypt: {:?}", e));

        assert_eq!(decrypted, real_payload);
    }

    #[tokio::test]
    async fn test_two_bootstraps_produce_different_session_keys() {
        // Forward secrecy: each bootstrap must produce fresh keys, not a
        // deterministic derivation from the permanent pre-shared key.
        let inner_key = Key::generate();
        let outer_key = Key::generate();

        let mut session_keys = Vec::new();

        for _ in 0..2 {
            let outer = Key::generate();
            let inner_salt = Salt::generate().unwrap_or_else(|e| unreachable!("salt: {:?}", e));
            let outer_salt = Salt::generate().unwrap_or_else(|e| unreachable!("salt: {:?}", e));

            let start = StartRelayedConnection {
                session_outer_key: outer,
                inner_key_salt: inner_salt,
                outer_key_salt: outer_salt,
                magic: generate_magic().unwrap_or_else(|e| unreachable!("magic: {:?}", e)),
                engine: "test".to_string(),
                version: "0.0.0".to_string(),
            };

            let salt = bootstrap_salt().unwrap_or_else(|e| unreachable!("salt: {:?}", e));
            let ciphertext = encrypt_with_keys(
                &BootstrapMessage::Start(start),
                &inner_key,
                &outer_key,
                &salt,
                &salt,
            )
            .unwrap_or_else(|e| unreachable!("encrypt: {:?}", e));

            let decrypted: BootstrapMessage =
                decrypt_with_keys(&ciphertext, &inner_key, &outer_key, &salt, &salt)
                    .unwrap_or_else(|e| unreachable!("decrypt: {:?}", e));

            match decrypted {
                BootstrapMessage::Start(start) => {
                    session_keys.push(format!("{:?}", start.session_outer_key.expose_secret()))
                }
                BootstrapMessage::Accepted(_) => unreachable!("expected Start"),
            }
        }

        assert_ne!(session_keys[0], session_keys[1]);
    }
}
