// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! Portal route discovery - each agent derives the route by which each portal
//! should reach it, and refuses instructions that arrive by any other route.
//!
//! See `docs/plans/portal-route-discovery-design.md` for the full design, and
//! `docs/specifications/security-review-2.md` §4.1 for the residual it closes.
//!
//! The short version: an agent knows from **its own config** which of its peers
//! is a portal (`type = "portal"`, finding R3), and originates the route
//! `<portal>.<me>` from that. Every other agent learns its route by being told
//! one by an upstream peer and appending its own name. Because the topology is
//! single-pathed and acyclic, two different routes to the same portal name
//! cannot legitimately occur - so a collision is an unambiguous signal that an
//! agent's config or state has been modified to introduce an impostor portal.

use crate::agent::Peer;
use crate::destination::Destination;
use crate::error::Error;
use crate::portal_identifier::PortalIdentifier;

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use tokio::sync::RwLock;

/// Most distinct portals a single peer may advertise to us. A zone has very
/// few portals; this exists only so a peer cannot exhaust memory by
/// advertising an unbounded number of invented portal names.
const MAX_PORTALS_PER_PEER: usize = 16;

/// Longest route we will accept. Comfortably exceeds
/// portal → provider → platform → instance; exists for the same reason as
/// `MAX_PORTALS_PER_PEER`.
const MAX_ROUTE_DEPTH: usize = 16;

static ROUTES: Lazy<RwLock<RouteTable>> = Lazy::new(|| RwLock::new(RouteTable::default()));

/// A single portal and the route by which it reaches the agent advertising it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortalRoute {
    portal: PortalIdentifier,
    route: Destination,
}

impl PortalRoute {
    pub fn new(portal: &PortalIdentifier, route: &Destination) -> Self {
        Self {
            portal: portal.clone(),
            route: route.clone(),
        }
    }

    pub fn portal(&self) -> &PortalIdentifier {
        &self.portal
    }

    pub fn route(&self) -> &Destination {
        &self.route
    }
}

impl std::fmt::Display for PortalRoute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} via {}", self.portal, self.route)
    }
}

#[derive(Debug, Clone)]
struct RouteEntry {
    /// The route from the portal to *this* agent, inclusive of this agent's own
    /// name - i.e. what we advertise onward unchanged.
    route: Destination,
    /// The peer that told us, or `None` if we originated it from our own
    /// config. Originated routes are never withdrawn.
    learned_from: Option<Peer>,
}

#[derive(Debug, Default)]
struct RouteTable {
    /// `(zone, portal name)` -> the route to us.
    routes: HashMap<(String, String), RouteEntry>,
    /// `(zone, portal name)` pairs for which we have seen two different routes.
    /// Instructions naming these are refused until an operator intervenes.
    collided: HashSet<(String, String)>,
}

fn key(zone: &str, portal: &str) -> (String, String) {
    (zone.to_string(), portal.to_string())
}

impl RouteTable {
    ///
    /// Originate the route for a portal this agent's *own config* declares
    /// (`type = "portal"`). This is the trust anchor for the whole scheme: it is
    /// the only statement about a portal's identity accepted without having been
    /// derived from somewhere else.
    ///
    fn originate(
        &mut self,
        portal: &PortalIdentifier,
        zone: &str,
        me: &str,
    ) -> Result<bool, Error> {
        let route = Destination::parse(&format!("{}.{}", portal.portal(), me))?;

        Ok(self.insert(
            zone,
            portal,
            RouteEntry {
                route,
                learned_from: None,
            },
        ))
    }

