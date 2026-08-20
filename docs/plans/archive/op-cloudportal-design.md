<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# op-cloudportal: Design & Implementation Plan

Status: **withdrawn**. The `op-cloudportal` agent described below was written,
then removed: on reflection the cloud integration does not need a bespoke
Rust agent at all. The cloud operators can run a stock `op-portal` and
`op-bridge` and put their own (Python) portal software behind it, holding
whatever state they need on their side of the bridge - see
[site-portal-api.md](../../specifications/site-portal-api.md) for the
contract such software implements. This document is kept only as a record
of the design decisions and rationale; the code it describes is no longer
in the tree (see git history for `cloudportal/`).

## 1. Goal

Add a new agent, `op-cloudportal`, that acts as the "cloud" portal in a
portal-to-portal relationship: a central "airr" portal creates Awards on it
(`create_project`/`create_award`), the same way any OpenPortal portal can
create Awards on a downstream/child portal. There is no real portal
management software (no Waldur) behind `op-cloudportal` — it is a
deliberately rough, self-contained prototype, in the same spirit as
`op-cloudaccount` (see `op-cloudaccount-design.md` in this same directory).

Two things `op-cloudportal` must do:

1. Accept the standard portal-level instructions (`create_project`,
   `update_project`, `remove_project`, `get_project`, `get_award`,
   `get_awards`, `get_projects`, `get_project_mapping`, `get_users`,
   `get_usage_report(s)`, `get_storage_report(s)`) from an upstream portal,
   storing Award state itself instead of relaying to a bridge/Waldur.
2. Let a human cloud operator review and approve pending Awards before
   anything is actually provisioned on the corresponding `op-cloudaccount`.

## 2. Non-goals (this iteration)

- Any real connection to Waldur or any other portal management software.
- Automatic provisioning. Award creation and infrastructure provisioning
  are deliberately decoupled (§7) — there is a human in the loop.
- A `Provider`/`Platform` layer between `op-cloudportal` and
  `op-cloudaccount`. This prototype talks to `op-cloudaccount` directly
  (§3) — `op-cloudaccount` doesn't check sender type, so nothing stops
  this, and building a full hierarchy for a rough prototype isn't worth it.
- Multi-currency / cross-offering aggregation, FX conversion, or anything
  else out of scope for `op-cloudaccount` (that project's non-goals apply
  transitively to anything this agent forwards to it).

## 3. Where this sits in the agent hierarchy

```
airr (Portal)
  |
  v  create_project someproject.cloud {"template":"aws", ...}
  |  (a plain, direct 2-hop send - airr and cloud are ordinary connected peers)
  v
op-cloudportal  (AgentType::Portal)
  - stores Award state itself (file-backed, one JSON file per project)
  - AwardDetails.template ("aws"/"azure"/...) picks which cloud provider
  - each template value maps (via config) to a specific op-cloudaccount peer
  - create_project / update_project / etc. handled locally - no bridge
  |
  |  (only after a human operator approves - see §7)
  v
op-cloudaccount  (AgentType::Instance, one process per cloud account)
  - add_project / add_user actually provision the award
```

`op-cloudportal` registers as `AgentType::Portal`, using
`templemeads::agent::portal::{process_args, run, Defaults}` — the same
framework module `op-portal` uses (with one addition to `run()` itself,
see §8).

### Why multiple offerings, not one

`op-cloudportal` can front more than one cloud provider — e.g. `aws` and
`azure`, each corresponding to its own `op-cloudaccount` process/account.
A project could in principle be awarded against either, so something has
to disambiguate which `op-cloudaccount` peer a given Award should
eventually be provisioned against. §4 covers how.

## 4. Addressing model: direct peer-to-peer, disambiguated by `template`

**This section originally proposed a "virtual-resource offering" model
mirroring `op-portal`'s `sync_offerings`/`virtual_resource_runner`
mechanism, gated on empirical verification. That verification was done —
by reading `templemeads::virtual_agent::send()` rather than a live spike,
which turned out to be conclusive — and the model doesn't hold. This
section replaces it.**

`virtual_agent::send()` is the function that runs whenever a job is sent
to a peer registered as `Type::Virtual`:

