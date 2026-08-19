<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# An example project portal

A small, complete, working implementation of
[project-portal-api.md](../../../docs/specifications/project-portal-api.md) — the
contract a *project portal* fulfils when another portal creates awards on its
infrastructure.

It exists to be **read**. If you are connecting portal software to OpenPortal,
this is the shortest path to understanding what your side has to do: every
instruction is one function, and the awkward parts of the contract — the ones
that are easy to get wrong and hard to discover — are commented where they bite
rather than left in the specification.

## This is not a production portal

Not "not yet" — not ever. It is missing, deliberately, everything a real
deployment needs and nothing that would teach you about OpenPortal:

* **No authentication on the operator API.** Anyone who can reach `/awards` can
  approve any award. A real portal puts its own authentication and authorisation
  in front of that.
* **No real state storage.** Awards are JSON files in a directory, read fresh
  every time. No transactions, no migrations, no concurrent-write safety, no
  backups.
* **No durability across restarts** for the set of already-handled job ids, and
  no sharing of it between workers — so a restart at the wrong moment can run a
  job twice.
* **No user management, quotas, accounting, or notification handling** beyond
  logging that a notification arrived.
* **No TLS, rate limiting, audit trail, or operational tooling.**

Take the shape, not the code. `store.py` in particular is written to be thrown
away.

## The files

| File | What it is |
|---|---|
| `portal.py` | **The contract.** One function per instruction, plus the dispatch that guarantees every job is answered. Read this first. |
| `store.py` | The portal's own state. The file you replace. |
| `app.py` | FastAPI: the two endpoints OpenPortal calls, plus a small operator API for approving awards and pushing usage figures. |
| `test_portal.py` | Drives every handler with synthetic jobs — no bridge, no agents, no network. Proves the example works, and shows that the contract is testable in isolation. |

## Running it

You need a running `op-bridge` with `--signal-url` and `--notification-url`
pointing at this application.

```bash
# 1. Build and install the openportal module from this repository
cd ../../..            # the workspace root
make python            # or: maturin develop -m python/Cargo.toml

# 2. Install the example's own dependencies
cd python/examples/portal
pip install -r requirements.txt

# 3. Point it at your bridge config and run it
export OPENPORTAL_CONFIG=~/.config/openportal/bridge.toml
export PORTAL_STATE_DIR=./portal-state
export PORTAL_AWARDING_PORTALS=ukri     # who may make awards here
uvicorn app:app --port 8080
```

The bridge should then be initialised with matching URLs:

```bash
op-bridge init \
    --signal-url       http://localhost:8080/signal/job \
    --notification-url http://localhost:8080/signal/notification
```

The tests need neither the bridge nor the config:

```bash
python test_portal.py
```

## Walking through one award

With the application running and an awarding portal called `ukri` configured,
here is the whole life of an award.

**1. `ukri` creates it, on a particular resource.** It addresses
`ukri.aip1.isambard-ai`, and that last element is a *virtual agent* on this
portal standing for one resource we run. The request is not "create an award",
it is "create a project on Isambard-AI". The job reaches `/signal/job`,
`portal.create_award` records it against that offering, and — because nobody has
approved it — answers `ManagedProjectPendingError`. This is *not* a failure. The
awarding portal logs it quietly and will ask again.

**2. An operator approves it, and says what we call it here.**

```bash
curl localhost:8080/awards
curl -X POST localhost:8080/awards/isambard-ai/myproject.ukri/approve \
     -H 'content-type: application/json' \
     -d '{"local_project_id": "proj001.aip1", "reason": "approved by the panel"}'
```

The offering is in the path because an award is identified by *both* — the same
name on `isambard3` would be a different award, for a different resource.

`local_project_id` is required, and it is the point of the whole exchange.
`ukri` knows this award as `myproject.ukri`; we now know the project we made for
it as `proj001.aip1`. Neither side could guess the other's name, so approval is
where ours gets decided.

**3. It goes live by itself, and `ukri` learns our name for it.** Nothing is
pushed back. `ukri` is already re-sending `create_award` every cycle, so the
next one gets a `ProjectMapping` instead of an error:

```
myproject.ukri:proj001.aip1
```