    ///
    /// Record routes advertised by `from`, and withdraw any it has retracted.
    /// Returns `true` if the table changed.
    ///
    fn receive(
        &mut self,
        from: &Peer,
        routes: &[PortalRoute],
        withdrawn: &[PortalIdentifier],
        me: &str,
    ) -> bool {
        let mut changed = false;

        if routes.len() > MAX_PORTALS_PER_PEER {
            tracing::warn!(
                "Peer {} advertised {} portal routes, above the limit of {} - ignoring all \
                 of them.",
                from,
                routes.len(),
                MAX_PORTALS_PER_PEER
            );
            return false;
        }

        for advertised in routes {
            // The advertised route must end with the advertising peer's own
            // name.
            //
            // This is what stops a *downstream* peer injecting a route
            // upstream: a peer can only ever claim to be the last hop of a route
            // it advertises. A downstream peer that advertises a
            // correctly-terminated route instead collides with the one we
            // already hold. See the design doc §4.3.
            if advertised.route.last() != from.name() {
                tracing::warn!(
                    "Ignoring route '{}' advertised by {}: it does not end with that peer's \
                     own name, so that peer is not the hop it claims to be.",
                    advertised,
                    from
                );
                continue;
            }

            if advertised.route.agents().len() >= MAX_ROUTE_DEPTH {
                tracing::warn!(
                    "Ignoring route '{}' advertised by {}: it is longer than the limit of \
                     {} hops.",
                    advertised,
                    from,
                    MAX_ROUTE_DEPTH
                );
                continue;
            }

            // Our own route is what they told us, with us on the end.
            let route = match Destination::parse(&format!("{}.{}", advertised.route, me)) {
                Ok(route) => route,
                Err(e) => {
                    tracing::warn!(
                        "Could not extend route '{}' from {} with our own name '{}': {}",
                        advertised,
                        from,
                        me,
                        e
                    );
                    continue;
                }
            };

            // Routes are scoped to the zone the advertising peer is in, and
            // never cross zones.
            if self.insert(
                from.zone(),
                &advertised.portal,
                RouteEntry {
                    route,
                    learned_from: Some(from.clone()),
                },
            ) {
                changed = true;
            }
        }

        for portal in withdrawn {
            if self.withdraw(from.zone(), portal, Some(from)) {
                changed = true;
            }
        }

        changed
    }

    fn insert(&mut self, zone: &str, portal: &PortalIdentifier, entry: RouteEntry) -> bool {
        let k = key(zone, &portal.portal());

        if let Some(existing) = self.routes.get(&k) {
            if existing.route == entry.route {
                // Idempotent re-advertisement - the common case on reconnect.
                return false;
            }

            // Two different routes claim to lead to the same portal. In a
            // single-pathed, acyclic topology that cannot legitimately happen,
            // so this is the signature of an agent whose config has been
            // modified to introduce an impostor portal. See the design doc §4.5.
            tracing::error!(
                "PORTAL ROUTE COLLISION for '{}' in zone '{}': already known via '{}' (from \
                 {}), now also advertised via '{}' (from {}). In a single-path topology this \
                 cannot happen legitimately - an agent's configuration has been changed, or \
                 an impostor portal has been introduced. Refusing to route any instruction \
                 naming '{}' until this is resolved.",
                portal,
                zone,
                existing.route,
                describe(&existing.learned_from),
                entry.route,
                describe(&entry.learned_from),
                portal
            );

            self.collided.insert(k);
            return false;
        }

        tracing::info!(
            "Portal '{}' in zone '{}' reaches us via '{}'",
            portal,
            zone,
            entry.route
        );

        self.routes.insert(k, entry);
        true
    }

    fn withdraw(&mut self, zone: &str, portal: &PortalIdentifier, from: Option<&Peer>) -> bool {
        let k = key(zone, &portal.portal());

        // Only the peer that told us may retract it; a route we originated from
        // our own config is never withdrawn.
        let should_remove = match self.routes.get(&k) {
            Some(entry) => match (&entry.learned_from, from) {
                (Some(learned), Some(from)) => learned == from,
                (Some(_), None) => true,
                _ => false,
            },
            None => false,
        };

        if should_remove {
            tracing::info!(
                "Withdrawing route to portal '{}' in zone '{}'",
                portal,
                zone
            );
            self.routes.remove(&k);
            self.collided.remove(&k);
            return true;
        }

        false
    }

    fn withdraw_all_from(&mut self, peer: &Peer) -> bool {
        let stale: Vec<(String, String)> = self
            .routes
            .iter()
            .filter(|(_, entry)| entry.learned_from.as_ref() == Some(peer))
            .map(|(k, _)| k.clone())
            .collect();

        for k in &stale {
            tracing::info!(
                "Withdrawing route to portal '{}' in zone '{}' - learned from {}, which has \
                 disconnected",
                k.1,
                k.0,
                peer
            );
            self.routes.remove(k);
            self.collided.remove(k);
        }

        !stale.is_empty()
    }

