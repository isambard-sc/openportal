// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use serde::{Deserialize, Serialize};
use std::cmp::{Ord, Ordering};

#[derive(Clone, PartialEq)]
pub struct Destination {
    agents: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Position {
    Upstream,
    Downstream,
    Destination,
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

    pub fn position(&self, agent: &str, previous: &str) -> Position {
        match self.agents.last() {
            Some(last) => {
                if last == agent {
                    Position::Destination
                } else {
                    let agent_index = self.agents.iter().position(|c| c == agent);
                    let previous_index = self.agents.iter().position(|c| c == previous);

                    match (agent_index, previous_index) {
                        (Some(agent_index), Some(previous_index)) => {
                            match Ord::cmp(&agent_index, &previous_index) {
                                Ordering::Greater => Position::Downstream,
                                Ordering::Less => Position::Upstream,
                                Ordering::Equal => Position::Error,
                            }
                        }
                        _ => Position::Error,
                    }
                }
            }
            None => Position::Error,
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

    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    pub fn is_valid(&self) -> bool {
        !self.agents.is_empty()
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
