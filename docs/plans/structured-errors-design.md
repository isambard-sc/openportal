<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# Structured errors: keeping the kind, not just the sentence

Status: **implemented**, as described below. `templemeads::joberror::JobError`
carries a job's failure, `greatwestern::errorkind` contributes this domain's
kinds, `Register` negotiates the capability, and the Python bindings prefer the
structured field and fall back to parsing prose only for an older peer.

The compatibility design in §3 is the part that matters and it held: the prose
in `result` is byte-for-byte what it always was, so nothing deployed has to
change, and a job from a peer that predates the field still yields a usable
kind. §5's smaller items are still open.

## 1. The problem

An error in OpenPortal is a `String`. `Job::errored()` takes one, the wire
carries one, and every structured thing an agent knew about the failure is gone
by the time anyone can act on it.

Watch a single failure travel. A portal decides an award needs human approval:

```
1. Portal (Python)   raise ManagedProjectPendingError("awaiting approval")
2. board handler     job.errored(exc)
                       -> "ManagedProjectPendingError: awaiting approval"
3. op-portal         format!("RuntimeError{{{}}}", message)
                       -> "RuntimeError{ManagedProjectPendingError: awaiting approval}"
4. Awarding portal   parse the string back into a class
```

Steps 2 and 4 are a private encoding smuggling a type through a field that
cannot hold one, and step 3 wraps it in a second one. `python/src/errors.rs`
owns both ends, so the two sides cannot drift — but the encoding was still
there, still ad hoc, and still the only reason the awarding portal could tell
"approve this" from "give up".

It is worse inside Rust, where there is no encoding at all.
`templemeads::Error` has fourteen variants and every one of them is
`#[error("{0}")]` — a wrapper around a string. Cross an agent boundary and the
variant is lost: `MissingAgent`, `InvalidInstruction` and `Delivery` all arrive
as the same anonymous sentence. Nothing downstream can branch on what went
wrong, only log it. (That part is still true — see §5.1.)

Three consequences worth naming:

* **Callers cannot make decisions.** The pending/rejected distinction is the
  clearest case — one means retry forever, the other means stop — and it
  survives today only because of a string prefix convention.
* **Errors cannot be classified in aggregate.** Diagnostics can count failures
  but cannot group them by kind, because there are no kinds.
* **The sentinels are fragile.** `ExpirationError{}` and `UnknownError{}` spent
  their whole life with doubled braces (fixed in this release) precisely because
  nothing typed was checking them — a compiler cannot spot a typo in a string.
  They are now `const`s in `joberror`, written and read from one place.

## 2. What a fix looks like

Give a job's failure a shape instead of a sentence:

```rust
pub struct JobError {
    /// A stable, machine-readable discriminant, e.g. "award_pending".
    kind: String,
    /// The human-readable detail. What the error string holds today.
    message: String,
    /// Where the failure originated, for diagnostics.
    origin: Option<String>,
}
```

`kind` is a string rather than an enum on purpose. templemeads is deliberately
domain-agnostic (see `docs/plans/archive/grammar-split-design.md`): it cannot
own a vocabulary of award decisions, because a different `Domain` will have
entirely different failures. So templemeads defines the *envelope* and a small
set of transport kinds — as built, `expired`, `unroutable`, `unsupported`,
`invalid`, `run` and `unknown` — and each `Domain` contributes its own, with
`greatwestern` supplying `award_pending`, `award_rejected` and
`award_permission`.

That keeps the split the workspace already has: the router carries the failure
without understanding it, and the domain at each end knows what it means.

## 3. Compatibility, which is most of the work

Every deployed agent, and `waldur-mastermind`, reads `result` as a string when
`state` is `Error`. The change cannot break them.

* **Serialise both.** Keep `result` as the human-readable string — for a domain
  error, exactly the `"<ClassName>: <message>"` it is today — and add the
  structured form in a new optional field. Old peers read what they always read;
  new peers prefer the structured field when it is present.
* **Decode by falling back.** A structured field that is absent means the peer
  is older: parse the string as now. `python/src/errors.rs`'s `decode()` becomes
  the fallback path rather than the only path, which is why it is worth having
  written it as a tested function rather than inline.
* **Retire nothing early.** The string form stays indefinitely. It is what a
  human reads in a log, and it costs a few dozen bytes.

Version negotiation already exists for exactly this kind of change — agents
exchange domain name and version on `Register` (`5da7ecc`) — so a peer's ability
to read the structured field is knowable rather than guessable.

## 4. What was built

| Piece | Where |
|-------|-------|
| `JobError`, the transport kinds, and inference from prose | `templemeads/src/joberror.rs` |
| `Job::errored_with`, `Job::error`, `Job::error_or_infer`, `Job::redact_error_origin` | `templemeads/src/job.rs` |
| `Domain::error_kind_for`, the hook a domain classifies through | `templemeads/src/domain.rs` |
| This domain's kinds (`award_pending`, `award_rejected`, …) | `greatwestern/src/errorkind.rs` |
| `Register`'s `supports_structured_errors`, and the per-peer record | `templemeads/src/command.rs`, `agent.rs`, `handler.rs` |
| Kind propagated through the portal instead of flattened | `portal/src/main.rs` |
| `origin` stripped from everything served to a portal | `templemeads/src/bridge_server.rs` (`outbound`) |
| Python prefers the kind, falls back to prose | `python/src/errors.rs`, `python/src/lib.rs` |

Two decisions worth recording:

**`Job::errored(message)` was kept, and infers a kind.** Rewriting every call
site was neither necessary nor desirable — the transport's own sentinels and the
domain's class names are recognisable, so existing callers acquired a kind
without being touched. `errored_with` is the honest path for new code, because
inference is a reading of prose and an explicit kind is not.

**`origin` does not leave the agent network.** It names the agent a failure
happened at, which is useful to an operator and is internal topology to anyone
else. `bridge_server::outbound` is the single funnel every job served to a
portal passes through, so a new endpoint cannot quietly skip the redaction. If
you later decide a portal should see it, deleting one line in `outbound` is the
whole change.

## 5. Still open

1. **`templemeads::Error`'s variants have no payloads.** All fourteen are
   `#[error("{0}")]`, so matching on one in-process tells you nothing the string
   did not. Unrelated to the wire, and the larger remaining piece.
2. **Count failures by kind in diagnostics.** The reporting is already there and
   only the grouping is missing, now that kinds exist.
3. **Let agents other than the portal set kinds explicitly.** Every agent still
   calls `errored()` and relies on inference; the ones that know exactly why
   they failed should say so with `errored_with`.

The sentinels are now `templemeads::joberror::EXPIRATION_ERROR` and
`UNKNOWN_ERROR` rather than string literals in two crates, which was item 2 of
this list.