```rust
pub async fn send(destination: &Option<Destination>, message: Message) -> Result<(), Error> {
    ...
    match process_message(message).await {   // <- this process's OWN handler
```

It calls `process_message` — the *same process's own* message handler,
the one registered for real incoming network connections. Sending to a
virtual peer never crosses the network: it just re-injects the message
into the local handler, as if it had arrived from outside. Virtual peers
exist so a portal can route jobs **it generates locally** (from its own
bridge) to a named sub-resource without a real connection for each one —
they are not, and cannot be, a way for a genuinely separate remote peer
(`airr`) to address a named sub-identity of another remote peer (`cloud`).
`airr` has no way to make its own outgoing `Command::send_to` calls
resolve to `cloud`'s privately-registered virtual peers, and the one code
path that could plausibly bridge that gap (`Submit`, in `portal_runner`)
is explicitly gated to `Bridge`-type senders only — a `Portal` sender like
`airr` hits `Err(MissingAgent(...))` today.

**Corrected model**: skip virtual resources and offerings entirely.

- `cloud` is a plain, direct peer of `airr` — an ordinary portal-to-portal
  connection, no different in kind from any other pair of connected agents
  in this codebase (the same shape as `ukri.toml`'s `[[service.servers]]`/
  `[[service.clients]]` entries, just without a bridge on either end).
- `create_project`/`get_award`/etc. are accepted **directly** by our own
  `cloudportal_runner`, with no sender-type gating — the same pattern
  `op-cloudaccount`/`op-cluster` already use (they don't check sender type
  either).
- **`AwardDetails.template`** is what picks the cloud provider. It's
  already a documented, real wire field for exactly this purpose — the
  instruction-protocol.md's own worked example is
  `create_project myproject.waldur {"template":"cpu-cluster", ...}`.
  `op-cloudportal` reads `template` (`"aws"`, `"azure"`, ...) and looks it
  up in a small config table (§9) mapping template value → `op-cloudaccount`
  peer name.

This is simpler than the original model in every respect: no offering
registration, no `sync_offerings`/virtual-peer machinery to reimplement
without a bridge, no uncertain wire-level delivery mechanics to verify.

## 5. State model

One JSON file per Award/project, under a configured `state-dir`, same
file-per-project / atomic-write convention as `op-cloudaccount`'s
`state.rs`:

```jsonc
{
  "details": { /* AwardDetails, see §6 */ },
  "offering": "aws",
  "status": "pending",   // "pending" | "approved" | "rejected"
  "provisioned_users": [] // UserIdentifiers already add_user'd on op-cloudaccount
}
```

`status` and `offering` are local bookkeeping — not part of the wire
protocol (`AwardDetails` itself is unchanged; see
`docs/specifications/instruction-protocol.md` for its full schema:
`name`, `template`, `description`, `members`, `start_date`/`end_date`,
`allocation`, `breakdown`, `award`/`call`/`project_link`/`renewal` links,
`notes`, `membership_control`, `allowed_domains`).

**No in-memory cache.** Unlike `op-cloudaccount`'s write-through cache,
`op-cloudportal`'s state should be read fresh from disk on every
instruction. Reason: approval (§7) happens via a *separate* one-off CLI
invocation while the main `op-cloudportal run` server process is
continuously running to receive jobs from `airr`. A write-through
in-memory cache in the long-running process would go stale the moment the
separate approval process edits the files on disk, and wouldn't reflect
the change until a restart. Given Awards are created/approved rarely
compared to normal request traffic, reading fresh every time costs
nothing and sidesteps the whole staleness question.

## 6. Instruction surface

Same instruction set `op-portal`'s `virtual_resource_runner` handles,
resolved against local file state instead of a bridge:

