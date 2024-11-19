// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Display;
use tokio::sync::RwLock;

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
}

impl Registrar {
    fn create_null() -> Self {
        Self {
            peers: HashMap::new(),
            peers_by_type: HashMap::new(),
            name: String::new(),
            typ: Type::Portal,
            zones: Vec::new(),
        }
    }

    fn register_self(&mut self, name: &str, agent_type: &Type) {
        self.name = name.to_string();
        self.typ = agent_type.clone();
    }

    fn register_peer(&mut self, peer: &Peer, agent_type: &Type) {
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
}

static REGISTAR: Lazy<RwLock<Registrar>> = Lazy::new(|| RwLock::new(Registrar::create_null()));

///
/// Register that the peer agent called 'name' is of type 'agent_type'
/// and is connecting from zone `zone`
///
pub async fn register_peer(peer: &Peer, agent_type: &Type) {
    REGISTAR.write().await.register_peer(peer, agent_type)
}

///
/// Register that this agent in this process is called `name` and
/// is of type `agent_type`
///
pub async fn register_self(name: &str, agent_type: &Type) {
    REGISTAR.write().await.register_self(name, agent_type)
}

///
/// Remove the agent called 'name' in the zone `zone` from the registry
///
pub async fn remove(peer: &Peer) {
    REGISTAR.write().await.remove(peer)
}

///
/// Return the names of all agents of a specified type
///
pub async fn get_all(agent_type: &Type) -> Vec<Peer> {
    REGISTAR.read().await.agents(agent_type)
}

///
/// Return the name of this agent
///
pub async fn name() -> String {
    REGISTAR.read().await.name.clone()
}

///
/// Return the name of the first portal agent in the system
///
pub async fn portal() -> Option<Peer> {
    REGISTAR.read().await.portal()
}

///
/// Return the name of the first account agent in the system
///
pub async fn account() -> Option<Peer> {
    REGISTAR.read().await.account()
}

///
/// Return the name of the first filesystem agent in the system
///
pub async fn filesystem() -> Option<Peer> {
    REGISTAR.read().await.filesystem()
}

///
/// Return the name of the first scheduler agent in the system
///
pub async fn scheduler() -> Option<Peer> {
    REGISTAR
        .read()
        .await
        .agents(&Type::Scheduler)
        .first()
        .cloned()
}

///
/// Return the type of the specified agent
///
pub async fn agent_type(peer: &Peer) -> Option<Type> {
    REGISTAR.read().await.peers.get(peer).cloned()
}

///
/// Return the first agent called 'name' - note that this
/// will return the first agent with this name, even if there
/// are multiple agents with the same name, but in different
/// zones
///
pub async fn find(name: &str) -> Option<Peer> {
    let registrar = REGISTAR.read().await;

    for (peer, _) in registrar.peers.iter() {
        if peer.name() == name {
            return Some(peer.clone());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    ///
    /// Only used by testing to clear out the registry
    ///
    async fn clear() {
        let mut registrar = REGISTAR.write().await;

        registrar.peers.clear();
        registrar.peers_by_type.clear();
    }

    #[tokio::test]
    async fn test_register() {
        // run all tests in one function, as they need to be serial
        // or they overwrite each other
        clear().await;
        register_peer(&Peer::new("test", "default"), &Type::Portal).await;
        let agents = get_all(&Type::Portal).await;
        assert_eq!(agents, vec![Peer::new("test", "default")]);

        clear().await;
        register_peer(&Peer::new("test", "internal"), &Type::Portal).await;
        remove(&Peer::new("test", "internal")).await;
        let agents = get_all(&Type::Portal).await;
        assert!(agents.is_empty());

        clear().await;
        register_peer(&Peer::new("test", "internal"), &Type::Portal).await;
        let agent = portal().await;
        assert_eq!(agent, Some(Peer::new("test", "internal")));

        clear().await;
        register_peer(&Peer::new("test", "local"), &Type::Account).await;
        let agent = account().await;
        assert_eq!(agent, Some(Peer::new("test", "local")));

        clear().await;
        register_peer(&Peer::new("test", "something"), &Type::Filesystem).await;
        let agent = filesystem().await;
        assert_eq!(agent, Some(Peer::new("test", "something")));

        clear().await;
        register_peer(&Peer::new("test1", "internal"), &Type::Portal).await;
        register_peer(&Peer::new("test2", "default"), &Type::Portal).await;
        register_peer(&Peer::new("test3", "internal"), &Type::Provider).await;
        remove(&Peer::new("test1", "internal")).await;

        let agents = get_all(&Type::Portal).await;
        assert_eq!(agents, vec![Peer::new("test2", "default")]);
        let agents = get_all(&Type::Provider).await;
        assert_eq!(agents, vec![Peer::new("test3", "internal")]);

        assert_eq!(portal().await, Some(Peer::new("test2", "default")));
        assert_eq!(account().await, None);
        assert_eq!(filesystem().await, None);
    }
}
