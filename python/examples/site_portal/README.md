<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# An example site portal

## Quick start

```bash
pip install -r requirements.txt
python example.py run
```

and follow the instructions that are printed there. Then come back here
to understand what this did in more detail.

## What is the setup?

OpenPortal is used to connect two portals: one that allocates awards
(the awards or allocator portal), and one that actually runs them via
projects (the site portal). This example is used to help you understand
how to implement a site portal.

## What is this?

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

This is deliberately **NOT** a production portal. It is a working example
to help you test and understand the contract. It is not a reference
implementation, and it is not a template for your own portal. As such,
it has a number of limitations that make it unsuitable for production use:

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

## The files

| File | What it is |
|---|---|
| `site_portal.py` | **The contract.** One function per instruction, plus the dispatch that guarantees every job is answered. Read this first. |
| `store.py` | The portal's own state: the resources this site offers, its awards, their attachment history, and per-project usage. This shows what state you will have to manage at your site. |
| `app.py` | FastAPI: the two endpoints OpenPortal calls, plus a small operator API for saying which resources the site offers, approving awards, pushing usage figures and declaring a month's accounting final. This shows how you could connect to OpenPortal via a FastAPI-based REST API, or what your own connection would need to handle if it wants to connect to the OpenPortal bridge directly |
| `test_site_portal.py` | Drives every handler with synthetic jobs — no bridge, no agents, no network. Proves the example works, and shows that the contract is testable in isolation. |
| `example.py` | Builds and runs a complete two-portal setup on your own machine — four agents, this application, and the wiring between them. Not part of the contract; it exists so you can *watch* the contract work. |

## Running the whole thing locally

There is a script that does the entire setup — both portals, both bridges, this
application, and the peering between them — into one git-ignored `data/`
directory:

```bash
# from this directory
pip install -r requirements.txt   # openportal, fastapi, uvicorn
python example.py start
```

The Python side needs no Rust toolchain — `openportal` is on PyPI and comes with
the requirements. The *agents* are the part that has to be built: `cargo build`
from the workspace root, or a release binary on your `PATH`. `example.py` looks
for `op-portal` and `op-bridge` on the `PATH`, then in `target/release` and
`target/debug`, and says which is missing if it cannot find them.

It prints what is running and, more to the point, the Python and `curl` calls
that walk an award through the steps below — adding a cluster, making the award,
approving it, pushing today's usage in and reading it back from the awards
portal's side. `python example.py stop` stops it again, and
`python example.py clean` also deletes `data/`.

What it builds is the smallest arrangement in which one portal can make an award
on another:

```
     your Python  ──►  allocator_bridge  ──►  allocator      (the awards portal)
                                                   │
                                                   ▼
     this app     ◄──  site_bridge       ◄──  site           (the site portal)
```

Both bridges get a config file for the `openportal` module, so you can drive
either end: `allocator_bridge` to *make* requests of the site, `site_bridge` to
see what this portal's own bridge holds. Everything binds to `127.0.0.1` on
ports in the 187xx range, and the agent configs are unencrypted — it is a
development toy, not a deployment.

Read the script if you would rather do it by hand: each step is one command,
and the comments say why it is there.

## Running it against your own bridge

You need a running `op-bridge` with `--signal-url` and `--notification-url`
pointing at this application.

```bash
# 1. Install the example's dependencies, `openportal` among them
pip install -r requirements.txt

# ...or, to run against an `openportal` built from this checkout instead:
#   cd ../../.. && make python      # or: maturin develop -m python/Cargo.toml

# 2. Point it at your bridge config and run it
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
carefully — most of the contract is visible in these steps.

### 0. The site says which resources it offers

Nothing can happen until your site advertises a resource. A fresh portal
advertises none, so the operators add one:

```bash
curl -X POST localhost:8080/offerings \
     -H 'content-type: application/json' \
     -d '{"name": "cluster1", "templates": ["standard", "large"]}'
