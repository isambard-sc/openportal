<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# An example site portal

A small, complete, working implementation of
[site-portal-api.md](../../../docs/specifications/site-portal-api.md) — the
contract a *site portal* fulfils when another portal creates awards on its
infrastructure.

It exists to be **read**. If you are connecting your site's portal to
OpenPortal, this is the shortest path to understanding what your side has to do:
every instruction is one function, and the awkward parts of the contract — the
ones that are easy to get wrong and hard to discover — are commented where they
bite rather than left in the specification.

## This is not a production site portal

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
| `site_portal.py` | **The contract.** One function per instruction, plus the dispatch that guarantees every job is answered. Read this first. |
| `store.py` | The portal's own state. The file you replace. |
| `app.py` | FastAPI: the two endpoints OpenPortal calls, plus a small operator API for approving awards, pushing usage figures and declaring a month's accounting final. |
| `test_site_portal.py` | Drives every handler with synthetic jobs — no bridge, no agents, no network. Proves the example works, and shows that the contract is testable in isolation. |

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
export PORTAL_AWARDING_PORTALS=allocator     # who may make awards here
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
python test_site_portal.py
```

## Walking through one award

With the application running and an awarding portal called `allocator`
configured, here is the whole life of an award. This is the part to read
carefully — most of the contract is visible in these seven steps.

### 1. The allocator asks for an award to be attached to a project

`allocator` addresses `allocator.site.cluster1`. That last element is a *virtual
agent* on this portal standing for one resource we run, so the request carries
which resource it is about.

What it is asking for is more subtle than it first looks. It is **not** "create
an award" — the award already exists; `allocator` decided it. It is *"connect
this award to a project on `cluster1`"*. Most often that will mean creating a new
project for it, but nothing requires that: you are free to attach the award to a
project that already exists, if that is what your site's records say should
happen.

Since this site wants a human to look at every award first, nothing is attached
yet, and `site_portal.create_award` answers with `ManagedProjectPendingError`.

**This is not a failure.** It is the honest answer to "is this award connected to
a project yet?" — not yet. `allocator` logs it quietly and **will repeat the
request periodically until it is approved or rejected**. That retry is what makes
the rest of this work, and it is why nothing below needs to push anything back.

### 2. The site operators see the pending awards, and decide

Their job is exactly two decisions: approve or reject.

```bash
curl localhost:8080/awards
```

**To approve, they must name the project the award is attached to** — either one
they have just created for it, or one that already exists:

```bash
curl -X POST localhost:8080/awards/cluster1/myaward1.allocator/approve \
     -H 'content-type: application/json' \
     -d '{"project": "myproject1", "reason": "approved by the panel"}'
```

Note they supply `myproject1`, **not** `myproject1.site`. The `.site` half is
added from the portal's own name, because it is the one part of the identifier an
operator can get wrong and cannot usefully vary — a project in somebody else's
namespace is not something this portal can claim. Sending the full identifier is
refused with a message saying so, rather than being quietly accepted.

The name must uniquely identify the project on this site, and must fit what a
`ProjectIdentifier` component allows:

| | |
|---|---|
| Characters | `A-Z`, `a-z`, `0-9`, `_`, `-` |
| Must not start with | `-` |
| Length | 1 to 64 characters |

So `myproject1`, `MyProject_1` and `my-project` are all fine; `my.project`,
`my project`, `café` and `-lead` are not.

The offering is in the path because an award is identified by *both* the resource
and the identifier — the same name arriving on `cluster2` would be a different
award, for a different resource.

**One project, one award — at a time.** A project can be attached to only one
award at any moment, so approving a second award onto `myproject1` is refused
with a `409`. The operator is free to *change* which project an award is attached
to, though: approving again with a different `project` moves it, and the project
it leaves behind becomes available to another award.

**To reject**, they say so, and the reason travels back:

```bash
curl -X POST localhost:8080/awards/cluster1/myaward1.allocator/reject \
     -H 'content-type: application/json' \
     -d '{"reason": "no capacity on cluster1 this quarter"}'
