<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# Portal route discovery: deriving the expected path from a portal

Status: **implemented**, with enforcement. `templemeads::portalroutes` exists as
designed below, wired into `handler.rs` (origination at startup, advertisement
on `Register`, receipt and re-advertisement, and the enforcement check),
`control_message.rs` (withdrawal on disconnect), and `command.rs`
(`Command::PortalRoutes` plus the `supports_portal_routes` capability flag on
`Register`). Covered by 17 unit tests.

The phased rollout of [§9](#9-phased-implementation) was **collapsed into a
single step at the maintainer's direction**: the detect-only phase existed to
validate the single-path assumption against a live estate, and two years of
operation with no topology change is stronger evidence than a soak period would
have produced. Any future change requires operator work, which carries its own
testing.

This closes the residual recorded in
[security-review-2.md](../specifications/security-review-2.md) §4.1 - that an
agent cannot tell a genuine portal from an impostor that has been provisioned
under the same name in the right topology position - without introducing
signing, asymmetric cryptography, or any hard-coded topology in an agent's
config.

## 1. Goal

An agent that receives an instruction naming portal `brics` should be able to
check that it arrived **by the route it should have arrived by**, and to notice
when two different routes claim to lead to the same portal.

Each agent learns that route from the network rather than being told it:

```
brics.aip1.clusters.shared
```

- `brics` is a portal. It originates nothing.
- `aip1` knows from **its own config** (`type = "portal"` on its `brics` peer
  entry) that `brics` is a portal. It therefore originates `brics.aip1` and
  pushes it to its non-portal peers.
- `clusters` receives `brics.aip1` from `aip1`, extends it to
  `brics.aip1.clusters`, and pushes that to *its* non-portal peers.
- `shared` receives `brics.aip1.clusters` and knows its own expected route is
  `brics.aip1.clusters.shared`.

`shared` can then reject any instruction naming `brics` that does not arrive on
that path, and can alarm if it is ever told about two different routes to a
portal called `brics`.

## 2. Non-goals

- **Proving portal authority cryptographically.** That is signing, designed out
  and deliberately deferred in
  [security-review-2.md](../specifications/security-review-2.md) §4.1. This
  scheme establishes *topological consistency*, which is a different and weaker
  property - see [§8](#8-security-properties).
- **Defending against a code-compromised intermediate.** An agent whose binary
  the attacker controls simply reports whichever route it likes. This scheme
  detects an agent whose **config or state** was modified while its code
  remained intact - which is exactly the residual §4.1 accepted, but it is not
  more than that.
- **Multi-path or cyclic topologies.** The design assumes, and depends on,
  exactly one route between any pair of agents (see [§3](#3-what-this-depends-on)).
- **Replacing the [R34](../specifications/security-review-2.md#r34)
  portal-ownership check.** That check keeps working with no protocol, no state
  and no peer cooperation, and remains the fallback for an agent that has not
  yet learned a route.

## 3. What this depends on

Three properties, each already true or already implemented:

1. **The topology is single-pathed and acyclic.** There is exactly one route
   between any pair of agents. This is a deliberate deployment property, and it
   is what makes both the propagation rule terminate and the collision rule
   sound. A topology that violated it would produce false collisions.
2. **An agent's own config declares which of its peers is a portal**
   ([R3](../specifications/security-review-2.md#r3)'s `type = "portal"`). This
   is the trust anchor: it is the only statement about portal identity an agent
   accepts without derivation.
3. **Routing is confined to a zone**, and a Job keeps the zone it arrived in as
   it is forwarded. Portal routes are therefore per-zone too ([§4.4](#44-zones)).

It also composes with, but does not depend on,
[R4](../specifications/security-review-2.md#r4)'s sender-adjacency check.
Adjacency proves each hop holds the key for the position it claims; a route
match proves the whole claimed path is the one the topology reports. Neither
implies the other, and both are cheap.

## 4. The protocol

### 4.1 Origination

An agent originates a route for each peer its **own config** declares
`type = "portal"`, in that peer's zone. The originated route is
`<portal>.<me>`.

A portal originates nothing about itself. An agent with no portal peer
originates nothing.

### 4.2 Propagation

On learning a route (by origination or from a peer), an agent sends it to
**every peer except**:

- the peer it learned that route from, and
- any peer declared `type = "portal"`.

Before sending, the agent appends its own name: having learned
`brics.aip1`, `clusters` sends `brics.aip1.clusters`.

In an acyclic single-path topology, no-backtrack propagation terminates and
reaches every agent exactly once. That is the whole loop-prevention story - no
TTL, no visited-set, no sequence numbers.

Propagation is **uniform**: routes are sent to every eligible peer, including
Account/Filesystem/Scheduler agents that will never check them. Gating
propagation on whether the recipient checks would couple two rules that are
better kept separate, for no saving worth having.

### 4.3 Acceptance rules

A received route is accepted only if **it ends with the sending peer's own
name**.

This is what prevents a *downstream* peer injecting a route upstream. If
`shared` advertises `brics.aip1.clusters` to `clusters`, the route ends in
`clusters`, not `shared` - inconsistent, so it is rejected and logged. If
`shared` instead advertises `brics.aip1.clusters.shared`, that passes the
consistency check, but `clusters` would extend it to
`brics.aip1.clusters.shared.clusters` and collide with the `brics.aip1` it
already holds - so the collision rule ([§4.5](#45-collision)) catches it. The
two rules cover each other.

A route is also rejected if it exceeds `MAX_ROUTE_DEPTH`, or if accepting it
would take this peer above `MAX_PORTALS_PER_PEER` - see [§4.6](#46-bounds).

### 4.4 Zones

Routes are scoped to a zone and never cross one. The table is keyed on
`name@zone`, matching every other peer registry in the framework. An agent
present in two zones keeps two independent tables and never propagates between
them.

This matters because portal-to-portal traffic has its own zone with its own
rules. A route learned in the portal-to-portal zone must never influence
routing decisions in an operator's estate zone.

### 4.5 Collision

The table maps `portal name -> route`, per zone.

- Re-advertising the **same route** for a portal is a no-op.
- Advertising a **different route** for a portal name already in the table is a
  collision.

A collision means two distinct paths claim to lead to the same portal, which in
a single-path topology cannot legitimately happen. It is the signature of an
agent whose config has been modified to introduce a second, impostor portal.

**On collision**: log at error level, raise an operator-visible notification,
and refuse to route instructions naming *that portal name* - and only that
portal name.

This is deliberately narrower than "disconnect the peer and enter a safe
state". A global safe state would let an attacker who can add one peer to one
compromised agent deliberately take down everything downstream of it, at will
and repeatably - converting a detection mechanism into an amplification
primitive. Confining the response to the affected portal keeps the full
detection value while bounding the blast radius of both a genuine attack and a
false positive.

**Withdrawal**: when a peer disconnects, the routes learned from it are
withdrawn (and the withdrawal propagated). Without this, a planned migration -
old path torn down, new path built - would present as a collision.

### 4.6 Bounds

A peer can advertise arbitrarily many portal names with arbitrarily long
routes. Both are bounded:

- `MAX_PORTALS_PER_PEER` - a small cap (16 is generous; a zone has very few
  portals).
- `MAX_ROUTE_DEPTH` - a small cap (16 comfortably exceeds
  portal→provider→platform→instance).

Exceeding either is logged and the advertisement rejected.

### 4.7 Where the check applies

The route check is applied wherever the
[R34](../specifications/security-review-2.md#r34) portal-ownership check is
applied - i.e. where `verify_portal_ownership` is set, which is Provider,
Platform and Instance agents, but *not* `instance::run_delegated` Instances such
as `op-cloudaccount`, and not Account/Filesystem/Scheduler agents, whose Jobs are
rooted at their delegating instance rather than at a portal.

Given a Job naming portal `P` arriving at agent `A`:

1. If `verify_portal_ownership` is off for `A`, no check (unchanged).
2. If `A` holds no route for `P`, see [§5](#5-ordering).
3. Otherwise the Job's destination must have the stored route as a **prefix**,
   with `A` at the correct position.

Prefix-matching is strictly stronger than R34's root check, which compares only
`destination.first()`.

## 5. Ordering

The claim that a route always arrives before any Job that needs it holds at the
wire level and **fails at the processing level**.

`paddington::exchange`'s event loop spawns one task per inbound message
([exchange.rs:258](../../paddington/src/exchange.rs)), so a WebSocket gives
ordered *delivery* but templemeads performs concurrent *processing*. Two
messages arriving back-to-back on one connection can be handled in either
order. There is also a case where the push genuinely cannot precede the Job:
an agent that connects last may receive a route push and a Job that was already
queued for it at essentially the same moment.

So the "no route yet" state must be handled explicitly rather than assumed
away. It is handled as **fail hard with a bounded wait**:

- If a Job names portal `P` and no route for `P` is yet known from the peer
  that delivered it, wait up to `ROUTE_WAIT_SECONDS` for one to arrive.
- If a route arrives within that window, apply the check normally.
- If none arrives, reject the Job.

Nothing is accepted without a route - this only distinguishes "not yet" from
"wrong". It reuses an existing idiom: `agent::wait_for(&peer, 30)` already does
exactly this in the Update and Delete paths, and the framework already waits for
a peer to `Register` before routing to it.

## 6. Wire format

A new variant on `templemeads::command::Command`:

```rust
PortalRoutes {
    /// Routes being advertised, each already extended with the sender's own
    /// name (so each ends with the sender - see §4.3).
    routes: Vec<PortalRoute>,
    /// Routes being withdrawn, by portal name (see §4.5).
    #[serde(default)]
    withdrawn: Vec<PortalIdentifier>,
}
```

with

```rust
struct PortalRoute {
    portal: PortalIdentifier,
    route: Destination,
}
```

`PortalIdentifier` validates against the identifier allow-list
([R18](../specifications/security-review-2.md#r18)) and `Destination` requires
at least two agents, which an originated route (`<portal>.<me>`) always
satisfies. Both therefore reject malformed input at deserialisation.

Sent on connection establishment, from `control_message.rs`'s `Connected`
handling - the same lifecycle point at which `Register` and `sync_board`
already fire - and again whenever an agent's own table changes. It must be sent
**after** `Register` in both directions, because both the origination rule
(§4.1) and the collision response depend on the declared types R3 supplies.

## 7. Backwards compatibility and rollout

Upstream agents (the paddington *listening* side, e.g. portals and providers)
are upgraded before downstream agents (the *dialling* side). That is the
deployment reality, and it happens to be the correct order for this feature:

- **New upstream, old downstream.** The old agent receives an unrecognised
  `Command`. `From<Message> for Command<L>`
  ([command.rs:331](../../templemeads/src/command.rs)) turns an unparseable
  payload into `Command::Error` rather than failing the connection, so this
  degrades to a logged error per push - not fatal, but noisy.

  **What was built instead: an explicit capability flag.** `Register` carries
  `supports_portal_routes: bool` (`#[serde(default)]`, so a peer that predates
  the field reads as `false`), and it is exchanged before any push - so a peer
  that would not understand the message is never sent one, and the
  `Command::Error` degradation above never occurs in practice. This is both
  simpler and more precise than parsing version strings, and it follows the
  existing precedent of `supports_nonce` on `PeerDetails`
  ([replay-protection-design.md](replay-protection-design.md) §5).

- **Old upstream, new downstream.** The new agent never receives a route. It
  must therefore treat "no route table for this peer at all" as *unchecked*,
  exactly as R3 treats an undeclared `type` - the same "absent means do not
  check" rule, so that a partially-upgraded fleet keeps working. This is
  distinct from §5's "route expected but not yet arrived", which waits and then
  rejects.

  Distinguishing the two requires knowing whether the peer is *capable* of
  sending routes, which the capability flag gives us directly. Without it the two
  states are indistinguishable and the feature could not fail hard safely.

- **Both new.** Full enforcement.

**Rollout is therefore: upgrade upstream → upgrade downstream.** Enforcement
switches itself on per peer as both ends become capable, so there is no separate
enable step. The same server-before-client shape as the salt-format rollout in
round 1's F15.

## 8. Security properties

**What it catches.** An agent whose config or state has been modified to
introduce an impostor portal, while its code remains intact. Concretely, both
forms of the residual recorded in §4.1:

- the attacker adds an intermediate peer `fake` to `clusters` and connects an
  impostor `brics` behind it - `clusters` derives both `brics.aip1` and
  `brics.fake`, and collides;
- the attacker adds the impostor directly to `clusters` as `type = "portal"` -
  `clusters` then holds a direct origination for `brics` *and* a learned route
  `brics.aip1`, and collides.

In both cases the detection happens at the compromised agent itself, which is
precisely where a code-intact agent will report honestly.

**What it does not catch.** A code-compromised agent, which reports one route
and lies. Nothing here helps, and little can: at some point the hierarchy has to
trust something. This is the boundary at which signing - and only signing -
would still hold, which is why §4.1 keeps it designed but unbuilt rather than
discarded.

**What it costs an attacker who is not detected.** Nothing changes: they must
still satisfy adjacency (R4), portal-ownership (R34), and the declared peer type
(R3).

## 9. Phased implementation

This was originally planned in three phases. In the event it was implemented in
one, for the reason given in the status note at the top.

1. ~~**Retain peer engine/version** in the registrar.~~ **Not needed.** A
   capability flag on `Register` (`supports_portal_routes`, `#[serde(default)]`)
   turned out to be both simpler and more precise than comparing version
   strings, and it follows the existing precedent of `supports_nonce` on
   `PeerDetails`. See [§7](#7-backwards-compatibility-and-rollout).
2. ~~**Derive and report**, with no enforcement.~~ Implemented, but not run as a
   separate phase.
3. ~~**Enforce.**~~ Implemented in the same change: the prefix check of §4.7 and
   the bounded wait of §5.

## 10. Testing

- Propagation over a synthetic tree: every agent derives the route the topology
  implies, exactly once, with no backtracking.
- Acceptance: a route not ending in the sender's name is rejected; a route
  exceeding the depth or count bounds is rejected.
- Collision: a second, different route for a known portal name alarms and
  disables that portal name only, leaving other portals routable.
- Withdrawal: a disconnect removes the route and propagates, and a subsequent
  re-advertisement of a *different* route does not alarm.
- Zones: a route learned in one zone never appears in another's table.
- Ordering: a Job arriving before its route waits and then succeeds if the route
  arrives, and is rejected if it does not.
- Backwards compatibility: an agent with no route table for a peer accepts Jobs
  from it unchecked; an old-shaped `Command` payload still deserialises to
  `Command::Error` rather than failing the connection.

## 11. Relationship to other documents

- [security-review-2.md](../specifications/security-review-2.md) §4.1 - the
  accepted residual this closes, and the signing design it defers.
  [R3](../specifications/security-review-2.md#r3) supplies the trust anchor,
  [R4](../specifications/security-review-2.md#r4) the adjacency check this
  composes with, and [R34](../specifications/security-review-2.md#r34) the
  ownership check it strengthens from a root comparison to a prefix match.
- [security-model.md](../specifications/security-model.md) §4/§6 - the
  four-layer authentication and zone model this sits on top of.
- [replay-protection-design.md](replay-protection-design.md) §5 - precedent for
  a negotiated, gradual rollout of a wire-format change.
- [highavailability.md](../specifications/highavailability.md) - client HA means
  several processes share one peer identity. They present the same name, so they
  do not produce distinct routes and do not collide; server HA via `op-proxy`
  likewise leaves the names in a route unchanged.
