// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Display;
use tokio::sync::RwLock;

use crate::error::Error;

#[derive(Debug, Clone, Hash, Serialize, PartialEq, Eq, Deserialize)]
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

        if parts.len() != 2 {
            return Err(Error::InvalidPeer(name.to_string()));
        }

        let peer_name = parts[0].trim();
        let peer_zone = parts[1].trim();

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

    fn register_peer(&mut self, peer: &Peer, agent_type: &Type, _engine: &str, _version: &str) {
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

        if !self.zones.contains(&peer.zone) {
            self.zones.push(peer.zone().to_owned());
        }
    }

    fn remove(&mut self, peer: &Peer) {
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

static REGISTRAR: Lazy<RwLock<Registrar>> = Lazy::new(|| RwLock::new(Registrar::create_null()));

///
/// Register that the peer agent called 'name' is of type 'agent_type'
/// and is connecting from zone `zone`
///
pub async fn register_peer(peer: &Peer, agent_type: &Type, engine: &str, version: &str) {
    REGISTRAR
        .write()
        .await
        .register_peer(peer, agent_type, engine, version)
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
/// Return whether or not the passed agent is virtual. Virtual
/// agents are either specifically added agents, or when we
/// send a message to ourselves (a virtual agent is created
/// per zone)
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
        )
        .await;
        let agent = portal(0).await;
        assert_eq!(agent, Some(Peer::new("test", "internal")));

        clear().await;
        register_peer(&Peer::new("test", "local"), &Type::Account, engine, version).await;
        let agent = account(0).await;
        assert_eq!(agent, Some(Peer::new("test", "local")));

        clear().await;
        register_peer(
            &Peer::new("test", "something"),
            &Type::Filesystem,
            engine,
            version,
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
        )
        .await;
        register_peer(
            &Peer::new("test2", "default"),
            &Type::Portal,
            engine,
            version,
        )
        .await;
        register_peer(
            &Peer::new("test3", "internal"),
            &Type::Provider,
            engine,
            version,
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
    }
}
