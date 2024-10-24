// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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

struct Registrar {
    agents: HashMap<String, Type>,
    by_type: HashMap<Type, Vec<String>>,
}

impl Registrar {
    fn new() -> Self {
        Self {
            agents: HashMap::new(),
            by_type: HashMap::new(),
        }
    }

    fn register(&mut self, name: &str, agent_type: &Type) {
        self.agents.insert(name.to_string(), agent_type.clone());
        self.by_type
            .entry(agent_type.clone())
            .or_default()
            .push(name.to_string());
    }

    fn remove(&mut self, name: &str) {
        if let Some(agent_type) = self.agents.remove(name) {
            if let Some(v) = self.by_type.get_mut(&agent_type) {
                v.retain(|n| n != name);
            }
        }
    }

    fn agents(&self, agent_type: &Type) -> Vec<String> {
        self.by_type
            .get(agent_type)
            .map(|v| v.to_vec())
            .unwrap_or_default()
    }

    ///
    /// Return the name of the first portal agent in the system
    ///
    fn portal(&self) -> Option<String> {
        self.by_type
            .get(&Type::Portal)
            .and_then(|v| v.first().cloned())
    }

    ///
    /// Return the name of the first account agent in the system
    ///
    fn account(&self) -> Option<String> {
        self.by_type
            .get(&Type::Account)
            .and_then(|v| v.first().cloned())
    }

    ///
    /// Return the name of the first filesystem agent in the system
    ///
    fn filesystem(&self) -> Option<String> {
        self.by_type
            .get(&Type::Filesystem)
            .and_then(|v| v.first().cloned())
    }
}

static REGISTAR: Lazy<RwLock<Registrar>> = Lazy::new(|| RwLock::new(Registrar::new()));

///
/// Register that the agent called 'name' is of type 'agent_type'
///
pub async fn register(name: &str, agent_type: &Type) {
    REGISTAR.write().await.register(name, agent_type)
}

///
/// Remove the agent called 'name' from the registry
///
pub async fn remove(name: &str) {
    REGISTAR.write().await.remove(name)
}

///
/// Return the names of all agents of a specified type
///
pub async fn get_all(agent_type: &Type) -> Vec<String> {
    REGISTAR.read().await.agents(agent_type)
}

///
/// Return the name of the first portal agent in the system
///
pub async fn portal() -> Option<String> {
    REGISTAR.read().await.portal()
}

///
/// Return the name of the first account agent in the system
///
pub async fn account() -> Option<String> {
    REGISTAR.read().await.account()
}

///
/// Return the name of the first filesystem agent in the system
///
pub async fn filesystem() -> Option<String> {
    REGISTAR.read().await.filesystem()
}

#[cfg(test)]
mod tests {
    use super::*;

    ///
    /// Only used by testing to clear out the registry
    ///
    async fn clear() {
        let mut registrar = REGISTAR.write().await;

        registrar.agents.clear();
        registrar.by_type.clear();
    }

    #[tokio::test]
    async fn test_register() {
        // run all tests in one function, as they need to be serial
        // or they overwrite each other
        clear().await;
        register("test", &Type::Portal).await;
        let agents = get_all(&Type::Portal).await;
        assert_eq!(agents, vec!["test"]);

        clear().await;
        register("test", &Type::Portal).await;
        remove("test").await;
        let agents = get_all(&Type::Portal).await;
        assert!(agents.is_empty());

        clear().await;
        register("test", &Type::Portal).await;
        let agent = portal().await;
        assert_eq!(agent, Some("test".to_string()));

        clear().await;
        register("test", &Type::Account).await;
        let agent = account().await;
        assert_eq!(agent, Some("test".to_string()));

        clear().await;
        register("test", &Type::Filesystem).await;
        let agent = filesystem().await;
        assert_eq!(agent, Some("test".to_string()));

        clear().await;
        register("test1", &Type::Portal).await;
        register("test2", &Type::Portal).await;
        register("test3", &Type::Provider).await;
        remove("test1").await;

        let agents = get_all(&Type::Portal).await;
        assert_eq!(agents, vec!["test2"]);
        let agents = get_all(&Type::Provider).await;
        assert_eq!(agents, vec!["test3"]);

        assert_eq!(portal().await, Some("test2".to_string()));
        assert_eq!(account().await, None);
        assert_eq!(filesystem().await, None);
    }
}
