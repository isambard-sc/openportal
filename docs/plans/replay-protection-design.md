<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# Message replay protection: an IPsec-style anti-replay window

Status: **implemented** (§7 steps 1-3; step 5's doc updates are this same
edit), **plus §5 revised** (what was originally a coordinated flag-day
rollout is now a negotiated one - see §5 and §9), **plus §10 added**
(handshake/bootstrap message replay protection, §9's other former
deferred item, now implemented).

`paddington::anti_replay` (`ReplayWindow`, `NoncedPayload`,
`HandshakeNonceState`) exists exactly as designed below, wired into
`Connection` (`ConnectionState::next_nonce`/`replay_window` for ongoing
traffic; the process-lifetime `HANDSHAKE_NONCE_STATE` registry for
`Handshake`/`PeerDetails`), `RelayedSession` (same ongoing-traffic fields),
and a second process-lifetime `BOOTSTRAP_NONCE_STATE` registry in
`relay.rs` for `StartRelayedConnection`/`RelayedConnectionAccepted`/
`SessionUnknown`. Covered by 8 unit tests against `ReplayWindow` directly
(the cases in §7 step 1), the capability-negotiation tests described in
§5, and the handshake/bootstrap tests described in §10.5 (49 `paddington`
unit tests total). Validated live against real `op-portal`/`op-provider`
processes, both directly connected and via a real `op-proxy`, confirming
ordinary traffic (`Register`, at minimum) still flows correctly with the
new wrapping in place - no false-positive replay rejections.

**Not yet done**: step 4's live "capture real wire bytes from a running
connection and replay them at the transport level" integration test -
covered at the protocol level (decrypt-the-same-ciphertext-twice, as
above) but not by literally intercepting a live WebSocket frame. Live
validation of §5's negotiation against an actual not-yet-upgraded peer
binary has also not been done (no old build was readily available to test
against) - only unit-tested by asserting an old-shaped, field-less
`PeerDetails`/`StartRelayedConnection`/`RelayedConnectionAccepted` JSON
deserialises with `supports_nonce: false`, and that `NoncedPayload::Legacy`
serialises identically to a bare string; the same caveat applies to §10's
`Handshake`/`PeerDetails` `nonce` field. The `docs/specifications` updates
the phased plan calls for (wire-protocol.md, security-model.md) are
included in this same change; this document is being left in `docs/plans/`
rather than moved to `archive/` for now.

## 1. Goal

Every OpenPortal message is already encrypted and authenticated (AEAD,
double-envelope - see [wire-protocol.md](../specifications/wire-protocol.md)
§3, [security-model.md](../specifications/security-model.md) §2). What
that does *not* protect against: an attacker (or the blind relay proxy
itself, or anyone else who can observe and re-inject wire traffic) simply
**capturing a legitimate, validly-encrypted message and sending it again
later**. Nothing currently checks "have I already processed this exact
message" - a captured `add_user`/`remove_project`/any other instruction
would decrypt and execute again, identically, the second time.

Add a nonce to every ongoing message and a sliding replay-detection window
on the receiving side, so a duplicate (or out-of-order-but-already-seen)
message is detected and dropped rather than reprocessed - **the same
algorithm IPsec anti-replay windows and WireGuard use**: a monotonically
increasing per-sender counter, and a receiver-side high-water-mark plus a
fixed-size bitmap of recently-accepted values. Deliberately not a novel
scheme - the value of "this is exactly the textbook anti-replay window" is
being able to point at decades of prior art rather than defend a bespoke
design.

This must work identically for a directly-connected peer and a peer
reached via a blind relay proxy ([blind-relay-proxy-design.md](archive/blind-relay-proxy-design.md))
- see §4.4 for why that falls out naturally rather than needing separate
handling.

## 2. Non-goals