```

This registers a cluster called `cluster1`, and names two templates that
the allocator may ask for, "standard" and "large".

Running get on the same URL shows what is offered:

```bash
curl localhost:8080/offerings
```

returns

```
{"portal":"site","awarding_portals":["allocator"],"offerings":[{"name":"cluster1","templates":["large","standard"],"since":"2026-08-27","awards":0,"destinations":["cluster1.site.allocator"],"registered":true}]}
```

This shows that our site portal is called `site`, and the allocator portal is
called `allocator` (these were both configured in config files
— the example script sets them up for you).

The `offerings` list shows that `cluster1` is offered, with the two templates,
`large` and `standard`. It also shows whether the offering is
currently registered with OpenPortal - note that the offering is
called `cluster1.site.allocator` - meaning that `cluster1` on the `site`
portal is offered to the `allocator` portal.

You can remove an offering via `DELETE /offerings/cluster1`.

The templates refer to the types of awards that you are willing to accept.
For example, you may accept "large" or "standard" awards in this case.
The template provides a way for you and the allocator to agree a shared
name to represent different types of awards. For example, a "standard" award
may have lower priority in queues, or less guaranteed resources than a "large"
award. It is entirely up to you and the allocator to decide what these
templates mean.

Note that the allocator has to say which template an award is against. If
it asks for a template you do not offer, the request is silently dropped.
This is a security measure: the allocator cannot cheaply
probe your site to see what templates you offer.

This gives rise to the least helpful failure in
the whole system: **a request for a resource that is not advertised is held, not
refused** (§1.1). It sits on the portal agent waiting for the offering to appear,
the caller waits out its timeout, and nothing anywhere says why. So if a
`create_award` never comes back, check `GET /offerings` first.

Withdrawing a resource ends its *reachability*, and nothing else. The awards made
on it stay on record — they still own the days they were attached for, and those
days may not have been collected yet (see step 8) — so `DELETE` reports how many
it kept, and adding the resource back makes them reachable again.

### 1. The allocator asks for an award to be attached to a project

`allocator` addresses `allocator.site.cluster1` and sends a request to
link an award to a project. This request looks like this:

```
allocator.site.cluster1 create_award myaward1.allocator
  {"name":"My First Award","template":"standard","members":{"alice@example.com":"Project Lead"}}
```

Here, the allocator is addressing your `cluster1` offering on your `site` portal.
It is asking to create an award (via the `create_award` instruction)
where it is telling you that it refers to the award as `myaward1.allocator`.

It is also providing some metadata about the award, including its name,
the template it is against, and a list of members and their roles.

The members are keyed by their email addresses, and the roles are strings
that have been pre-agreed between you and the allocator. Typically they will
be things like "Project Lead", "Project Co-Lead" or "Project Member".
They provide a way for you and the allocator to agree on how the project
team should be represented, what their responsibilities are, and what
permissions they should have on the project.

This request is more subtle than it first looks. It is **not** "create
an award" — the award already exists; `allocator` decided it. It is *"connect
this award to a project on your site's `cluster1`"*. Most often that will mean
creating a new project for it, but nothing requires that: you are free to
attach the award to a project that already exists, if that is what your
site's records say should happen.

All create award requests should be reviewed by a human operator. So your
site portal should store these requests somewhere for you to review. In
the meantime, it sends back a `ManagedProjectPendingError` to the allocator.
This tells the allocator that you have received the request and are
reviewing it. The allocator will keep periodically retrying the request
until you approve or reject it.

### 2. You, as the site operator see the pending awards, and decide

Your job is to provide a human-in-the-loop review of whether or not to
approve the award request. This isn't to judge the allocator's decision,
but instead provides a cybersecurity check to ensure that new awards are
not created without your knowledge. You can see the pending awards by running:

```bash
curl localhost:8080/awards
```

**To approve, you must provide a short, unique identifier
for the project the award should be attached to** — either one
you have just created for it, or one that already exists:

```bash
curl -X POST localhost:8080/awards/cluster1/myaward1.allocator/approve \
     -H 'content-type: application/json' \
     -d '{"project": "myproject1", "reason": "approved by the panel"}'
```

This must uniquely identify the project on this site, and must fit this
requirement:

| | |
|---|---|
| Characters | `A-Z`, `a-z`, `0-9`, `_`, `-` |
| Must not start with | `-` |
| Length | 1 to 64 characters |

So `myproject1`, `MyProject_1` and `my-project` are all fine; `my.project`,
`my project`, `café` and `-lead` are not.

Typically, sites will automatically generate their identifiers. But manually
created identifiers are ok too.

In this case, the identifier is `myproject1`. In OpenPortal this is combined
with the site portal's name (in this case `site`) to form a unique full
identifier for the project: `myproject1.site`.

Note how the allocator had asked for the `myaward1.allocator` award to be
attached to this project. This is because their unique award identifier was
`myaward1` and thier portal name was `allocator`. In accepting the award,
we've now created a linkage (or mapping) from the allocator's
`myaward1.allocator` award to our `myproject1.site` project. This is why
you will see `myaward1.allocator:myproject1.site` in the logs and
in the usage reports. The allocator will also receive this mapping
the next time they try to create the award. They will then know that the
award is live and connected to the project.

Note that you called a URL with the offering is in the path because an
award is requested against a specific offering, in this case the request
was to link `myaward1.allocator` to a project on the `cluster1` offering.

You can freely change which projects are linked to which awards, as long as
you keep a record of the changes. You should also follow the rule that
a project can only be linked to one award at a time. And any accounting
data you send back to the allocator from a project will resolve only to the
award it was linked against. Typically this is by assigning one award per
day, and allocating the entire usage within a day to the last award linked
on that day. However, it is up to you how you choose this division,
as long as you keep a record of the changes and can report them back to the
allocator if requested.

You don't have to approve the award. If something looks wrong, then you
can reject it. **To reject**, give a reason so the allocator knows why it
was rejected:

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
`myproject1.site`. You push in accounting data against your own
project identifier, e.g. `myproject1.site`. For example, assuming
we get TODAY via

```bash
TODAY=$(date +%F)
```

then we could push in today's usage by `alice@example.com` of 12.5 hours like this:

```bash
curl -X PUT localhost:8080/projects/myproject1.site/usage \
     -H 'content-type: application/json' \
     -d "{\"hours\": {\"$TODAY\": {\"alice@example.com\": 12.5}}}"