| Instruction | Behaviour |
|---|---|
| `create_project` / `create_award` | Store a new Award record with `status: pending`, `offering` set from `AwardDetails.template` (§4). Returns a `ProjectMapping` immediately — the *award* is recorded even though nothing is provisioned yet (§7). Reject with a clear error if `template` is missing or doesn't match a configured offering (§9, §10). |
| `update_project` / `update_award` | Merge the incoming `AwardDetails` into the existing record (per-field merge semantics as specified for `AwardDetails`, e.g. `breakdown` merges key-by-key, `notes` appends). Does not change `status`. |
| `remove_project` | Remove the Award record. Does **not** automatically deprovision on `op-cloudaccount` — that's a separate, deliberate step (mirrors `op-cloudaccount`'s own "don't auto-delete" philosophy). |
| `get_project` / `get_award` | Return the stored `AwardDetails`. |
| `get_awards` | Return `AwardDetails` for every Award under this portal. |
| `get_projects` | Return `ProjectMapping` for every Award. |
| `get_project_mapping` | Return the `ProjectMapping` for one Award. |
| `get_users` | Return `UserMapping`s derived from `AwardDetails.members`. |
| `get_usage_report` / `get_usage_reports` | Forward to whichever `op-cloudaccount` the Award's `offering` maps to (§9), and return its answer directly — `op-cloudportal` does not compute usage itself. |
| `get_storage_report` / `get_storage_reports` | Return an empty report (`ProjectStorageReport::new(project)` / `StorageReport::new(portal)`) rather than erroring — cloud accounts don't have a POSIX-style filesystem/quota concept, but returning empty is safer than failing a caller that always asks for both usage and storage. |
| everything else | `Error::InvalidInstruction`, matching the catch-all pattern every other agent uses. |

## 7. The approval workflow

Award creation and infrastructure provisioning are deliberately decoupled
— there must be a human in the loop, since provisioning spends real money
on a real cloud account. `create_project` only ever records the Award；
nothing is provisioned until a cloud operator explicitly approves it.

**Why this isn't a wire instruction**: `approve`/`reject` are pure local
admin actions — `airr` never sends them, only the cloud operator invokes
them, and they don't need to be understood by any other agent in the
network. Adding them to the shared `greatwestern::grammar::Instruction`
enum would grow a protocol every other agent has to at least
pattern-match against, for a verb that is specific to this one prototype
agent's admin workflow. Recommend instead: **bespoke CLI subcommands
local to `op-cloudportal`**, sitting alongside (not instead of) the
standard `init`/`client`/`server`/`extra`/`secret`/`run` subcommands every
agent already has:

```bash
op-cloudportal list-pending
op-cloudportal approve --project someproject.cloud
op-cloudportal reject  --project someproject.cloud --reason "..."
```

**These CLI subcommands never touch the network themselves.** Reaching
`op-cloudaccount` requires a real paddington connection, and those are
only established by `paddington::run()` inside the long-running `run`
server process — a one-off CLI invocation never calls that, so it has no
live connections to send anything over (this is also why the framework's
existing one-shot mechanism, §8, never makes outbound calls to other
agents: none of its existing users need to). So:

- `approve` just flips the Award's `status` to `approved` on disk — a
  pure file edit, no different from `reject`.
- `reject` flips `status` to `rejected` (with an optional reason recorded,
  e.g. as an `AwardDetails.notes` entry).
- The **live `op-cloudportal run` process** — which does have real
  connections — runs a small background `tokio::spawn`ed poller
  alongside the normal job-handling loop (started in `main()` before
  calling `run()`; no changes to shared `templemeads` code needed for
  this part). Every N seconds it scans the state directory for Awards
  with `status: approved` whose `provisioned_users` doesn't yet cover all
  of `AwardDetails.members`, looks up the `op-cloudaccount` peer for their
  `offering` (§9), and calls `add_project`/`add_user` for whatever's
  missing, updating `provisioned_users` as each succeeds.

This keeps the CLI tool trivial (pure state-file I/O, no networking) and
means provisioning naturally retries on the next poll if it partially
fails — the same idempotency property §10 already called for, just
achieved by polling instead of by the CLI call itself being retried.

## 8. One-shot CLI mode (separate, smaller addition)

