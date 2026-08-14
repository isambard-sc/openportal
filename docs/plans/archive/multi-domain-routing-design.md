<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# Multi-domain routing: a domain-oblivious `Erased` Domain

Status: **implemented** (§9 steps 1-5). `templemeads::erased::Erased`,
the `domain`/`domain_version` fields on `Job`/`Notification`,
`agent::ensure_job_domain_matches`/`ensure_notification_domain_matches`, and
the `provider` agent's switch to `Job<Erased>` (dropping its `greatwestern`
dependency entirely) are all in place and covered by unit/integration tests,
including the cross-domain round-trip proofs in §10 (real `Job<Hpc>`/
`Notification<Hpc>` relayed through `Job<Erased>`/`Notification<Erased>` and
re-serialised byte-identical, including through multiple chained hops).
A live multi-process run has since happened incidentally, as part of
testing [blind-relay-proxy-design.md](blind-relay-proxy-design.md): a real
`op-provider` binary (compiled against `Erased`) connected to a real
`op-portal` binary (compiled against `Hpc`/`greatwestern`) and registered
successfully - `op-provider`'s `Register` correctly reported
`domain=erased`, and the connection was accepted and processed normally
despite the domain mismatch, confirming `Erased` behaves correctly in a
live setting and doesn't break ordinary connection/registration. **Still
not done**: a dedicated live test of `op-provider` actually *routing* a
Job between two different-domain leaf agents over real connections (as
opposed to the in-process serialisation round-trips in §10, or the
incidental registration above). Step 9.6 (leaving other routing-role
agents on their current `Domain`) was a deliberate non-action, not skipped
work. This document is kept as the design record; see the code (and
`writing-a-domain.md` §1.1) for current behaviour.

## 1. Goal

Today, an OpenPortal agent binary is compiled against exactly one
`L: templemeads::domain::Domain` (see
[grammar-split-design.md](grammar-split-design.md) and
[writing-a-domain.md](../../specifications/writing-a-domain.md)), and every
agent in a deployment must speak the same `L` to interoperate at all. That's
fine for leaf agents (`freeipa`, `slurm`, `filesystem`, ...) - they only ever
need to understand *one* vocabulary, their own.

It's an unnecessary restriction for **routing-only** agents - `provider`,
`clusters` (the `platform` role), and similar hops that exist purely to
forward a `Job` or `Notification` one step closer to its destination and
never inspect, execute, or construct an `Instruction`/`NotificationEvent`
themselves. The goal is to let a single router process sit between agents
speaking *different* `Domain`s - or even multiple, simultaneously, in one
deployment - without being recompiled per domain and without forking
templemeads.

Concretely: `type Job = templemeads::job::Job<Erased>;` in a router's
`main.rs`, instead of `Job<SomeConcreteDomain>`, and that router transparently
relays both Jobs and Notifications belonging to *any* `Domain`, unchanged -
**and** a leaf agent that finally executes a Job, or handles a Notification,
can independently verify which `Domain` actually produced it, regardless of
how many domain-oblivious hops it passed through to get there (§7).

## 2. Non-goals

- **Making leaf agents domain-oblivious.** An agent that actually executes
  business logic (`match job.instruction() { ... }` or
  `match notification.event() { ... }`) must stay compiled against one
  concrete `Domain`, exactly as today. This design only touches agents that
  never do that match at all.
- **Cross-domain translation.** A router relays opaque bytes; it never
  converts a `greatwestern` instruction/event into some other domain's
  equivalent. Two leaf agents on either side of an `Erased` router still
  need to natively understand whatever lands in their own inbox - the
  router doesn't make incompatible domains compatible, it just stops being
  the reason two *compatible-with-each-other-if-they-could-only-connect*
  topologies can't share a routing tier.
- **A new wire format.** No change to how `Job`/`Command`/`Notification`
  serialise, beyond the four new optional fields in §7 (two on `Job`, two on
  `Notification`). The whole design leans on the fact that the wire format
  is already domain-oblivious for *routing* (§4) - if that stopped being
  true, this design would need rethinking.
