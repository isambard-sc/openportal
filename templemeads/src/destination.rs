// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::error::Error;
use crate::named::NamedType;

use serde::{Deserialize, Serialize};

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

        // A destination is round-tripped through a whitespace-separated command
        // string (`Job::parse` takes the first token as the destination), so an
        // agent name containing whitespace would silently truncate the path -
        // `"a b.aip1.brics get_projects"` addresses `a`, not `a b.aip1.brics`.
        // Such a name has never worked; reject it here rather than let it
        // produce a destination that means something else downstream.
        if let Some(agent) = agents.iter().find(|a| a.chars().any(char::is_whitespace)) {
            return Err(Error::Parse(format!(
                "Invalid destination '{}' - agent name '{}' contains whitespace",
                destination, agent
            )));
        }

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
        let Some(agent_index) = self.agents.iter().position(|c| c == agent) else {
            return Position::Error;
        };

        let Some(previous_index) = self.agents.iter().position(|c| c == previous) else {
            return Position::Error;
        };

        // `previous` must be our *immediate* neighbour in the path, not merely
        // present somewhere in it.
        //
        // This is the check that binds a Job's claimed route to the peer that
        // actually delivered it. `previous` is the authenticated sender, stamped
        // by paddington from the connection's own `ClientConfig`
        // (`connection.rs`), so it cannot be forged without that link's
        // pre-shared key - whereas the route is an unvalidated `Vec<String>`
        // straight off the wire. Requiring adjacency therefore means an agent
        // can only claim a position in the path for which it holds the key.
        //
        // Previously any sender appearing *anywhere* in the path was accepted,
        // and reaching the last agent returned `Destination` without looking at
        // the sender at all - so a peer could hand its neighbour a Job claiming
        // to have come from the portal and have it forwarded onward bearing that
        // neighbour's authority. See
        // `docs/specifications/security-review-2.md` (finding R4).
        let from_upstream = previous_index + 1 == agent_index;
        let from_downstream = agent_index + 1 == previous_index;

        match (from_upstream, from_downstream) {
            // Travelling downstream, and we are the final hop: this Job is for
            // us.
            (true, _) if agent_index + 1 == self.agents.len() => Position::Destination,
            // Travelling downstream, with further hops beyond us.
            (true, _) => Position::Downstream,
            // A result travelling back up the path.
            (_, true) => Position::Upstream,
            // The sender is not adjacent to us - either it named a position it
            // does not occupy, or the route does not describe a path through
            // both of us.
            _ => Position::Error,
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
            destination_vec.push(Destination::parse(dest_str)?);
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

impl Destinations {
    /// The destination at `index`, or `None` if `index` is out of range.
    ///
    /// This replaces an `impl Index<usize> for Destinations`, which had no
    /// callers anywhere in the workspace and was the only remaining operation
    /// that could panic on an out-of-range index. `Index` is required by its
    /// `std` contract to panic, so it could not be made safe - only exempted
    /// from `clippy::indexing_slicing`. Removing it leaves that deny with no
    /// exceptions in non-test code. See
    /// `docs/specifications/security-review-2.md` (finding R1).
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
    fn test_destination_agent_names_cannot_contain_whitespace() {
        // A destination is the first whitespace-separated token of a command
        // string, so whitespace inside an agent name would make the parsed
        // destination differ from the written one.
        assert!(Destination::parse("portal.provider.cluster").is_ok());

        for bad in [
            "a b.aip1.brics",
            "aip1.a b.brics",
            "aip1.brics extra",
            "aip1.brics\tx",
            "aip1.brics\nx",
        ] {
            assert!(
                Destination::parse(bad).is_err(),
                "{:?} must be rejected as a destination",
                bad
            );
        }
    }

    #[test]
    fn test_position_requires_the_sender_to_be_adjacent() {
        // Regression test for finding R4. `position` accepted any sender that
        // appeared *anywhere* in the path, and returned `Destination` for the
        // last agent without looking at the sender at all.
        let d = Destination::parse("portal.provider.clusters.cluster")
            .unwrap_or_else(|e| unreachable!("parse: {:?}", e));

        // The legitimate hop-by-hop flow, downstream.
        assert_eq!(d.position("provider", "portal"), Position::Downstream);
        assert_eq!(d.position("clusters", "provider"), Position::Downstream);
        assert_eq!(d.position("cluster", "clusters"), Position::Destination);

        // ...and the results coming back upstream.
        assert_eq!(d.position("clusters", "cluster"), Position::Upstream);
        assert_eq!(d.position("provider", "clusters"), Position::Upstream);
        assert_eq!(d.position("portal", "provider"), Position::Upstream);

        // Now the attack: a sender that is in the path but is *not* our
        // neighbour. Previously `clusters` accepted this from `portal` (index 0
        // < index 2, so "downstream") and forwarded it onward under its own
        // authority.
        assert_eq!(d.position("clusters", "portal"), Position::Error);
        assert_eq!(d.position("cluster", "portal"), Position::Error);
        assert_eq!(d.position("cluster", "provider"), Position::Error);
        assert_eq!(d.position("portal", "cluster"), Position::Error);
        assert_eq!(d.position("portal", "clusters"), Position::Error);

        // A sender not in the path at all is still an error.
        assert_eq!(d.position("cluster", "attacker"), Position::Error);
        // ...as is a recipient not in the path.
        assert_eq!(d.position("attacker", "clusters"), Position::Error);
        // ...and a sender claiming to be us.
        assert_eq!(d.position("clusters", "clusters"), Position::Error);
    }

    #[test]
    fn test_position_accepts_every_real_flow_shape() {
        // Adjacency must not break any destination shape the codebase actually
        // builds. Each of these is taken from a real construction site.

        // portal -> provider -> platform -> instance (portal/src/main.rs:250,
        // via the Submit destination supplied by the bridge)
        let d = Destination::parse("brics.isambard.aip2.cluster1")
            .unwrap_or_else(|e| unreachable!("parse: {:?}", e));
        assert_eq!(d.position("isambard", "brics"), Position::Downstream);
        assert_eq!(d.position("aip2", "isambard"), Position::Downstream);
        assert_eq!(d.position("cluster1", "aip2"), Position::Destination);

        // The minimal hierarchy: portal -> instance.
        let d =
            Destination::parse("portal.cluster").unwrap_or_else(|e| unreachable!("parse: {:?}", e));
        assert_eq!(d.position("cluster", "portal"), Position::Destination);
        assert_eq!(d.position("portal", "cluster"), Position::Upstream);

        // bridge -> portal (templemeads/src/bridge.rs:143)
        let d =
            Destination::parse("bridge.portal").unwrap_or_else(|e| unreachable!("parse: {:?}", e));
        assert_eq!(d.position("portal", "bridge"), Position::Destination);
        assert_eq!(d.position("bridge", "portal"), Position::Upstream);

        // instance -> account/filesystem/scheduler (cluster/src/main.rs:566ff)
        let d = Destination::parse("cluster.freeipa")
            .unwrap_or_else(|e| unreachable!("parse: {:?}", e));
        assert_eq!(d.position("freeipa", "cluster"), Position::Destination);
        assert_eq!(d.position("cluster", "freeipa"), Position::Upstream);

        // a delegating peer -> a delegated instance (`instance::run_delegated`)
        let d = Destination::parse("delegator.instance")
            .unwrap_or_else(|e| unreachable!("parse: {:?}", e));
        assert_eq!(d.position("instance", "delegator"), Position::Destination);

        // the offering shape: resource.local-portal.remote-portal
        let d = Destination::parse("resource.localportal.remoteportal")
            .unwrap_or_else(|e| unreachable!("parse: {:?}", e));
        assert_eq!(d.position("localportal", "resource"), Position::Downstream);
        assert_eq!(
            d.position("remoteportal", "localportal"),
            Position::Destination
        );
    }

    #[test]
    fn test_position_on_a_self_referential_path() {
        // A path naming the same agent twice cannot be used to bounce a Job.
        // Indices are first-occurrence, so the repeat is simply not adjacent.
        let d = Destination::parse("a.b.b.c").unwrap_or_else(|e| unreachable!("parse: {:?}", e));
        assert_eq!(d.position("b", "b"), Position::Error);

        let d = Destination::parse("a.b.a.b.c").unwrap_or_else(|e| unreachable!("parse: {:?}", e));
        assert_eq!(d.position("b", "a"), Position::Downstream);
        // `a` claiming to have received from the *second* `b` gets nothing new -
        // it is still just the adjacent pair.
        assert_eq!(d.position("a", "b"), Position::Upstream);
        assert_eq!(d.position("c", "a"), Position::Error);
    }

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
        // `c` accepting a Job from `a` used to be `Destination` - the last agent
        // in the path was returned as the destination without the sender being
        // checked at all. That is the behaviour finding R4 removes: `a` is not
        // `c`'s neighbour, so `c` must refuse it. See
        // `test_position_requires_the_sender_to_be_adjacent`.
        assert_eq!(destination.position("c", "a"), Position::Error);
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