    fn routes_for_peer(&self, peer: &Peer) -> Vec<PortalRoute> {
        self.routes
            .iter()
            .filter(|((zone, _), entry)| {
                zone == peer.zone() && entry.learned_from.as_ref() != Some(peer)
            })
            .filter_map(|((_, portal), entry)| {
                PortalIdentifier::parse(portal)
                    .ok()
                    .map(|portal| PortalRoute::new(&portal, &entry.route))
            })
            .collect()
    }

    fn expected_route(&self, zone: &str, portal: &str) -> Option<Destination> {
        self.routes.get(&key(zone, portal)).map(|e| e.route.clone())
    }

    fn is_collided(&self, zone: &str, portal: &str) -> bool {
        self.collided.contains(&key(zone, portal))
    }

    fn zone_has_portal_route(&self, zone: &str) -> bool {
        self.routes.keys().any(|(z, _)| z == zone)
    }
}

fn describe(learned_from: &Option<Peer>) -> String {
    match learned_from {
        Some(peer) => peer.to_string(),
        None => "our own config".to_string(),
    }
}

// The public API is a thin async wrapper around the table above. The logic
// itself is deliberately synchronous and operates on a `RouteTable` value, so
// that it can be tested without touching this global - the tests would
// otherwise race each other through it.

/// See `RouteTable::originate`.
pub async fn originate(portal: &PortalIdentifier, zone: &str, me: &str) -> Result<bool, Error> {
    ROUTES.write().await.originate(portal, zone, me)
}

/// See `RouteTable::receive`.
pub async fn receive(
    from: &Peer,
    routes: &[PortalRoute],
    withdrawn: &[PortalIdentifier],
    me: &str,
) -> bool {
    ROUTES.write().await.receive(from, routes, withdrawn, me)
}

///
/// Withdraw every route learned from `peer` - called when that peer
/// disconnects, so a later topology change does not present as a collision
/// against a stale route.
///
pub async fn withdraw_all_from(peer: &Peer) -> bool {
    ROUTES.write().await.withdraw_all_from(peer)
}

///
/// The routes to advertise to `peer`: everything we know in that peer's zone,
/// except what we learned from that peer itself (no-backtrack propagation,
/// which in an acyclic topology is what makes this terminate).
///
pub async fn routes_for_peer(peer: &Peer) -> Vec<PortalRoute> {
    ROUTES.read().await.routes_for_peer(peer)
}

/// The route by which `portal` should reach us in `zone`, if we know one.
pub async fn expected_route(zone: &str, portal: &str) -> Option<Destination> {
    ROUTES.read().await.expected_route(zone, portal)
}

///
/// Whether we have seen two conflicting routes for this portal, and are
/// therefore refusing to route instructions naming it.
///
pub async fn is_collided(zone: &str, portal: &str) -> bool {
    ROUTES.read().await.is_collided(zone, portal)
}

///
/// Whether we know of *any* portal reachable in `zone` - i.e. whether this zone
/// carries portal-rooted traffic at all.
///
/// This is the signal that separates the two kinds of zone an agent can sit in,
/// and it needs no configuration of its own because routes are already
/// zone-scoped and only ever propagate away from a portal within one zone.
///
/// - A zone with a portal route is where instructions arrive *from* a portal, so
///   an instruction naming a portal must be properly rooted at it.
/// - A zone with none is internal: an instance and the account, filesystem and
///   scheduler agents it delegates to. Those agents legitimately create jobs
///   that name a portal on a destination rooted at themselves -
///   `freeipa.shared get_local_home_dir john.aiproject.brics` is the canonical
///   example, and `op-freeipa` passes `check_portal = false` when building it
///   for exactly that reason. Applying the portal rule there would reject
///   ordinary traffic.
///
/// See `docs/plans/portal-route-discovery-design.md` §4.7.
///
pub async fn zone_has_portal_route(zone: &str) -> bool {
    ROUTES.read().await.zone_has_portal_route(zone)
}

