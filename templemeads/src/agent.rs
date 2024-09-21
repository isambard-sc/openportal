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

pub mod account {
    pub use crate::account::run;
    pub use crate::agent_core::process_args;
    pub use crate::agent_core::Config;
    pub use crate::agent_core::Defaults;
}

pub mod bridge {
    pub use crate::agent_bridge::*;
}

pub mod filesystem {
    pub use crate::account::run;
    pub use crate::agent_core::process_args;
    pub use crate::agent_core::Config;
    pub use crate::agent_core::Defaults;
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
