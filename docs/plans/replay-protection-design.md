<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# Message replay protection: an IPsec-style anti-replay window

Status: **implemented** (§7 steps 1-3; step 5's doc updates are this same
edit). `paddington::anti_replay` (`ReplayWindow`, `NoncedPayload`) exists
exactly as designed below, wired into both `Connection`
(`ConnectionState::next_nonce`/`replay_window`, checked in
`send_message`/both post-handshake receive loops) and `RelayedSession`
(same two fields, checked in `relay::send`/`handle_incoming_envelope`).
Covered by 8 unit tests against `ReplayWindow` directly (the cases in §7
step 1) plus an extension of the relay design's own
`test_full_bootstrap_and_message_exchange_in_process` proving a captured,
validly-encrypted ciphertext is rejected on replay even though it decrypts
identically both times. Validated live against real `op-portal`/
`op-provider` processes, both directly connected and via a real
`op-proxy`, confirming ordinary traffic (`Register`, at minimum) still
flows correctly with the new wrapping in place - no false-positive replay
rejections.

**Not yet done**: step 4's live "capture real wire bytes from a running
connection and replay them at the transport level" integration test -
covered at the protocol level (decrypt-the-same-ciphertext-twice, as
above) but not by literally intercepting a live WebSocket frame. The
`docs/specifications` updates the phased plan calls for (wire-protocol.md,
security-model.md) are included in this same change; this document is
being left in `docs/plans/` rather than moved to `archive/` for now.

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
- **A gradual, one-agent-at-a-time rollout.** Unlike the `domain`/
  `domain_version` fields added to `Register` (purely additive - `Register`
  was already a structured object), this change alters the *shape* of the
  encrypted content for ongoing traffic, from a bare string to a small
  wrapper object. An agent that hasn't been upgraded cannot parse the new
  shape at all. This is a coordinated redeployment for whichever agents
  talk to each other (like raising a minimum protocol version), not a
  silently-compatible field addition - see §5.
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

The `Legacy(String)` arm only exists to fail closed (a JSON object
plainly isn't a JSON string) rather than panic if an unrelated payload
somehow reaches this deserialisation path - it does not provide a
compatibility bridge to un-upgraded peers (§2, §5).

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

## 5. Rollout: a coordinated change, not a gradual one

Both ends of a connection (direct) or a relayed pair need to be upgraded
together. An upgraded sender talking to a not-yet-upgraded receiver
produces a `{nonce, payload}` object where the old code expects a bare
string; deserialisation fails, the message is logged and dropped (the
existing "malformed message" handling already does this - no crash, no
silent corruption), and that one hop simply doesn't work until both sides
are redeployed. This is different from how `domain`/`domain_version` were
introduced (§2), and worth being explicit about rather than implying a
graceful mixed-version rollout is possible here - it isn't, because the
change is to the *shape* of the encrypted envelope, not an additive field
on an already-structured message.

## 6. Security properties

| Property | Holds? | Notes |
|---|---|---|
| A captured, validly-encrypted ongoing message cannot be replayed to re-trigger its effect | **Yes** | Rejected by the receiver's window regardless of who replays it - the sender, the proxy, or a third party who captured wire traffic |
| Out-of-order (but not yet seen) delivery still works | Yes | The 1024-entry window tolerates reordering within it; only genuine duplicates or messages older than the window are rejected |
| The proxy can forge a nonce, or strip/rewrite one to defeat the check | **No** | The nonce is inside the AEAD-authenticated ciphertext; the proxy has neither the permanent nor the session keys for either relayed peer |
| A legitimate reconnect/re-bootstrap is mistaken for a replay | No | Nonce state is reset alongside the fresh session it protects (§4.3), not persisted across it |
| Handshake/bootstrap messages are replay-protected by this mechanism | **No (see §2, §9)** | Out of scope for this pass; covered by narrower, pre-existing protections instead |

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

- **Handshake/bootstrap message replay** (§2). A dedicated pass could add
  the same `ReplayWindow` machinery to `Handshake`/`PeerDetails` and to
  `StartRelayedConnection`/`Accepted`/`SessionUnknown`, scoped per
  permanent pre-shared key rather than per session (since, unlike a
  session, the permanent key doesn't change across reconnects - the
  window would need to live on `RelayedPeer`, which is config-derived and
  long-lived, not on the ephemeral `RelayedSession`). Left out here to
  keep this pass's surface area to ongoing traffic only.
- **A negotiated, gradual rollout.** If a live fleet ever needs one-at-a-
  time upgrades instead of a coordinated flag-day, that would need an
  explicit capability advertised during `Register`/`PeerDetails` (mirroring
  `domain`/`domain_version`'s pattern) with format-switching gated on
  confirming the peer supports it - deliberately not attempted here (§5).
- **Persisted window state across a process restart.** Not attempted -
  see §2. A restarting agent's peers simply get a fresh window the moment
  a new connection/session is established, exactly as session keys
  themselves already work.