- **Protecting the handshake/bootstrap messages themselves.** `Handshake`/
  `PeerDetails` (direct connections, [wire-protocol.md](../specifications/wire-protocol.md)
  §4) and `StartRelayedConnection`/`RelayedConnectionAccepted`/
  `SessionUnknown` (relayed bootstrap, §7.1/§7.3) are out of scope for this
  pass. They already have narrower protections that are adequate for now:
  the four-layer connection authentication for direct connections
  ([security-model.md](../specifications/security-model.md) §4), and
  `magic` correlation plus `SessionUnknown`-driven self-healing for relayed
  bootstrap (a replayed `Start` causes, at worst, a spurious re-key that
  repairs itself the next time the legitimate peer sends anything - see
  §9 for the fuller argument for leaving this for a later pass rather than
  silently ignoring it).
- **Changing the shape of ongoing traffic without negotiation.** Unlike the
  `domain`/`domain_version` fields added to `Register` (purely additive -
  `Register` was already a structured object), wrapping ongoing traffic in
  `{nonce, payload}` changes the *shape* of the encrypted content from a
  bare string to an object - an agent that hasn't been upgraded cannot
  parse that shape at all. §5 (revised after initial implementation)
  covers how a gradual rollout is nonetheless made possible: by advertising
  support for the new shape via an already-structured, safely-extensible
  message and gating on the peer's confirmed support, rather than by
  changing the ongoing-traffic shape itself unconditionally.
- **Persisting nonce/window state across restarts.** Deliberately reset on
  every fresh connection/session, piggybacking on state that's already
  reconstructed from scratch on reconnect - see §4.3. No new on-disk state,
  no new failure mode if a process crashes mid-window.
- **A configurable window size, retry policy, or enforcement mode.** Fixed
  constants (§4.2), drop-and-log on rejection (matching how every other
  "this message looks wrong" case in this codebase already behaves - see
  e.g. the per-message domain checks). Can be revisited if real operational
  experience calls for it.

## 3. Current state: why replay isn't already blocked

Traced through the actual code, not assumed:

- The per-message `info` value mixed into HKDF key derivation
  ([wire-protocol.md](../specifications/wire-protocol.md) §3.3,
  `envelope_message`/`deenvelope_message` in
  [connection.rs](../../paddington/src/connection.rs)) is generated fresh
  for every message and transmitted alongside the ciphertext, in the
  clear. It exists so that two *different*, legitimate messages are never
  encrypted under the same derived key. It does **not** prevent replay: a
  captured message's `info` values travel with it, so replaying the exact
  same bytes re-derives the exact same key and decrypts successfully
  again. Freshness of the derived key and freshness of the *message* are
  different properties, and only the first one is currently guaranteed.
- `Connection::send_message` ([connection.rs:373](../../paddington/src/connection.rs))
  and the post-handshake receive loops
  ([connection.rs:866](../../paddington/src/connection.rs),
  [connection.rs:1438](../../paddington/src/connection.rs)) pass a bare
  `&str`/`String` straight to `envelope_message`/`deenvelope_message` - no
  sequence number, timestamp, or any other per-message identifier exists
  anywhere in that path.