///
/// Whether `destination` is consistent with `route` - i.e. the Job travelled
/// the path we expect this portal's instructions to travel.
///
/// A prefix match rather than equality, because we may be an intermediate hop
/// with further agents beyond us. This is strictly stronger than comparing only
/// `destination.first()`, which is all the R34 check can do on its own.
///
pub fn destination_matches_route(destination: &Destination, route: &Destination) -> bool {
    let destination = destination.agents();
    let route = route.agents();

    match destination.len() >= route.len() {
        true => destination.starts_with(&route),
        false => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn portal(name: &str) -> PortalIdentifier {
        PortalIdentifier::parse(name).unwrap_or_else(|e| unreachable!("portal: {:?}", e))
    }

    fn dest(d: &str) -> Destination {
        Destination::parse(d).unwrap_or_else(|e| unreachable!("destination: {:?}", e))
    }

    fn advert(p: &str, route: &str) -> PortalRoute {
        PortalRoute::new(&portal(p), &dest(route))
    }

    #[test]
    fn test_origination_and_propagation_down_a_chain() {
        // aip1 knows from its own config that brics is a portal, and originates
        // `brics.aip1` from that - the trust anchor for everything below.
        let mut aip1_table = RouteTable::default();
        assert!(aip1_table
            .originate(&portal("brics"), "default", "aip1")
            .unwrap_or_else(|e| unreachable!("originate: {:?}", e)));
        assert_eq!(
            aip1_table.expected_route("default", "brics"),
            Some(dest("brics.aip1"))
        );

        // What aip1 advertises onward is exactly what it stored.
        let advertised = aip1_table.routes_for_peer(&Peer::new("clusters", "default"));
        assert_eq!(advertised.len(), 1);
        assert_eq!(advertised[0].route(), &dest("brics.aip1"));

        // clusters receives it and appends itself.
        let aip1 = Peer::new("aip1", "default");
        let mut clusters_table = RouteTable::default();
        assert!(clusters_table.receive(&aip1, &advertised, &[], "clusters"));
        assert_eq!(
            clusters_table.expected_route("default", "brics"),
            Some(dest("brics.aip1.clusters"))
        );

        // ...and clusters advertises that to shared, which appends itself.
        let clusters = Peer::new("clusters", "default");
        let advertised = clusters_table.routes_for_peer(&Peer::new("shared", "default"));
        let mut shared_table = RouteTable::default();
        assert!(shared_table.receive(&clusters, &advertised, &[], "shared"));
        assert_eq!(
            shared_table.expected_route("default", "brics"),
            Some(dest("brics.aip1.clusters.shared"))
        );

        // Re-advertisement of the same route is a no-op - the common case on
        // reconnect, and what stops it being mistaken for a collision.
        assert!(!shared_table.receive(&clusters, &advertised, &[], "shared"));
        assert!(!shared_table.is_collided("default", "brics"));
    }

    #[test]
    fn test_route_must_end_with_the_advertising_peer() {
        let mut table = RouteTable::default();

        // A downstream peer trying to inject a route upstream: `shared`
        // advertises a route ending in `clusters`, which it is not.
        let shared = Peer::new("shared", "default");
        assert!(!table.receive(
            &shared,
            &[advert("brics", "brics.aip1.clusters")],
            &[],
            "clusters"
        ));
        assert_eq!(table.expected_route("default", "brics"), None);

        // A correctly-terminated route from the same peer passes this rule - it
        // is the collision rule that catches that case instead.
        assert!(table.receive(
            &shared,
            &[advert("brics", "brics.aip1.shared")],
            &[],
            "clusters"
        ));
    }

    #[test]
    fn test_collision_disables_only_the_affected_portal() {
        let mut table = RouteTable::default();

        let aip1 = Peer::new("aip1", "default");
        let fake = Peer::new("fake", "default");

        assert!(table.receive(&aip1, &[advert("brics", "brics.aip1")], &[], "clusters"));
        assert!(table.receive(&aip1, &[advert("other", "other.aip1")], &[], "clusters"));

        // The attack: an attacker-added peer `fake` claims to reach `brics` too.
        assert!(!table.receive(&fake, &[advert("brics", "brics.fake")], &[], "clusters"));

        assert!(table.is_collided("default", "brics"));
        // The original route is retained rather than replaced.
        assert_eq!(
            table.expected_route("default", "brics"),
            Some(dest("brics.aip1.clusters"))
        );

        // Only that portal is affected. A global safe state would let an
        // attacker who can add one peer take down everything downstream at will.
        assert!(!table.is_collided("default", "other"));
        assert_eq!(
            table.expected_route("default", "other"),
            Some(dest("other.aip1.clusters"))
        );
    }

    #[test]
    fn test_routes_are_scoped_to_a_zone() {
        let mut table = RouteTable::default();

        let a = Peer::new("aip1", "zone-a");
        let b = Peer::new("aip1", "zone-b");

        assert!(table.receive(&a, &[advert("brics", "brics.aip1")], &[], "clusters"));
        // The same portal name in another zone is a separate entry, not a
        // collision.
        assert!(table.receive(&b, &[advert("brics", "brics.aip1")], &[], "clusters"));

        assert!(!table.is_collided("zone-a", "brics"));
        assert!(!table.is_collided("zone-b", "brics"));

        // ...and a route is only ever advertised into its own zone.
        assert_eq!(
            table.routes_for_peer(&Peer::new("shared", "zone-b")).len(),
            1
        );
        assert_eq!(
            table.routes_for_peer(&Peer::new("shared", "zone-a")).len(),
            1
        );
    }

    #[test]
    fn test_no_backtrack_propagation() {
        let mut table = RouteTable::default();

        let aip1 = Peer::new("aip1", "default");
        assert!(table.receive(&aip1, &[advert("brics", "brics.aip1")], &[], "clusters"));

        // Never advertised back to the peer we learned it from - which in an
        // acyclic topology is the whole loop-prevention story.
        assert!(table.routes_for_peer(&aip1).is_empty());

        // ...but is advertised onward to everyone else.
        let onward = table.routes_for_peer(&Peer::new("shared", "default"));
        assert_eq!(onward.len(), 1);
        assert_eq!(onward[0].route(), &dest("brics.aip1.clusters"));
    }

    #[test]
    fn test_withdrawal_on_disconnect_allows_a_later_topology_change() {
        let aip1 = Peer::new("aip1", "default");
        let aip2 = Peer::new("aip2", "default");

        // A migration with the old route still held looks exactly like an
        // attack, which is why withdrawal matters.
        let mut table = RouteTable::default();
        assert!(table.receive(&aip1, &[advert("brics", "brics.aip1")], &[], "clusters"));
        assert!(!table.receive(&aip2, &[advert("brics", "brics.aip2")], &[], "clusters"));
        assert!(table.is_collided("default", "brics"));

        // With the disconnect withdrawing it first, the new route is clean.
        let mut table = RouteTable::default();
        assert!(table.receive(&aip1, &[advert("brics", "brics.aip1")], &[], "clusters"));
        assert!(table.withdraw_all_from(&aip1));
        assert_eq!(table.expected_route("default", "brics"), None);

        assert!(table.receive(&aip2, &[advert("brics", "brics.aip2")], &[], "clusters"));
        assert!(!table.is_collided("default", "brics"));
        assert_eq!(
            table.expected_route("default", "brics"),
            Some(dest("brics.aip2.clusters"))
        );

        // Only the peer that told us may retract a route.
        let mut table = RouteTable::default();
        assert!(table.receive(&aip1, &[advert("brics", "brics.aip1")], &[], "clusters"));
        assert!(!table.receive(&aip2, &[], &[portal("brics")], "clusters"));
        assert_eq!(
            table.expected_route("default", "brics"),
            Some(dest("brics.aip1.clusters"))
        );
        assert!(table.receive(&aip1, &[], &[portal("brics")], "clusters"));
        assert_eq!(table.expected_route("default", "brics"), None);

        // An originated route is never withdrawn by a peer disconnecting - it
        // comes from our own config, not from the network.
        let mut table = RouteTable::default();
        assert!(table
            .originate(&portal("brics"), "default", "aip1")
            .unwrap_or_else(|e| unreachable!("originate: {:?}", e)));
        assert!(!table.withdraw_all_from(&Peer::new("anyone", "default")));
        assert_eq!(
            table.expected_route("default", "brics"),
            Some(dest("brics.aip1"))
        );
    }

    #[test]
    fn test_bounds_are_enforced() {
        let aip1 = Peer::new("aip1", "default");

        // Too many portals from one peer - all rejected, not just the excess,
        // so a flood cannot partially populate the table either.
        let mut table = RouteTable::default();
        let many: Vec<PortalRoute> = (0..MAX_PORTALS_PER_PEER + 1)
            .map(|i| advert(&format!("portal{}", i), "brics.aip1"))
            .collect();
        assert!(!table.receive(&aip1, &many, &[], "clusters"));
        assert_eq!(table.expected_route("default", "portal0"), None);

        // Exactly at the limit is fine.
        let mut table = RouteTable::default();
        let at_limit: Vec<PortalRoute> = (0..MAX_PORTALS_PER_PEER)
            .map(|i| advert(&format!("portal{}", i), "brics.aip1"))
            .collect();
        assert!(table.receive(&aip1, &at_limit, &[], "clusters"));

        // Too deep a route.
        let mut table = RouteTable::default();
        let hops: Vec<String> = (0..MAX_ROUTE_DEPTH).map(|i| format!("a{}", i)).collect();
        let deep = format!("{}.aip1", hops.join("."));
        assert!(!table.receive(&aip1, &[advert("brics", &deep)], &[], "clusters"));
        assert_eq!(table.expected_route("default", "brics"), None);
    }

    #[test]
    fn test_zone_has_portal_route_separates_upstream_from_internal_zones() {
        // The signal that decides whether the portal rule applies at all. An
        // instance sits in two zones: an upstream one carrying portal-rooted
        // traffic, and an internal one holding the agents it delegates to. Only
        // the first should ever see the rule enforced.
        let mut table = RouteTable::default();

        assert!(!table.zone_has_portal_route("default"));
        assert!(!table.zone_has_portal_route("aip1-clusters-shared"));

        // A route learned upstream marks that zone, and only that zone.
        let clusters = Peer::new("clusters", "default");
        assert!(table.receive(
            &clusters,
            &[advert("brics", "brics.aip1.clusters")],
            &[],
            "shared"
        ));

        assert!(table.zone_has_portal_route("default"));
        assert!(!table.zone_has_portal_route("aip1-clusters-shared"));

        // ...and the route is never advertised into the internal zone, so it
        // cannot mark it by accident. This is what keeps `freeipa`, `slurm` and
        // `filesystem` unaware that a portal exists at all.
        assert!(table
            .routes_for_peer(&Peer::new("freeipa", "aip1-clusters-shared"))
            .is_empty());

        // Withdrawing the last route in a zone deactivates it again.
        assert!(table.withdraw_all_from(&clusters));
        assert!(!table.zone_has_portal_route("default"));
    }

    #[test]
    fn test_destination_matches_route_is_a_prefix_match() {
        let route = dest("brics.aip1.clusters");

        // Exactly the route, and the route with further hops beyond us.
        assert!(destination_matches_route(
            &dest("brics.aip1.clusters"),
            &route
        ));
        assert!(destination_matches_route(
            &dest("brics.aip1.clusters.shared"),
            &route
        ));

        // A different root - the case the R34 check already caught.
        assert!(!destination_matches_route(
            &dest("attacker.aip1.clusters"),
            &route
        ));
        // The right root reached by the wrong path - what R34 alone misses, and
        // the reason this scheme exists.
        assert!(!destination_matches_route(
            &dest("brics.fake.clusters"),
            &route
        ));
        assert!(!destination_matches_route(&dest("brics.clusters"), &route));
        // Too short to contain the route at all.
        assert!(!destination_matches_route(&dest("brics.aip1"), &route));
    }

    #[tokio::test]
    async fn test_global_wrappers_delegate_to_the_table() {
        // Thin coverage of the async wrappers themselves. Uses a zone unique to
        // this test, since the global table is shared with every other test in
        // the binary.
        let zone = "portalroutes-wrapper-test";
        let aip1 = Peer::new("aip1", zone);

        assert_eq!(expected_route(zone, "brics").await, None);
        assert!(receive(&aip1, &[advert("brics", "brics.aip1")], &[], "clusters").await);
        assert_eq!(
            expected_route(zone, "brics").await,
            Some(dest("brics.aip1.clusters"))
        );
        assert!(!is_collided(zone, "brics").await);
        assert!(withdraw_all_from(&aip1).await);
        assert_eq!(expected_route(zone, "brics").await, None);
    }
}
