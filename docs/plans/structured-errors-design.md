<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# Structured errors: keeping the kind, not just the sentence

Status: **proposed**. Nothing below is implemented. The Python half of the
problem — errors arriving in Python as an untyped `OSError` — is fixed already
(see `python/src/errors.rs` and the changelog); this document is about the half
underneath it, which the fix works around rather than solves.

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
now owns both ends, so the two sides cannot drift — but the encoding is still
there, still ad hoc, and still the only reason the awarding portal can tell
"approve this" from "give up".

It is worse inside Rust, where there is no encoding at all.
`templemeads::Error` has fourteen variants and every one of them is
`#[error("{0}")]` — a wrapper around a string. Cross an agent boundary and the
variant is lost: `MissingAgent`, `InvalidInstruction` and `Delivery` all arrive
as the same anonymous sentence. Nothing downstream can branch on what went
wrong, only log it.

Three consequences worth naming:

* **Callers cannot make decisions.** The pending/rejected distinction is the
  clearest case — one means retry forever, the other means stop — and it
  survives today only because of a string prefix convention.
* **Errors cannot be classified in aggregate.** Diagnostics can count failures
  but cannot group them by kind, because there are no kinds.
* **The sentinels are fragile.** `ExpirationError{}` and `UnknownError{}` spent
  their whole life with doubled braces (fixed in this release) precisely because
  nothing typed was checking them — a compiler cannot spot a typo in a string.

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
set of transport kinds (`expired`, `unroutable`, `timeout`, `internal`), and
each `Domain` contributes its own — `greatwestern` supplying `award_pending`,
`award_rejected`, `unsupported_command`.

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

## 4. Scope

Touches `templemeads` (the `JobError` type, `Job::errored`, the `Error` enum's
variants gaining kinds), `greatwestern` (its own kinds), `portal` (the
`RuntimeError{…}` wrapping becomes a kind, not a format string), `bridge` and
`python` (prefer the structured field, keep the parser as fallback), plus the
wire-protocol and JSON type specifications.

It is a bigger change than it first looks, almost entirely because of §3, and it
is not urgent: the Python fix already gives portal authors the typed errors they
actually branch on. This is the tidier foundation underneath, worth doing when
the wire protocol is next opened up rather than on its own.

## 5. Smaller things worth doing first

Each of these is independently useful and none needs the above:

1. **Give `templemeads::Error`'s variants real payloads.** Even without wire
   changes, `#[error("{0}")]` on all fourteen makes in-process matching useless.
2. **Make the sentinels constants.** `ExpirationError{}` and `UnknownError{}`
   are written in one crate and parsed in another. A shared `const` removes a
   whole class of typo.
3. **Count failures by kind in diagnostics** once kinds exist — the reporting
   is already there, and only the grouping is missing.
