<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# High Availability

This document describes how OpenPortal agents can be run redundantly so that
the failure of a single host does not take down a peer relationship. There
are two independent mechanisms:

- **Client HA** ([§2](#2-client-ha-direct-connections)) — several physical
  processes present the same identity to one server; the server arbitrates
  which one is "primary" and the rest wait in standby. This has existed
  since early in the project.
- **Server HA** ([§3](#3-server-ha-via-the-blind-relay-proxy)) — several
  physical processes present the same identity to a shared `op-proxy`
  instead of to a real listening server; the proxy applies exactly the
  same client-HA arbitration to *them*, and the existing blind-relay
  bootstrap/recovery machinery transparently handles routing traffic to
  whichever one is currently live. This is a later insight - it was not
  designed in deliberately, it fell out of composing three mechanisms that
  already existed for unrelated reasons (client HA itself, the blind relay
  proxy, and the `SessionUnknown` restart-recovery path) - see §3.1 for the
  trace through the actual code.

[§4](#4-why-a-brief-failover-outage-is-not-a-problem) explains why the
short gap in connectivity during either kind of failover is expected to be
invisible (or at worst a harmless retry) to portal software, and
[§5](#5-practical-deployment-guidance) gives guidance on where HA is
actually worth deploying.

---

## 1. Overview

An OpenPortal agent's role in a `Job`'s path ([instruction-protocol.md](instruction-protocol.md),
[wire-protocol.md](wire-protocol.md) §1) is entirely stateless from one
message to the next - see §4.3 for what "stateless" means here and its one
current exception. That statelessness is what makes both kinds of HA in
this document possible without any special coordination between replicas:
any replica that ends up "primary" or "live" can pick up exactly where a
failed one left off, because there is no replica-local state to hand over
in the first place.

Both mechanisms share the same underlying primitive:
`paddington::exchange::check_standby`/`locked_register`
([exchange.rs](../../paddington/src/exchange.rs)) arbitrate, for a given
peer identity (`name@zone`), which of possibly several physical
connections claiming that identity is "primary" - the rest are told
they're secondary and wait. Client HA uses this directly, on a real
listening server. Server HA uses the exact same arbitration, just applied
to connections *to the proxy* rather than to the real destination server -
see §3.

---

## 2. Client HA (direct connections)

### 2.1 The scenario

Several physical processes for the same logical client (e.g. two `op-cluster`
instances on separate management nodes, both configured with the identical
name, zone, and pre-shared key pair for their connection to a given server)
connect to one real, listening server. Only one can usefully be "the"
connection at a time; the rest should wait, and one of them should be
promoted automatically if the active one fails.

### 2.2 How it works

- When a server accepts a new connection, before completing the handshake
  it calls `exchange::check_standby(peer_name, peer_zone)`
  ([exchange.rs:517](../../paddington/src/exchange.rs)), which checks
  whether it already has a *registered* connection under that identity:
  - No existing connection → `StandbyStatus::primary()`.
  - An existing connection → `StandbyStatus::secondary_client()` (the new
    connection is told it's the secondary one).
- This status rides home in the server's `PeerDetails` reply
  ([wire-protocol.md](wire-protocol.md) §4.3). A client told it's
  secondary enters a polling loop
  ([connection.rs:921-977](../../paddington/src/connection.rs)): once a
  second, it sends a `CheckStandby` control message and waits for a fresh
  `StandbyStatus` back. The server re-runs `check_standby` on every poll
  and mirrors the same loop from its side
  ([connection.rs:1509-1594](../../paddington/src/connection.rs)).
- Only once a client is told `is_primary()` does it proceed past the
  handshake and call `exchange::register`
  ([exchange.rs:643](../../paddington/src/exchange.rs)) - which is also
  where the *previous* primary's connection, once it fails, gets removed:
  any I/O error on a connection's read/write loop calls
  `set_error`/`closed_connection`
  ([connection.rs:549](../../paddington/src/connection.rs)), which calls
  `exchange::unregister` immediately (backstopped by a 300-second
  staleness watchdog for a connection that's silently stuck rather than
  cleanly closed - [connection.rs:453](../../paddington/src/connection.rs)).
- **Failover timing**: bounded by however fast the server's own transport
  notices the primary's connection died, plus up to one second for the
  secondary's next poll. Not instant, but typically sub-second to a few
  seconds.
- A `StandbyWaiter` RAII guard ([exchange.rs:191](../../paddington/src/exchange.rs))
  tracks how many connections are currently waiting in standby for a given
  identity, purely as a DoS safeguard - more than 16 simultaneous standby
  connections for one identity causes new ones to be rejected outright
  ([exchange.rs:530](../../paddington/src/exchange.rs)).
- Standby (secondary) connections are not idle while they wait: they still
  receive `Sync` job-board updates from the server
  ([wire-protocol.md](wire-protocol.md) §4.4), so a promoted secondary
  starts from a reasonably fresh view of in-flight jobs rather than an
  empty board - on top of the full reconciliation §4.1 describes for
  every fresh connection regardless.

### 2.3 A currently-unused counterpart

`StandbyStatus` also has a `server_is_secondary` flag and the exchange has
a matching `set_is_secondary()`/`set_is_primary()` pair
([exchange.rs:378-422](../../paddington/src/exchange.rs)), intended for a
server to mark *itself* as a standby replica. Nothing in the codebase
today ever calls `set_is_secondary()`/`set_is_primary()` - this path is
unreachable in practice, reserved scaffolding rather than an active
feature. It is not what makes server HA possible - that comes from an
entirely different mechanism, §3.

### 2.4 The limitation this document exists to address

None of the above helps if the thing being made redundant is a **server**
(something other agents dial into) rather than a client - you cannot
usefully run three physical processes all listening on the same
`ip:port`, and a client has no way to be told "try this other address
instead" mid-connection. That asymmetry is why, until now, only client-side
redundancy was considered practical.

---

## 3. Server HA (via the blind relay proxy)

### 3.1 The insight, traced through the actual code

`op-proxy` ([docs/plans/archive/blind-relay-proxy-design.md](../plans/archive/blind-relay-proxy-design.md)
- see also `paddington::relay`) was built to let two agents that can each
only make outbound connections still talk to each other, by relaying
opaque ciphertext between them. It was not built with HA in mind. But
composing it with §2's already-existing client-HA arbitration turns out to
give genuine server HA, for free:

1. **Several physical server processes can present the identical identity
   to the proxy.** A relayed "server" role (`RelayedRole::Server` in
   `paddington::relay` - a `clients` config entry with `proxy` set) still
   makes an entirely ordinary, *direct* paddington connection to the proxy
   itself. If server1, server2, and server3 are all configured with the
   same name, zone, and pre-shared key pair for that connection, the proxy
   sees three physical connections claiming one identity - exactly §2's
   scenario. §2's arbitration applies completely unmodified: the proxy
   registers whichever connects first as primary and holds the rest in
   standby, promoting one automatically if the primary's connection dies.
2. **The proxy always routes to whoever is currently registered, not to a
   specific process.** `proxy_handler` forwards a `RelayEnvelope` via
   ordinary `exchange::send(Message::send_to(&envelope.to, ...))` - a
   name-keyed registry lookup, with no idea that "server" might currently
   mean server1 or server2. Whichever of them is primary at that moment is
   where traffic goes, automatically.
3. **`SessionUnknown`, built for restart recovery, is structurally
   identical to failover recovery.** A relayed session's ephemeral keys
   (`RelayedSession`, `paddington::relay::SESSIONS`) live in one process's
   memory - they are not, and cannot be, shared across server1/2/3. When
   server2 is promoted and next receives relayed traffic, it has no
   session for that peer - indistinguishable, from its own point of view,
   from having just restarted and lost its session. It sends
   `SessionUnknown` exactly as the restart-recovery design already
   specifies ([wire-protocol.md](wire-protocol.md) §7.3); the relayed
   client (the only side that can initiate) clears its cached session and
   immediately re-bootstraps against whoever the proxy now routes it to -
   server2. No code path needed to know a failover, rather than a restart,
   had occurred.

None of these three mechanisms were written with each other in mind. Server
HA is an emergent property of composing them, not a feature that was
deliberately implemented - which is also why it needs stating explicitly
here, rather than being obvious from reading any one of the three pieces
of code in isolation.

### 3.2 Requirements

- server1/2/3 must be configured with the **exact same paddington identity**
  (name, zone, and pre-shared key pair) for their connection to the proxy -
  a deliberate operational step (copying the same invite/keys to every
  replica), the same requirement §2's client HA already has for redundant
  clients.
- The proxy's `RelayPolicy` needs only a single `allow` pair covering that
  shared name - it has no idea multiple physical processes exist behind
  it, since the policy is name-keyed, not connection-keyed.
- The agent being made HA this way must be **server-only** - see §3.4.

### 3.3 Failover sequence, end to end

1. server1 is primary on the proxy; server2 and server3 sit in standby,
   polling once a second.
2. server1's host crashes. The proxy's own connection to it fails; its
   entry is unregistered (§2.2's mechanism, unmodified).
3. server2's (or server3's) next `CheckStandby` poll sees no existing
   registration for that identity and is told it's primary; it registers.
4. The relayed client's next attempt to reach "server" - whether real
   application traffic or the periodic keepalive
   ([connection.rs:1117](../../paddington/src/connection.rs)) - gets
   forwarded by the proxy to server2 (now the registered connection).
5. server2 has no `RelayedSession` for this peer; it replies
   `SessionUnknown`.
6. The relayed client clears its session and re-bootstraps
   (`StartRelayedConnection`); the proxy forwards this to server2; server2
   completes the bootstrap and a fresh session is established (fresh session
   keys, not forward secrecy - see [security-model.md](security-model.md) §2.5).
7. Traffic flows again, now via server2, with the client and any upstream
   agent none the wiser about which physical replica is behind "server" -
   see §4.1 for how in-flight jobs are reconciled once this reconnects.

### 3.4 What this does not cover

An agent that is simultaneously a *client* of some peers and a *server* to
others cannot be made HA this way - or at least, not without materially
more design work than this document covers. Client HA (§2) and server HA
(§3) each assume the agent being made redundant plays only one of those
two roles in the relationship being protected; an agent playing both
roles at once has no clean way to reconcile "which of me is primary" for
each role independently. This is a deliberate scope limit, not an
oversight: most of OpenPortal's agent types are cleanly one or the other
for any given peer relationship, so this covers the common case rather
than the general one.

A smaller, purely operational wrinkle: `paddington::eventloop::run`
starts a real `TcpListener` whenever an agent has any `clients()` entry at
all ([eventloop.rs:30-33](../../paddington/src/eventloop.rs)), even if
every one of them is relayed-only and the agent will never receive a real
inbound connection on that port. Harmless (an unused open port) as long as
replicas don't collide on it, but worth knowing when planning where
replicas are deployed.

---

## 4. Why a brief failover outage is not a problem

Both kinds of failover above take a real, non-zero amount of time - typically
sub-second to a few seconds, never instant. Several independent layers of
the system mean this is expected to surface as, at worst, a harmless retry
rather than a visible failure.

### 4.1 Board sync reconciles in-flight jobs on every fresh connection

Every time a Templemeads connection completes its handshake, it goes
through `Register` then `Sync`
([wire-protocol.md](wire-protocol.md) §5) - the newly (re)connected side
sends its entire current job board state, and `sync_from_peer`
(`templemeads/src/job.rs`) reconciles it against what the recipient
already knows: jobs it hasn't seen are queued, jobs that already completed
are just updated rather than re-run, and jobs still in flight are resumed
from wherever the recipient's own board thinks they are. This isn't
special-cased HA logic - it's the same reconciliation that already runs
after *any* reconnect (a network blip, a process restart), and it applies
identically whether the reconnect happened because a process restarted or
because a standby replica was just promoted. This is the auto-healing
layer that makes §2/§3's connectivity gap safe to have at all: nothing
needs to be "handed over" between replicas, because the next handshake
resynchronises the board from scratch.

### 4.2 Retries and idempotency turn a blip into a retry, not a failure

Every OpenPortal command is designed to be safe to retry. Duplicate `Put`s
for a job already pending are recognised and reconciled rather than
double-executed - `templemeads` tracks up to 100 duplicates per original
job and resolves them all to the original's eventual outcome
([notes.md](notes.md) §2). Portal software talking to OpenPortal (directly,
or via `op-bridge`) receives an explicit exception or error for a failed
request, and is expected to retry it - which is safe precisely because
commands are idempotent. In practice, a failover-induced gap is expected
to be seen by portal software as, at worst, one retried request rather
than a lost or duplicated one.

### 4.3 Agents are stateless (with one temporary, known exception)

OpenPortal agents are designed to hold no state of their own beyond a
`Job`'s board entry - which is exactly the state §4.1's board sync already
reconciles on reconnect. This is *why* any replica can pick up from any
other with no special handover step: there is nothing replica-specific to
lose.

The one current exception is the cloud agents (`op-cloudaccount`,
`op-cloudportal`) - see the crate notes in
[../../CLAUDE.md](../../CLAUDE.md). They hold project/user assignment or
Award state as local JSON files because there is not yet any real
cloud-side portal software to push that state to. This is understood to
be a temporary gap tied to the cloud integration's maturity, not a
statement that OpenPortal agents are meant to be stateful in general - it
should close once proper portal software exists on that side.

### 4.4 What this doesn't claim

None of the above means a failover is entirely free - it's a real gap in
connectivity, and a job whose result was needed during exactly that window
will see a delay or a retry. The claim is narrower and more useful: the
combination of board-sync reconciliation, idempotent commands, and
portal-side retry means that gap is not expected to surface as data loss
or a permanently failed job, just as a brief, recoverable blip.

---

## 5. Practical deployment guidance

The most valuable place to deploy HA is wherever a real **host** failure is
plausible - a physical or virtual machine going down entirely, not a
process crashing. Leaf/client agents (the edges of the network - e.g. the
agents nearest a portal or nearest the actual infrastructure being
managed) are typically the ones deployed across multiple independent
management nodes for exactly this reason, and are the natural fit for
client HA (§2).

Most other agents run inside a process supervisor (systemd) or an
orchestrator (Kubernetes) that already restarts an unhealthy process
automatically - covering the far more common failure mode (a crashed or
hung process, not a dead host) without needing either HA mechanism in this
document at all. Server HA (§3) is worth reaching for on top of that when
an agent is server-only and the underlying host itself - not just the
process - is a plausible failure point; it composes with, rather than
replaces, orchestrator-level auto-restart.

---

## 6. Source File Reference

| Concept | Source file |
|---------|-------------|
| `StandbyStatus`, `PeerDetails.standby_status` | `paddington/src/connection.rs` |
| Client-side standby polling loop (`make_connection`) | `paddington/src/connection.rs` |
| Server-side standby mirroring loop (`handle_connection`) | `paddington/src/connection.rs` |
| `check_standby`, `locked_register`, `register`, `unregister`, `StandbyWaiter` | `paddington/src/exchange.rs` |
| `set_is_server`, `is_client_only`, `set_is_secondary`/`set_is_primary` (unused) | `paddington/src/exchange.rs` |
| Connection watchdog / staleness detection | `paddington/src/connection.rs` (`Connection::watchdog`) |
| Blind relay bootstrap, `RelayedSession`, `RelayPolicy`, `SessionUnknown` | `paddington/src/relay.rs` |
| Server listener startup (`TcpListener::bind`), client-loop startup | `paddington/src/eventloop.rs`, `paddington/src/server.rs` |
| `Register`/`Sync` handshake sequence | `templemeads/src/handler.rs`, `templemeads/src/job.rs` (`sync_from_peer`) |
| Duplicate job detection and resolution | `templemeads/src/board.rs`, `templemeads/src/job.rs` |