Both sides now hold the pair, and from here the award ID and the project ID are
two names for one thing. This is the most useful consequence of the retry
contract — approval needs no notification path of its own.

**4. Usage figures are pushed in — against *our* identifier.** Your accounting
produces figures for `proj001.aip1` and has never heard of `myproject.ukri`, so
that is what it posts against:

```bash
curl -X PUT localhost:8080/projects/proj001.aip1/usage \
     -H 'content-type: application/json' \
     -d '{"hours": {"2026-08-01": {"alice@bristol.ac.uk": 12.5}}}'
```

or, if your parser already produces OpenPortal types, push a complete
`ProjectUsageReport`:

```bash
curl -X PUT localhost:8080/projects/proj001.aip1/usage \
     -H 'content-type: application/json' \
     -d '{"report": { ... ProjectUsageReport JSON ... }}'
```

Both end up in the same store, and `get_usage_report` serves either.

Note the endpoints under `/awards` are keyed on `ukri`'s identifier and this one
on ours. That is not an inconsistency — it is the two namespaces, and the
mapping is what joins them.

**5. `ukri` collects them, and gets an answer in its own namespace.** It asks
`ukri.aip1.isambard-ai get_usage_report myproject.ukri`. The figures were recorded against
`proj001.aip1`, so `build_usage_report` assembles the report in our namespace
and then `remap_project`s it into theirs — `alice.proj001.aip1` becomes
`alice.myproject.ukri`, and the member's email is untouched because it is the
same person either way. It answers from what was pushed, with no computing on
the request path, because there are only about thirty seconds to answer in.

**6. The same question asked of the wrong resource returns nothing.** Ask
`ukri.aip1.isambard3 get_usage_report myproject.ukri` and the answer is an
*empty* report, not an error — the project is not on Isambard 3, so nothing was
used there. An awarding portal sweeping every offering it knows about to find
which one holds an award relies on that.

## The seven things worth taking away

Each of these is commented at the point it matters in the code, but they are the
reason the example exists.

1. **The offering says which resource, and scopes everything.** It is a virtual
   agent standing for one resource you run, and it is part of what is being
   asked rather than a permission to ask it. Awards are keyed on
   `(offering, project id)`; answers are scoped by it; and a question about a
   project that is not on this resource returns empty, not an error. Reading it
   as an access-control list is the mistake this example is arranged to prevent
   — which is why it offers two resources rather than one.

2. **The mapping is where two portals agree what to call a thing.** You decide
   your own `ProjectIdentifier` for an award when you provision it, and return
   it as the second half of the `ProjectMapping`. It is what the awarding portal
   joins on, and what your usage figures translate through. Until it exists you
   have nothing honest to return — which is why an unapproved award answers with
   an error instead.

3. **Failing is a normal answer, and *which* failure matters.**
   `ManagedProjectPendingError` means "not yet, ask again" and is benign;
   `ManagedProjectRejectedError` means "no" and is terminal. Confusing them
   either strands an award that only needed approving, or leaves the caller
   retrying forever against a decision that will never change.

4. **Everything is retried, so everything must be idempotent.** `create_award`
   arrives repeatedly for awards you already hold. `update_award` arrives for
   awards you have never seen. A duplicate job id must not do the work twice.

5. **Never leave a job unanswered.** `portal.answer()` is built so that a
   handler returning, a handler raising, and a handler crashing all produce a
   posted result. Silence becomes a two-minute timeout for whoever is waiting.

6. **You have thirty seconds, not two minutes.** The job expiry is two minutes
   but the caller gives up long before that. Serve reports from cache.

7. **Implement as much or as little as you want.** There is no minimum set.
   Decline what you do not implement with
   `OpenPortalUnsupportedCommandError` — clearly, so a caller can tell "I don't
   do that" from "I'm broken". This example does not implement `get_users`, and
   neither does Waldur.

## Other languages

Nothing here is Python-specific except the convenience of the `openportal`
module. The contract is HTTP and JSON, and
[project-portal-api.md](../../../docs/specifications/project-portal-api.md)
specifies it in language-neutral terms — including the wire form of the error
classes, so a portal in another language can produce and read them. If you build
an equivalent example in another language, it belongs alongside this one.