Independently of §7, `templemeads::account::run()` (and `filesystem`/
`scheduler`) already support a **one-shot command** mode
([templemeads/src/account.rs:39-79](../../../templemeads/src/account.rs#L39-L79)):
`run --one-shot "instruction args"` synthesizes a local `Envelope`/`Job`,
runs it through the real instruction handler, pretty-prints the JSON
result, and exits — no network listener. `templemeads::portal::run()`
doesn't have this yet.

**Recommend adding it**, mirroring `account.rs` exactly. This is a small,
additive, backward-compatible change to shared framework code (behaves
identically to today if no `--one-shot` flag is given) and is generically
useful for debugging/testing *any* portal agent, `op-portal` included —
e.g. `op-cloudportal run --one-shot "get_awards cloud"` to inspect state
without needing a live network peer. Note this only works for the
locally-answered instructions (§6) — `get_usage_report(s)` needs a real
connection to `op-cloudaccount`, which one-shot mode (by design, like
every other one-shot user today) doesn't have. This is unrelated to the
approve/reject workflow in §7, which deliberately does *not* go through
the Instruction/Job pipeline.

## 9. Config surface (new)

- `state-dir` — where Award JSON files live.
- `offerings` — a table mapping `AwardDetails.template` value →
  `op-cloudaccount` peer name, as an `extra` key/value pair following the
  existing `instance-groups`-style comma-separated convention used
  elsewhere: `"aws:cloudaccount-aws,azure:cloudaccount-azure"`. Looked up
  purely in-process (a `HashMap`, populated from config at startup) — no
  peer registration needed, since `op-cloudaccount` peers are ordinary
  connected peers reached the normal way (`agent::find`).

## 10. Failure modes & defensive behaviours

Following the rest of the codebase's warn-don't-fail philosophy:

- `create_project` with a `template` value that isn't in the configured
  `offerings` table is a hard error (`InvalidInstruction` or similar) —
  unlike most of this design's "warn and continue" choices, there's no
  sensible default cloud provider to fall back to, so this one genuinely
  should fail loudly and immediately rather than silently drop the Award.
- A `get_usage_report`/`get_usage_reports` call for an Award whose
  `op-cloudaccount` peer can't be found or doesn't respond in time should
  log a warning and return an empty report, not error the whole request.
- `approve` on an Award that's already `approved` is idempotent — re-runs
  the "provision any not-yet-provisioned members" step rather than
  failing, so it's safe to re-run after a partial failure (e.g. `add_user`
  succeeded for two of three members before a crash).
- `update_project`/`remove_project` on an unknown project: `NotFound`,
  matching `op-cloudaccount`'s existing convention.

## 11. Open questions

- Does `airr` (or whatever configures it) expect template/offering names
  to be fixed (`aws`, `azure`) or should they be arbitrary/operator-chosen
  per deployment? This design assumes fixed, config-driven names.
- Should `reject` notify `airr` in any way (a `Notification`), or is a
  human simply expected to communicate the rejection out-of-band for now?
  Leaning towards out-of-band for this prototype, but worth confirming.

## 12. Phased implementation plan

1. **Skeleton agent**: new `cloudportal` crate, `AgentType::Portal`,
   `Cargo.toml` modeled on `portal/Cargo.toml`. Wire up
   `create_project`/`get_project`/`get_award`/`get_awards`/`get_projects`/
   `get_project_mapping`/`get_users`/`update_project`/`remove_project`
   against file-backed state (§5) only, resolving `template` → offering
   (§4, §9).
2. **Add the one-shot CLI mode to `templemeads::portal::run()`** (§8), add
   `list-pending`/`approve`/`reject` as bespoke subcommands (§7, pure
   state-file edits), and add the background provisioning poller
   (`tokio::spawn`ed in `main()` before `run()`) that does the actual
   `add_project`/`add_user` calls against the resolved `op-cloudaccount`
   peer for approved-but-unprovisioned Awards.
3. **Wire `get_usage_report(s)`/`get_storage_report(s)`** to forward to
   `op-cloudaccount` / return empty respectively.
4. **Hardening**: unit tests for the Award state store (add/get/update/
   merge semantics, approve/reject idempotency), `cargo fmt`/`cargo
   clippy` clean.

## 13. Testing strategy

- Award state persistence: create/update/remove, restart (reload from
  disk), assert state matches — same shape as `op-cloudaccount`'s
  round-trip test.
- `AwardDetails` merge semantics on `update_project`: `breakdown` merges
  key-by-key, `notes` appends, `membership_control` overwrites if present.
- `approve` idempotency: simulate a partially-provisioned Award
  (`provisioned_users` containing some but not all members) and confirm
  re-running `approve` only provisions the missing ones.
- Template resolution: `create_project` with a `template` not present in
  the configured `offerings` table fails clearly rather than silently
  dropping the Award or picking an arbitrary default.
