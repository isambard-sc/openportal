<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# A blind relay proxy for outbound-only agents

Status: **implemented and integrated into every real agent** (§7 all
steps). `paddington::relay` (bootstrap, envelope forwarding,
`RelayPolicy`), the `proxy` config field, the self-describing `Invite`
(carries the relay's name so the importing side auto-detects it), the
`op-proxy` binary, and the `client --add --proxy` / auto-detecting
`server --add` CLI are all implemented and covered by unit tests (mutual
key contribution, blindness of the raw bytes, wrong-key rejection,
default-deny policy in both directions, config round-tripping).

Every `templemeads`-based agent can now act as a relayed peer -
`templemeads::handler::run_with_relay` (called by every agent's `run()`
instead of `paddington::set_handler`/`paddington::run` directly) wires up
`relay::configure`, the relay dispatch handler, and
`bootstrap_all_as_client`; `paddington::exchange::send` transparently
falls back to the relay for a peer with no real connection, so relaying
is invisible to templemeads' own `Command::send_to` and keepalive code;
`paddington::eventloop::run` skips dialling relayed `servers` entries
directly.

Step 5's live three-process end-to-end test **has now been done**, for
real: `op-proxy` + `op-portal` + `op-cloudportal`, three genuinely
separate OS processes (not tokio tasks in one test - paddington's
connection registry and this design's relay state are process-global
singletons, the same constraint noted for the `Erased` design's live
smoke test in
[multi-domain-routing-design.md](multi-domain-routing-design.md)). The
bootstrap completed, both sides logged their synthesised `Connected`
event, and a real templemeads `Register` command sent immediately
afterwards was relayed and processed correctly on the other side - proof
that ordinary templemeads traffic, not just the bootstrap handshake
itself, works transparently through the proxy. See
[agent-configuration.md](../../specifications/agent-configuration.md)
§3.11.1 for the exact command sequence used.

**Post-implementation refinements**, found through real multi-process
testing rather than anticipated in the design below (the code and its own
doc comments are the authoritative record; this is a pointer, not a
duplicate):

- Bootstrap retries indefinitely (matching `client::run`'s cadence for
  direct connections) instead of giving up after one attempt, so
  startup-ordering races (the other side not connected to the proxy yet)
  resolve themselves.
- A relayed session's zone (meaningful to the two relayed peers) and the
  real connection's zone to the relay itself (what paddington's own
  connection registry is keyed on) are tracked and used separately, on
  both the relayed peers' side and the proxy's own side - they are very
  often, but not necessarily, the same zone.
