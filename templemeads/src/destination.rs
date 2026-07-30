// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::error::Error;
use crate::named::NamedType;

use serde::{Deserialize, Serialize};
use std::cmp::{Ord, Ordering};

impl NamedType for Destination {
    fn type_name() -> String {
        "Destination".to_string()
    }
}

impl NamedType for Destinations {
    fn type_name() -> String {
        "Destinations".to_string()
    }
}

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

    pub fn second(&self) -> String {
        // there are always at least two agents in a destination
        self.agents.get(1).unwrap_or(&"".to_string()).clone()
    }

    pub fn last(&self) -> String {
        // there are always at least two agents in a destination
        self.agents.last().unwrap_or(&"".to_string()).clone()
    }

    pub fn reverse(&self) -> Self {
        let mut agents = self.agents.clone();
        agents.reverse();
        Self { agents }
    }

    pub fn position(&self, agent: &str, previous: &str) -> Position {
        match self.agents.contains(&previous.to_string()) {
            false => Position::Error,
            true => self.position_internal(agent, previous),
        }
    }

    /// The agent one hop further along the path than `agent`, or `None` if
    /// `agent` is not in the path or is already the last hop. Expressed with
    /// `get`/`checked_sub` so it cannot panic - see
    /// docs/specifications/security-review-2.md (finding R1).
    pub fn next(&self, agent: &str) -> Option<String> {
        let index = self.agents.iter().position(|c| c == agent)?;
        self.agents.get(index + 1).cloned()
    }

    /// The agent one hop back along the path from `agent`, or `None` if
    /// `agent` is not in the path or is already the first hop.
    pub fn previous(&self, agent: &str) -> Option<String> {
        let index = self.agents.iter().position(|c| c == agent)?;
        self.agents.get(index.checked_sub(1)?).cloned()
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

///
/// This struct represents a vector of Destinations
///
#[derive(Clone, PartialEq, Default)]
pub struct Destinations {
    destinations: Vec<Destination>,
}

impl Destinations {
    pub fn new(destinations: &[Destination]) -> Self {
        let mut unique_destinations: Vec<Destination> = Vec::new();

        // messy, but avoids the need to implement Hash and Eq for Destination
        for dest in destinations {
            if !unique_destinations.contains(dest) {
                unique_destinations.push(dest.clone());
            }
        }

        Self {
            destinations: unique_destinations,
        }
    }

    pub fn parse(destinations: &str) -> Result<Self, Error> {
        // remove a `[` and `]` if they exist at the beginning and end of the string
        let trimmed = destinations.trim();
        let trimmed = trimmed
            .strip_prefix('[')
            .unwrap_or(trimmed)
            .strip_suffix(']')
            .unwrap_or(trimmed)
            .trim();

        let destination_strings: Vec<&str> = trimmed
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect(); // filter out empty strings

        let mut destination_vec: Vec<Destination> = Vec::new();

        for dest_str in destination_strings {
            match Destination::parse(dest_str) {
                Ok(dest) => destination_vec.push(dest),
                Err(e) => return Err(e),
            }
        }
        Ok(Destinations::new(&destination_vec))
    }

    pub fn add(&self, destinations: Destinations) -> Destinations {
        let mut new_destinations = self.destinations.clone();
        for dest in destinations.destinations {
            if !self.destinations.contains(&dest) {
                new_destinations.push(dest);
            }
        }
        Destinations {
            destinations: new_destinations,
        }
    }

    pub fn remove(&self, destinations: Destinations) -> Destinations {
        let mut new_destinations = self.destinations.clone();
        for dest in destinations.destinations {
            if let Some(pos) = new_destinations.iter().position(|x| *x == dest) {
                new_destinations.remove(pos);
            }
        }
        Destinations {
            destinations: new_destinations,
        }
    }
}

impl std::fmt::Debug for Destinations {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self.destinations.as_slice() {
            [] => write!(f, "[]"),
            [single] => write!(f, "{}", single),
            many => {
                let dest_strings: Vec<String> = many.iter().map(|d| d.to_string()).collect();
                write!(f, "[{}]", dest_strings.join(", "))
            }
        }
    }
}

impl std::fmt::Display for Destinations {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self.destinations.as_slice() {
            [] => write!(f, "[]"),
            [single] => write!(f, "{}", single),
            many => {
                let dest_strings: Vec<String> = many.iter().map(|d| d.to_string()).collect();
                write!(f, "[{}]", dest_strings.join(", "))
            }
        }
    }
}

// serialise and deserialise as a single string
impl Serialize for Destinations {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.destinations.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Destinations {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let destinations = Vec::deserialize(deserializer)?;
        Ok(Destinations { destinations })
    }
}

///
/// Implement converstion to a Vec<Destination>
///
impl From<Destinations> for Vec<Destination> {
    fn from(destinations: Destinations) -> Self {
        destinations.destinations
    }
}

///
/// Implement traits so that this can be used as a read-only list
///
impl std::ops::Deref for Destinations {
    type Target = Vec<Destination>;

    fn deref(&self) -> &Self::Target {
        &self.destinations
    }
}

///
/// Implement traits so that we can look up a destination by index.
///
/// Note that `Index` is required by its `std` contract to panic on an
/// out-of-range index, so this is the one indexing operation in the crate
/// that can. Prefer [`Destinations::get`] anywhere the index is derived from
/// data rather than known to be in range - the release profile sets
/// `panic = "abort"`, so a panic here would terminate the agent. See
/// `docs/specifications/security-review-2.md` (finding R1).
///
impl std::ops::Index<usize> for Destinations {
    type Output = Destination;

    fn index(&self, index: usize) -> &Self::Output {
        &self.destinations[index]
    }
}

impl Destinations {
    /// The destination at `index`, or `None` if `index` is out of range.
    /// The non-panicking counterpart to indexing.
    pub fn get(&self, index: usize) -> Option<&Destination> {
        self.destinations.get(index)
    }
}

///
/// Implement traits so that we can get the length of the destinations
///
impl std::ops::Index<std::ops::RangeFull> for Destinations {
    type Output = [Destination];

    fn index(&self, _index: std::ops::RangeFull) -> &Self::Output {
        &self.destinations
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