- `paddington::relay`'s ongoing-traffic path
  ([relay.rs](../../paddington/src/relay.rs), `send()` and
  `handle_incoming_envelope`'s post-bootstrap branch) does the same thing
  independently, with its own session keys - also no identifier.
- Nothing downstream (templemeads' `Command`/`Job`/`Notification`
  processing) tracks "have I seen this before" either - `Job::id()` is a
  UUID generated once per Job, but a *replayed* message carries the same
  UUID as the original by definition, so it wouldn't help even if checked.

## 4. Chosen approach

### 4.1 What gets a nonce, and where it lives on the wire

A new wrapper type, defined once and reused by both paths:

```rust
#[serde(untagged)]
enum NoncedPayload {
    Nonced { nonce: u64, payload: String },
    Legacy(String),
}
```

Sent in place of the bare payload string, through the *same*
`envelope_message`/`deenvelope_message` primitives as today - so the nonce
is inside the AEAD-authenticated ciphertext, not a bystander plaintext
field. That matters: a nonce the proxy (or an attacker) could see and
overwrite in the clear would be worthless, since they could just replay
with a substituted, unused value. Being encrypted-and-authenticated is
what makes it enforceable at all.

The `Legacy(String)` arm serves two purposes: defensively recognising
whatever an old-shaped payload actually looks like on decrypt (rather than
failing outright, since a bare JSON string plainly isn't a JSON object),
and - since §5 was revised - being the deliberate *sending* shape used for
a peer that hasn't confirmed it understands `Nonced` (`NoncedPayload::
for_peer`, `anti_replay.rs`). It serialises as exactly a bare string, so an
old peer's receive path sees precisely the format it already expects.

### 4.2 The window itself

A small, self-contained module (`paddington::anti_replay`, name chosen to
be unambiguous next to `paddington::relay`), with no new dependency:

```rust
struct ReplayWindow {
    initialized: bool,
    highest: u64,
    bitmap: [u64; 16],   // 1024 bits: 1024 trackable nonces behind `highest`
}
```

- Bit `j` (0 = most recent) represents nonce `highest - j`.
- First nonce ever seen from a peer: accepted unconditionally, becomes
  `highest`, sets bit 0.
- A nonce `N > highest`: shift the whole bitmap left by `N - highest` bits
  (aging every existing entry; a shift of ≥1024 clears it entirely - no
  history left to check against), set bit 0, `highest = N`. Accepted.
- A nonce `highest - 1024 < N <= highest`: check bit `highest - N`. Set →
  reject (already seen). Unset → set it, accept.
- A nonce `N <= highest - 1024`: reject outright (too old to have a slot
  in the window at all).

1024 was your own suggested size and is a perfectly ordinary choice for
this (RFC 6479 discusses windows from 64 up to 2^15 depending on expected
reordering/throughput; WireGuard uses a similar bitmap scheme). 128 bytes
per peer relationship - trivial at OpenPortal's scale, and a named
constant if it ever needs tuning.

The out-of-order tolerance is necessary, not defensive-in-depth for its
own sake: `exchange::event_loop` already dispatches received messages to
worker tasks concurrently
([exchange.rs:243](../../paddington/src/exchange.rs)), so strict in-order
delivery was never guaranteed even before considering replay.

### 4.3 Where the state lives, and why it resets for free

- **Direct connections**: `next_nonce: u64` and `replay_window:
  ReplayWindow` added to `ConnectionState`
  ([connection.rs:44](../../paddington/src/connection.rs)) - already the
  `Arc<StdMutex<_>>`-shared, per-connection mutable state `Connection`
  uses for everything else that changes per-message (`last_activity`).
- **Relayed sessions**: the same two fields added to `RelayedSession`
  ([relay.rs:137](../../paddington/src/relay.rs)).

Neither needs explicit "reset on reconnect" logic: a fresh `Connection`
is constructed per new physical connection already, and a fresh
`RelayedSession` is constructed - deliberately, for forward secrecy - on
every bootstrap, overwriting whatever was in `SESSIONS` before. Nonce
state initialised fresh alongside the session keys it's protecting is a
consequence of the existing lifecycle, not new machinery.

### 4.4 Why this covers proxied connections automatically

`Connection::send_message` is the *only* caller of `envelope_message` for
ongoing traffic, and `exchange::send` is the *only* caller of
`send_message` - which is also where the relay fallback lives
(`paddington::relay::send`, added in
[blind-relay-proxy-design.md](archive/blind-relay-proxy-design.md) §4.2.2).
Wiring the nonce in at `Connection::send_message`/the receive loops for
direct traffic and at `relay::send`/`handle_incoming_envelope` for relayed
traffic means every kind of application-level message - Jobs,
Notifications, keepalives, all of it - gets the same protection uniformly,
without templemeads or any higher layer needing to know it exists. The
proxy itself is, as ever, irrelevant to this: it never sees inside the
ciphertext, so it can no more see or forge a nonce than it can read a
payload.

## 5. Rollout: negotiated, gradual (revised after initial implementation)

The first version of this design (§9 in its original form) treated this as
a coordinated flag-day: both ends of a connection or relayed pair had to
upgrade together, since an upgraded sender's `{nonce, payload}` object
would fail to deserialise for an old receiver expecting a bare string.
After shipping and testing that version, the operational reality turned
out to be that clients lag servers significantly - a flag-day rollout
across an entire fleet isn't realistic. This section describes the
capability-negotiation mechanism added instead.

**The insight that makes this possible without weakening anything**: the
*receive* path was already tolerant of both shapes from the start (§4.1) -
`NoncedPayload`'s `#[serde(untagged)]` `Legacy(String)` arm exists
precisely because decrypting a bare string must not fail outright. Only the
*send* path needed to change - to stop assuming every peer understands the
new shape.

**Advertising support**: a new `supports_nonce: bool` field, set to `true`
whenever *this* code constructs the message describing itself:

- `PeerDetails` (direct connections, `connection.rs`) - exchanged as the
  last step of the handshake, exactly like `version`/`name`/`zone`.
- `StartRelayedConnection` / `RelayedConnectionAccepted` (relayed
  bootstrap, `relay.rs`) - the client's/server's respective half of a
  relayed bootstrap.

Each is `#[serde(default)]`, so a not-yet-upgraded peer's message (which
simply lacks the field, since it predates this change) deserialises as
`supports_nonce: false` - correctly, since that peer genuinely doesn't
understand the new shape yet. This is safe to add unconditionally (unlike
wrapping ongoing traffic itself) because `PeerDetails`/the bootstrap
messages were already structured objects - adding a field to them is
exactly the kind of purely-additive change `domain`/`domain_version`
already established as safe for `Register` (§2).

**Learning and remembering the peer's capability**: captured once, at the
point each handshake/bootstrap completes, as connection/session-lifetime
state - `Connection::peer_supports_nonce` (a plain field, parallel to
`inner_key`/`peer`, since it never changes again for that connection's
lifetime) and `RelayedSession::peer_supports_nonce` (populated from
`accepted.supports_nonce` in `bootstrap()`, or `start.supports_nonce` in
`handle_start()`). Both reset for free on reconnect/re-bootstrap, for the
same reason `next_nonce`/`replay_window` already do (§4.3): a fresh
`Connection`/`RelayedSession` is constructed from scratch each time, so a
peer that gets upgraded mid-fleet-rollout is correctly detected as soon as
it reconnects, without either side needing to restart to notice.

**Gating the send path**: `NoncedPayload::for_peer(nonce, payload,
peer_supports_nonce)` (`anti_replay.rs`) - the single, shared decision
point both `Connection::send_message` and `relay::send` now call instead
of unconditionally wrapping. Sends `Nonced { nonce, payload }` if the peer
confirmed support; otherwise `Legacy(payload)`, which serialises as
*exactly* the bare string an old peer's receive path already expects - not
a lesser or degraded encoding, byte-identical to what shipped before this
whole feature existed.

**What this buys, concretely**: a server can be upgraded first and
immediately gains full nonce protection for every peer that has *also*
been upgraded, while continuing to interoperate, unprotected but
functioning, with peers that haven't been upgraded yet. There is no
flag-day: each pairwise relationship gets replay protection independently,
the moment both its ends happen to be upgraded, with no coordination
required beyond that.

**What this does not buy**: replay protection against a peer that hasn't
been upgraded. If either end of a pair is still old, that pair's traffic
is exactly as replay-vulnerable as it was before this whole feature
shipped - there is no way to force protection on an old peer that cannot
speak the new shape. This is an explicit, accepted trade-off for
gradual-rollout compatibility, not an oversight.

## 6. Security properties

| Property | Holds? | Notes |
|---|---|---|
| A captured, validly-encrypted ongoing message cannot be replayed to re-trigger its effect | **Yes** | Rejected by the receiver's window regardless of who replays it - the sender, the proxy, or a third party who captured wire traffic |
| Out-of-order (but not yet seen) delivery still works | Yes | The 1024-entry window tolerates reordering within it; only genuine duplicates or messages older than the window are rejected |
| The proxy can forge a nonce, or strip/rewrite one to defeat the check | **No** | The nonce is inside the AEAD-authenticated ciphertext; the proxy has neither the permanent nor the session keys for either relayed peer |
| A legitimate reconnect/re-bootstrap is mistaken for a replay | No | Nonce state is reset alongside the fresh session it protects (§4.3), not persisted across it |
| Handshake/bootstrap messages are replay-protected by this mechanism | **No (see §2, §9)** | Out of scope for this pass; covered by narrower, pre-existing protections instead |
| A pair where either end hasn't been upgraded gets replay protection | **No, by design (§5)** | Falls back to the pre-nonce, unprotected behaviour for that pair only - other, upgraded pairs are unaffected |
| A not-yet-upgraded peer can be tricked into parsing the new `{nonce, payload}` shape | **No** | The send path never emits it unless that specific peer's `PeerDetails`/bootstrap message confirmed `supports_nonce` (§5) |

## 7. Phased implementation plan

1. `paddington::anti_replay`: `ReplayWindow` (as in §4.2) and
   `NoncedPayload` (§4.1). Unit tests covering: first-ever nonce accepted;
   strictly increasing sequence all accepted; exact duplicate rejected;
   out-of-order-but-within-window accepted once, rejected on repeat;
   nonce older than the window rejected; a single huge jump forward
   correctly clears/re-establishes the window rather than panicking or
   miscomputing the shift.
2. `ConnectionState`: add `next_nonce`/`replay_window`. Wire into
   `Connection::send_message` (wrap outgoing payload) and both
   post-handshake receive loops (unwrap, check-and-record, drop with a
   logged warning on rejection - mirroring the existing
   de-envelope-failure handling immediately above each call site).
3. `RelayedSession`: same two fields, wired into `relay::send` and
   `handle_incoming_envelope`'s ongoing-traffic branch.
4. Integration test: two real paddington services exchanging several
   messages normally (all accepted, in order), then a captured raw
   ciphertext blob replayed at the transport level and confirmed
   rejected (not reprocessed) - proving the property end-to-end, not just
   that `ReplayWindow` is correct in isolation.
5. Update [wire-protocol.md](../specifications/wire-protocol.md) and
   [security-model.md](../specifications/security-model.md) (§9, added
   alongside this design) once implemented; move this document to
   `archive/`.
6. *(Added after initial implementation, see §5.)* Negotiated rollout:
   `supports_nonce` field on `PeerDetails` and on
   `StartRelayedConnection`/`RelayedConnectionAccepted`;
   `Connection::peer_supports_nonce`/`RelayedSession::peer_supports_nonce`
   populated at handshake/bootstrap completion; `NoncedPayload::for_peer`
   as the shared send-path gate. Unit tests: an old-shaped (field-missing)
   `PeerDetails`/`StartRelayedConnection`/`RelayedConnectionAccepted`
   deserialises with `supports_nonce: false`; `NoncedPayload::for_peer`
   wraps only when told the peer supports it, and its `Legacy` output
   serialises identically to a bare string.

## 8. Testing strategy

- **Textbook window behaviour**: the six cases in §7 step 1, directly
  against `ReplayWindow` - no network, no encryption, fast and exhaustive.
- **Wire-level replay, for real**: capture the literal bytes sent for one
  message on a live connection, let normal traffic continue, then
  re-inject the captured bytes and confirm the receiver drops it (logged,
  not processed) - the same "prove it against real bytes, not just
  logically" standard used for the relay design's blindness tests
  ([blind-relay-proxy-design.md](archive/blind-relay-proxy-design.md) §8).
- **Relayed parity**: the same replay-and-reject test repeated over a
  relayed connection via a real `op-proxy`, confirming the protection is
  identical whether or not a proxy sits in between.
- **Reconnect does not false-positive**: disconnect and reconnect (direct)
  or force a re-bootstrap (relayed), then confirm a message using a nonce
  value that would have been "already seen" under the *old* session's
  window is accepted under the new one - proving the reset in §4.3 works
  as intended, not just written and assumed correct.

## 9. Deferred: what this design still doesn't cover

- ~~Handshake/bootstrap message replay~~ - **implemented, see §10.**
- ~~A negotiated, gradual rollout~~ - **implemented, see §5**. The live
  fleet's clients turned out to lag servers significantly, so a coordinated
  flag-day wasn't realistic; `supports_nonce` on `PeerDetails`/the relayed
  bootstrap messages now makes the rollout gradual instead.
- **Persisted window state across a process restart.** Not attempted -
  see §2. A restarting agent's peers simply get a fresh window the moment
  a new connection/session is established, exactly as session keys
  themselves already work. §10's per-peer handshake/bootstrap window is
  also in-memory-only, for the same reason.

## 10. Addendum: handshake/bootstrap message replay protection

Status: **implemented**. Picks up §9's first deferred item.

### 10.1 What's actually replayable, traced through the code

§2 originally waved this off with "narrower protections... adequate for
now." Tracing through what a captured handshake/bootstrap message can
actually be used for (not assumed) turns up a real gap, but a narrower one
than "the whole handshake is unprotected":

- **`StartRelayedConnection` (relayed bootstrap client → server)** - this
  message *alone* is sufficient for `handle_start` (`relay.rs`) to
  overwrite `SESSIONS` for that peer and fire a `Connected` event, with no
  further live input needed from whoever sent it. A captured `Start`
  replayed later forces the server side to silently reset its live session
  for that peer and re-announce it connected - a genuine, repeatable
  disruption (the peer's *real* session is torn down and replaced without
  it doing anything), not merely "a spurious re-key that repairs itself"
  as §2 characterised it before this was traced through properly.
- **`SessionUnknown` (either direction)** - similarly self-sufficient: the
  receiver wipes its cached session and (if it holds the client role)
  immediately re-bootstraps, purely from receiving this one message. A
  single captured `SessionUnknown` can be replayed indefinitely to force
  constant re-bootstrap churn between two peers, long after whatever
  restart originally produced it.
- **`RelayedConnectionAccepted`** - already effectively replay-proof: `magic`
  is freshly random per `bootstrap()` call and consumed exactly once via
  `PENDING_BOOTSTRAPS.remove(&accepted.magic)`; a replayed `Accepted`
  carries a `magic` that no longer matches anything pending and is dropped
  today, logged as "unrecognised magic... (stale or forged)."
- **Direct `Handshake`** - replaying the client's message alone gets the
  server to generate a fresh session key and reply, but the connection
  cannot actually complete: `PeerDetails` is encrypted under that freshly
  *and randomly* generated session key (`Key::generate()`, not derived
  from anything replayable), which an attacker replaying captured bytes
  has no way to produce. So a replayed `Handshake` cannot impersonate a
  peer or hijack a session - it can only make the server do wasted
  handshake-processing work for a connection that will never complete.
  Real, but a mild resource-consumption concern, not an authentication
  bypass.
- **Direct `PeerDetails`** - for the same reason, a captured `PeerDetails`
  from one connection cannot be replayed against a *different* connection
  attempt: its encryption key is freshly random each time, so old
  ciphertext simply fails to decrypt under the new one.

So `Start` and `SessionUnknown` are where nonce protection actually closes
a real, repeatable disruption; `Handshake`/`PeerDetails`/`Accepted` get it
too below, for uniformity and because it's cheap once the machinery
exists, but the honest security value there is smaller (defense-in-depth
for `Handshake`, decorative for `Accepted`/`PeerDetails`).

### 10.2 Scoping: persistent per-peer state, not per-connection

§4.3's ephemeral, reset-every-reconnect window works for ongoing traffic
*because* the session keys it protects are also fresh every reconnect - a
replayed nonce against a new session is moot, since the new session's key
differs too. That reasoning doesn't hold here: the *permanent* pre-shared
key pair (`Handshake`'s first message, and all three relayed bootstrap
messages, are encrypted wholly or partly under it) never changes across
reconnects. A window that reset per connection would accept nonce 0 all
over again on every fresh connection attempt - exactly as useless as no
window at all, since a replay attempt just needs a new connection to reset
the check.

The window instead needs to live for as long as the peer relationship
itself - a new, shared type in `anti_replay.rs`:

```rust
#[derive(Debug, Default)]
struct HandshakeNonceState {
    next_nonce: u64,
    replay_window: ReplayWindow,
}
```

Structurally identical to `ConnectionState`'s/`RelayedSession`'s existing
nonce fields (same two-line `take_next_nonce`, same delegation to
`ReplayWindow` for the receive-side check) - the only thing that changes
is *where it lives* and *when it resets* (never, short of a process
restart - consistent with §2's existing "no persisted window state"
non-goal, now extended to this window too).

- **Direct connections**: a new process-lifetime registry in
  `connection.rs`, keyed by `"{name}@{zone}"` (the same peer-identity
  convention `exchange.rs`'s connection registry already uses), separate
  from `ConnectionState` since it must outlive any one `Connection`.
- **Relayed bootstrap**: the same struct, keyed by plain peer name in
  `relay.rs` (matching `RELAY_CONFIG`/`SESSIONS`/`PENDING_BOOTSTRAPS`'s
  existing keying), separate from `RelayedSession` for the same reason.

Both maps are populated lazily (`entry(...).or_default()`) on first
contact with a given peer - a never-before-seen peer's first nonce is
simply accepted, exactly as `ReplayWindow` already handles "first nonce
ever seen" for ongoing traffic.

### 10.3 Wire changes and backward compatibility

**Relayed bootstrap - no backward compatibility needed** (`op-proxy` isn't
deployed yet, per §5's own reasoning for why it didn't need this either):
a plain, required `nonce: u64` added to `StartRelayedConnection` and
`RelayedConnectionAccepted`, and `BootstrapMessage::SessionUnknown`
(previously a unit variant) becomes `SessionUnknown { nonce: u64 }`. No
`#[serde(default)]`, no negotiation - every relayed peer speaks the same
version from day one.

**Direct connections - backward compatibility needed, and simpler than §5's
than expected**: `Handshake` and `PeerDetails` are *already* structured
objects (unlike ongoing traffic's bare string), so adding `#[serde(default)]
nonce: Option<u64>` to each is exactly as safe as `domain`/`domain_version`
on `Register` was, or `supports_nonce` on `PeerDetails` itself (§5) - a
pre-upgrade peer's message simply lacks the field and deserialises with
`nonce: None`, which is read as "no nonce to check, accept unconditionally"
(the pre-this-feature behaviour, for that one message).

Crucially, **this needs no capability-negotiation step at all** - the
wrinkle flagged when this was first discussed (that `Handshake` is
exchanged *before* `PeerDetails`, the point capability is normally learned,
so a not-yet-known peer's very first `Handshake` couldn't be gated on a
capability learned only afterward) turns out not to apply, because there's
no gating decision to make in the first place. §5's negotiation exists
because sending the wrong *shape* to an old peer breaks it outright
(a bare string vs. an object). Adding an optional field to an
already-structured message has no such failure mode: serde silently
ignores unknown fields on the receiving end by default, so a not-yet-
upgraded peer decoding a `nonce`-bearing `Handshake`/`PeerDetails` simply
never notices the extra field. This side always sends its own nonce,
unconditionally, on every message, to every peer, upgraded or not - there
is nothing to learn or remember before deciding whether it's safe to do
so. The only asymmetry is on receipt: check the nonce if present, skip the
check if absent (`None`) - which is exactly how `ReplayWindow`
already treats "first nonce ever seen" as an unconditional accept, just
applied per-message instead of per-window.

(One consequence worth naming plainly: this means direct-connection
handshake replay protection is bidirectionally live from the very first
connection between two upgraded peers - there is no "first connection is
still unprotected while capability is learned" gap the way there might
have been with a negotiated approach.)

### 10.4 Where the checks land

- **Client (`make_connection`)**: takes a nonce for its outgoing `Handshake`
  before sending; checks the server's replied `Handshake`'s nonce once
  decrypted; takes a nonce for its outgoing `PeerDetails`; checks the
  server's replied `PeerDetails`'s nonce once decrypted and peer identity
  is confirmed.
- **Server (`handle_connection`)**: mirrors the above - checks the client's
  `Handshake` nonce once peer identity is resolved (this only becomes
  possible after the four-layer IP/crypto matching already narrows down
  which configured peer this is, so the check happens right after that
  point, not before); takes a nonce for its own `Handshake` reply; checks
  the client's `PeerDetails` nonce; takes a nonce for its own `PeerDetails`
  reply.
- **Relayed `bootstrap()`**: takes a nonce for the outgoing `Start`; checks
  the returned `Accepted`'s nonce (redundant with `magic`, kept for
  uniformity - see §10.1) before completing.
- **Relayed `handle_start()`**: checks the incoming `Start`'s nonce first,
  before doing anything else (generating a session key, replying, or
  touching `SESSIONS`) - this is the check that actually matters (§10.1);
  takes a nonce for the outgoing `Accepted`.
- **Relayed `notify_session_unknown()`**: takes a nonce for the outgoing
  `SessionUnknown`.
- **Relayed `handle_incoming_envelope()`**: checks an incoming
  `SessionUnknown`'s nonce before acting on it (the other check that
  actually matters); checks an incoming `Accepted`'s nonce after its
  `magic` already matched, before forwarding it through the pending
  bootstrap's oneshot channel.

Rejection on failure mirrors the existing convention for every other
"this message looks wrong" case at each of these call sites: log a
warning and return `Err`/drop, no special handling.

### 10.5 Testing

- `HandshakeNonceState` unit tests mirroring `ReplayWindow`'s own (first
  nonce accepted, duplicate rejected, `None` always accepted) - thin,
  since the underlying logic is already exhaustively tested; these confirm
  the wiring, not `ReplayWindow` itself again.
- An old-shaped (field-missing) `Handshake`/`PeerDetails` JSON deserialises
  with `nonce: None` and is accepted without a replay check - proving the
  backward-compatibility claim in §10.3 directly, the same way §5's
  `supports_nonce`-defaults-to-`false` tests do for ongoing traffic.
- A replayed `StartRelayedConnection`/`SessionUnknown` (same nonce value
  reused) is rejected the second time - proving §10.1's actual threat is
  closed.

### 10.6 What this still doesn't cover

- **Process-restart persistence** - as already accepted for the ongoing-
  traffic window (§9), this window is in-memory only. A process restart
  gets a fresh window for every peer, exactly as if it were a new peer
  relationship - no worse than today's behaviour, since there was no replay
  protection here at all before this addendum.
- **A genuine authentication bypass via `Handshake`/`PeerDetails` replay**
  was never actually possible (§10.1) - this addendum closes a real
  disruption vector (`Start`/`SessionUnknown`) and adds defense-in-depth
  where the risk was already low, but it isn't "fixing a hole that let
  attackers impersonate a peer," since no such hole existed once the
  fresh-session-key behaviour was traced through properly.