```

`allocator`'s next request then gets `ManagedProjectRejectedError`, which tells it
the award is refused. Unlike pending, that is terminal: `allocator` records the
award as errored and stops asking.

### 3. On approval, the two portals learn each other's names for the thing

Nothing is pushed back to `allocator`. It is already re-sending `create_award`,
so the next one simply succeeds, and what it returns is the mapping:

```
myaward1.allocator:myproject1.site
```

`allocator` supplied the identifier on the left; we chose the one on the right -
`myproject1`, qualified with our own portal name into `myproject1.site`.
Neither side could have guessed the other's, and now both hold the pair — so the
allocator and the site portal agree on the linkage and know what they are talking
about. From here the award and the project are two names for one thing.

That is also what makes approval need no notification path of its own: the
retrying request *is* the delivery mechanism.

### 4. Usage figures are pushed in — against *our* identifier

Your accounting is the source of truth, and it produces figures for
`myproject1.site`. It has never heard of `myaward1.allocator`:

```bash
curl -X PUT localhost:8080/projects/myproject1.site/usage \
     -H 'content-type: application/json' \
     -d '{"hours": {"2026-08-01": {"alice@bristol.ac.uk": 12.5}}}'
```

or, if your parser already produces OpenPortal types, push a complete
`ProjectUsageReport`:

```bash
curl -X PUT localhost:8080/projects/myproject1.site/usage \
     -H 'content-type: application/json' \
     -d '{"report": { ... ProjectUsageReport JSON ... }}'
```

Both end up in the same store, and `get_usage_report` serves either.

Note that everything under `/awards` is keyed on `allocator`'s identifier and this
endpoint on ours. That is not an inconsistency — those are the two namespaces,
and the mapping from step 3 is what joins them.

### 5. The allocator asks for usage, and gets it in its own namespace

It asks `allocator.site.cluster1 get_usage_report myaward1.allocator`. The
figures were recorded against `myproject1.site`, so `build_usage_report`
assembles the report in our namespace and then `remap_project`s it into theirs:
`alice.myproject1.site` becomes `alice.myaward1.allocator`, while the member's
email is untouched, because that is the same person either way.

It answers from what was pushed, with no computing on the request path — there
are only about thirty seconds to answer in.

### 6. How often you are asked, and how to make it stop

Usage is not asked for once. `get_usage_report` arrives on every sync cycle, and
almost always for **`this_month`** — the allocator is watching a month fill up,
not fetching a finished ledger. Your answer carries a flag that decides whether
it comes back for that month again.

Each day in a `ProjectUsageReport` is either complete or not, and the report as
a whole is complete when every day in it is. Complete means one specific thing:
*these figures will not change.* An allocator that receives a complete month
records it and stops asking; one that receives an incomplete month asks again
next cycle.

So which months you get asked about is, in part, up to you:

* **The current month is always re-requested,** whatever you say about it. The
  reference implementation will not even store it as complete — the month is
  still running, so the claim cannot be true yet.
* **A past month is re-requested until you report it complete.** Once you do,
  the allocator has what it needs and moves on.

The asking therefore stops when, and only when, you say the month is settled —
which is why this example makes that an explicit operations decision rather than
inferring it:

```bash
# "August's accounting is settled — stop asking."
curl -X POST localhost:8080/projects/myproject1.site/usage/finalise \
     -H 'content-type: application/json' \
     -d '{"month": "2026-08"}'
```

and, because a late correction can always land, it can be taken back:

```bash
curl -X POST localhost:8080/projects/myproject1.site/usage/finalise \
     -H 'content-type: application/json' \
     -d '{"month": "2026-08", "final": false}'
