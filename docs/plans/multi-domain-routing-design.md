<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# Multi-domain routing: a domain-oblivious `Erased` Domain

Status: **draft design** — not yet implemented. This document records a
design sketched in conversation so it can be picked up, reviewed, or handed
to someone else without re-deriving it. No code has been changed yet.

## 1. Goal

Today, an OpenPortal agent binary is compiled against exactly one
`L: templemeads::domain::Domain` (see
[grammar-split-design.md](archive/grammar-split-design.md) and
[writing-a-domain.md](../specifications/writing-a-domain.md)), and every
agent in a deployment must speak the same `L` to interoperate at all. That's
fine for leaf agents (`freeipa`, `slurm`, `filesystem`, ...) - they only ever
need to understand *one* vocabulary, their own.

It's an unnecessary restriction for **routing-only** agents - `provider`,
`clusters` (the `platform` role), and similar hops that exist purely to
forward a `Job`/`Notification` one step closer to its destination and never
inspect, execute, or construct an `Instruction` themselves. The goal is to
let a single router process sit between agents speaking *different*
`Domain`s - or even multiple, simultaneously, in one deployment - without
being recompiled per domain and without forking templemeads.

Concretely: `type Job = templemeads::job::Job<Erased>;` in a router's
`main.rs`, instead of `Job<SomeConcreteDomain>`, and that router transparently
relays Jobs/Notifications belonging to *any* `Domain`, unchanged - **and** a
leaf agent that finally executes a Job can independently verify which
`Domain` actually produced it, regardless of how many domain-oblivious hops
it passed through to get there (§7).

## 2. Non-goals

- **Making leaf agents domain-oblivious.** An agent that actually executes
  business logic (`match job.instruction() { ... }`) must stay compiled
  against one concrete `Domain`, exactly as today. This design only touches
  agents that never do that match at all.
- **Cross-domain translation.** A router relays opaque bytes; it never
  converts a `greatwestern` instruction into some other domain's equivalent.
  Two leaf agents on either side of an `Erased` router still need to
  natively understand whatever lands in their own inbox - the router doesn't
  make incompatible domains compatible, it just stops being the reason two
  *compatible-with-each-other-if-they-could-only-connect* topologies can't
  share a routing tier.
- **A new wire format.** No change to how `Job`/`Command`/`Notification`
  serialise, beyond the two new optional fields in §7. The whole design
  leans on the fact that the wire format is already domain-oblivious (§4) -
  if that stopped being true, this design would need rethinking.
- **Auto-detecting whether an agent is "routing-only."** That's a per-agent
  judgement call the operator/implementor makes when choosing `Erased` vs. a
  concrete `Domain` for a given binary - see §8 for what breaks if you choose
  wrong.

## 3. Current state: why a router can't be domain-oblivious today

Traced through the actual code, not assumed:

