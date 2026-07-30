// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Display;
use tokio::sync::RwLock;
use ts_rs::TS;

use crate::domain::Domain;
use crate::error::Error;

#[derive(Debug, Clone, Hash, Serialize, PartialEq, Eq, Deserialize, TS)]
#[ts(export)]
pub enum Type {
    Portal,
    Provider,
    Platform,
    Instance,
    Bridge,
    Account,
    Filesystem,
    Scheduler,
    Virtual,
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Type::Portal => write!(f, "portal"),
            Type::Provider => write!(f, "provider"),
            Type::Platform => write!(f, "platform"),
            Type::Instance => write!(f, "instance"),
            Type::Bridge => write!(f, "bridge"),
            Type::Account => write!(f, "account"),
            Type::Filesystem => write!(f, "filesystem"),
            Type::Scheduler => write!(f, "scheduler"),
            Type::Virtual => write!(f, "virtual"),
        }
    }
}

pub mod account {
    pub use crate::account::run;
    pub use crate::agent_core::process_args;
    pub use crate::agent_core::Config;
    pub use crate::agent_core::Defaults;
}

pub mod bridge {
    pub use crate::agent_bridge::*;
}

pub mod custom {
    pub use crate::agent_core::Config;
    pub use crate::custom::run;
}

pub mod filesystem {
    pub use crate::agent_core::process_args;
    pub use crate::agent_core::Config;
    pub use crate::agent_core::Defaults;
    pub use crate::filesystem::run;
}

pub mod instance {
    pub use crate::agent_core::process_args;
    pub use crate::agent_core::Config;
    pub use crate::agent_core::Defaults;
    pub use crate::instance::run;
}

pub mod platform {
    pub use crate::agent_core::process_args;
    pub use crate::agent_core::Config;
    pub use crate::agent_core::Defaults;
    pub use crate::platform::run;
}

pub mod portal {
    pub use crate::agent_core::process_args;
    pub use crate::agent_core::Config;
    pub use crate::agent_core::Defaults;
    pub use crate::portal::run;
}

pub mod provider {
    pub use crate::agent_core::process_args;
    pub use crate::agent_core::Config;
    pub use crate::agent_core::Defaults;
    pub use crate::provider::run;
}

pub mod scheduler {
    pub use crate::agent_core::process_args;
    pub use crate::agent_core::Config;
    pub use crate::agent_core::Defaults;
    pub use crate::scheduler::run;
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, Hash, PartialEq, Eq)]
pub struct Peer {
    name: String,
    zone: String,
}

impl Peer {
    pub fn new(name: &str, zone: &str) -> Self {
        Self {
            name: name.to_string(),
            zone: zone.to_string(),
        }
    }

