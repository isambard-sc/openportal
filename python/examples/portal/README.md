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

**1. `ukri` creates it.** The job reaches `/signal/job`, `portal.create_award`
records it, and — because nobody has approved it — answers
`ManagedProjectPendingError`. This is *not* a failure. The awarding portal logs
it quietly and will ask again.

**2. An operator looks at it.**

```bash
curl localhost:8080/awards
curl -X POST localhost:8080/awards/myproject.ukri/approve \
     -H 'content-type: application/json' -d '{"reason": "approved by the panel"}'
```

**3. It goes live by itself.** Nothing is pushed back to `ukri`. It is already
re-sending `create_award` every cycle, so the next one gets a `ProjectMapping`
instead of an error and the award is live. This is the single most useful
consequence of the retry contract: approval needs no notification path of its
own.

**4. Usage figures are pushed in.** Your accounting is the source of truth, so
your parsers push:

```bash
curl -X PUT localhost:8080/awards/myproject.ukri/usage \
     -H 'content-type: application/json' \
     -d '{"hours": {"2026-08-01": {"alice@bristol.ac.uk": 12.5}}}'
```

or, if your parser already produces OpenPortal types, push a complete
`ProjectUsageReport`:

```bash
curl -X PUT localhost:8080/awards/myproject.ukri/usage \
     -H 'content-type: application/json' \
     -d '{"report": { ... ProjectUsageReport JSON ... }}'
```

Both end up in the same store, and `get_usage_report` serves either.

**5. `ukri` collects them**, and `get_usage_report` answers from what was
pushed — no computing on the request path, because there are only about thirty
seconds to answer in.

## The five things worth taking away

Each of these is commented at the point it matters in the code, but they are the
reason the example exists.

1. **Failing is a normal answer, and *which* failure matters.**
   `ManagedProjectPendingError` means "not yet, ask again" and is benign;
   `ManagedProjectRejectedError` means "no" and is terminal. Confusing them
   either strands an award that only needed approving, or leaves the caller
   retrying forever against a decision that will never change.

2. **Everything is retried, so everything must be idempotent.** `create_award`
   arrives repeatedly for awards you already hold. `update_award` arrives for
   awards you have never seen. A duplicate job id must not do the work twice.

3. **Never leave a job unanswered.** `portal.answer()` is built so that a
   handler returning, a handler raising, and a handler crashing all produce a
   posted result. Silence becomes a two-minute timeout for whoever is waiting.

4. **You have thirty seconds, not two minutes.** The job expiry is two minutes
   but the caller gives up long before that. Serve reports from cache.

5. **Implement as much or as little as you want.** There is no minimum set.
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
