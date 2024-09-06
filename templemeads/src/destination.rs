// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq)]
pub struct Destination {
    agents: Vec<String>,
}

pub enum Position {
    Upstream,
    Downstream,
    Endpoint,
    Error,
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

    pub fn position(&self, agent: &str, previous: &str) -> Position {
        if let Some(index) = self.agents.iter().position(|c| c == agent) {
            if index == self.agents.len() - 1 {
                return Position::Endpoint;
            } else if let Some(previous_index) = self.agents.iter().position(|c| c == previous) {
                if index < previous_index {
                    Position::Upstream
                } else if index > previous_index {
                    Position::Downstream
                } else {
                    // cannot have the same index as the previous
                    Position::Error
                }
            } else {
                Position::Error
            }
        } else {
            Position::Error
        }
    }

    pub fn is_endpoint(&self, agent: &str) -> bool {
        if let Some(last) = self.agents.last() {
            last == agent
        } else {
            false
        }
    }

    pub fn is_downstream(&self, agent: &str, previous: &str) -> bool {
        if let Some(index) = self.agents.iter().position(|c| c == agent) {
            if let Some(previous_index) = self.agents.iter().position(|c| c == previous) {
                index > previous_index
            } else {
                false
            }
        } else {
            false
        }
    }

    pub fn is_upstream(&self, agent: &str, previous: &str) -> bool {
        if let Some(index) = self.agents.iter().position(|c| c == agent) {
            if let Some(previous_index) = self.agents.iter().position(|c| c == previous) {
                index < previous_index
            } else {
                false
            }
        } else {
            false
        }
    }

    pub fn next(&self, agent: &str) -> Option<String> {
        // find the index of the agent in the components
        if let Some(index) = self.agents.iter().position(|c| c == agent) {
            // if the index is not the last one
            if index < self.agents.len() - 1 {
                Some(self.agents[index + 1].clone())
            } else {
                None
            }
        } else {
            None
        }
    }

    pub fn previous(&self, agent: &str) -> Option<String> {
        // find the index of the agent in the components
        if let Some(index) = self.agents.iter().position(|c| c == agent) {
            // if the index is not the first one
            if index > 0 {
                Some(self.agents[index - 1].clone())
            } else {
                None
            }
        } else {
            None
        }
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