    pub fn parse(name: &str) -> Result<Self, Error> {
        let parts: Vec<&str> = name.split('@').collect();

        // Destructured rather than indexed - see
        // docs/specifications/security-review-2.md (finding R1).
        let [peer_name, peer_zone] = parts.as_slice() else {
            return Err(Error::InvalidPeer(name.to_string()));
        };

        let peer_name = peer_name.trim();
        let peer_zone = peer_zone.trim();

        if peer_name.is_empty() || peer_zone.is_empty() {
            return Err(Error::InvalidPeer(name.to_string()));
        }

        Ok(Self {
            name: peer_name.to_string(),
            zone: peer_zone.to_string(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn zone(&self) -> &str {
        &self.zone
    }
}

impl Display for Peer {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}@{}", self.name, self.zone)
    }
}

struct Registrar {
    peers: HashMap<Peer, Type>,
    peers_by_type: HashMap<Type, Vec<Peer>>,
    /// The `Domain` a peer identified itself as speaking, if known - either
    /// because it sent one, or because `Domain::assume_legacy_domain_version`
    /// resolved one on its behalf. Peers with neither are simply absent here.
    peer_domains: HashMap<Peer, PeerDomain>,
    name: String,
    typ: Type,
    zones: Vec<String>,
    engine: String,
    version: String,
    start_time: chrono::DateTime<chrono::Utc>,
    /// Whether this agent should cascade health checks to its peers
    /// Set to false for leaf nodes (e.g., FreeIPA) that bridge zones
    cascade_health: bool,
}

impl Registrar {
    fn create_null() -> Self {
        Self {
            peers: HashMap::new(),
            peers_by_type: HashMap::new(),
            peer_domains: HashMap::new(),
            name: String::new(),
            typ: Type::Portal,
            zones: Vec::new(),
            engine: String::new(),
            version: String::new(),
            start_time: chrono::Utc::now(),
            cascade_health: true, // Default to cascading
        }
    }

    fn register_self(
        &mut self,
        name: &str,
        agent_type: &Type,
        engine: &str,
        version: &str,
        cascade_health: bool,
    ) {
        self.name = name.to_string();
        self.typ = agent_type.clone();
        self.engine = engine.to_string();
        self.version = version.to_string();
        self.start_time = chrono::Utc::now();
        self.cascade_health = cascade_health;
    }

    fn register_peer(
        &mut self,
        peer: &Peer,
        agent_type: &Type,
        _engine: &str,
        _version: &str,
        domain: Option<&str>,
        domain_version: Option<&str>,
    ) {
        if self.peers.contains_key(peer) {
            // we cannot register a virtual agent that overwrites an existing agent
            if agent_type == &Type::Virtual {
                return;
            }

            // remove the old entry
            self.remove(peer);
        }

        self.peers.insert(peer.clone(), agent_type.clone());
        self.peers_by_type
            .entry(agent_type.clone())
            .or_default()
            .push(peer.clone());

        match (domain, domain_version) {
            (Some(name), Some(version)) => {
                self.peer_domains.insert(
                    peer.clone(),
                    PeerDomain {
                        name: name.to_owned(),
                        version: version.to_owned(),
                    },
                );
            }
            _ => {
                self.peer_domains.remove(peer);
            }
        }

        if !self.zones.contains(&peer.zone) {
            self.zones.push(peer.zone().to_owned());
        }
    }

    fn remove(&mut self, peer: &Peer) {
        self.peer_domains.remove(peer);

        if let Some(agent_type) = self.peers.remove(peer) {
            if let Some(v) = self.peers_by_type.get_mut(&agent_type) {
                v.retain(|p| *p != *peer);
            }

            // make sure to update the zones list - this is a bit nasty,
            // there are better ways to do it ;-)
            self.zones.clear();

            for (peer, _) in self.peers.iter() {
                if !self.zones.contains(&peer.zone) {
                    self.zones.push(peer.zone.clone());
                }
            }
        }
    }

    fn agents(&self, agent_type: &Type) -> Vec<Peer> {
        self.peers_by_type
            .get(agent_type)
            .map(|v| v.to_vec())
            .unwrap_or_default()
    }

    ///
    /// Return the name of the first portal agent in the system
    ///
    fn portal(&self) -> Option<Peer> {
        self.peers_by_type
            .get(&Type::Portal)
            .and_then(|v| v.first().cloned())
    }

    ///
    /// Return the name of the first bridge agent in the system
    ///
    fn bridge(&self) -> Option<Peer> {
        self.peers_by_type
            .get(&Type::Bridge)
            .and_then(|v| v.first().cloned())
    }

    ///
    /// Return the name of the first account agent in the system
    ///
    fn account(&self) -> Option<Peer> {
        self.peers_by_type
            .get(&Type::Account)
            .and_then(|v| v.first().cloned())
    }

    ///
    /// Return the name of the first filesystem agent in the system
    ///
    fn filesystem(&self) -> Option<Peer> {
        self.peers_by_type
            .get(&Type::Filesystem)
            .and_then(|v| v.first().cloned())
    }

    ///
    /// Return the name of the first scheduler agent in the system
    ///
    fn scheduler(&self) -> Option<Peer> {
        self.peers_by_type
            .get(&Type::Scheduler)
            .and_then(|v| v.first().cloned())
    }
}

/// The `Domain` a connected peer identified itself as speaking - its name
/// (e.g. `"greatwestern"`) and version. See `agent::peer_domain`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerDomain {
    pub name: String,
    pub version: String,
}

static REGISTRAR: Lazy<RwLock<Registrar>> = Lazy::new(|| RwLock::new(Registrar::create_null()));

///
/// Register that the peer agent called 'name' is of type 'agent_type'
/// and is connecting from zone `zone`. `domain`/`domain_version` are the
/// peer's `Domain` identity, already resolved (including any legacy
/// fallback) by the caller - `None` if genuinely unknown.
///
pub async fn register_peer(
    peer: &Peer,
    agent_type: &Type,
    engine: &str,
    version: &str,
    domain: Option<&str>,
    domain_version: Option<&str>,
) {
    REGISTRAR
        .write()
        .await
        .register_peer(peer, agent_type, engine, version, domain, domain_version)
}

///
/// Return the `Domain` the given peer identified itself as speaking, if
/// known - either because it told us directly, or because we resolved it
/// via `Domain::assume_legacy_domain_version` for an older peer.
///
pub async fn peer_domain(peer: &Peer) -> Option<PeerDomain> {
    REGISTRAR.read().await.peer_domains.get(peer).cloned()
}

///
/// Check that `peer` is confirmed to be speaking the same `Domain` (`L`)
/// this agent is compiled against, forcibly disconnecting it otherwise.
/// This is fail-closed: a peer whose domain is genuinely unknown (no
/// `Register` fields, and no `Domain::assume_legacy_domain_version`
/// resolved one) is treated the same as a peer confirmed to be speaking a
/// *different* domain - both get disconnected, since neither can be
/// trusted to exchange `Instruction`s/`NotificationEvent`s meaningfully
/// with this agent.
///
/// This is opt-in: call it wherever your agent needs the guarantee (e.g.
/// after `ControlCommand::Connected`, or from your own `Register` handling) -
/// templemeads never calls this itself, since a `Domain` mismatch is
/// otherwise harmless to the framework (it just means the two agents will
/// never usefully exchange Jobs/Notifications; nothing about the transport,
/// board sync, or health checks depends on both sides matching).
///
pub async fn ensure_domain_matches<L: Domain>(peer: &Peer) -> Result<(), Error> {
    let expected = L::name();
    let actual = peer_domain(peer).await;

    // A peer that has explicitly identified itself as `templemeads::erased::Erased`
    // is always accepted here, regardless of `L` - it's templemeads' own
    // built-in, domain-oblivious router implementation (not a foreign
    // vocabulary that happens to have a matching name), and by construction
    // it never inspects or executes Instruction/NotificationEvent content -
    // only relays it - so it poses none of the risk this connection-level
    // check exists to catch. The content-level risk (a message that reached
    // this agent via such a router but doesn't actually belong to `L`) is
    // what `ensure_job_domain_matches`/`ensure_notification_domain_matches`
    // guard against instead, at the point of execution - deliberately NOT
    // given the same exception, since accepting "erased" as a message's own
    // provenance there would defeat the reason those checks exist. See
    // `docs/plans/archive/multi-domain-routing-design.md` §8.1.
    let peer_is_known_router = actual
        .as_ref()
        .is_some_and(|d| d.name == crate::erased::Erased::name());

    if peer_is_known_router || actual.as_ref().is_some_and(|d| d.name == expected) {
        return Ok(());
    }

    let message = match actual {
        Some(d) => format!(
            "Peer {} speaks domain '{}' (version {}), but this agent speaks '{}' - disconnecting",
            peer, d.name, d.version, expected
        ),
        None => format!(
            "Peer {} did not report a domain (and none could be assumed for it) - \
             this agent speaks '{}' - disconnecting",
            peer, expected
        ),
    };

    tracing::warn!("{}", message);
    if let Err(e) = paddington::disconnect(peer.name(), peer.zone()).await {
        // Not fatal to reporting the incompatibility - the peer may
        // already be gone, or never had a live connection to begin with.
        tracing::warn!("Failed to disconnect incompatible peer {}: {}", peer, e);
    }

    Err(Error::Incompatible(message))
}

///
/// Verify that `job`'s own recorded `Domain` (see `Job::domain`) matches
/// this agent's `L`, before executing it. Falls back to `sender`'s
/// connection-level domain (`peer_domain`, which already folds in
/// `Domain::assume_legacy_domain_version`) for a Job with no domain of its
/// own - e.g. one from a peer running templemeads from before this field
/// existed. Fail-closed: if neither signal resolves a match, returns
/// `Err(Error::Incompatible(...))`.
///
/// Unlike `ensure_domain_matches`, this never disconnects `sender` - a
/// single misrouted Job (e.g. one that passed through a domain-oblivious
/// router, see `docs/plans/archive/multi-domain-routing-design.md`) doesn't mean
/// the connection itself is bad. Opt-in: call this as the first thing your
/// runner does, if your agent needs the guarantee.
///
pub async fn ensure_job_domain_matches<L: Domain>(
    job: &crate::job::Job<L>,
    sender: &Peer,
) -> Result<(), Error> {
    let expected = L::name();

    let actual_domain = match job.domain() {
        Some(d) => Some(d.to_string()),
        None => peer_domain(sender).await.map(|d| d.name),
    };

    if actual_domain.as_deref() == Some(expected) {
        return Ok(());
    }

    Err(Error::Incompatible(format!(
        "Job {} has domain '{}', but this agent speaks '{}'",
        job.id(),
        actual_domain.as_deref().unwrap_or("unknown"),
        expected
    )))
}

///
/// Verify that `notification`'s own recorded `Domain` (see
/// `Notification::domain`) matches this agent's `L`, before handing it to a
/// notify runner. Same fallback/fail-closed logic as
/// `ensure_job_domain_matches`.
///
/// Notifications already have no delivery guarantee and no return channel
/// ([notification-protocol.md](../../docs/specifications/notification-protocol.md)
/// §8), so a mismatch here simply means the notification should be dropped
/// (logged, not delivered) - one more entry in the same "best-effort"
/// bucket every other notification delivery failure already falls into, not
/// a new kind of error a caller needs to handle specially.
///
pub async fn ensure_notification_domain_matches<L: Domain>(
    notification: &crate::notification::Notification<L>,
    sender: &Peer,
) -> Result<(), Error> {
    let expected = L::name();

    let actual_domain = match notification.domain() {
        Some(d) => Some(d.to_string()),
        None => peer_domain(sender).await.map(|d| d.name),
    };

    if actual_domain.as_deref() == Some(expected) {
        return Ok(());
    }

    Err(Error::Incompatible(format!(
        "Notification {} has domain '{}', but this agent speaks '{}'",
        notification.id(),
        actual_domain.as_deref().unwrap_or("unknown"),
        expected
    )))
}

///
/// Register that this agent in this process is called `name` and
/// is of type `agent_type`
///
pub async fn register_self(
    name: &str,
    agent_type: &Type,
    engine: &str,
    version: &str,
    cascade_health: bool,
) {
    REGISTRAR
        .write()
        .await
        .register_self(name, agent_type, engine, version, cascade_health);
}

///
/// Return a Peer that represent this agent. If 'zone' is None,
/// then the default "local" zone is used
///
pub async fn get_self(zone: Option<&str>) -> Peer {
    let registrar = REGISTRAR.read().await;

    Peer::new(&registrar.name, zone.unwrap_or("local"))
}

/// Check whether this agent should cascade health checks to its peers
pub async fn should_cascade_health() -> bool {
    REGISTRAR.read().await.cascade_health
}

///
/// Remove the agent called 'name' in the zone `zone` from the registry
///
pub async fn remove(peer: &Peer) {
    REGISTRAR.write().await.remove(peer)
}

///
/// Return the names of all agents of a specified type
///
pub async fn get_all(agent_type: &Type) -> Vec<Peer> {
    REGISTRAR.read().await.agents(agent_type)
}

///
/// Return whether or not there is a virtual agent registered
/// with the specified name
///
pub async fn has_virtual(peer: &Peer) -> bool {
    let registrar = REGISTRAR.read().await;

    match registrar.peers_by_type.get(&Type::Virtual) {
        Some(v) => v.contains(peer),
        None => false,
    }
}

///
/// Return the name of this agent
///
pub async fn name() -> String {
    REGISTRAR.read().await.name.clone()
}

///
/// Return the engine name of this agent
///
pub async fn engine() -> String {
    REGISTRAR.read().await.engine.clone()
}

///
/// Return the version of this agent
///
pub async fn version() -> String {
    REGISTRAR.read().await.version.clone()
}

///
/// Return the start time of this agent
///
pub async fn start_time() -> chrono::DateTime<chrono::Utc> {
    REGISTRAR.read().await.start_time
}

///
/// Return the agent type of this agent
///
pub async fn my_agent_type() -> Type {
    REGISTRAR.read().await.typ.clone()
}

///
/// Return all registered peers
///
pub async fn all_peers() -> Vec<Peer> {
    REGISTRAR.read().await.peers.keys().cloned().collect()
}

///
/// Return all real, non-virtual registered peers
///
pub async fn real_peers() -> Vec<Peer> {
    let registrar = REGISTRAR.read().await;
    registrar
        .peers
        .iter()
        .filter_map(|(peer, agent_type)| {
            if agent_type != &Type::Virtual {
                Some(peer.clone())
            } else {
                None
            }
        })
        .collect()
}

///
/// Return the name of the first portal agent in the system.
/// Note that this will wait for up to 30 seconds for a portal
/// agent to be registered before returning None
///
pub async fn portal(wait: u64) -> Option<Peer> {
    let now = std::time::SystemTime::now();
    let wait = std::time::Duration::from_secs(wait);

    loop {
        match REGISTRAR.read().await.portal() {
            Some(peer) => return Some(peer),
            None => match now.elapsed() {
                Ok(elapsed) => {
                    if elapsed > wait {
                        return None;
                    }
                }
                Err(_) => return None,
            },
        }

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

///
/// Return the name of the first bridge agent in the system
/// Note that this will wait for up to 30 seconds for a bridge
/// agent to be registered before returning None
///
pub async fn bridge(wait: u64) -> Option<Peer> {
    let now = std::time::SystemTime::now();
    let wait = std::time::Duration::from_secs(wait);

    loop {
        match REGISTRAR.read().await.bridge() {
            Some(peer) => return Some(peer),
            None => match now.elapsed() {
                Ok(elapsed) => {
                    if elapsed > wait {
                        return None;
                    }
                }
                Err(_) => return None,
            },
        }

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

///
/// Return the name of the first account agent in the system
/// Note that this will wait for up to 30 seconds for an account
/// agent to be registered before returning None
///
pub async fn account(wait: u64) -> Option<Peer> {
    let now = std::time::SystemTime::now();
    let wait = std::time::Duration::from_secs(wait);

    loop {
        match REGISTRAR.read().await.account() {
            Some(peer) => return Some(peer),
            None => match now.elapsed() {
                Ok(elapsed) => {
                    if elapsed > wait {
                        return None;
                    }
                }
                Err(_) => return None,
            },
        }

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

///
/// Return the name of the first filesystem agent in the system
/// Note that this will wait for up to 30 seconds for a filesystem
/// agent to be registered before returning None
///
pub async fn filesystem(wait: u64) -> Option<Peer> {
    let now = std::time::SystemTime::now();
    let wait = std::time::Duration::from_secs(wait);

    loop {
        match REGISTRAR.read().await.filesystem() {
            Some(peer) => return Some(peer),
            None => match now.elapsed() {
                Ok(elapsed) => {
                    if elapsed > wait {
                        return None;
                    }
                }
                Err(_) => return None,
            },
        }

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

///
/// Return the name of the first scheduler agent in the system
/// Note that this will wait for up to 30 seconds for a scheduler
/// agent to be registered before returning None
///
pub async fn scheduler(wait: u64) -> Option<Peer> {
    let now = std::time::SystemTime::now();
    let wait = std::time::Duration::from_secs(wait);

    loop {
        match REGISTRAR.read().await.scheduler() {
            Some(peer) => return Some(peer.clone()),
            None => match now.elapsed() {
                Ok(elapsed) => {
                    if elapsed > wait {
                        return None;
                    }
                }
                Err(_) => return None,
            },
        }

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

///
/// Wait for up to 'wait' seconds for the agent 'peer' to be registered.
/// This will raise an error if there is no agent registered within
/// this time.
///
pub async fn wait_for(peer: &Peer, wait: u64) -> Result<(), Error> {
    if peer.name() == name().await {
        // we don't need to wait for ourselves
        return Ok(());
    }

    let now = std::time::SystemTime::now();
    let wait = std::time::Duration::from_secs(wait);

    loop {
        if REGISTRAR.read().await.peers.contains_key(peer) {
            return Ok(());
        }

        match now.elapsed() {
            Ok(elapsed) => {
                if elapsed > wait {
                    return Err(Error::NotFound(format!(
                        "Agent {} not found as it is not connected",
                        peer
                    )));
                }
            }
            Err(_) => {
                return Err(Error::NotFound(format!(
                    "Agent {} not found as it is not connected",
                    peer
                )))
            }
        }

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

///
/// Return the type of the specified agent
///
pub async fn agent_type(peer: &Peer) -> Option<Type> {
    let registrar = REGISTRAR.read().await;

    match registrar.peers.get(peer) {
        Some(agent_type) => Some(agent_type.clone()),
        None => match peer.name() == registrar.name {
            true => Some(registrar.typ.clone()),
            false => None,
        },
    }
}

///
/// Return whether or not the passed agent is itself
///
pub async fn is_self(peer: &Peer) -> bool {
    let registrar = REGISTRAR.read().await;

    peer.name() == registrar.name
}

///
/// Return whether or not the passed agent is virtual. Virtual
/// agents are either specifically added agents, or when we
/// send a message to ourselves (a virtual agent is created
/// per zone). Note that this will return true if the
/// agent is itself or if this agent is a virtual agent
///
/// To return only non-self virtual agents, use
/// is_virtual(peer) && !is_self(peer)
///
pub async fn is_virtual(peer: &Peer) -> bool {
    let registrar = REGISTRAR.read().await;

    match peer.name() {
        n if n == registrar.name => true,
        _ => registrar
            .peers_by_type
            .get(&Type::Virtual)
            .is_some_and(|v| v.contains(peer)),
    }
}

///
/// Return the first agent called 'name' - note that this
/// will return the first agent with this name, even if there
/// are multiple agents with the same name, but in different
/// zones
///
pub async fn find(name: &str, wait: u64) -> Option<Peer> {
    let now = std::time::SystemTime::now();
    let wait = std::time::Duration::from_secs(wait);

    loop {
        let registrar = REGISTRAR.read().await;

        for (peer, _) in registrar.peers.iter() {
            if peer.name() == name {
                return Some(peer.clone());
            }
        }

        match now.elapsed() {
            Ok(elapsed) => {
                if elapsed > wait {
                    return None;
                }
            }
            Err(_) => return None,
        }

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    ///
    /// Only used by testing to clear out the registry
    ///
    async fn clear() {
        let mut registrar = REGISTRAR.write().await;

        registrar.peers.clear();
        registrar.peers_by_type.clear();
        registrar.peer_domains.clear();
    }

    #[tokio::test]
    async fn test_register() {
        // run all tests in one function, as they need to be serial
        // or they overwrite each other
        let engine = "templemeads";
        let version = "0.0.10";
        clear().await;
        register_peer(
            &Peer::new("test", "default"),
            &Type::Portal,
            engine,
            version,
            None,
            None,
        )
        .await;
        let agents = get_all(&Type::Portal).await;
        assert_eq!(agents, vec![Peer::new("test", "default")]);

        clear().await;
        register_peer(
            &Peer::new("test", "internal"),
            &Type::Portal,
            engine,
            version,
            None,
            None,
        )
        .await;
        remove(&Peer::new("test", "internal")).await;
        let agents = get_all(&Type::Portal).await;
        assert!(agents.is_empty());

        clear().await;
        register_peer(
            &Peer::new("test", "internal"),
            &Type::Portal,
            engine,
            version,
            Some("test-domain"),
            Some("0.0.0"),
        )
        .await;
        let agent = portal(0).await;
        assert_eq!(
            peer_domain(&Peer::new("test", "internal")).await,
            Some(PeerDomain {
                name: "test-domain".to_owned(),
                version: "0.0.0".to_owned(),
            })
        );
        assert_eq!(agent, Some(Peer::new("test", "internal")));

        clear().await;
        register_peer(
            &Peer::new("test", "local"),
            &Type::Account,
            engine,
            version,
            None,
            None,
        )
        .await;
        let agent = account(0).await;
        assert_eq!(agent, Some(Peer::new("test", "local")));

        clear().await;
        register_peer(
            &Peer::new("test", "something"),
            &Type::Filesystem,
            engine,
            version,
            None,
            None,
        )
        .await;
        let agent = filesystem(0).await;
        assert_eq!(agent, Some(Peer::new("test", "something")));

        clear().await;
        register_peer(
            &Peer::new("test1", "internal"),
            &Type::Portal,
            engine,
            version,
            None,
            None,
        )
        .await;
        register_peer(
            &Peer::new("test2", "default"),
            &Type::Portal,
            engine,
            version,
            None,
            None,
        )
        .await;
        register_peer(
            &Peer::new("test3", "internal"),
            &Type::Provider,
            engine,
            version,
            None,
            None,
        )
        .await;
        remove(&Peer::new("test1", "internal")).await;

        let agents = get_all(&Type::Portal).await;
        assert_eq!(agents, vec![Peer::new("test2", "default")]);
        let agents = get_all(&Type::Provider).await;
        assert_eq!(agents, vec![Peer::new("test3", "internal")]);

        assert_eq!(portal(0).await, Some(Peer::new("test2", "default")));
        assert_eq!(account(0).await, None);
        assert_eq!(filesystem(0).await, None);

        // ensure_domain_matches - kept in this same serial test (rather
        // than its own #[tokio::test]) for the reason noted at the top:
        // parallel tests would otherwise race on the shared REGISTRAR.
        use crate::test_domain::TestDomain;

        clear().await;
        let matching = Peer::new("matching", "domaintest");
        let mismatched = Peer::new("mismatched", "domaintest");
        let unknown = Peer::new("unknown", "domaintest");

        register_peer(
            &matching,
            &Type::Portal,
            engine,
            version,
            Some("test-domain"),
            Some("0.0.0"),
        )
        .await;
        assert!(ensure_domain_matches::<TestDomain>(&matching).await.is_ok());

        register_peer(
            &mismatched,
            &Type::Portal,
            engine,
            version,
            Some("greatwestern"),
            Some("0.32.2"),
        )
        .await;
        assert!(ensure_domain_matches::<TestDomain>(&mismatched)
            .await
            .is_err());

        // never registered at all - fail closed, same as a known mismatch
        assert!(ensure_domain_matches::<TestDomain>(&unknown).await.is_err());

        // a peer that identifies as the built-in Erased router is always
        // accepted here, regardless of L - see the doc comment above.
        let router = Peer::new("router", "domaintest");
        register_peer(
            &router,
            &Type::Provider,
            engine,
            version,
            Some(crate::erased::Erased::name()),
            Some(crate::erased::Erased::version()),
        )
        .await;
        assert!(ensure_domain_matches::<TestDomain>(&router).await.is_ok());

        // ensure_job_domain_matches / ensure_notification_domain_matches -
        // same reasoning for staying in this serial test as above.
        use crate::job::Job;
        use crate::notification::Notification;
        use crate::test_domain::TestNotificationEvent;

        #[allow(clippy::expect_used)]
        let job = Job::<TestDomain>::parse("a.b something", false).expect("valid instruction");
        // Job::parse always stamps the agent's own domain - matches by construction.
        assert_eq!(job.domain(), Some("test-domain"));
        assert!(ensure_job_domain_matches::<TestDomain>(&job, &unknown)
            .await
            .is_ok());

        // Simulate a Job relayed from a different domain: same shape, but
        // its own recorded `domain` doesn't match this agent's.
        #[allow(clippy::expect_used)]
        let mut job_json: serde_json::Value =
            serde_json::from_str(&job.to_json().expect("serialises")).expect("valid json");
        job_json["domain"] = serde_json::Value::String("greatwestern".to_string());
        #[allow(clippy::expect_used)]
        let foreign_job: Job<TestDomain> =
            serde_json::from_value(job_json).expect("still deserialises");
        assert_eq!(foreign_job.domain(), Some("greatwestern"));
        assert!(
            ensure_job_domain_matches::<TestDomain>(&foreign_job, &unknown)
                .await
                .is_err()
        );

        // Simulate a legacy Job with no `domain` field at all: falls back
        // to the sender peer's connection-level domain.
        #[allow(clippy::expect_used)]
        let mut legacy_json: serde_json::Value =
            serde_json::from_str(&job.to_json().expect("serialises")).expect("valid json");
        #[allow(clippy::expect_used)]
        legacy_json
            .as_object_mut()
            .expect("job serialises as an object")
            .remove("domain");
        #[allow(clippy::expect_used)]
        let legacy_job: Job<TestDomain> =
            serde_json::from_value(legacy_json).expect("still deserialises, domain defaults");
        assert_eq!(legacy_job.domain(), None);
        // `matching` was registered above with domain "test-domain".
        assert!(
            ensure_job_domain_matches::<TestDomain>(&legacy_job, &matching)
                .await
                .is_ok()
        );
        // `unknown` was never registered - fails closed.
        assert!(
            ensure_job_domain_matches::<TestDomain>(&legacy_job, &unknown)
                .await
                .is_err()
        );

        #[allow(clippy::expect_used)]
        let destination = crate::destination::Destination::parse("a.b").expect("valid destination");
        let notification = Notification::<TestDomain>::new(
            destination,
            TestNotificationEvent::Echo("hello".to_string()),
        );
        assert_eq!(notification.domain(), Some("test-domain"));
        assert!(
            ensure_notification_domain_matches::<TestDomain>(&notification, &unknown)
                .await
                .is_ok()
        );
    }
}
