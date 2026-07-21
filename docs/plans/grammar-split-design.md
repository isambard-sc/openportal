<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# Splitting the command grammar out of templemeads: Design & Implementation Plan

Status: **draft design** — not yet implemented. This document records the
design decided in conversation so it can be picked up, reviewed, or handed
to someone else without re-deriving it. No code has been changed yet.

**Decisions locked in**: the new crate is named **`greatwestern`** (continuing
the `paddington`/`templemeads` UK-rail-terminus theme — GWR is the historic
and current operator of the line connecting the two); `PortalIdentifier`
stays in `templemeads` (see §8); everything else in `grammar.rs`
(`Instruction` and every other identifier/report type) moves to
`greatwestern` (see §8).

## 1. Goal

A developer outside the HPC/Waldur domain this codebase was built for wants
to reuse `paddington` (the wire protocol) and `templemeads` (the job/agent
framework) for their own domain, with their own command vocabulary, without
forking either crate. Concretely: `templemeads::grammar::Instruction` and
the identifier types it's built from (`ProjectIdentifier`, `UserIdentifier`,
`PortalIdentifier`, ...) need to become a *pluggable, compile-time choice*,
not a fixed enum baked into the framework. Two agents built against
different grammars are not expected to interoperate — that's an accepted
consequence, not a defect to design around.

## 2. Non-goals (this iteration)

- Runtime/dynamic grammar switching. The choice is made once, at compile
  time, per agent binary — matching the request exactly. No plugin loading,
  no `dyn` dispatch for the instruction type itself.
- Changing anything in `paddington`. The coupling this document addresses
  is entirely inside `templemeads`; `paddington` already only ever moves
  opaque bytes/JSON between peers and has no grammar dependency today.
- Preserving wire compatibility between a domain-specific grammar and the
  existing HPC one. They're different types; they were never meant to
  interoperate.
- Redesigning the HPC command vocabulary itself. `Instruction` and its
  identifier types move house, but their shape does not change.

## 3. Current coupling audit

Before designing the split, the actual coupling was traced through the
code (not assumed). It's wider than "the `Instruction` enum":