- **Auto-detecting whether an agent is "routing-only."** That's a per-agent
  judgement call the operator/implementor makes when choosing `Erased` vs. a
  concrete `Domain` for a given binary - see §8 for what breaks if you choose
  wrong.

## 3. Current state: why a router can't be domain-oblivious today

Traced through the actual code, not assumed - Jobs and Notifications turn
out to have the *same* routing story but a *different* wire-format story, so
both are covered here.

### 3.1 Jobs

- `Job<L>`'s `command` field is a private `Command<L>` struct holding
  `{ destination: Destination, instruction: L::Instruction }`
  ([job.rs:198-214](../../../templemeads/src/job.rs#L198)), and routing
  (`Position::Downstream` in `handler.rs`) only ever looks at `destination` -
  it never touches `instruction` to decide where a Job goes next. So far, so
  domain-oblivious.
- The blocker is *deserialisation*. `Command<L>`'s custom `Deserialize`
  ([job.rs:183-194](../../../templemeads/src/job.rs#L183)) calls
  `Command::parse(&s, false)`, which calls `L::parse_instruction(...)`
  ([job.rs:119](../../../templemeads/src/job.rs#L119)) - a call that
  **fails** if the incoming instruction string doesn't belong to `L`'s
  grammar. A router compiled with `L = greatwestern::Hpc` simply cannot
  deserialise (and therefore cannot relay) a Job whose instruction belongs to
  a different domain - `serde_json::from_str` returns `Err`, and
  `impl<L: Domain> From<Message> for Command<L>`
  ([command.rs:328-333](../../../templemeads/src/command.rs#L328)) silently
  turns that into a `Command::Error`, dropping the Job rather than
  forwarding it.
- Nothing else in the routing path cares about `L::Instruction` at all:
  `diagnostics.rs`/`health.rs` already store `job.instruction().to_string()`
  (a `String`, via `Display`, not the typed value) precisely because they're
  meant to be domain-agnostic - confirmed during the original grammar-split
  audit (`grammar-split-design.md` §3). `result`/`result_type` on `Job` are
  already untyped (`Option<String>`) - a router never calls
  `job.completed()`/`job.result::<T>()` since it never executes anything.

So the gap for Jobs is: **`Domain::parse_instruction` must always succeed**
for a router to be able to relay arbitrary domains' Jobs.

### 3.2 Notifications

- `notification::send()` ([notification.rs:113-171](../../../templemeads/src/notification.rs#L113))
  and the `Position::Downstream` arm of `handler.rs`'s notification dispatch
  ([handler.rs:555-566](../../../templemeads/src/handler.rs#L555)) route purely
  on `notification.destination()` - `event` is never inspected to decide
  where a Notification goes next, exactly like Jobs.
- The blocker is again *deserialisation* - but the mechanism is different
  from Jobs, not the same one. `Notification<L>` derives `Serialize`/
  `Deserialize` directly on the struct
  ([notification.rs:19-25](../../../templemeads/src/notification.rs#L19)):
  `event: L::NotificationEvent` is deserialised by `L::NotificationEvent`'s
  own (plain, derived) `Deserialize` impl - **not** via
  `Domain::parse_notification_event`, which is only ever called from
  `Notification::parse(s: &str)`
  ([notification.rs:40-51](../../../templemeads/src/notification.rs#L40)), the
  text-command entry point (e.g. a bridge's `POST /notify`). A router only
  ever receives already-serialised `Notification<L>` values over the wire
  and relays them via ordinary struct deserialisation - it never calls
  `Notification::parse` on anything, so `parse_notification_event` being
  permissive doesn't, by itself, help a router at all.

So the gap for Notifications is different: **`L::NotificationEvent`'s
`Deserialize` impl must succeed on any valid JSON**, regardless of what
`Domain` produced it. §4 explains why that's a materially different
requirement than "any string parses"; §5 designs `RawNotificationEvent`
to actually satisfy it.

## 4. Key insight: the wire format is domain-oblivious for routing - but Jobs and Notifications get there differently

`Command<L>` (the private struct behind `Job.command`) serialises via
`Display` ([job.rs:165-169](../../../templemeads/src/job.rs#L165)) to a single
string: `"<destination> <instruction-display>"` - this is the `"command"`
field documented in [json-types.md](../../specifications/json-types.md) §Job.
It is **not** a structured `{destination: ..., instruction: {...}}` object.
So if a `Domain`'s `Instruction` type is defined so that `parse_instruction`
always succeeds and `Display` reproduces exactly what it was given, a
`Job<ThatDomain>` round-trips **byte-for-byte identically** to whatever
`Job<RealDomain>` the originating leaf agent serialised.

**`NotificationEvent` has no equivalent custom string serialisation.**
Verified empirically (there is no test for this in the repo - it was checked
by serialising a real `Notification<TestDomain>` and reading the JSON): the
wire form is

```json
{"id": "...", "destination": "a.b", "event": {"UserAdded": "chris.project.brics"}}
```

- a **structured JSON object**, one key per enum variant, produced by
`NotificationEvent`'s ordinary `#[derive(Serialize, Deserialize)]`. (This
also means the existing
[notification-protocol.md](../../specifications/notification-protocol.md) §4
documentation of `"event": "<event-string>"` was wrong - fixed alongside
this design.) Consequently, `Erased::NotificationEvent` can't just be a
`String` wrapper the way `Erased::Instruction` can - it needs to preserve
**arbitrary JSON shape**, since it has no idea what shape a given `Domain`'s
events take. §5 uses `serde_json::Value` for exactly this reason.

## 5. Chosen approach: an `Erased` Domain in templemeads

Add a new, small module - `templemeads::erased` - defining a `Domain`
implementation that is a total, non-validating passthrough for both
Instructions and NotificationEvents:

```rust
// in templemeads::erased (NEW module)

/// The raw text of an instruction this agent doesn't understand and never
/// needs to - captured verbatim so it can be forwarded unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawInstruction(String);

impl Display for RawInstruction {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The raw JSON shape of a notification event this agent doesn't
/// understand, plus the one structured case every `Domain` must support
/// (see `Domain::wrap_forward`). Untagged: serde tries `Forward` first
/// (matches only if the JSON has exactly a Notification's shape - `id`,
/// `destination`, `event` keys), falling through to `Raw` - a
/// `serde_json::Value` - for everything else, which **always** succeeds,
/// preserving whatever JSON shape the real `Domain`'s event serialised as
/// (see §4). `Value`'s own `Serialize` reproduces that JSON byte-for-byte
/// on the way back out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RawNotificationEvent {
    Forward(Box<Notification<Erased>>),
    Raw(serde_json::Value),
}
// Display: Forward(n) => "forward [{}]" (matching every other Domain's
// Forward variant); Raw(v) => v's compact JSON text - `parse` never
// produces this from a plain string (see below), so Display here is only
// ever exercised for logging an already-deserialised wire value.

/// A `Domain` that understands nothing and forwards everything - for
/// routing-only agents that sit between leaf agents speaking real,
/// possibly-different `Domain`s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Erased;

impl Domain for Erased {
    type Instruction = RawInstruction;
    type NotificationEvent = RawNotificationEvent;

    fn parse_instruction(s: &str) -> Result<Self::Instruction, Error> {
        Ok(RawInstruction(s.to_string())) // never fails
    }

    fn parse_notification_event(s: &str) -> Result<Self::NotificationEvent, Error> {
        // Only reachable via Notification::parse (a text command, e.g. a
        // bridge's POST /notify) - never via ordinary wire deserialisation
        // (§3.2). Wraps the text as a JSON string value; never produces
        // `Forward`, matching every other Domain's convention that Forward
        // is infrastructure-only and not parseable from text.
        Ok(RawNotificationEvent::Raw(serde_json::Value::String(s.to_string())))
    }

    fn name() -> &'static str {
        "erased"
    }

    fn version() -> &'static str {
        env!("CARGO_PKG_VERSION") // templemeads' own version
    }

    // owning_portal: default `None` - a router can't evaluate a
    // domain-specific ownership policy it doesn't understand. This is safe:
    // incoming `Command<L>` deserialisation always calls
    // `Command::parse(&s, false)` (check_portal = false) regardless of `L"
    // (job.rs:189) - only a *portal* agent parsing a fresh, human/bridge-
    // supplied command string with check_portal = true relies on this, and
    // a portal is a leaf role, never `Erased`.

    fn assume_legacy_domain_version(_engine_version: &str) -> Option<&'static str> {
        None // `Erased` has no pre-split history to claim
    }

    fn wrap_forward(inner: Notification<Self>) -> Self::NotificationEvent {
        RawNotificationEvent::Forward(Box::new(inner))
    }
}
```

A router agent's `main.rs` then reads exactly like any other agent's, just
with `type Job = templemeads::job::Job<templemeads::erased::Erased>;` - no
change to `agent_core`, `board.rs`, `handler.rs`, or the wire format at all
(beyond §7's addition).

## 6. What changes, and what doesn't

| | |
|---|---|
| **New code** | One new module, `templemeads::erased` (§5) - roughly the size of `templemeads::test_domain` - plus the four new fields in §7 (two on `Job`, two on `Notification`). |
| **Changed code** | Only the specific router-role agents an operator chooses to switch from `Job<SomeDomain>` to `Job<Erased>` - a type-alias change, nothing structural. |
| **Unchanged** | `board.rs`, `command.rs`, the wire format's overall shape, every leaf agent, every existing `Domain` implementation (including `greatwestern`). |

This is deliberately the cheapest possible design for the capability: it
adds one implementation of an existing trait, rather than a parallel
type-erasure mechanism (`Box<dyn Any>`, a second generic parameter, etc.) -
see §11 for why that heavier alternative was rejected once already, in a
closely related context.

## 7. Per-message domain provenance: verifying at the destination, not just the connection

### 7.1 Why connection-level checking isn't enough once `Erased` exists

`agent::ensure_domain_matches::<L>(peer)` and the `domain`/`domain_version`
fields on `Register` (added before this design existed - see
[wire-protocol.md](../../specifications/wire-protocol.md) §Register) tell an
agent what `Domain` its **directly connected peer** speaks. That's exactly
right for a leaf agent talking directly to another leaf agent. It stops
being useful the moment an `Erased` router sits in between: the peer a
destination leaf agent is directly connected to is the *router*, whose
`Domain::name()` is `"erased"` - not the `Domain` of whoever actually
authored the Job or Notification several hops upstream. Connection-level
checking literally cannot see through a domain-oblivious hop.

What's needed is a way for the **true originating `Domain`** to travel with
the message itself, hop-for-hop, surviving any number of `Erased` relays, so
the agent that finally acts on it can check - independent of who its
immediate neighbour is. This is good practice even in deployments with no
`Erased` router at all: it catches an instruction/event that happens to
parse successfully under the *wrong* domain's grammar (two domains can
coincidentally share syntax, or JSON shape, for different meanings) before
it's ever acted on, not just after the fact.

### 7.2 Correction: this belongs on `Job`/`Notification`, not `Envelope`

The natural place to reach for this is `Envelope<L>`/`NotificationEnvelope<L>`
- but neither is **ever serialised over the wire**. Both are purely local,
in-process wrappers: every call site that constructs one builds it fresh,
right before handing the message to the registered runner - for Jobs,
[handler.rs:316](../../../templemeads/src/handler.rs#L316) (the generic
dispatch path) and the same pattern independently in the `account`,
`filesystem`, `portal`, and `scheduler` role modules
([account.rs:57](../../../templemeads/src/account.rs#L57),
[filesystem.rs:57](../../../templemeads/src/filesystem.rs#L57),
[portal.rs:53](../../../templemeads/src/portal.rs#L53),
[scheduler.rs:56](../../../templemeads/src/scheduler.rs#L56)); for
Notifications,
[handler.rs:574](../../../templemeads/src/handler.rs#L574) (destination) and
[handler.rs:605](../../../templemeads/src/handler.rs#L605) (bridge sidecar),
plus [notification.rs:120](../../../templemeads/src/notification.rs#L120)
(self-addressed). What actually travels hop-to-hop over the wire is
`Job<L>`/`Notification<L>` themselves, via
`Command::Put/Update/Delete { job: Job<L> }` /
`Command::Notify { notification: Notification<L> }`
([command.rs:29-42](../../../templemeads/src/command.rs#L29)).

So the provenance tag has to live on `Job<L>` and `Notification<L>`, not
their respective Envelopes, to survive being relayed. Two new fields on
each:

```rust
pub struct Job<L: Domain> {
    // ...existing fields...

    /// The `Domain::name()` that authored this Job's instruction, set once
    /// at `Job::parse()` and never touched again - including by any
    /// `Erased` hop it passes through, which relays it as just another
    /// opaque field it doesn't need to understand (exactly like
    /// `RawInstruction`). `None` only for a Job from a peer running
    /// templemeads from before this field existed.
    #[serde(default)]
    domain: Option<String>,

    /// The domain's version, alongside `domain`.
    #[serde(default)]
    domain_version: Option<String>,
}

pub struct Notification<L: Domain> {
    // ...existing fields...

    /// Same idea as `Job::domain` - set once at `Notification::new()`/
    /// `Notification::parse()`, surviving any number of `Erased` relays
    /// unmodified (relayed as an opaque JSON string field, exactly like
    /// the rest of `Notification`'s shape passes through `RawNotificationEvent`
    /// unmodified - see §5).
    #[serde(default)]
    domain: Option<String>,
    #[serde(default)]
    domain_version: Option<String>,
}
```

Populated in `Job::parse()`
([job.rs:238](../../../templemeads/src/job.rs#L238)) and in both
`Notification::new()`/`Notification::parse()`
([notification.rs:28-51](../../../templemeads/src/notification.rs#L28)) with
`Some(L::name().to_string())` / `Some(L::version().to_string())` -
mirroring exactly how `Register` picked up `domain`/`domain_version` for the
connection-level check, just captured once per-message instead of once
per-connection. An `Erased` router constructs no new `Job`/`Notification` of
its own (it only ever relays ones it received), so it never overwrites these
fields with its own `"erased"` identity - the tag genuinely reflects the
true origin, end to end.

### 7.3 The destination-side checks

Two new functions, mirroring each other:

- `agent::ensure_job_domain_matches::<L>(job: &Job<L>, sender: &Peer) -> Result<(), Error>`,
  called immediately before a runner is invoked (alongside each
  `Envelope::new(...)` call site in §7.2).
- `agent::ensure_notification_domain_matches::<L>(notification: &Notification<L>, sender: &Peer) -> Result<(), Error>`,
  called immediately before a notify runner is invoked (alongside each
  `NotificationEnvelope::new(...)` call site in §7.2).

Both follow the same logic:

1. If the message's `domain` is `Some(d)`: compare `d` to `L::name()`. Match
   → `Ok`. Mismatch → `Err(Error::Incompatible(...))`.
2. If `domain` is `None` (a message from before this field existed): fall
   back to the *connection-level* signal already built for `Register` -
   `agent::peer_domain(sender)` - which already folds in
   `Domain::assume_legacy_domain_version` for exactly this situation. This
   is weaker (single-hop only) but is the best available signal for an old
   message, and matches today's behaviour exactly for a deployment with no
   `Erased` router in it.
3. Otherwise (still unknown after both checks): fail-closed, same
   philosophy as `ensure_domain_matches` - `Err(Error::Incompatible(...))`.

**Deliberately not the same failure mode as `ensure_domain_matches`** for
either. That function disconnects the *peer*, because a connection-level
mismatch means every future message from that peer is suspect. A single
misrouted Job or Notification doesn't mean the connection is bad - most
other traffic relayed over it may well be correctly addressed. So:

- `ensure_job_domain_matches` errors the *Job* (`job.errored("...")`, same
  as any other execution failure) and leaves the connection alone.
- `ensure_notification_domain_matches` simply causes the notification to be
  dropped (logged, not delivered to the notify runner) - Notifications
  already have no return channel and no delivery guarantee
  ([notification-protocol.md](../../specifications/notification-protocol.md)
  §8), so "drop and log" is the existing failure mode for every other kind
  of notification delivery problem too; this is one more reason added to
  that same bucket, not a new kind of failure a caller needs to newly
  handle.

Like `ensure_domain_matches`, both are opt-in - templemeads doesn't call
them automatically, since not every agent needs the guarantee (e.g. an
`Erased` router itself never executes/handles anything, so it has no use
for either check). Agents that do want it call the appropriate one as the
very first thing in their runner/notify runner, or templemeads could wire
them in as a default pre-check for any agent that opts in via
`set_my_service_details`/`set_notify_runner` - left as an implementation
choice for §9.

## 8. Gotchas / interactions with existing features

### 8.1 `ensure_domain_matches` and routers

`agent::ensure_domain_matches::<L>(peer)` checks that a connected peer's
`Register` domain equals `L::name()`, disconnecting otherwise - including
when the peer's domain is unknown (fail-closed, by design). **An `Erased`
router's `name()` is `"erased"`, which will never equal a leaf agent's real
domain name.** Without an exception, a leaf agent that calls
`ensure_domain_matches::<Hpc>(&router_peer)` against a directly-connected
`Erased` router would disconnect it - exactly backwards from what's wanted,
and bad enough in practice (it would mean `ensure_domain_matches` and
multi-domain routing are simply incompatible with each other) that it's
worth fixing in `ensure_domain_matches` itself rather than only in
documentation: **it special-cases a peer whose registered domain is exactly
`templemeads::erased::Erased::name()`, accepting it regardless of `L`.**
This isn't templemeads reaching for knowledge of a foreign vocabulary - the
`Erased` domain is templemeads' own, defined alongside this check in the
same crate - and it's safe precisely because `Erased` never inspects or
executes `Instruction`/`NotificationEvent` content, only relays it, so it
can't misinterpret anything belonging to `L`.

**The per-message checks (§7.3) do not get the same exception, and must
not.** A `Job`/`Notification` that genuinely claims `domain: "erased"` as
its *own* provenance means no real `Domain` ever validated it - exactly the
case those checks exist to catch. In practice this can't arise from normal
operation (an `Erased` router never constructs a new Job/Notification, only
relays ones it received), so the distinction is about what the checks
*would* do if it somehow did: `ensure_domain_matches` answers "can I trust
this connection to relay my traffic faithfully" (yes, even via a router);
`ensure_job_domain_matches`/`ensure_notification_domain_matches` answer "was
this specific message actually produced by my vocabulary" (no, if it
self-reports as `"erased"`) - two different questions that happen to look
similar. This needs to be prominent in `writing-a-domain.md`, so the
distinction isn't rediscovered the hard way in production.

### 8.2 `owning_portal` / `check_portal`

Covered in §5's code comment - a non-issue because `check_portal` is only
ever `true` when a *portal* agent parses a fresh command string from a
human or bridge, and a portal is a leaf role. A router only ever receives
already-parsed `Command<L>` values over the wire, which always deserialise
with `check_portal = false`.

### 8.3 `RawNotificationEvent`'s `Forward` vs. `Raw` ambiguity

`#[serde(untagged)]` tries `Forward` before falling back to `Raw` (§5).
`Forward` only matches JSON that happens to look exactly like a
`Notification` object (`id`/`destination`/`event` keys). It is
vanishingly unlikely but not impossible for some other `Domain`'s genuine
event variant to accidentally have that exact shape (e.g. a hypothetical
`SomeEvent { id: Uuid, destination: String, event: String }` tuple/struct
variant) and be mis-parsed as a `Forward` instead of passed through as
`Raw`. Worth a property test (§10) precisely because it's the one place
`Erased`'s passthrough isn't *unconditionally* transparent. If this ever
bites in practice, the fix is to make `Forward`'s wire shape distinguishable
(e.g. wrap it in a single-key object like `{"__erased_forward": {...}}`)
rather than relying on structural shape-sniffing - deferred unless/until
it's shown to matter.

### 8.4 Diagnostics/health/logging

Already domain-agnostic (§3) - nothing to change. `RawInstruction`'s
`Display` reproduces the original instruction text, and `RawNotificationEvent`'s
`Raw(Value)` variant reproduces the original event JSON, so log lines
through an `Erased` router look identical to what a same-domain router
would have logged.

### 8.5 A router still needs *a* runner and notify runner

Every agent registers an `AsyncRunnable<L>` and an `AsyncNotifyRunnable<L>`,
called if the agent is ever the final destination of a Job/Notification. A
pure router should never legitimately be a destination for either;
`templemeads::erased` should ship small default implementations of both
that error/log clearly (e.g. `Error::UnknownInstruction("this agent only
routes Jobs, it does not execute them")`) rather than reusing
`default_runner` (which calls `envelope.job().execute()` - not meaningful
for a `RawInstruction`) or the generic `default_notify_runner` (which is
harmless to reuse as-is, since it only logs - but doing so for a message
genuinely addressed to the router itself is still worth flagging as
suspicious in that log line).

## 9. Phased implementation plan

1. Add `templemeads::erased` (§5): `RawInstruction`, `RawNotificationEvent`,
   `Erased`, router-appropriate default runner and notify runner. Unit
   tests: round-trip parse/Display for `RawInstruction`; round-trip
   serialise/deserialise for `RawNotificationEvent` against real
   `greatwestern::NotificationEvent` JSON (not just synthetic values); the
   `Forward`/`Raw` disambiguation (§8.3); and a two-domain proof analogous
   to the one used for the original `Domain` split
   (`grammar-split-design.md` §12) - serialise a `Job<Hpc>`/`Notification<Hpc>`,
   deserialise as `Job<Erased>`/`Notification<Erased>`, re-serialise, and
   assert byte-identical output.
2. Add the `domain`/`domain_version` fields to both `Job<L>` and
   `Notification<L>`, populated in `Job::parse()` and
   `Notification::new()`/`parse()` (§7.2). Useful independently of `Erased`
   and can land first/separately - pure backward-compatible addition
   (`#[serde(default)]`), same shape as the `Register` fields.
3. Add `agent::ensure_job_domain_matches` and
   `agent::ensure_notification_domain_matches` (§7.3), and decide the
   automatic-vs-opt-in question for how agents wire them into their
   runner/notify-runner dispatch.
4. Pick `provider` as the proof of concept -
   [templemeads::provider::run](../../../templemeads/src/provider.rs#L15)
   doesn't even take a runner argument (it hardcodes `None` to
   `set_my_service_details`, so it can only ever run the generic
   `default_runner`), and `provider/src/main.rs` has zero references to
   `Instruction`/`.instruction()` - it is structurally incapable of
   containing domain-specific business logic today, making it the
   lowest-risk candidate by construction, not just by convention. Switch its
   type alias to `Job<Erased>`/`Envelope<Erased>`, drop its `greatwestern`
   dependency entirely, and confirm a manual multi-hop routing scenario -
   for both Jobs and Notifications - still passes unchanged, including a
   leaf agent on the far side successfully calling
   `ensure_job_domain_matches`/`ensure_notification_domain_matches` and
   seeing the *original* domain, not `"erased"`.
   - **Motivating topology**: a single `provider` fronting *multiple*
     portals, each potentially speaking a different `Domain`, each routed
     to its own downstream cluster backend speaking the matching `Domain` -
     supported for free, since `paddington`'s peer model is already N-to-N
     (`agent::get_all(&Type::Portal)` already returns a `Vec`, not a single
     peer) and routing is `Destination`-only for both Jobs and Notifications
     (§3). One `Erased` provider replaces what would otherwise need to be
     one recompiled provider binary per `Domain` in play.
5. Document the `ensure_domain_matches` vs.
   `ensure_job_domain_matches`/`ensure_notification_domain_matches` usage
   rule (§8.1) prominently in `writing-a-domain.md`.
6. Leave `clusters`/other routing-role agents on their current concrete
   `Domain` unless/until there's an actual multi-domain deployment need -
   this design makes the switch available, it doesn't mandate it.

## 10. Testing strategy

- **Round-trip fidelity (Jobs)**: for a representative sample of real
  `greatwestern::Instruction` variants, serialise as `Job<Hpc>`, deserialise
  as `Job<Erased>`, re-serialise, and diff against the original bytes -
  must be identical, including the new `domain`/`domain_version` fields.
- **Round-trip fidelity (Notifications)**: same, for a representative
  sample of real `greatwestern::NotificationEvent` variants (including at
  least one with no inner data, one with a single identifier argument, and
  the `Forward` variant itself) - `Notification<Hpc>` → `Notification<Erased>`
  → re-serialise, byte-identical.
- **Rejects nothing**: `Erased::parse_instruction`/`parse_notification_event`
  must never return `Err` for any input string, and `RawNotificationEvent`'s
  `Deserialize` must never fail for any syntactically valid JSON value
  (property-test both, including empty/malformed strings and arbitrary JSON
  shapes) - the whole point is that a router can't reject what it doesn't
  understand.
- **`Forward`/`Raw` disambiguation** (§8.3): confirm a genuine `Forward`
  notification round-trips as `Forward`, and a battery of real
  `greatwestern::NotificationEvent` JSON shapes all round-trip as `Raw`
  rather than being mis-parsed as `Forward`.
- **Multi-domain proof**: two different toy `Domain`s (e.g.
  `templemeads::test_domain::TestDomain` and a second toy domain) both
  routing successfully - Jobs *and* Notifications - through one `Erased`
  agent in the same test process, each arriving at its respective
  (correctly-typed) leaf agent intact, *and* each leaf agent's
  `ensure_job_domain_matches`/`ensure_notification_domain_matches` call
  confirming the correct origin domain despite the intermediate `Erased`
  hop.
- **Provenance survives multiple `Erased` hops**: a Job/Notification relayed
  through two or more chained `Erased` routers still carries its original
  `domain`/`domain_version` unchanged at the far end.
- **Legacy fallback**: a `Job`/`Notification` JSON blob with no
  `domain`/`domain_version` keys at all still deserialises
  (`#[serde(default)]`), and the respective `ensure_*_domain_matches`
  correctly falls back to the sender's connection-level `peer_domain`.
- **Runner/notify-runner safety**: confirm the default `Erased`
  implementations of both error/log rather than panic if a message is ever
  actually addressed to the router itself.

## 11. Rejected/deferred alternative: a second, heavier type-erasure layer

The original grammar-split design considered and rejected a fully
type-erased `Job` payload (`(String, String)` json+type-name, downcast per
agent) for the *entire* framework
([grammar-split-design.md](grammar-split-design.md) §11) - rejected
because it costs every leaf agent a fallible downcast for a property
(compile-time exhaustive matching) they'd otherwise get for free.

That rejection doesn't apply here: `Erased` isn't a general replacement for
`Job<L>`/`Notification<L>`, it's one more implementation of the *existing*
`Domain` trait, opted into only by agents that were never going to
pattern-match on `Instruction`/`NotificationEvent` anyway. Leaf agents keep
exactly the ergonomics they have today; only the subset of agents that
already don't need typed instructions/events gain the ability to stop
pretending they do.