- A `SessionUnknown` bootstrap message (encrypted with the permanent
  pre-shared key, so the proxy can't forge it) lets a relayed *server*
  that restarted and lost its session state tell its relayed *client*
  peer to redo the handshake immediately, rather than silently dropping
  every message until something else notices.
- The "one proxy per service" restriction floated in §4.3 below turned
  out to be an unnecessary simplification - nothing in the protocol
  itself required it (each relayed peer already named its own relay
  independently), so it was removed: a service can freely use different
  proxies for different relayed peers.

## 1. Goal

Two agents - say `airr` and `brics` - both sit behind networks that only
permit *outbound* connections; neither can open a port the other can reach.
Today's paddington model requires exactly one side of any pair to be a
"server" (listening) and the other a "client" (dialling) - if neither can
listen, they simply cannot connect to each other at all.

Add a relay agent (`op-proxy`) that both `airr` and `brics` connect to as
ordinary outbound clients, which then transparently carries traffic between
them - such that, above the relay mechanism itself, `airr` and `brics`
interact exactly as if directly connected: same handshake-established trust,
same message flow, same templemeads-level view of each other as a normal
peer.

**The proxy must be blind**: it relays without ever being able to decrypt
the actual `airr`↔`brics` payload. It legitimately learns *metadata* (that
`airr` and `brics` are talking, roughly how much and when) but never
*content*. This is a hard requirement, not a nice-to-have - a relay that
could read everyone's traffic would itself become exactly the kind of
centralised, privileged position OpenPortal's whole peer-to-peer design
otherwise avoids (see [security-model.md](../../specifications/security-model.md)).

## 2. Non-goals

- **General N-to-N mesh discovery.** The proxy relays specific,
  operator-configured `(from, to)` pairs it's been told to bridge - it does
  not auto-discover or freely interconnect every client that happens to
  connect to it. Two agents connected to the same proxy are *not*
  reachable to each other through it unless explicitly authorised (§4.3).
- **Store-and-forward.** Both real hops (`airr`↔proxy and proxy↔`brics`)
  must be live simultaneously for a relayed message to get through. If
  either is down, delivery fails exactly the way a direct connection failure
  fails today (queued/retried at the paddington/templemeads level as
  normal) - the proxy holds no persistent queue of its own.
- **Anonymity or traffic hiding.** This is a relay, not a mixnet. The proxy
  (and anyone observing it) can see connection metadata - who is talking to
  whom, approximate timing and volume. Only *content* is hidden.
- **Changing `paddington`'s core `Connection`/handshake protocol.** Both
  real hops are ordinary, unmodified paddington connections. Everything new
  in this design sits in the message-dispatch layer (§4.2) and config
  (§4.3), not in `connection.rs`'s handshake state machine.

## 3. Current state: why this doesn't already work

Traced through the actual code, not assumed:

- `Message` ([message.rs:10-15](../../../paddington/src/message.rs#L10)) is a flat
  `{sender, recipient, zone, payload}` struct. There is no "final
  destination distinct from the peer I'm actually connected to" concept
  anywhere in it.
- `Exchange::send()` ([exchange.rs:685-703](../../../paddington/src/exchange.rs#L685))
  resolves the target purely by looking up `connections: HashMap<String,
  Connection>` ([exchange.rs:108](../../../paddington/src/exchange.rs#L108))
  keyed by `get_recipient(&message)` - i.e. `message.recipient` must name a
  peer this process has an actual, live, handshake-completed `Connection`
  to. There is no forwarding table, no indirection.
- On receipt, the event loop unconditionally overwrites the incoming
  message's recipient with this agent's own name -
  `message.set_recipient(&name)`
  ([exchange.rs:256](../../../paddington/src/exchange.rs#L256)) - immediately
  before dispatch. Whatever the wire actually carried in that field is
  discarded; every message is currently assumed to be addressed to whoever
  answers the connection it arrived on.
- The handshake ([connection.rs](../../../paddington/src/connection.rs), see
  [wire-protocol.md](../../specifications/wire-protocol.md) §4) is strictly
  two-party over one physical WebSocket - salt exchange via HTTP headers,
  session key negotiation, `PeerDetails` exchange. Nothing about it
  supports (or needs to support) a third party.
- Two things work in this design's favour, though:
  - `paddington::crypto::Key`/`SecretKey`
    ([crypto.rs:111,128](../../../paddington/src/crypto.rs#L111)) - `encrypt<T>`/
    `decrypt<T>` ([crypto.rs:259,300](../../../paddington/src/crypto.rs#L259)) -
    have no dependency on a live `Connection` at all. Any application code
    can encrypt/decrypt an arbitrary value with an arbitrary key.
  - The `Invite` mechanism
    ([invite.rs:13](../../../paddington/src/invite.rs#L13),
    `save`/`load` at [invite.rs:118,106](../../../paddington/src/invite.rs#L118))
    that bootstraps trust between any two peers today is **already
    out-of-band** - an admin generates a file on one side and manually
    copies it to the other. It requires no live connection between the two
    parties to set up, which is exactly the situation `airr` and `brics`
    are in.

So the gap is narrow: paddington already has everything needed to encrypt a
payload for a peer it has no live connection to, and to bootstrap that trust
out-of-band. What's missing is (a) a way to say "this message, though
physically going out over my connection to the proxy, is logically for
someone else" and (b) a way for the proxy to act on that without being able
to read the content.

## 4. Chosen approach

### 4.1 Two independent trust relationships, not one relayed one

`airr` and `brics` each keep a completely ordinary, unmodified paddington
connection to the proxy - normal `Invite`, normal handshake, normal session
keys, normal reconnection behaviour. Nothing about *that* relationship is
new.

Separately, `airr` and `brics` establish their **own** pre-shared key pair
with **each other** - via the exact same `Invite` file mechanism, just
naming each other instead of a live address, and critically, exchanged
through a channel the proxy never sees (the same out-of-band step - email,
`scp`, a secrets manager, whatever an operator already uses to distribute
today's invite files - just not routed through the proxy). This key pair is
never sent to, or known by, the proxy. It is used only to *bootstrap* the
relayed connection (§4.2) - not for ongoing traffic - so its exposure is
deliberately minimised.

### 4.2 A relayed connection still has a client and a server

Paddington's client/server asymmetry is preserved for the *virtual*
`airr`↔`brics` relationship, even though both sides are physically only
ever clients of the proxy. One side is configured as the relayed
**server** (`airr`, in the running example) - it doesn't dial anything for
this relationship, it *waits*, exactly as a real server waits for a
connection. The other is the relayed **client** (`brics`) - it *initiates*.
This gives the relayed pair a proper connection lifecycle (established,
torn down, re-established) instead of just a bag of independently-encrypted
messages, which turns out to matter a lot for reusing the rest of
paddington's machinery unchanged (§4.2.2).

#### 4.2.1 Bootstrap: a relayed handshake that mirrors the real one

The real handshake ([connection.rs](../../../paddington/src/connection.rs),
[wire-protocol.md](../../specifications/wire-protocol.md) §4) gets its session
keys from **both** sides, not one: the client generates a fresh outer key
and sends it (`session_key` in `Handshake`,
[connection.rs:562-565](../../../paddington/src/connection.rs#L562)); the
server generates its own fresh inner key and sends *that* back
([connection.rs:1191-1194](../../../paddington/src/connection.rs#L1191)). Both
ends up with the same pair - `inner_key` from the server, `outer_key` from
the client ([connection.rs:830-831](../../../paddington/src/connection.rs#L830),
[1408-1409](../../../paddington/src/connection.rs#L1408)). Neither side alone
controls the final keys. The relayed bootstrap mirrors this exactly, just
carried as two `RelayEnvelope`-wrapped messages instead of two raw WebSocket
frames:

```rust
/// brics (relayed client) → airr (relayed server), via proxy.
/// Encrypted with the permanent airr<->brics pre-shared keys (§4.1) -
/// the only thing this key pair is ever used for.
struct StartRelayedConnection {
    session_outer_key: SecretKey,   // freshly generated by brics
    inner_key_salt: Salt,           // freshly generated by brics
    outer_key_salt: Salt,           // freshly generated by brics
    magic: [u8; 32],                // freshly generated by brics
    engine: String,
    version: String,
}

/// airr → brics, via proxy. Same permanent pre-shared keys - the last
/// message that ever uses them.
struct RelayedConnectionAccepted {
    session_inner_key: SecretKey,   // freshly generated by airr
    magic: [u8; 32],                // echoes the client's magic
    engine: String,
    version: String,
}
```

The salts have no live handshake to travel over (there's no HTTP header
exchange here), so `brics` just generates and sends them directly - they
don't need confidentiality, only freshness, exactly like the real
handshake's salts. `magic` exists so `airr` can unambiguously recognise a
successfully-decrypted payload as *this specific* message type (this is the
relayed equivalent of the real protocol's `PeerDetails.version() == 2`
check) and so the response can be bound to a specific bootstrap attempt
rather than any earlier one.

After this exchange, both sides hold the identical `{inner_key (from
airr), outer_key (from brics)}` pair - the same shape a direct connection
ends up with. **This is where forward secrecy actually comes from**: not
from a separate upgrade step, but from the fact that this bootstrap runs
every time the relayed connection is (re-)established, each time producing
a fresh pair, exactly as reconnecting a direct connection does today.

#### 4.2.2 After bootstrap: reuse, don't reinvent

Once the session keys exist, all further traffic between `airr` and `brics`
uses paddington's *existing* per-message double-envelope scheme -
`envelope_message`/`deenvelope_message`
([connection.rs:216,247](../../../paddington/src/connection.rs#L216)), which
already derives a fresh per-message sub-key from a fresh random salt on
every single message (wire-protocol.md §3.4) - just fed the *relayed*
session keys/salts instead of a live connection's. These two functions are
currently `fn`-private and coupled to `TokioMessage` (the WebSocket frame
type); the plan is a small, behaviour-preserving refactor extracting their
string-in/string-out core to a `pub(crate)` function both the real
`Connection` code and the new relay code call - a shared implementation, not
a second one to keep in sync by hand.

The resulting ciphertext is wrapped, as before, in a `RelayEnvelope` so the
proxy knows where to send it:

```rust
#[derive(Serialize, Deserialize)]
pub struct RelayEnvelope {
    from: String,       // the true originating peer, not the proxy
    to: String,         // the true final peer, not the proxy
    zone: String,
    ciphertext: String, // opaque to the proxy in every case, bootstrap or not
}
```

**Sending**: `paddington::relay::send_via(relay_peer, to, zone, payload)`
encrypts `payload` with the current session keys for `to` (bootstrapping
first via §4.2.1 if no session exists yet), wraps the result in a
`RelayEnvelope`, and sends it to `relay_peer` (the proxy) via the ordinary
`Message::send_to`/`Exchange::send` path, unchanged.

**Relaying** (at the proxy): the handler recognises a `RelayEnvelope`
payload (a fourth category alongside the Control/Keepalive/Regular
[wire-protocol.md](../../specifications/wire-protocol.md) §2 already
documents), checks its policy (§4.3) for `(from, to)`, and - if allowed -
sends the **same, untouched** envelope on to `to`. The proxy never
distinguishes a bootstrap message from an ordinary one, and never needs to -
both are equally opaque to it. This is the entire relay operation: look up a
policy, look up a connection, forward unchanged.

**Receiving**: `paddington::relay::wrap_handler(inner_handler)` intercepts
incoming `RelayEnvelope`s addressed to this agent. A `StartRelayedConnection`
or `RelayedConnectionAccepted` (decryptable only with the permanent
pre-shared key) establishes or confirms the session as in §4.2.1 and fires
a synthesised `ControlCommand::Connected` (§4.2.3) - it is never handed to
`inner_handler`. Anything else, once decrypted with the now-established
session keys, is synthesised into an ordinary `Message { sender: "airr",
recipient: "brics", zone, payload: <decrypted> }` and passed to
`inner_handler` exactly as if it had arrived over a direct connection.
Anything that isn't a relevant `RelayEnvelope` at all passes through
unchanged - the wrapper is a no-op for all of an agent's normal,
non-relayed traffic.

#### 4.2.3 A real connection lifecycle, not just decrypted messages

Because the bootstrap in §4.2.1 is a genuine, one-time "this relationship is
now live" event - not just "here's another decryptable message" - each side
can synthesise its own local `ControlCommand::Connected { agent, zone,
engine, version }` the moment it completes, and feed it into the *existing*
`templemeads::control_message::process_control_message` unchanged
([control_message.rs](../../../templemeads/src/control_message.rs)). That
already triggers `Register`/`Sync`/queued-job delivery for a normal
connection - it does exactly the same thing here, for free, because as far
as that code is concerned a connection *did* just get established. This is
what makes the design in §4.2.2 genuinely transparent rather than merely
"decryption happens to work": there is a real "connected" moment for
templemeads to hook, not just a stream of individually-authenticated
messages with no notion of when the relationship started.

`brics`'s templemeads/agent-registry code cannot distinguish this from a
direct connection at all, at any point after the bootstrap completes - which
is the "transparent" property the goal in §1 asks for.

### 4.3 Config additions

Rather than a parallel `relayed_peers` list, a relayed peer is an ordinary
entry in the *existing* `servers`/`clients` lists
([config.rs:503-504](../../../paddington/src/config.rs#L503)) - just with a new
`proxy` field standing in for the field that doesn't apply when there's no
direct address:

```rust
pub struct ServerConfig {
    name: String,
    url: String,           // ignored when `proxy` is set
    proxy: Option<String>, // NEW - name of a `servers` entry to relay via
    zone: String,
    inner_key: SecretKey,  // unchanged in kind - now used only to bootstrap (§4.2.1)
    outer_key: SecretKey,
}

pub struct ClientConfig {
    name: String,
    ip: IpOrRange,          // ignored when `proxy` is set
    proxy: Option<String>,  // NEW - name of a `servers` entry to expect relaying via
    zone: String,
    inner_key: SecretKey,
    outer_key: SecretKey,
}
```

`brics`'s config has an `airr` entry under `servers` with
`proxy: Some("proxy")` instead of a `url` - reach `airr` by sending
`StartRelayedConnection` via the real, direct `servers` entry named
`"proxy"`. `airr`'s config has a `brics` entry under `clients` with
`proxy: Some("proxy")` instead of an `ip` allowlist - authentication comes
from the relayed handshake itself (§4.2.1) succeeding, which is a stronger
guarantee than an IP check, not a weaker substitute for one.

Each relayed peer entry names its own relay independently, and nothing
requires them to agree - a service can mix relayed and directly-connected
`clients`/`servers` entries freely, and can use *different* proxies for
different relayed peers, as long as each named relay is itself a known
`servers` entry (`ServiceConfig::check_relay_exists`, checked at
config-load time). An earlier revision of this design added a top-level
`ServiceConfig.proxy` field and restricted every relayed peer to naming
the same one, as a simplification - "how do I find out whether I've been
introduced to this proxy at all" - but that field was never actually read
by anything except its own validation check (`paddington::relay::configure`
already resolved each relayed peer's relay independently), so the
restriction was dropped once that became clear, with no changes needed
anywhere else in the protocol.

Operator-facing UX is unchanged: the existing `client --add`/`server --add`
commands ([docs/cmdline](../../cmdline/README.md)) generate and consume
`Invite` files exactly as today; only the resulting config entry carries
`proxy` instead of `ip`/`url` when the operator says the peer is reached via
a relay.

**On the proxy:** an explicit allow-list, default-deny:

```rust
pub struct RelayPolicy {
    pairs: Vec<(String, String)>,  // unordered; checked both directions
}
```

Configured directly by the proxy operator (not derived from anything the
relayed agents send) - this is the proxy's own authorisation boundary,
matching the user's framing: *"if brics is allowed to connect to airr"* is
a decision the proxy operator makes explicitly, not something `airr` or
`brics` can request into existence themselves.

### 4.4 The `op-proxy` agent

A new, minimal binary crate. It needs **no `templemeads` dependency at
all** - no Jobs, no Boards, no `Domain` - it's a pure `paddington` service,
closer in spirit to [docs/echo](../../echo/README.md) than to any `op-*`
templemeads agent: `ServiceConfig` with `clients` for every agent it relays
for, a `RelayPolicy`, and a message handler that does exactly the "relay"
step in §4.2 and nothing else. It never constructs a `Job`, never touches
`greatwestern` or any other `Domain` - the payload it forwards is opaque to
it in every sense, not just cryptographically.

## 5. Security properties and honest limitations

| Property | Holds? | Notes |
|---|---|---|
| Proxy cannot read `airr`↔`brics` payload content | **Yes** | Ongoing traffic is encrypted with session keys the proxy never has (§4.2.2); even the bootstrap messages that use the permanent pre-shared key (§4.2.1) are opaque to it |
| Proxy learns `airr`↔`brics` are communicating, and roughly how much/when | Yes (by design, §2) | Not a mixnet |
| `sender` on the receiving end is authenticated, not just asserted | Yes | Decryption only succeeds with the correct key (permanent, for bootstrap; session, for everything after - §4.2) |
| Forward secrecy (compromise of a key doesn't expose past traffic) | **Yes, per relayed session** | Every `StartRelayedConnection` bootstrap (§4.2.1) produces a fresh, mutually-contributed session key pair, exactly like a direct connection reconnecting. The *permanent* pre-shared key's exposure is minimised to bootstrap messages only - see below for what's still not covered |
| Proxy can forge messages appearing to be from `airr` to `brics` | **No** | It never holds either the permanent pre-shared key or any negotiated session key for that pair |
| Proxy can replay a previously-seen envelope | Bootstrap: mitigated by `magic` (§4.2.1). Ongoing traffic: same exposure as any relayed byte stream | Worth a stronger nonce/counter scheme if a given deployment's threat model needs more than the per-message HKDF salting already provides (§9) |

The `airr`↔`brics` `Invite` exchange (§4.1) **must** happen over a channel
the proxy does not see. This is an operational requirement, not something
the software can enforce - exactly analogous to how today's direct `Invite`
exchange already requires an out-of-band step for any peer pair, relayed or
not.

**What's still not covered**: a session that stays live for a long time
without reconnecting never re-runs the bootstrap, so it never re-keys -
identical to how a direct connection behaves today (§9 has a sketched
follow-up for periodic re-keying beyond connection-lifetime parity, if a
deployment needs more than "as good as a direct connection already is").

## 6. Interactions with existing features

- **Zones.** A relay explicitly bridges what may be two different zones,
  but only for the pairs the proxy's `RelayPolicy` (§4.3) names - it does
  not create a general hole between zones. `RelayEnvelope.zone` carries the
  zone the `(from, to)` pair itself operates in, independent of whatever
  zone each side uses for its own connection to the proxy.
- **Reconnection/health.** Each real hop (`airr`↔proxy, proxy↔`brics`)
  reconnects independently exactly as today. A relayed peer's effective
  reachability is the AND of both hops being up - there's no additional
  proxy-side state to recover, since the proxy holds no queue (§2).
- **Diagnostics.** The proxy should log relay activity (`from`, `to`,
  timestamp, size) for operator auditability - never `ciphertext` content,
  which it can't read anyway, and never logged in a way that implies it
  could.
- **`Erased` / multi-domain routing**
  ([multi-domain-routing-design.md](multi-domain-routing-design.md)). A
  genuinely different concern operating at a different layer: `Erased` is a
  `templemeads::domain::Domain` that lets one process route
  Jobs/Notifications between agents speaking *different vocabularies*,
  fully visible to that process (it just doesn't validate content
  semantically). This design's proxy operates *below* templemeads entirely
  and is blind by requirement, not by choice of `Domain`. The two are
  independent and could compose (an `Erased` router could itself be one of
  the two real hops a `RelayPeer` connects through) but neither depends on
  the other.

## 7. Phased implementation plan

1. **Preparatory refactor**: extract `envelope_message`/`deenvelope_message`'s
   string-in/string-out core out of their `TokioMessage`-coupled wrappers
   into a shared `pub(crate)` function (§4.2.2). Purely internal,
   behaviour-preserving - existing direct-connection tests must pass
   unchanged before moving on.
2. `paddington::relay`: `RelayEnvelope`, `StartRelayedConnection`,
   `RelayedConnectionAccepted`, `send_via`, `wrap_handler`, `RelayPolicy`.
   Unit tests: the bootstrap produces identical `{inner_key, outer_key}` on
   both sides, from independent contributions, given the two messages in
   isolation (no live connection involved at all); a `RelayedConnectionAccepted`
   whose `magic` doesn't match the outstanding `StartRelayedConnection` is
   rejected; `wrap_handler` passes non-`RelayEnvelope` payloads through
   unchanged; `wrap_handler` rejects a `RelayEnvelope` whose `from` names an
   unconfigured peer (fail closed, not silently drop-and-continue as normal
   traffic).
3. `paddington::config`: `proxy` field on `ServerConfig`/`ClientConfig`,
   `ServiceConfig::proxy`, the "every relayed client must name the same
   proxy" validation (§4.3). Round-trip config-file (de)serialisation
   tests, including the validation failure case.
4. `op-proxy` binary crate: `ServiceConfig` + `RelayPolicy` + the
   relay-only handler (§4.4). No `templemeads` dependency.
5. End-to-end test: three real paddington services in one test process -
   `airr` (relayed server), `brics` (relayed client), `proxy` - `airr` and
   `brics` each connected only to `proxy`, never to each other. `brics`
   initiates; confirm both sides converge on the same session keys, confirm
   each fires its own `ControlCommand::Connected` (§4.2.3), then send a
   message `brics` → `airr` and confirm `airr` receives it with `sender`
   reporting `"brics"` and the correct decrypted payload. Confirm the raw
   bytes leaving `brics`'s connection and arriving at `proxy` do **not**
   contain the plaintext payload, the session keys, or the permanent
   pre-shared key anywhere (the actual "blind" property, tested, not just
   asserted).
6. Confirm a `RelayPolicy` that doesn't list `(airr, brics)` causes the
   proxy to drop the envelope and log a warning, not forward it.
7. Document the config-file shape and the out-of-band key-exchange
   requirement (§4.1, §5) prominently - this is the one step that can't be
   automated or made foolproof by the software, so it needs to be very
   clear to operators.

## 8. Testing strategy

- **Mutual key contribution**: neither side's session keys are fully
  determined without the other's message - assert that two independent
  `StartRelayedConnection`/`RelayedConnectionAccepted` bootstraps (same
  permanent pre-shared key, different runs) produce different session
  key pairs each time (proves freshness, not a deterministic derivation).
- **Blindness, empirically checked**: capture the literal bytes sent from
  `brics` to the proxy - for both the bootstrap and ordinary traffic - and
  assert the plaintext payload, the session keys, and the permanent
  pre-shared key do not appear anywhere in them (not just "trust the
  encryption") - the same spirit as the `Erased` design's round-trip proofs
  ([multi-domain-routing-design.md](multi-domain-routing-design.md) §10).
- **Authentication**: a bootstrap or data `RelayEnvelope` encrypted under
  the *wrong* key (simulating a proxy - or anyone else - trying to forge a
  message as `brics`) fails to decrypt at `airr` and is dropped, not
  silently misattributed.
- **Policy enforcement**: proxy forwards only explicitly-allowed
  `(from, to)` pairs; every other combination is dropped and logged, never
  silently forwarded by default.
- **Transparency**: once the bootstrap completes, `airr`'s own
  `templemeads` code (Board, agent registry, `peer_domain`, etc.) cannot
  distinguish a relayed connection from `brics` from a directly-connected
  one - assert this by running the *same* templemeads-level test twice,
  once with a direct connection and once relayed, and diffing observable
  state.
- **Hop independence**: killing the proxy↔`airr` connection doesn't affect
  `brics`↔proxy; reconnecting either hop independently, followed by a fresh
  bootstrap, restores relayed delivery without needing to touch the other
  hop.
- **Re-bootstrap on reconnect**: after either real hop drops and
  reconnects, `brics` re-runs `StartRelayedConnection` and the resulting
  session keys differ from the previous session's - confirming forward
  secrecy actually holds across a reconnect, not just in the abstract.

## 9. Deferred: what this design still doesn't cover

The mutual bootstrap in §4.2.1 already closes the forward-secrecy gap this
section used to describe as future work - a fresh session key pair now
comes from every relayed (re-)connection, matching a direct connection's
properties. Left genuinely open, as smaller and more clearly-optional
follow-ups:

- **Re-keying within a single long-lived session.** A direct connection
  doesn't re-key mid-session either (it only gets a fresh pair on
  reconnect), so this isn't a regression - but a relayed session, precisely
  because it's expected to serve exactly the use case of agents that would
  otherwise never connect at all, may plausibly stay "connected" for far
  longer stretches than a typical direct connection does. Periodic
  re-bootstrap (e.g. every N hours, initiated by the relayed client exactly
  as a fresh connection would be) is a small addition on top of §4.2.1, not
  a new protocol - worth doing once the base mechanism is proven, not
  essential to ship it.
- **High-availability standby status** ([wire-protocol.md](../../specifications/wire-protocol.md)
  §4.4) for relayed pairs - out of scope for v1; `StartRelayedConnection`/
  `RelayedConnectionAccepted` don't carry a `standby_status` equivalent.
- **Stronger replay protection** than the per-message HKDF salting already
  provides (§5) - a sequence counter or timestamp window in `RelayEnvelope`,
  if a specific deployment's threat model needs it.