| File | Coupling | Notes |
|---|---|---|
| `templemeads/src/job.rs` | `Command { destination, instruction: Instruction }` ([job.rs:96-100](../../templemeads/src/job.rs#L96)) is the private struct backing every `Job`. | This is the core coupling point. |
| `templemeads/src/job.rs` | `Command::parse`'s `check_portal` block ([job.rs:127-227](../../templemeads/src/job.rs#L127)) pattern-matches ~35 concrete `Instruction` variants to enforce "a job about user/project X can only be issued via X's portal". | This is domain **policy**, not transport logic, sitting inside supposedly-generic job parsing. |
| `templemeads/src/command.rs` (control-plane `Command`, distinct from the grammar `Command` above) | `enum Command { Put { job: Job }, Update { job: Job }, ... }` embeds `Job` directly. | Needs the same generic parameter as `Job`. |
| `templemeads/src/notification.rs` | `NotificationEvent` embeds `UserIdentifier`/`ProjectIdentifier` directly, with its own hand-written `parse()`, structurally identical in spirit to `Instruction`. | A second, independent piece of domain vocabulary alongside `Instruction`. |
| `templemeads/src/bridge_server.rs` | Imports `PortalIdentifier` directly to resolve `/api/...` HTTP paths to a portal scope. | **Not actually a leak** (revised — see §8): `PortalIdentifier` is decided to live in `templemeads`, not the domain crate, since it names a position in the fixed agent hierarchy (`agent::Type::Portal`), not domain vocabulary. This file was already reaching for the right type. |
| `templemeads/src/storage.rs`, `storagereport.rs`, `usagereport.rs` | Import identifier types and/or `NamedType` from `grammar`. | `Quota`/`Volume`/`UsageReport`/`StorageReport` are HPC-accounting vocabulary — decided to move to `greatwestern` in full (see §8). |
| `templemeads/src/diagnostics.rs`, `health.rs` | Import `NamedType` only; diagnostics already stores `instruction: String` (via `Display`), not a concrete `Instruction`. | **Not actually domain-coupled** beyond the misplaced `NamedType` trait — good news, see §6. |
| Every agent binary (`freeipa`, `slurm`, `filesystem`, `cluster`, `portal`, `bridge`, `cloudaccount`, `localaccount`, `docs/job`, `docs/cmdline/*`) | `match job.instruction() { Instruction::AddUser(..) => ... }` in `main.rs`. | Expected/fine — this is exactly the code that's supposed to be domain-specific. |
| `python/src/lib.rs` | Extensively calls `job.result::<grammar::UserIdentifier>()` etc. and re-exports grammar types to Python. | Inherently pinned to one concrete grammar (see §9) — not a framework concern. |

Useful existing precedent found during the audit: `Job::completed<T>()` /
`Job::result<T>()` ([job.rs:629](../../templemeads/src/job.rs#L629),
[job.rs:737](../../templemeads/src/job.rs#L737)) already treat a job's
*result* payload generically — stored as `(json string, type-name string)`
behind a `Serialize + NamedType` / `DeserializeOwned` bound, not a concrete
enum. And `agent_core::Config<T = ()>` / `Defaults<T = ()>`
([agent_core.rs:24](../../templemeads/src/agent_core.rs#L24)) already
thread a generic, per-agent payload type through every `agent::{instance,
account, scheduler, filesystem, portal, platform}::run()` function. Both
are exactly the shape of change this design needs to make — this is not an
alien pattern for the codebase, just one that needs to be applied one level
higher, to the instruction/notification types themselves.

## 4. Chosen approach: a single `Domain` trait, one generic parameter

Two shapes were considered (see §11 for the rejected alternative). Decision:
**a generic type parameter threaded through templemeads's core types,
bounded by one trait that a domain crate implements.**

```rust
// in templemeads, e.g. templemeads::domain
pub trait Domain: Clone + Send + Sync + 'static {
    type Instruction: Clone + PartialEq + std::fmt::Debug + std::fmt::Display
        + Serialize + for<'de> Deserialize<'de> + NamedType + Send + Sync + 'static;

    type NotificationEvent: Clone + PartialEq + std::fmt::Debug
        + Serialize + for<'de> Deserialize<'de> + Send + Sync + 'static;

    fn parse_instruction(s: &str) -> Result<Self::Instruction, Error>;
    fn parse_notification_event(s: &str) -> Result<Self::NotificationEvent, Error>;

    /// The portal that "owns" this instruction, i.e. whose name a
    /// destination's first hop must match, if this instruction has one.
    /// `PortalIdentifier` lives in templemeads itself (see §8), not the
    /// domain crate, so this returns the real type, not a bare string.
    /// Default: no such policy. The HPC domain overrides this to reproduce
    /// today's check_portal behaviour (see §7).
    fn owning_portal(_instruction: &Self::Instruction) -> Option<PortalIdentifier> {
        None
    }
}
```

One generic parameter (`L: Domain`, read "language") is threaded through
templemeads, rather than two independent parameters for instructions and
notifications — an agent binary picks exactly one domain, once, and every
generic type in the framework closes over it the same way. Associated
types (not a second/third type parameter) carry `Instruction` and
`NotificationEvent`, since they always travel together and a bare `L` reads
better at every call site than `Job<I, N>`.

`Destination` is **not** touched — the audit (§3, and confirmed by reading
`destination.rs` in full) shows it only borrows the misplaced `NamedType`
trait from `grammar.rs`; it has no actual dependency on domain vocabulary.
Routing/addressing is already domain-agnostic today.

## 5. What becomes generic

| Type / module | Change |
|---|---|
| `job::Job` → `Job<L: Domain>` | `command: Command<L>` field, `instruction()` returns `L::Instruction`. |
| `job::Envelope` → `Envelope<L: Domain>` | Wraps `Job<L>`. |
| `job::Command` (private) → `Command<L: Domain>` | `instruction: L::Instruction`; `parse` calls `L::parse_instruction`. |
| `command::Command` (control-plane enum) → `Command<L: Domain>` | `Put { job: Job<L> }` etc. |
| `board::Board` → `Board<L: Domain>`, `Waiter`, `BoardJobStats` | `jobs: HashMap<Uuid, Job<L>>`; stats/bookkeeping carry no payload so may stay largely unchanged mechanically. |
| `bridgeboard.rs`, `bridgestate.rs` | The process-global board storage becomes generic over the one `L` the binary was compiled with. |
| `runnable::AsyncRunnable` → `AsyncRunnable<L: Domain>` | `fn(Envelope<L>) -> Pin<Box<dyn Future<Output = Result<Job<L>, Error>> + Send>>`. Still a bare `fn` pointer, just parameterized. |
| `notification::{Notification, NotificationEnvelope}` → `Notification<L>`, `NotificationEnvelope<L>` | `event: L::NotificationEvent`. |
| `handler.rs` (`process_message`, `set_my_service_details`, ...) | Parameterized over `L`; this is where most of the mechanical churn lands. |
| `agent::{instance, account, scheduler, filesystem, portal, platform, custom}::run()` | Each gains `<L: Domain>`, following the `Config<T>` precedent already in `agent_core.rs`. |
| `bridge_server.rs` | No change needed beyond adding `<L>` where it touches `Job`/`Command` — its `PortalIdentifier` import already points at the right (templemeads-native) type, see §7. |
| `restart.rs` | Confirmed (by reading it) to have **no** grammar/instruction references today — only needs `<L>` added to the `Job`/`Board` types it touches, no logic changes. |
| `diagnostics.rs`, `health.rs` | Confirmed to store rendered `String`s already (`instruction: job.instruction().to_string()`), so these only need an `L: Domain` bound to call `.to_string()` on — no structural change, no domain import once `NamedType` moves (§6). |

## 6. Preparatory cleanup: `NamedType`

`NamedType` (the trait that gives ts-rs-exported types a string name, used
by `Job::completed<T>`'s result encoding) is defined in `grammar.rs`
([grammar.rs:18](../../templemeads/src/grammar.rs#L18)) but is genuinely
generic — `storage.rs`, `health.rs`, and `diagnostics.rs` all depend on it
for types that have nothing to do with the command grammar. **Move it out
of `grammar.rs` into a neutral home first** (e.g. `templemeads::named`, or
fold into `job.rs` where `completed<T>` lives) as an independent, safe,
non-breaking first PR. This shrinks every subsequent diff and removes a
false "grammar dependency" from four files before the real work starts.

## 7. Relocating the `check_portal` policy

`Command::parse`'s scope-ownership check (§3) is HPC-specific *policy*
(which instruction variants carry a user/project, and therefore imply an
owning portal), but the *type* it checks against, `PortalIdentifier`, is a
templemeads concept (§8) — it names a position in the fixed agent hierarchy
(`agent::Type::Portal`), and is the type this trust boundary ("who may
submit a job targeting this scope") is expressed in throughout the
codebase, independent of which domain vocabulary rides on top.

So the policy becomes the default-`None` `Domain::owning_portal()` method
(§4): templemeads's `Command::parse` calls `L::owning_portal(&instruction)`
and, if `Some(portal)` comes back, compares `portal.portal()` against
`destination.first()` — otherwise (the default for a new domain) no such
check runs. The HPC domain crate (`greatwestern`) implements `owning_portal`
by porting the existing match arms verbatim (`AddUser`/`RemoveUser`/
`CreateProject`/... → `user.portal_identifier()` / `project.portal_identifier()`,
methods that already exist on `UserIdentifier`/`ProjectIdentifier` today),
preserving today's behaviour exactly for existing agents.

One small mechanical wrinkle this surfaces: `ProjectIdentifier::portal_identifier()`
and `UserIdentifier::portal_identifier()` currently construct a
`PortalIdentifier` via a direct struct literal (`PortalIdentifier { portal:
self.portal.clone() }`), relying on same-module field access
([grammar.rs:187-191](../../templemeads/src/grammar.rs#L187)). Once
`PortalIdentifier` lives in `templemeads` and `ProjectIdentifier`/
`UserIdentifier` live in `greatwestern`, that field is no longer visible
cross-crate — trivial fix, swap the struct literal for the existing public
`PortalIdentifier::parse(&self.portal)` constructor. Similarly, `impl
From<ProjectIdentifier> for PortalIdentifier`
([grammar.rs:200](../../templemeads/src/grammar.rs#L200)) has to be dropped
entirely: implementing a foreign trait (`From`, from `std`) for a foreign
type (`PortalIdentifier`, now in `templemeads`) from within `greatwestern`
violates Rust's orphan rules. The handful of call sites using `.into()`
switch to calling `.portal_identifier()` directly — the method that already
does the actual work today, the `From` impl was only ever sugar over it.

## 8. Resolved: crate name and the domain-vocabulary boundary

**Crate name: `greatwestern`.** Follows the existing convention of naming
framework-level crates after the two UK rail termini the wire protocol
metaphor is built on (`paddington`, `templemeads`) — Great Western Railway
is, historically and currently, the operator of the line connecting them,
which fits a crate carrying "the specific service/vocabulary that rides
on the generic track" better than a merely-descriptive name would.
Trade-off noted and accepted: it doesn't self-describe to a newcomer the
way `hpc-grammar` would.

**Vocabulary boundary — what moves to `greatwestern` vs what stays in
`templemeads`:**

| Stays in `templemeads` | Moves to `greatwestern` |
|---|---|
| `PortalIdentifier` | `Instruction` (the enum itself) |
| `NamedType` (§6) | `ProjectIdentifier`, `UserIdentifier`, `UserOrProjectIdentifier`, `ProjectMapping`, `UserMapping`, `UserOrProjectMapping` |
| `Destination`/`Destinations`/`Position` | `Quota`, `Volume` (`storage.rs`), `UsageReport`, `ProjectUsageReport`, `DailyProjectUsageReport`, `Usage` (`usagereport.rs`), `StorageReport`, `ProjectStorageReport` (`storagereport.rs`) |
| `agent::Type`/`Peer` (hierarchy roles) | `Hour`, `Date`, `DateRange`, `ProjectTemplate`, `Node`, `Allocation`, `DomainPattern`, `Link`, `Note`, `MembershipControl`, `AwardDetails` — every remaining grammar.rs type |
| | `NotificationEvent`'s variants (though `Notification<L>`/`NotificationEnvelope<L>` themselves stay in templemeads, generic) |

Rationale for the split, resolved from the discussion:

- **`PortalIdentifier` stays** because it names a fixed position in
  templemeads's agent hierarchy (`agent::Type::Portal` already exists,
  independent of any grammar) and because it's the type the framework's
  trust boundary — "which peer may submit a job targeting this scope" — is
  expressed in (§7). It is not domain vocabulary; a brand-new domain still
  has Portals in the same structural sense the HPC one does.
- **Everything else moves**, including `ProjectIdentifier`/`UserIdentifier`
  despite being "tied to the instructions" — precisely because they *are*
  tied to the instructions (a chemistry-lab domain has no use for
  "project"/"user" as concepts any more than it has use for `AddUser`), and
  because `UsageReport`/`StorageReport` are confirmed HPC-accounting
  concepts with no generic equivalent worth inventing. A genuinely new
  domain should find zero leftover HPC vocabulary in what's supposed to be
  a generic framework crate, `PortalIdentifier` (and the fixed agent-role
  concept it belongs to) being the one deliberate exception.

## 9. What does *not* change

- **`paddington`** — untouched entirely.
- **`Destination`/`Destinations`/`Position`** — already domain-agnostic
  (§4), no change beyond no longer importing `NamedType` from `grammar`.
- **`agent.rs`'s `Peer`/`Type` (AgentType)** — these describe the agent
  hierarchy (Portal/Provider/Platform/Instance/Account/...), which is
  structural to templemeads, not part of the swappable vocabulary. A new
  domain still slots into Portal→Provider→...→leaf; only what's carried
  *inside* a `Job` changes.
- **`PortalIdentifier`** — stays in templemeads for the same structural
  reason as `agent::Type::Portal` above (§8); it moves module (out of
  `grammar.rs`, likely into `agent.rs` alongside `Peer`/`Type`, or its own
  small module) but not crate.
- **`python/src/lib.rs`, `op-bridge`** — these are consumers of one
  specific, concrete instantiation (today's HPC domain) and stay that way.
  "Generic templemeads" does not imply generic Python bindings; the
  bridge/python crates simply become `templemeads::job::Job<greatwestern::Hpc>`
  consumers instead of `templemeads::job::Job` consumers.
- **The wire format itself, content-wise.** `Job<greatwestern::Hpc>` serializes to
  the same JSON shape `Job` does today, because `greatwestern::Hpc::Instruction`
  is the existing `Instruction` enum, moved not modified. This is a
  Rust-API break (import paths, one added type parameter everywhere), not a
  network-protocol break, for every agent that stays on the HPC domain.

## 10. Phased implementation plan

1. **`NamedType` relocation** (§6). Independent, non-breaking, small PR.
   Confirms the boundary is as clean as the audit suggests before the real
   work starts.
2. **Introduce the `Domain` trait** in templemeads (§4), unused at first.
   No existing code changes yet — just land the trait and its bounds so
   the shape can be reviewed independently of the (large) mechanical
   refactor.
3. **Parameterize templemeads's core types** (§5) over `L: Domain`, but
   temporarily instantiate everything internally with the *existing*
   `grammar::Instruction`/`NotificationEvent` still living in
   `templemeads::grammar` (i.e. `type L = grammar::Hpc;` defined inside
   templemeads for now). This isolates "does the generic refactor compile
   and pass the existing test suite" from "did the domain vocabulary move
   correctly" — two separately-verifiable steps instead of one large one.
4. **Move the domain vocabulary** out of `templemeads::grammar` and
   `storage.rs`/`storagereport.rs`/`usagereport.rs` (§8's table) into
   `greatwestern`, implement `Domain` for it there, delete
   `templemeads::grammar`. `PortalIdentifier` and `NamedType` are pulled out
   to their own templemeads-native homes first (not moved to
   `greatwestern`). Fix up the two `PortalIdentifier`-construction call
   sites flagged in §7 (`.portal_identifier()` via `PortalIdentifier::parse`
   instead of a private-field struct literal; drop the now-orphan-rule-
   violating `From<ProjectIdentifier> for PortalIdentifier` impl). Existing
   agents switch their imports from `templemeads::grammar::*` to
   `greatwestern::*`, and from `templemeads::job::Job` to
   `templemeads::job::Job<greatwestern::Hpc>` (a single type alias per agent
   binary, e.g. `type Job = templemeads::job::Job<greatwestern::Hpc>;` near
   the top of each `main.rs`, keeps the rest of that binary's code
   mostly unchanged — every other call site already just says `Job`).
5. **Update `docs/job`, `docs/cmdline/*`, `docs/echo`** — these examples
   currently reference `templemeads::grammar` too and are the natural
   place to *also* add one small "toy" second domain (a 3-4 variant
   `Instruction` enum, e.g. extending `docs/job`), proving in-tree that
   two domains coexist and don't interoperate (§12).
6. **`python/src/lib.rs`, `op-bridge`** — update to `greatwestern` + the type
   alias, mechanical only, per §9.
7. **ts-rs export test** (currently in `templemeads/src/lib.rs`, exporting
   `grammar`'s `Link`/`Note`/`MembershipControl`/`AwardDetails` etc.) moves
   to the new domain crate — TS bindings are inherently an artifact of one
   concrete instantiation, so this test cannot meaningfully stay generic in
   templemeads.
8. **Workspace/docs update**: `Cargo.toml` workspace members, `CLAUDE.md`'s
   crate list and description of `templemeads`/`grammar`, updated to
   reflect `greatwestern` and templemeads's now-generic framing.
9. **Hardening**: `cargo fmt`/`cargo clippy` clean across the workspace;
   full existing test suite green at every step above, not just at the end.

## 11. Rejected alternative: type-erased payload

Considered: keep `Job` concrete/non-generic, and store `Instruction`
type-erased (a `(String, String)` json+type-name pair, exactly like
`Job::result<T>()` already does for results), with each agent downcasting
back to its own concrete enum at the top of its handler. This is a much
smaller diff — `Job`, `Board`, `Envelope` etc. never need a type parameter
at all.

Rejected because: it gives up compile-time exhaustive matching at the
point every agent binary actually consumes instructions (today's
`match job.instruction() { Instruction::AddUser(..) => ... }` becomes a
fallible parse-then-match, one extra `Result` to handle per agent, for a
property — "this agent's grammar is fixed at compile time" — that's
already true. The generic-parameter approach costs more diff churn once,
centrally, in exchange for every agent binary keeping exactly the
ergonomics it has today. Given the request explicitly frames the grammar
choice as compile-time, paying the refactor cost in the framework once is
the better trade than paying an ergonomics cost in every agent forever.

## 12. Testing strategy

- **Step 3 (§10) is a checkpoint, not just an intermediate state**: the
  entire existing test suite (`make test`) must pass unmodified in
  behaviour once the generic refactor lands but before anything moves out
  of templemeads — this isolates "generic refactor is behaviour-preserving"
  from "vocabulary relocation is correct" as two independently falsifiable
  claims.
- **Two-domain proof, in-tree**: build a second, minimal domain (§10 step
  5) alongside the real HPC one and a matching toy pair of agents. Assert,
  as an actual test: (a) both compile and run against the same
  `templemeads`/`paddington`; (b) a `Job<Toy>` serialized to JSON fails to
  deserialize as `Job<Hpc>` (proving incompatibility is enforced by the
  type system, not merely assumed); (c) the toy agent's exhaustive
  `match` on its own `Instruction` still compiles with no downcast/`Any`
  involved.
- **Existing HPC-domain regression**: every current agent's existing tests
  (freeipa, slurm, filesystem, cluster, portal, bridge, cloudaccount,
  localaccount) pass unchanged in behaviour after the type-alias switch —
  this is meant to be invisible to them at runtime.
- `cargo clippy` clean workspace-wide, given `unwrap_used`/`expect_used`
  are denied — the `Domain` trait's bounds should be chosen so that
  generic code never needs an escape hatch (e.g. `owning_portal` returning
  a real `Option<PortalIdentifier>` rather than requiring a fallible downcast
  or a stringly-typed comparison).
