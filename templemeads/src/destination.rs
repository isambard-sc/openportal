// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq)]
pub struct Destination {
    agents: Vec<String>,
}

impl Destination {
    pub fn new(destination: &str) -> Self {
        Self {
            agents: destination.split('.').map(|s| s.to_string()).collect(),
        }
    }

    pub fn agents(&self) -> Vec<String> {
        self.agents.clone()
    }

    pub fn contains(&self, agent: &str) -> bool {
        self.agents.iter().any(|c| c == agent)
    }

    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    pub fn next(&self, agent: &str, previous: &str) -> Option<String> {
        // find the index of the agent in the components
        if let Some(index) = self.agents.iter().position(|c| c == agent) {
            // is the previous agent before or after us - this sets the direction of travel
            if let Some(previous_index) = self.agents.iter().position(|c| c == previous) {
                if index > previous_index {
                    // we are after the previous agent - return the agent after us
                    if let Some(next) = self.agents.get(index + 1) {
                        return Some(next.clone());
                    }
                } else {
                    // we are before the previous agent - return the agent before us
                    if let Some(next) = self.agents.get(index - 1) {
                        return Some(next.clone());
                    }
                }
            }
        }

        // nothing matched, there is no "next" agent
        None
    }
}

impl std::fmt::Debug for Destination {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.agents.join("."))
    }
}

impl std::fmt::Display for Destination {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.agents.join("."))
    }
}

// serialise and deserialise as a single string
impl Serialize for Destination {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Destination {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(Self::new(&s))
    }
}