```

Or, if your parser already produces OpenPortal types, push a complete
`ProjectUsageReport`:

```bash
curl -X PUT localhost:8080/projects/myproject1.site/usage \
     -H 'content-type: application/json' \
     -d "{\"report\": { ... ProjectUsageReport JSON ... }}"
```

(you can see the specification of `ProjectUsageReport` in
[json-types.md](../../../docs/specifications/json-types.md#projectusagereport)).

### 5. The allocator asks for usage, and gets it in its own namespace

The allocator will periodically ask for usage reports for awards that are
active and linked at your site. It will send request like this:

```
allocator.site.cluster1 get_usage_report myaward1.allocator this_month
```

This is asking for the most recent full month's accounting for all used
to be billed against the award `myaward1.allocator`. Your job is to provide
the usage figures for the project that was linked to that award, in this case
`myproject1.site`.

The usage reports you were pushing in against your own project identifier
in this example are now filtered to the days that were attached to the award.
Identifiers from your project to the allocators award are remapped, and the
result is returned to the allocator. This is a very quick operation,
because the usage is stored against your own project, and the award is derived
from the attachment history. You have about 30 seconds to provide the report,
or else it will time out and the allocator will try again. If computing the
report takes longer, then use a background reporter to compute it, and serve
it from cache when it is ready. All OpenPortal calls are designed to be
idempotent and re-tryable. The allocator portal will keep re-trying the
request until it gets a valid response.

### 6. Finalised versus unfinalised reports

Note that accounts do change and may need to be revised. OpenPortal has the
concept of a "finalised" report, which is a report that will not change. The
allocator will keep asking for usage reports until it receives a finalised report
for a given month. It is up to you to decide when a report is finalised,
and to tell the allocator when that is the case. You can do this by calling
the `finalise` endpoint for a given month. For example, to finalise the
report for August 2026, you would call:

```bash
curl -X POST localhost:8080/projects/myproject1.site/usage/finalise \
     -H 'content-type: application/json' \
     -d '{"month": "2026-08"}'
```

Note that you can't finalise the current month - this is obviously because
things can still change.

* **The current month is always re-requested,** whatever you say about it. The
  reference implementation will not even store it as complete — the month is
  still running, so the claim cannot be true yet.
* **A past month is re-requested until you report it complete.** Once you do,
  the allocator has what it needs and moves on.

Note, that once finalised the allocator will stop asking for a usage
report. If you need to make a later correction then you need to let the
allocator know. They can remove the "finalised" flag and can re-request
the accounting from you once it is ready. Your accounts are always treated
as the source of truth, and the allocator will always accept your usage
figures as the final word.

### 7. The same question asked of the wrong resource returns nothing

Ask `allocator.site.cluster2 get_usage_report myaward1.allocator` and the answer
is an **empty** report, not an error. The award is attached to a project on
`cluster1`, so nothing was used on `cluster2`.

### 8. The award is updated

The allocator may send an update to the award, for example to change its name,
or to add or remove members. This is done via the `update_award` instruction:

```
allocator.site.cluster1 update_award myaward1.allocator
   {"name":"My First Award","members":{"alice@example.com":"Project Lead","bob@example.com":"Project Member"}}
```

These updates can be approved automatically, and should apply to whichever project
you have attached the award to. The update does not change the attachment, and
does not change the usage figures. It is simply a way for the allocator to keep
the award's metadata in sync with the project it is attached to.

Note that some allocators may require that the members of the award are kept
in sync with the members of the project. In this case, you should ensure that
the members of the project are updated to match the members of the award. This
is not a requirement of the OpenPortal contract, but it is a common practice
to ensure that the project and award are kept in sync.

Because of this, the `update_award` instruction will always send the full set
of metadata associated with an award. You can infer that, if someone is
missing from the members list, that they have been removed.
If someone is added, then they have been added.

### 9. The award is removed — and the project carries on

Eventually `allocator` sends a request to detach an award from a project. This
is the command

```
allocator.site.cluster1 remove_award myaward1.allocator
```

It is the mirror image of step 1: step 1 asked us to
*attach* an award to a project, and this asks us to *detach* it. It says nothing
about the project.

So `myproject1.site` carries on exactly as before — its accounts, its files, its
members, its identity. Whether a project outlives its award is a question for
the site, answered through the site's own processes.

What detachment ends is `myproject1.site`'s ability to bill usage against
`myaward1.allocator`, and it ends it *per day*:

```
      Aug 10                Aug 20                     Aug 21
      myaward1 attached     myaward1 removed,          nothing attached
                            myaward2 attached
      ├─────────────────────┤                          │
      Aug 10-19  → myaward1 │ Aug 20 → myaward2, all of it
                            └──────────────────────────┤
                                                       Aug 21 → nobody
