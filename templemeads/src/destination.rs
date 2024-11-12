// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::error::Error;

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
    pub fn parse(destination: &str) -> Result<Self, Error> {
        let agents: Vec<String> = destination
            .split('.')
            .filter_map(|s| match s.is_empty() {
                false => Some(s.to_string()),
                true => None,
            })
            .collect();

        match agents.len() {
            0 => Err(Error::Parse(format!(
                "Invalid empty destination '{}'",
                destination
            ))),
            1 => Err(Error::Parse(format!(
                "Invalid single agent destination '{}'",
                destination
            ))),
            _ => Ok(Self { agents }),
        }
    }

    pub fn agents(&self) -> Vec<String> {
        self.agents.clone()
    }

    fn position_internal(&self, agent: &str, previous: &str) -> Position {
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

    pub fn first(&self) -> String {
        // there are always at least two agents in a destination
        self.agents.first().unwrap_or(&"".to_string()).clone()
    }

    pub fn last(&self) -> String {
        // there are always at least two agents in a destination
        self.agents.last().unwrap_or(&"".to_string()).clone()
    }

    pub fn position(&self, agent: &str, previous: &str) -> Position {
        match self.agents.contains(&previous.to_string()) {
            false => Position::Error,
            true => self.position_internal(agent, previous),
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
        match Destination::parse(&s) {
            Ok(destination) => Ok(destination),
            Err(e) => Err(serde::de::Error::custom(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_destination_new() {
        #[allow(clippy::unwrap_used)]
        let destination = Destination::parse("a.b.c").unwrap();
        assert_eq!(destination.agents(), vec!["a", "b", "c"]);
    }

    #[test]
    fn test_destination_position() {
        #[allow(clippy::unwrap_used)]
        let destination = Destination::parse("a.b.c").unwrap();
        assert_eq!(destination.position("a", ""), Position::Error);
        assert_eq!(destination.position("b", "a"), Position::Downstream);
        assert_eq!(destination.position("c", "b"), Position::Destination);
        assert_eq!(destination.position("a", "b"), Position::Upstream);
        assert_eq!(destination.position("b", "c"), Position::Upstream);
        assert_eq!(destination.position("c", "a"), Position::Destination);
        assert_eq!(destination.position("c", "d"), Position::Error);
        assert_eq!(destination.position("d", "c"), Position::Error);
    }

    #[test]
    fn test_destination_next() {
        #[allow(clippy::unwrap_used)]
        let destination = Destination::parse("a.b.c").unwrap();
        assert_eq!(destination.next("a"), Some("b".to_string()));
        assert_eq!(destination.next("b"), Some("c".to_string()));
        assert_eq!(destination.next("c"), None);
    }

    #[test]
    fn test_destination_previous() {
        #[allow(clippy::unwrap_used)]
        let destination = Destination::parse("a.b.c").unwrap();
        assert_eq!(destination.previous("a"), None);
        assert_eq!(destination.previous("b"), Some("a".to_string()));
        assert_eq!(destination.previous("c"), Some("b".to_string()));
    }

    #[test]
    fn test_destination_display() {
        #[allow(clippy::unwrap_used)]
        let destination = Destination::parse("a.b.c").unwrap();
        assert_eq!(destination.to_string(), "a.b.c");
    }

    #[test]
    fn test_destination_serialise() {
        #[allow(clippy::unwrap_used)]
        let destination = Destination::parse("a.b.c").unwrap();
        let serialised = serde_json::to_string(&destination).unwrap_or_else(|_| "".to_string());
        assert_eq!(serialised, "\"a.b.c\"");
    }

    #[test]
    fn test_destination_deserialise() {
        #[allow(clippy::unwrap_used)]
        let deserialised: Destination = serde_json::from_str("\"a.b.c\"").unwrap();
        #[allow(clippy::unwrap_used)]
        let expected = Destination::parse("a.b.c").unwrap();
        assert_eq!(deserialised, expected);
    }
}