```

The alternative — guessing from the calendar, "the day has passed, so it must be
settled" — is what the example deliberately does *not* do. A scheduler outage, a
job record that lands late, a billing correction: any of them moves a number
after the month has ended. Only the team running the accounting knows when their
own pipeline has settled, so only they get to say so.

There is one trap here, and it is easy to walk into. A report containing **no
days at all** is complete, vacuously — "every day I contain is complete" is true
of nothing. So a month whose figures have simply not been ingested yet would
answer *"nothing was used, and that is final"*, and be believed.
`build_usage_report` guards it by writing an explicit zero-usage,
**not**-complete day for any month in the requested range that it has no data
for and has not been told is final. That says the honest thing instead: nothing
so far, ask again.

Note also which way the two mistakes run. Never finalising a month costs you one
request per sync cycle and nothing else — a small, permanent bill. Finalising
early is the expensive direction: the allocator records what it has and stops
asking, and a correction that arrives afterwards is never collected. When in
doubt, leave it open.

> **One caveat about today's Waldur.** It currently sweeps only the last two
> months and never looks further back, so at the moment a month that is never
> declared complete stops being asked about after about thirty days rather than
> being retried. That is a bug on the allocator side and is being fixed to retry
> every month from the award's start date until it is reported complete. Nothing
> in this example changes either way — which is the point of implementing
> against the rule rather than against the current behaviour.

### 7. The same question asked of the wrong resource returns nothing

Ask `allocator.site.cluster2 get_usage_report myaward1.allocator` and the answer
is an **empty** report, not an error. The award is attached to a project on
`cluster1`, so nothing was used on `cluster2`. An allocator sweeping every
offering it knows about, to find which one holds a given award, depends on that.

## The nine things worth taking away

Each of these is commented at the point it matters in the code, but they are the
reason the example exists.

1. **The offering says which resource, and scopes everything.** It is a virtual
   agent standing for one resource you run, and it is part of what is being
   asked rather than a permission to ask it. Awards are keyed on
   `(offering, project id)`; answers are scoped by it; and a question about a
   project that is not on this resource returns empty, not an error. Reading it
   as an access-control list is the mistake this example is arranged to prevent
   — which is why it offers two resources rather than one.

2. **An award is *attached* to a project, not the same thing as one.**
   `create_award` asks you to connect an award to a project on a resource.
   Usually you will create one for it; you are equally free to attach it to a
   project that already exists. A project holds at most one award at a time, and
   you may move an award to a different project whenever your records say so.

3. **The mapping is where two portals agree what to call a thing.** You name
   the project you attached — just its own name, qualified with your portal —
   and it is returned as the second half of the `ProjectMapping`. It is what the allocator joins on, and
   what your usage figures translate through. Until an award is attached you
   have nothing honest to put there — which is why an unattached award answers
   with an error instead.

4. **Failing is a normal answer, and *which* failure matters.**
   `ManagedProjectPendingError` means "not yet, ask again" and is benign;
   `ManagedProjectRejectedError` means "no" and is terminal. Confusing them
   either strands an award that only needed approving, or leaves the caller
   retrying forever against a decision that will never change.

5. **Everything is retried, so everything must be idempotent.** `create_award`
   arrives repeatedly for awards you already hold. `update_award` arrives for
   awards you have never seen. A duplicate job id must not do the work twice.

6. **Never leave a job unanswered.** `site_portal.answer()` is built so that a
   handler returning, a handler raising, and a handler crashing all produce a
   posted result. Silence becomes a two-minute timeout for whoever is waiting.

7. **You have thirty seconds, not two minutes.** The job expiry is two minutes
   but the caller gives up long before that. Serve reports from cache.

8. **Implement as much or as little as you want.** There is no minimum set.
   Decline what you do not implement with
   `OpenPortalUnsupportedCommandError` — clearly, so a caller can tell "I don't
   do that" from "I'm broken". This example does not implement `get_users`, and
   neither does Waldur.

9. **`is_complete` is a promise, and it is yours to make.** It tells the
   allocator a month's figures will not change, so it need not ask again.
   Nothing in the code can know when accounting has settled; your operations
   team can, so this example asks them rather than guessing from the calendar.
   Leaving a month open costs one request per cycle; closing it early loses
   every correction that arrives later. And note that an empty report is
   complete *vacuously* — the one way to make that promise by accident.

## Other languages

Nothing here is Python-specific except the convenience of the `openportal`
module. The contract is HTTP and JSON, and
[site-portal-api.md](../../../docs/specifications/site-portal-api.md)
specifies it in language-neutral terms — including the wire form of the error
classes, so a portal in another language can produce and read them. If you build
an equivalent example in another language, it belongs alongside this one.