- `Job<L>`'s `command` field is a private `Command<L>` struct holding
  `{ destination: Destination, instruction: L::Instruction }`
  ([job.rs:198-214](../../templemeads/src/job.rs#L198)), and routing
  (`Position::Downstream` in `handler.rs`) only ever looks at `destination` -
  it never touches `instruction` to decide where a Job goes next. So far, so
  domain-oblivious.
- The blocker is *deserialisation*. `Command<L>`'s custom `Deserialize`
  ([job.rs:183-194](../../templemeads/src/job.rs#L183)) calls
  `Command::parse(&s, false)`, which calls `L::parse_instruction(...)`
  ([job.rs:119](../../templemeads/src/job.rs#L119)) - a call that
  **fails** if the incoming instruction string doesn't belong to `L`'s
  grammar. A router compiled with `L = greatwestern::Hpc` simply cannot
  deserialise (and therefore cannot relay) a Job whose instruction belongs to
  a different domain - `serde_json::from_str` returns `Err`, and
  `impl<L: Domain> From<Message> for Command<L>`
  ([command.rs:328-333](../../templemeads/src/command.rs#L328)) silently
  turns that into a `Command::Error`, dropping the Job rather than
  forwarding it.
- Nothing else in the routing path cares about `L::Instruction` at all:
  `diagnostics.rs`/`health.rs` already store `job.instruction().to_string()`
  (a `String`, via `Display`, not the typed value) precisely because they're
  meant to be domain-agnostic - confirmed during the original grammar-split
  audit (`grammar-split-design.md` §3). `result`/`result_type` on `Job` are
  already untyped (`Option<String>`) - a router never calls
  `job.completed()`/`job.result::<T>()` since it never executes anything.

So the entire gap is: **one required trait method, `Domain::parse_instruction`,
must always succeed for a router to be able to relay arbitrary domains'
Jobs.** Nothing else about `Board`, `handler.rs`'s routing logic, or the wire
format needs to change.

## 4. Key insight: the wire format is already domain-oblivious

`Command<L>` serialises via `Display`
([job.rs:165-169](../../templemeads/src/job.rs#L165)) to a single string:
`"<destination> <instruction-display>"` - this is the `"command"` field
documented in [json-types.md](../specifications/json-types.md) §Job. It is
**not** a structured `{destination: ..., instruction: {...}}` object. This
means: if a `Domain`'s `Instruction` type is defined so that
`parse_instruction` always succeeds (rather than validating a grammar) and
`Display` reproduces exactly what it was given, then a `Job<ThatDomain>`
round-trips **byte-for-byte identically** to whatever `Job<RealDomain>` the
originating leaf agent serialised - the router never needs to understand the
bytes to pass them through unchanged.

## 5. Chosen approach: an `Erased` Domain in templemeads

Add a new, small module - `templemeads::erased` - defining a `Domain`
implementation that is a total, non-validating passthrough:

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

/// The raw text of a notification event this agent doesn't understand,
/// plus the one structured case every `Domain` must support (see
/// `Domain::wrap_forward`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RawNotificationEvent {
    Raw(String),
    Forward(Box<Notification<Erased>>),
}
// Display: Raw(s) => s; Forward(n) => "forward [{}]", matching the
// convention every other Domain's Forward variant already follows.

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
        Ok(RawNotificationEvent::Raw(s.to_string())) // never fails
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
| **New code** | One new module, `templemeads::erased` (§5) - roughly the size of `templemeads::test_domain` - plus the two new `Job<L>` fields in §7. |
| **Changed code** | Only the specific router-role agents an operator chooses to switch from `Job<SomeDomain>` to `Job<Erased>` - a type-alias change, nothing structural. |
| **Unchanged** | `board.rs`, `command.rs`, `notification.rs`, the wire format's overall shape, every leaf agent, every existing `Domain` implementation (including `greatwestern`). |

This is deliberately the cheapest possible design for the capability: it
adds one implementation of an existing trait, rather than a parallel
type-erasure mechanism (`Box<dyn Any>`, a second generic parameter, etc.) -
see §11 for why that heavier alternative was rejected once already, in a
closely related context.

## 7. Per-job domain provenance: verifying at the destination, not just the connection

### 7.1 Why connection-level checking isn't enough once `Erased` exists

`agent::ensure_domain_matches::<L>(peer)` and the `domain`/`domain_version`
fields on `Register` (added before this design existed - see
[wire-protocol.md](../specifications/wire-protocol.md) §Register) tell an
agent what `Domain` its **directly connected peer** speaks. That's exactly
right for a leaf agent talking directly to another leaf agent. It stops
being useful the moment an `Erased` router sits in between: the peer a
destination leaf agent is directly connected to is the *router*, whose
`Domain::name()` is `"erased"` - not the `Domain` of whoever actually
authored the Job several hops upstream. Connection-level checking literally
cannot see through a domain-oblivious hop.

What's needed is a way for the **true originating `Domain`** to travel with
the Job itself, hop-for-hop, surviving any number of `Erased` relays, so the
agent that finally executes it can check - independent of who its immediate
neighbour is. This is good practice even in deployments with no `Erased`
router at all: it catches an instruction string that happens to parse
successfully under the *wrong* domain's grammar (two domains can coincidentally
share syntax for different meanings) before it's ever executed, not just
after the fact.

### 7.2 Correction: this belongs on `Job`, not `Envelope`

The natural place to reach for this is `Envelope<L>` - but `Envelope` is
**never serialised over the wire**. It's a purely local, in-process wrapper:
every call site that constructs one builds it fresh, right before handing a
Job to the registered runner -
[handler.rs:316](../../templemeads/src/handler.rs#L316) (the generic
dispatch path used by `instance`/`custom`/etc.), and the same pattern
independently in the `account`, `filesystem`, `portal`, and `scheduler` role
modules
([account.rs:57](../../templemeads/src/account.rs#L57),
[filesystem.rs:57](../../templemeads/src/filesystem.rs#L57),
[portal.rs:53](../../templemeads/src/portal.rs#L53),
[scheduler.rs:56](../../templemeads/src/scheduler.rs#L56)). What actually
travels hop-to-hop over the wire is `Job<L>` itself, via
`Command::Put/Update/Delete { job: Job<L> }`
([command.rs:29-37](../../templemeads/src/command.rs#L29)).

So the provenance tag has to live on `Job<L>`, not `Envelope<L>`, to survive
being relayed. Concretely, two new fields on the `Job<L>` struct
([job.rs:198](../../templemeads/src/job.rs#L198)):

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
```

Populated in `Job::parse()`
([job.rs:238](../../templemeads/src/job.rs#L238)) with
`Some(L::name().to_string())` / `Some(L::version().to_string())` -
mirroring exactly how `Register` picked up `domain`/`domain_version` for the
connection-level check, just captured once per-Job instead of once
per-connection. An `Erased` router constructs no new `Job` of its own (it
only ever relays one it received), so it never overwrites this field with
its own `"erased"` identity - the tag genuinely reflects the true origin,
end to end.

### 7.3 The destination-side check

A new function, `agent::ensure_job_domain_matches::<L>(job: &Job<L>, sender: &Peer) -> Result<(), Error>`,
called immediately before a runner is invoked (i.e. alongside each of the
`Envelope::new(...)` call sites in §7.2):

1. If `job.domain()` is `Some(d)`: compare `d` to `L::name()`. Match → `Ok`.
   Mismatch → `Err(Error::Incompatible(...))`.
2. If `job.domain()` is `None` (a Job from before this field existed):
   fall back to the *connection-level* signal already built for `Register` -
   `agent::peer_domain(sender)` - which already folds in
   `Domain::assume_legacy_domain_version` for exactly this situation. This
   is weaker (single-hop only) but is the best available signal for an old
   Job, and matches today's behaviour exactly for a deployment with no
   `Erased` router in it.
3. Otherwise (still unknown after both checks): fail-closed, same
   philosophy as `ensure_domain_matches` - `Err(Error::Incompatible(...))`.

**Deliberately not the same failure mode as `ensure_domain_matches`.** That
function disconnects the *peer*, because a connection-level mismatch means
every future Job from that peer is suspect. A single misrouted Job doesn't
mean the connection is bad - most other Jobs relayed over it may well be
correctly addressed. So this function only errors the *Job*
(`job.errored("...")`, same as any other execution failure), and leaves the
connection alone.

Like `ensure_domain_matches`, this is opt-in - templemeads doesn't call it
automatically, since not every agent needs the guarantee (e.g. an `Erased`
router itself never executes anything, so it has no use for this check).
Agents that do want it call it as the very first thing in their runner, or
templemeads could wire it in as a default pre-check for any agent that opts
in via `set_my_service_details` - left as an implementation choice for §9.

## 8. Gotchas / interactions with existing features

### 8.1 `ensure_domain_matches` and routers must not be mixed carelessly

`agent::ensure_domain_matches::<L>(peer)` checks that a connected peer's
`Register` domain equals `L::name()`, disconnecting otherwise - including
when the peer's domain is unknown (fail-closed, by design). **An `Erased`
router's `name()` is `"erased"`, which will never equal a leaf agent's real
domain name.** A leaf agent that calls `ensure_domain_matches::<Hpc>(&router_peer)`
against a directly-connected `Erased` router would disconnect it - exactly
backwards from what's wanted. (This is precisely why §7 exists as a
separate, per-Job check rather than trying to stretch the connection-level
one to cover it.)

This is not a bug to fix in `ensure_domain_matches` - that function does
precisely what it says for the case it's meant for (two agents that must
share a vocabulary to interoperate). It's a usage rule to document clearly
once `Erased` exists: **only call `ensure_domain_matches` between agents
that are expected to actually understand each other's `Instruction`s** (e.g.
two leaf agents directly exchanging domain-specific Jobs). Don't call it
against a peer whose role is routing-only - use `ensure_job_domain_matches`
(§7.3) instead, at the point of execution. This needs to be prominent in
`writing-a-domain.md` once `Erased` lands, so it isn't rediscovered the hard
way in production.

### 8.2 `owning_portal` / `check_portal`

Covered in §5's code comment - a non-issue because `check_portal` is only
ever `true` when a *portal* agent parses a fresh command string from a
human or bridge, and a portal is a leaf role. A router only ever receives
already-parsed `Command<L>` values over the wire, which always deserialise
with `check_portal = false`.

### 8.3 Diagnostics/health/logging

Already domain-agnostic (§3) - nothing to change. `RawInstruction`'s
`Display` reproduces the original instruction text, so log lines through an
`Erased` router look identical to what a same-domain router would have
logged.

### 8.4 A router still needs *a* runner

Every agent registers an `AsyncRunnable<L>`, called if the agent is ever
the final destination of a Job. A pure router should never legitimately be
a destination; `templemeads::erased` should ship a small default runner
that errors clearly (e.g. `Error::UnknownInstruction("this agent only
routes Jobs, it does not execute them")`) rather than reusing
`default_runner` (which calls `envelope.job().execute()` - not meaningful
for a `RawInstruction`).

## 9. Phased implementation plan

1. Add `templemeads::erased` (§5): `RawInstruction`, `RawNotificationEvent`,
   `Erased`, a router-appropriate default runner. Unit tests: round-trip
   parse/Display for `RawInstruction`/`RawNotificationEvent`, and a
   two-domain proof analogous to the one used for the original `Domain`
   split (`grammar-split-design.md` §12) - serialise a `Job<Hpc>`, deserialise
   it as `Job<Erased>`, re-serialise, and assert byte-identical output.
2. Add the `domain`/`domain_version` fields to `Job<L>` and populate them
   in `Job::parse()` (§7.2). This is useful independently of `Erased` and
   can land first/separately - it's pure backward-compatible addition
   (`#[serde(default)]`), same shape as the `Register` fields.
3. Add `agent::ensure_job_domain_matches` (§7.3) and decide the
   automatic-vs-opt-in question for how agents wire it into their runner
   dispatch.
4. Pick one existing routing-role agent (`provider` is the simplest - it
   only forwards, per `docs/README.md`'s description of the role) and
   switch its type alias to `Job<Erased>`/`Envelope<Erased>` as a proof of
   concept. Confirm its existing tests (if any) and a manual multi-hop
   routing scenario still pass unchanged, including a leaf agent on the far
   side successfully calling `ensure_job_domain_matches` and seeing the
   *original* domain, not `"erased"`.
5. Document the `ensure_domain_matches` vs. `ensure_job_domain_matches`
   usage rule (§8.1) prominently in `writing-a-domain.md`.
6. Leave `clusters`/other routing-role agents on their current concrete
   `Domain` unless/until there's an actual multi-domain deployment need -
   this design makes the switch available, it doesn't mandate it.

## 10. Testing strategy

- **Round-trip fidelity**: for a representative sample of real
  `greatwestern::Instruction` variants, serialise as `Job<Hpc>`, deserialise
  as `Job<Erased>`, re-serialise, and diff against the original bytes -
  must be identical, including the new `domain`/`domain_version` fields.
- **Rejects nothing**: `Erased::parse_instruction`/`parse_notification_event`
  must never return `Err` for any input string (property-test with
  arbitrary strings, including empty and malformed ones) - the whole point
  is that a router can't reject what it doesn't understand.
- **Multi-domain proof**: two different toy `Domain`s (e.g.
  `templemeads::test_domain::TestDomain` and a second toy domain) both
  routed successfully through one `Erased` agent in the same test process,
  each arriving at its respective (correctly-typed) leaf agent intact, *and*
  each leaf agent's `ensure_job_domain_matches` call confirming the correct
  origin domain despite the intermediate `Erased` hop.
- **Provenance survives multiple `Erased` hops**: a Job relayed through two
  or more chained `Erased` routers still carries its original `domain`/
  `domain_version` unchanged at the far end.
- **Legacy fallback**: a `Job` JSON blob with no `domain`/`domain_version`
  keys at all still deserialises (`#[serde(default)]`), and
  `ensure_job_domain_matches` correctly falls back to the sender's
  connection-level `peer_domain`.
- **Runner safety**: confirm the default `Erased` runner errors rather than
  panicking if a Job is ever actually addressed to the router itself.

## 11. Rejected/deferred alternative: a second, heavier type-erasure layer

The original grammar-split design considered and rejected a fully
type-erased `Job` payload (`(String, String)` json+type-name, downcast per
agent) for the *entire* framework
([grammar-split-design.md](archive/grammar-split-design.md) §11) - rejected
because it costs every leaf agent a fallible downcast for a property
(compile-time exhaustive matching) they'd otherwise get for free.

That rejection doesn't apply here: `Erased` isn't a general replacement for
`Job<L>`, it's one more implementation of the *existing* `Domain` trait,
opted into only by agents that were never going to pattern-match on
`Instruction` anyway. Leaf agents keep exactly the ergonomics they have
today; only the subset of agents that already don't need typed instructions
gain the ability to stop pretending they do.