```

A good rule to follow is *the award the project was last attached to on that day*.
Because the handover happened during 20 August, **the whole of that day** goes to
`myaward2` — not just the hours after the swap. A day is indivisible; usage is
accounted daily and splitting one would need per-hour attribution nobody keeps.
And had nothing replaced `myaward1`, 20 August would still have been its own,
with 21 August the first day billed to nobody. Removal therefore bites *at most*
the day after it happens.

You don't have to do this, but it does make things cleaner. What you do
have to ensure is that no usage *after* the removal is billed to the award
that was removed.

In the case of this example, two things follow, and the example is arranged around them.

**Removal keeps the record.** `remove_award` marks the award detached; it does
not delete it, and it does not touch the usage. The award still owns every day up
to and including 20 August, and the allocator has very likely not collected those
yet — the last days of an award are the least likely to have been collected. The
tempting shortcut is to delete the row, and it has a failure mode worse than an
error: `get_usage_report` would then return an *empty* report, an empty report is
vacuously complete (step 6), and we would be telling `allocator` that nothing was
ever used and that this is final. The final days of every award would vanish
quietly. So the operator API keeps working after removal too — the last figures
can still be pushed, and the month can still be declared final.

**Usage is stored against our project, not against the award.** This is why
`store.py` keeps usage in `projects/` rather than on the award record. Which
award owns a day is *derived* when a report is built, by asking the attachment
history. File a day's usage under "the award attached right now" as it arrives
and re-attribution becomes impossible — the record of what happened has been
overwritten by an answer that was only provisional.

There is a nice consequence for step 6. A day whose attachment changed has to be
re-reported to *both* awards: `myaward1` needs to see 20 August leave, and
`myaward2` needs to see it arrive. Neither is settled the moment the day begins,
which is a second and quite separate reason completeness cannot be inferred from
a calendar.

Finally, if `allocator` still holds the award it will keep sending
`create_award` for it every cycle. The example puts it back in the pending queue
rather than silently re-attaching it: attaching is an operator's decision, and it
may well be a *different* project this time. Its earlier attachment periods are
kept, because those days are still its days.

## The ten things worth taking away

Each of these is commented at the point it matters in the code, but they are the
reason the example exists.

1. **The offering says which resource, and scopes everything.** In
   OpenPortal terms, it is created as a virtual agent standing for one
   resource you run, and it is part of what is being
   asked rather than a permission to ask it. Awards are keyed on
   `(offering, project id)`; answers are scoped by it; and a question about a
   project that is not on this resource returns empty, not an error. Reading it
   as an access-control list is the mistake this example is arranged to prevent
   — which is why it is worth adding two resources rather than one and asking
   each of them about the other's awards. And the set of them is *state*, not a
   constant: `POST /offerings` adds one, `DELETE` withdraws one, and OpenPortal
   is told the complete new set each time.

2. **An award is *attached* to a project, not the same thing as one.**
   `create_award` asks you to connect an award to a project on a resource.
   Usually you will create one for it; you are equally free to attach it to a
   project that already exists. A project holds at most one award at a time, and
   you may move an award to a different project whenever your records say so.

3. **The mapping is where two portals agree what to call a thing.** You name
   the project you attached — just its own name, qualified with your portal —
   and it is returned as the second half of the `ProjectMapping`. It is what
   the allocator joins on, and what your usage figures translate through.
   Until an award is attached you have nothing honest to put there —
   which is why an unattached award answers with an error instead.

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

10. **`remove_award` detaches an award; it never deletes a project.** And it
    does not end the award's history: it still owns every day up to and
    including its last attached day, so keep the record and keep those days
    reportable. Billing is per-day and the day belongs to whichever award was
    attached last during it, so store usage against your own project and work
    out the owner when you build the report — not the other way round.

## Other languages

Nothing here is Python-specific except the convenience of the `openportal`
module. The contract is HTTP and JSON, and
[site-portal-api.md](../../../docs/specifications/site-portal-api.md)
specifies it in language-neutral terms — including the wire form of the error
classes, so a portal in another language can produce and read them. If you build
an equivalent example in another language, it belongs alongside this one.
