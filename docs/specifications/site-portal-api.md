<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# Site Portal API Specification

**Status:** Normative for the `greatwestern` domain
**Audience:** Anyone connecting their site's portal to OpenPortal — the software
that runs a site's resources and owns its projects and their members
(e.g. Waldur).

---

## 0. What this document is

The `op-bridge` agent gives portal software an HTTP API. Most of that API is
about the portal *asking* OpenPortal to do something. This document specifies
the other direction: the requests OpenPortal sends **to** the portal, which the
portal must answer.

Those requests are how another portal — an *awarding portal* running its own
OpenPortal agents — creates and queries awards on your infrastructure. Between
them, the instructions in §4 let an awarding portal create an award on your
portal, ask what happened to it, and collect usage and storage reports, without
either side sharing a database or a credential. You implement as many of them as
you have answers for (§4.0).

This is deliberately a separate document from
[bridge-api.md](bridge-api.md). That one specifies the HTTP transport — how
requests are signed, what the endpoints are, how the bridge board works. This
one specifies the *contract*: which requests arrive, what each answer must
contain, and what the deadlines are. Read bridge-api.md first for the
mechanics; use this one when writing the handler.

| You want to know | Read |
|------------------|------|
| How to sign a request, what the endpoints are | [bridge-api.md](bridge-api.md) |
| What to return for `get_award`, and by when | this document |
| The exact JSON of `AwardDetails`, `UsageReport`, … | [json-types.md](json-types.md) |
| The instruction string grammar | [instruction-protocol.md](instruction-protocol.md) |
| Which error to raise, and what the caller does with it | this document, §3.3 |
| Doing it in Python | [python-api.md](python-api.md) |
| A worked implementation to read | [`python/examples/site_portal/`](../../python/examples/site_portal/) |
| A portal that already does all this in production | `waldur-mastermind`, `src/waldur_openportal/` (§7) |

---

## 1. The shape of a portal-to-portal exchange

Two portals, each with their own OpenPortal agents. Throughout this document
they are named after what they do:

* **`allocator`** — the *awarding portal*. Allocates awards, and wants them
  provisioned somewhere else; hence the name.
* **`site`** — the *site portal* (yours). Runs the resources, owns the projects
  and their members, and provisions what `allocator` asks for.

`site` advertises one or more **offerings**: named resources that `allocator` may
address. An offering is registered as `<resource>.<site>.<allocator>` —
`cluster1.site.allocator` reads "the resource `cluster1`, offered by `site`, to
`allocator`".

**Note the two forms are reversed.** You *register* `cluster1.site.allocator`,
and `allocator` *addresses* `allocator.site.cluster1` — a destination always
starts with the sender and ends with what is being addressed, while a
registration starts with the thing being offered. The middle element is your own
portal either way.

A request then flows:

```
 1.  allocator's portal software submits, through its own bridge:
         allocator.site.cluster1 create_award myaward1.allocator {…AwardDetails…}

 2.  allocator's own agent  ─────────►  site's agent
         (the destination's first hop is the sending portal itself;
          the last hop names the offering)

 3.  the site's agent recognises the offering and re-issues it
     to its own bridge, tagged with where it came from:
         site.<bridge>.cluster1 create_project myaward1.allocator {…}
         forwarded_for = allocator.site.cluster1

 4.  its bridge puts the Job on its board and signals your portal:
         GET <signal_url>?job_id=<uuid>

 5.  YOUR PORTAL: fetch the job, do the work, post the result.        ← §3

 6.  The result travels back the way it came, to allocator.
```

Everything above step 5 is handled by the agents. Steps 0 and 5 are yours.

### 1.1 Step 0: you must advertise offerings first

Until an offering exists, requests from `allocator` have nowhere to land — they are
held and only delivered once the offering is registered. On startup (and
whenever the set changes) call `POST /sync_offerings` with the complete list:

```json
["cluster1.site.allocator", "cluster2.site.allocator"]
```

`sync_offerings` is a *replace*, not a merge — anything absent is withdrawn.
Use `add_offerings` / `remove_offerings` for incremental changes, and
`get_offerings` to read the current set.

The middle element must be your own site's agent name, or the offering is
rejected.

### 1.2 Telling portal-to-portal requests apart

Two things distinguish an awarding portal's request from a local one:

**`forwarded_for`** carries the original destination, e.g.
`allocator.site.cluster1`. Its **first** element is the portal that asked; its
**last** element is the offering they came in through. This is the field to
authorise against — it is set by your own portal agent, not by the caller.

**Identifiers in a request name the awarding portal, not you.** An award made
by `allocator` arrives as `myaward1.allocator` — never rewritten into your namespace —
and its members as `alice.myaward1.allocator`. You will have your own identifier for
the project you create for it (§4.1.1), but that is yours to return, not
something to expect in an incoming request. Key your records on the full
identifier as sent: the same project name may exist under two different awarding
portals, and those are different projects.

A locally-originated request has `forwarded_for` absent or naming only your own
portal.

### 1.3 The offering says *which resource*, and scopes everything

An offering is a **virtual agent** on your portal: a name the awarding portal
addresses directly, standing for one resource you run. `site` might offer
`cluster1` and `cluster2`, and `allocator` addresses them as
`allocator.site.cluster1` and `allocator.site.cluster2`.

It is easy to read the offering as an access-control list — a set of names to
check a request against — and that reading will produce a portal that answers
the wrong questions. **The offering is part of what is being asked, not a
permission to ask it.**

`create_award` sent to `allocator.site.cluster1` is a request to create a project
*on `cluster1`*. The `template` in the `AwardDetails` is interpreted in the
context of that resource — in Waldur it selects the organisation, the default
offerings and the billing the project is created with, all of which belong to
the resource — so the same template name may be offered on one and not another.
When you provision, the project you create is tied to that resource, and the
identifier you return in the mapping (§4.1.1) names a project on it.

Three consequences:

* **Key your records on `(offering, project_id)`.** `myaward1.allocator` on
  `cluster1` and `myaward1.allocator` on `cluster2` are two different awards
  for two different resources. The reference implementation keys its own records
  exactly this way.
* **Scope every answer by the offering the request came through.** `get_awards`
  through `cluster1` means "what does `allocator` have *on `cluster1`*".
* **A question about a project that is not on this resource is not an error.**
  An awarding portal sweeping every offering it knows about will ask each one
  about each award. The offering that holds nothing answers with an **empty
  report**, not a failure — see §4.3.

Read the offering from `forwarded_for`'s last element, falling back to the
job's own destination (`site.<bridge>.cluster1`) which ends the same way for
a locally-originated request.

Worked through, with `allocator` holding two awards on `site`:

| `allocator` sends | on | `site` creates | and returns |
|---|---|---|---|
| `create_award myaward1.allocator` to `allocator.site.cluster1` | `cluster1` | `myproject1.site` | `myaward1.allocator:myproject1.site` |
| `create_award myaward2.allocator` to `allocator.site.cluster2` | `cluster2` | `myproject2.site` | `myaward2.allocator:myproject2.site` |

`get_usage_report myaward1.allocator` through `cluster1` answers with the usage of
`myproject1.site`, translated into `allocator`'s namespace. The same request
through `cluster2` answers with an **empty** report: `myaward1.allocator` is not on
`cluster2`. Nothing is wrong — it is simply not there.

---

## 2. The `*_award` vocabulary

Awarding portals speak of awards; the wire vocabulary grew up around projects.
Both spellings are accepted, and each `*_award` form is an exact synonym of the
corresponding `*_project` form — same instruction, same arguments, same result:

| Award spelling | Canonical spelling |
|----------------|--------------------|
| `create_award` | `create_project` |
| `update_award` | `update_project` |
| `remove_award` | `remove_project` |
| `get_award` | *(no project equivalent — returns `AwardDetails`)* |
| `get_awards` / `list_awards` | *(no project equivalent)* |

You may receive **either** spelling: an agent that sends `create_award`
produces the same `create_project` instruction internally, and that canonical
form is what appears in the `command` field of the Job you fetch. Dispatch on
the canonical name and you will handle both.

---

## 3. The request/response cycle

### 3.1 Receiving

The bridge calls `GET <signal_url>?job_id=<uuid>` as each job arrives. On that
signal, fetch the job with `POST /fetch_job` (body: the bare JSON UUID string),
or fetch everything outstanding with `GET /fetch_jobs`.

Configure `signal_url` at `op-bridge init --signal-url …`. If it is unset the
bridge logs a warning and the job simply waits to be polled — workable for
development, but see the deadline in §3.4.

The signal endpoint carries no credential of its own. The job id is what
authorises the call: it is a random UUID known only to the bridge and to you, so
a signal naming an id the bridge does not have is not a request you should act
on. The reference implementation answers an unknown id with `403 Forbidden` and
treats the id as the shared secret it is — do the same rather than leaving the
endpoint open to anything that can reach it.

Signals repeat. The same job id can arrive more than once (a retried signal
racing a `/fetch_jobs` sweep is the common case), so key your own record of the
job on its id and drop the duplicate rather than running the work twice — see
§3.5.

A fetched job looks like:

```json
{
  "id":            "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "created":       1700000000,
  "changed":       1700000005,
  "expires":       1700000120,
  "version":       2,
  "command":       "site.bridge.cluster1 get_award myaward1.allocator",
  "state":         "Pending",
  "result":        null,
  "result_type":   null,
  "forwarded_for": "allocator.site.cluster1"
}
```

`state` is capitalised on the wire — `Created`, `Pending`, `Running`,
`Complete`, `Error`, `Duplicate`. Python lower-cases it for display, so
`str(job.state)` is `"pending"` while the JSON says `"Pending"`; compare with
`job.state == "pending"` and let the binding handle it.

`command` is the destination path followed by the instruction. Parse the
instruction rather than the whole string: in Python, `job.instruction.command`
gives the verb (`get_award`) and `job.instruction.arguments` the argument list
(`["myaward1.allocator"]`).

### 3.2 Responding

Post the whole job back, with `state` set to `complete` and the result filled
in, to `POST /send_result`. The bridge matches it to the board by `id`, so the
`id` must be unchanged and the `version` must be higher than the one you
fetched.

In Python, `job.completed(value)` does all of that — it sets the state, bumps
the version, serialises the value and stamps `result_type` from the value's
type. Pass a real `openportal` object (`ProjectMapping`, `AwardDetails`,
`ProjectUsageReport`, …), not a dict: the type is what selects the
`result_type`, and the awarding portal deserialises against it.

Note the double encoding: `result` is a JSON **string containing JSON**. A
`ProjectMapping` result appears on the wire as:

```json
"state": "Complete",
"result": "\"myaward1.allocator:myproject1.site\"",
"result_type": "ProjectMapping"
```

A failure is not double-encoded — the message is carried as-is, and
`result_type` names the failure rather than a type:

```json
"state": "Error",
"result": "ManagedProjectPendingError: awaiting approval",
"result_type": "Error",
"error": {"kind": "award_pending", "message": "ManagedProjectPendingError: awaiting approval"}
```

`error` is the same failure with a machine-readable `kind` beside the prose.
Read it if you can — it is what the sending agent decided, rather than something
recovered from text — and fall back to `result` when it is absent, which means
the far side predates it.

### 3.3 Failing, and saying *why*

Return a failure as a completed exchange, not by staying silent: set the state
to `error` with a message (`job.errored(...)` in Python) and post it. Silence is
the one thing to avoid — it turns a clear failure into a timeout for whoever is
waiting.

For half the contract, failing *is* the answer. An award that needs human
approval, an award whose template you do not offer, an award whose allocation
exceeds what you will grant — none of these have a `ProjectMapping` to return
(§4.1), and all of them are ordinary outcomes rather than faults. So the error
carries the meaning, and the awarding portal acts on it.

#### The error classes

The convention is a **typed error carried in the message**: the class name, a
colon and a space, then the human-readable detail.

```
ManagedProjectPendingError: awaiting approval by a site administrator
```

Your message is wrapped once more on its way back — the portal agent turns it
into `RuntimeError{…}` — so the awarding portal sees:

```
RuntimeError{ManagedProjectPendingError: awaiting approval by a site administrator}
```

The `openportal` Python module defines this hierarchy, raises the right class
when a job comes back in error, and encodes it again when you fail one. Use the
classes rather than hand-writing the prefix; the string form is specified here
so that non-Python portals can produce and read it too.

Inside the agent network the class is also carried as a structured `kind` on the
job — `award_pending`, `award_rejected` — so the agents in between do not have
to read prose to route a failure. You do not have to produce that yourself: the
bridge derives it from the class name you send, which is why sending the
specified spelling matters. `job.error_kind` reads it back, and
[json-types.md](json-types.md) lists the kinds.

| Class | Base | Meaning to the caller |
|-------|------|-----------------------|
| `OpenPortalError` | `OSError` | Base of the hierarchy. Catch this to catch everything. |
| `OpenPortalOtherError` | `OpenPortalError` | An unexpected failure with no more specific class. What an unrecognised message decodes to. |
| `OpenPortalUnsupportedCommandError` | `OpenPortalError` | The instruction is not implemented here. Distinguishes "I don't do that" from "that went wrong". |
| `ManagedProjectPermissionError` | `OpenPortalError` | Base for the two award-decision outcomes below. |
| `ManagedProjectPendingError` | `ManagedProjectPermissionError` | **Not yet — ask again.** The request was understood and accepted, and is waiting on something (typically human approval). Not a fault. |
| `ManagedProjectRejectedError` | `ManagedProjectPermissionError` | **No.** The request was refused, and re-sending it unchanged will be refused again. |

#### Pending and rejected are treated very differently

This distinction is the reason the taxonomy exists, so it is worth being precise
about what the awarding portal does with each.

**`ManagedProjectPendingError` is benign and expected.** `waldur-mastermind`
logs it at debug, skips that synchronisation cycle, and tries again later
(`tasks.py`, `sync_remote_allocation_usage` and friends). The award stays
healthy in its records. An award parked awaiting approval for a week produces
this error on every cycle for a week, and nothing is wrong. Raise it whenever
the honest answer is "come back later" — and expect to be taken at your word.

**`ManagedProjectRejectedError` is terminal.** The awarding portal records the
award as errored, marks its allocation erred, and writes an audit entry
(`remote_project_service.py`, `record_award_rejected`). It stops treating the
award as workable. Raise it when re-asking cannot help: an unknown template, a
missing entitlement key, an end date already in the past, an allocation above
what you will ever grant.

Getting these the wrong way round is costly in both directions. A rejection
where you meant "pending" strands an award that only needed approving. A pending
where you meant "rejected" leaves the awarding portal retrying forever against a
decision that will never change.

### 3.4 Deadlines

**Jobs expire two minutes after creation.** After that the awarding portal
receives `ExpirationError{}` and your late result is discarded.

That is the outer limit, and it is not the one that will bite you. The awarding
portal has its own, shorter patience: `waldur-mastermind` abandons a request
after **30 seconds** and raises a timeout locally, long before the job expires
(`remoteclient.py`, `RemoteOpenPortalClient.run`). A result you post at 45
seconds is still accepted by the bridge and still travels back, but the caller
has already stopped waiting for it.

**Budget 30 seconds, not 90.** If you cannot answer in that window — a report
that takes minutes to compute — answer immediately from whatever you have
cached and compute out of band. The next request will collect the fresh figure,
and because callers retry (§3.5) there will be a next request.

Consequences worth designing around:

* **Poll frequently** if you rely on `/fetch_jobs` rather than `signal_url`. A
  60-second poll interval leaves no margin.
* **Your `signal_url` endpoint should return quickly** — 2xx as soon as the job
  is queued internally, not after it has been processed. The bridge retries a
  failed signal 5 times at 2-second intervals and then *removes the job from
  the board and errors it*, so a slow or flapping signal endpoint fails
  requests outright.

The reference implementation does exactly this: the signal handler records the
job and hands it to a Celery worker, then returns 200 immediately
(`api.py`, `fetch_job`). The work — and the answer — happens in the worker.

### 3.5 Everything is retried, so make everything idempotent

This is the single most important property to design for, and the one most
easily missed: **the awarding portal re-sends requests it has already sent
successfully.**

`waldur-mastermind` re-issues `create_award` on every synchronisation cycle for
every award it believes exists — the code comment reads "add it again just to be
sure" (`remotebackend.py`, `check_added_allocation`). It is not asking you to
create a second project. It is re-asserting the award's current state and
expecting you to reconcile. Alongside that, a periodic sweep
(`refresh_remote_projects`) re-reads awards in case a notification was missed.

What follows from this:

* **`create_award` for an award you already hold is normal traffic, not an
  error.** Look it up by identifier, merge the supplied details into what you
  have, and return the same mapping you returned last time. The reference
  implementation keys on `(destination, project_id)` and does a
  get-or-create.
* **`update_award` for an award you have never seen is also normal.** The
  reference implementation treats it as a create — it builds the award and
  routes it through its approval path rather than failing.
* **Failing a request is cheap.** A request you reject or cannot answer today
  will be asked again on the next cycle. Missing one is not a lost update, so
  prefer a clean, honest failure over a guess.
* **Duplicate deliveries of the *same* job must not do the work twice.** Record
  the job id and skip a job you have already run (`api.py` does a get-or-create
  on the id and only dispatches a job still in `pending`).

Retrying applies to your answers too: post the result, and if the post itself
fails, post it again. The reference implementation retries `send_result` five
times at one-second intervals before giving up.

---

## 4. The contract

For each instruction the portal may receive: the arguments it carries, and what
a successful result must contain. Types are as specified in
[json-types.md](json-types.md).

### 4.0 You may implement as much or as little of this as you need

This section is a menu, not a checklist. A connected portal implements the
instructions it has something to say about and declines the rest — there is no
minimum set, and no instruction whose absence stops the others from working.
`waldur-mastermind`, the reference implementation, does not implement every
entry below.

What matters is that you decline *clearly*. An instruction you do not implement
should come back as `OpenPortalUnsupportedCommandError` (§3.3), so the caller
can tell "this portal doesn't do that" apart from "that portal is broken". The
reference implementation raises a plain `Unknown command <name>` for anything
its dispatcher does not recognise (`tasks.py`, `run_job`), and callers sniff for
that text — so either form is understood in practice, and the typed class is the
better one to send.

Decline; never ignore. An unimplemented instruction that goes unanswered is
indistinguishable from an outage until the job expires (§3.4).

Which instructions you *will* be sent depends on what the awarding portal wants
from you. If it only ever creates awards and collects usage figures, that is all
you need to answer.

### 4.1 Awards

| Instruction | Arguments | Must return | Wire form |
|-------------|-----------|-------------|-----------|
| `create_project` / `create_award` | `<project_id> <AwardDetails JSON>` | `ProjectMapping` | `"myaward1.allocator:myproject1.site"` |
| `update_project` / `update_award` | `<project_id> <AwardDetails JSON>` | `ProjectMapping` | `"myaward1.allocator:myproject1.site"` |
| `remove_project` / `remove_award` | `<project_id>` | `ProjectMapping` | `"myaward1.allocator:myproject1.site"` |
| `get_project` | `<project_id>` | `AwardDetails` | object |
| `get_award` | `<project_id>` | `AwardDetails` | object |
| `get_awards` / `list_awards` | `<portal_id>` | `Vec<AwardDetails>` | array of objects |
| `get_projects` | `<portal_id>` | `Vec<ProjectMapping>` | array of **strings** |
| `get_project_mapping` | `<project_id>` | `ProjectMapping` | `"myaward1.allocator:myproject1.site"` |

Notes:

* **`ProjectMapping`** is `<their project id>:<your project id>` — a string,
  not an object, and the most important thing you return. See §4.1.1.
#### 4.1.1 The mapping is where the two sides agree what to call a thing

The awarding portal knows the award as `myaward1.allocator`. You create something
for it and know that as, say, `myproject1.site`. Neither side can guess the other's
name, and until they have been exchanged there is no way to say "that award" and
"that project" and mean the same object.

The `ProjectMapping` you return is that exchange. **Its second half is your own
`ProjectIdentifier` for the award** — a full `<project>.<your-portal>`
identifier in your namespace, naming the project you created *on the resource
the award came in through* (§1.3), not a bare group name:

```
myaward1.allocator:myproject1.site
```

Once you have returned it, the award ID and the project ID are two names for one
thing at this interface, and both sides hold the pair.

**Deciding it is part of provisioning.** You cannot answer with a mapping before
you have created the project and named it, which is exactly why an award still
awaiting approval answers with `ManagedProjectPendingError` instead (§3.3) —
there is no honest identifier to put in the second half yet.

**This is also the join for everything else.** Your accounting records usage
against `myproject1.site` and has never heard of `myaward1.allocator`; the awarding
portal asks `get_usage_report myaward1.allocator` and has never heard of
`myproject1.site`. The mapping is what lets you answer. In practice: build the
report against your own identifier, because that is the namespace the figures
were recorded in, and then translate it — in Python,
`report.remap_project(their_id)` rewrites the project and rebuilds every
`UserIdentifier` with it, so `alice.myproject1.site` becomes `alice.myaward1.allocator`
while the member's email stays as it is.

The second half is validated as a mapping target, so it is restricted to
`A-Za-z0-9._-` (no leading `-`, no leading or trailing `.`, no `..`). A portal
with no identifier scheme of its own may reuse the incoming project name, but
qualify it with your own portal so the mapping still says something.

* **A mapping is returned only on success.** `create_award` and `update_award`
  answer with a mapping when the award is in place; when it is not, they answer
  with an *error*, and that error is how the outcome is reported. This is not an
  edge case — a portal that queues awards for human approval has no local
  project, and therefore no local group to name, until someone approves it.
  There is nothing truthful it could put in a mapping. See §3.3 for which error
  to raise; `ManagedProjectPendingError` is the one that means "not yet, ask me
  again".
* **`remove_award` answers with `<project_id>:None`.** The award is gone, so
  there is no project of yours left to name; the literal string `None` fills the
  slot. The same form appears in `get_projects` for an award that has no project
  yet because nobody has approved it.
* **`update_*` is a merge.** Only the fields present in the supplied
  `AwardDetails` change; absent fields keep their current values. `members` and
  `allowed_domains`, when present, replace what you hold wholesale rather than
  adding to it — both are sets the awarding portal owns, so an update naming
  fewer entries means fewer. An `allowed_domains` of `[]` permits nobody, and is
  distinct from omitting the field.
* **`get_projects` returns mappings, `get_awards` returns details.** They are
  easy to confuse and the return types are different shapes.
* `get_project` is retained for compatibility; new callers use `get_award`.

The `AwardDetails` object is specified in full in
[json-types.md §ProjectDetails](json-types.md). Two fields are load-bearing for
portal-to-portal work:

* **`template`** names the kind of thing being asked for, and `key` may be
  required to prove entitlement to it. Reject a `create_award` whose template
  you do not offer, with a clear error, rather than provisioning a default.
* **`membership_control`** states whether *you* may change membership or roles
  independently of the awarding portal: `open` (default when absent),
  `members_only`, `roles_only`, `locked`. Honour it — the awarding portal is
  entitled to assume you do.

### 4.2 Members

| Instruction | Arguments | Must return |
|-------------|-----------|-------------|
| `get_users` | `<project_id>` | `Vec<UserMapping>` |

A `UserMapping` is `<user_id>:<local_user>:<local_group>`, e.g.
`alice.myaward1.allocator:alice@example.ac.uk:myaward1.allocator`.

At the portal layer the member's **email address is the `local_user`** — a
portal has no Unix accounts to name, and the email is the portal-level
equivalent. This is supported explicitly: `local_user` accepts either a Unix
account name or an email address, and a value containing `@` is recognised as
an address and validated as one.

The address grammar is narrower than RFC 5321 allows — local part from
`A-Za-z0-9._+-`, then a hostname of at least two labels — because the same
field carries Unix account names elsewhere in the network. Quoted local parts
and the rarely-used `!#$%&'*/=?^`{|}~` characters are rejected. A mapping is
rejected outright if the address does not fit, so if you hold exotic addresses,
substitute a sanitised form rather than letting the whole `get_users` response
fail.

`local_group` is **not** widened this way — it names a Unix group elsewhere in
the network, so it must stay within `A-Za-z0-9._-` (no leading `-`, no leading
or trailing `.`, no `..`). Reusing the project identifier, as above, is the
convention.

Returning an empty array is a valid answer for a project with no members.

**In practice members travel with the award, not through `get_users`.**
`waldur-mastermind` is one of the portals that does not implement `get_users`
(§4.0) — a request for it comes back as an unsupported command. What it does
instead is fold the live project membership into the `AwardDetails.members` it
returns from `get_award` (`board.py`, `get_award`), mapping each local role back
to the awarding portal's spelling.

So populate `AwardDetails.members` whether or not you implement `get_users`:
that is the route callers actually read today.

### 4.3 Usage and storage reports

| Instruction | Arguments | Must return |
|-------------|-----------|-------------|
| `get_usage_report` | `<project_id> <DateRange>` | `ProjectUsageReport` |
| `get_usage_reports` | `<portal_id> <DateRange>` | `UsageReport` |
| `get_storage_report` | `<project_id> <DateRange>` | `ProjectStorageReport` |
| `get_storage_reports` | `<portal_id> <DateRange>` | `StorageReport` |

The `<DateRange>` argument is either an explicit range or one of the keywords
`today`, `yesterday`, `this_week`, `last_week`, `this_month`, `last_month`,
`this_year`, `last_year`. It defaults to `this_week` when omitted.

* **Usage reports are per-day.** A `ProjectUsageReport` holds `reports` keyed by
  date, plus a `users` map from `UserIdentifier` to local username. The
  portal-level `UsageReport` wraps per-project reports keyed by project.
* **Report against the identifier you were asked about**, not your own. The
  request names the awarding portal's project, so the answer must too — even
  though your figures are recorded against your own. §4.1.1 covers the
  translation.
* **Storage reports are point-in-time.** The top-level fields of a
  `ProjectStorageReport` are the *latest* snapshot; `daily_reports` holds older
  ones, at most one per date. The date range therefore selects history, not the
  current figure.
* **Usage and storage are independent requests.** They are issued separately, by
  separate scheduled tasks, in no particular order
  (`sync_remote_allocation_usage` and `sync_remote_allocation_storage` in
  `tasks.py`). Neither waits for the other and neither is abandoned because the
  other failed. A portal that serves usage but not storage is a coherent
  position: answer `get_usage_report` properly and let `get_storage_report`
  fail, and the usage figures still arrive.
* **Empty still beats absent.** Where you genuinely have no storage to report,
  an empty `ProjectStorageReport` says "nothing here" and an error says
  "something is broken". The first is the truth and it is what a caller can act
  on — but since the two requests are independent, choosing the error costs you
  only the storage figures.
* **A project that is not on this resource reports empty.** A request for a
  project whose award was created through a *different* offering is a fair
  question with the answer "nothing was used here" (§1.3). Answer it with an
  empty report. An awarding portal may ask every offering it knows about which
  ones hold a given award, and failing the ones that do not would break that
  sweep for no reason.

### 4.4 What you will not receive

The bridge board carries only the instructions above. Everything else in the
`greatwestern` vocabulary — `add_user`, `remove_user`, `add_local_user`,
`get_home_dir`, quota and limit instructions — is southbound, handled by the
account, filesystem, and scheduler agents. A portal never sees them.

Requests reach the bridge only from the offerings registered in §1.1 (and
`sync_offerings` from your own portal agent); anything else is refused before
it reaches your HTTP API.

---

## 5. Notifications

Separately from jobs, the network sends fire-and-forget **notifications**
northbound — `award_added`, `award_removed`, `award_changed`, user events from
the agents below. These are informational: there is nothing to answer, and no
result is expected.

Delivery is a pull model, so the notification body is never posted to an
unauthenticated endpoint:

```
1. Bridge: GET <notification_url>?notification_id=<uuid>
2. You:    POST /fetch_notification  with the bare UUID
3. You:    return 200 to the original GET once you have it
```

The bridge tries 3 times at 2-second intervals, then logs and drops. Configure
`notification_url` with `op-bridge init --notification-url …`. See
[notification-protocol.md](notification-protocol.md) for the event vocabulary.

---

## 6. Implementation checklist

1. Register offerings at startup with `POST /sync_offerings`, and re-register
   whenever the set changes (§1.1).
2. Expose a `signal_url` endpoint that treats the job id as a shared secret,
   queues it, and returns 2xx immediately (§3.1, §3.4).
3. Fetch with `POST /fetch_job`; also run a slower `GET /fetch_jobs` sweep to
   catch anything a missed signal left behind.
4. Dispatch on the canonical instruction name — the `*_award` spellings arrive
   as their `*_project` equivalents (§2).
5. Authorise against `forwarded_for`, and store records under the full
   awarding-portal identifier (§1.2).
6. Read the offering from the request, key your records on
   `(offering, project_id)`, and scope every answer by it. A project that is
   not on the requested resource reports empty, not an error (§1.3).
7. Decide your own `ProjectIdentifier` for an award when you provision it, and
   return it as the second half of the `ProjectMapping`. It is what the awarding
   portal joins on, and what your usage figures translate through (§4.1.1).
8. Pick the instructions you will answer; decline the rest with
   `OpenPortalUnsupportedCommandError` (§4.0). Return the exact type in §4 for
   the ones you keep, and never let a job go unanswered (§3.3).
9. Fail with the right class — `ManagedProjectPendingError` for "not yet",
   `ManagedProjectRejectedError` for "no" — because the caller treats them
   completely differently (§3.3).
10. Make every handler idempotent: `create_award` for an award you already hold
   is normal traffic, and duplicate job ids must not do the work twice (§3.5).
11. Answer within 30 seconds, not the two-minute expiry; serve slow reports from
    cache (§3.4).
12. Report usage against the identifier you were asked about, not your own
    (§4.1.1, §4.3).
13. Handle `membership_control` and reject unknown `template` values (§4.1), and
    populate `AwardDetails.members` (§4.2).
14. Fetch and acknowledge notifications (§5).

---
## 7. Source file reference

| Concept | Source file |
|---------|-------------|
| Which instructions reach the portal, and the board/signal cycle | `bridge/src/main.rs` |
| Offering registration and virtual agents (bridge side) | `bridge/src/main.rs` (`sync_offerings`) |
| Offering registration and portal-to-portal routing (portal side) | `portal/src/main.rs` (`sync_offerings`, `virtual_resource_runner`) |
| The result type expected for each instruction | `portal/src/main.rs` (`get_award`, `get_users`, …) |
| Job lifetime and expiry | `templemeads/src/job.rs` |
| `AwardDetails`, `ProjectMapping`, `UserMapping` | `greatwestern/src/grammar.rs` |
| `local_user` Unix/email forms and the guard on Unix use | `templemeads/src/validate.rs` (`LocalUser`) |
| Usage and storage report types | `greatwestern/src/usagereport.rs`, `greatwestern/src/storagereport.rs` |
| Generated TypeScript definitions for every result type | `greatwestern/bindings/` |
| The error classes and their wire encoding | `python/src/lib.rs` (`OpenPortalError` and subclasses) |

### 7.1 The example portal

[`python/examples/site_portal/`](../../python/examples/site_portal/) implements this
document — every instruction, the approval path, the retry contract, the
answer-everything guarantee — in about 400 lines of commented Python, with a
test suite that drives each handler without needing a bridge. It is written to be
read rather than deployed, and its README is explicit about what a production
portal would have to add.

Start there if you are implementing this contract. Then read
`waldur-mastermind` below for what it looks like at full size.

### 7.2 The production implementation

`waldur-mastermind` implements this contract on both sides, in
`src/waldur_openportal/` (branch `feature_airrportal`). It is the most useful
thing to read alongside this document, because it shows what a real portal
actually does rather than what it minimally must.

| What you want to see | File |
|----------------------|------|
| Every instruction handler — create/update/remove/get award, reports | `board.py` (`OpenPortalBoard`) |
| Instruction dispatch, and failure turned into an errored job | `tasks.py` (`run_job`) |
| The signal endpoint: 403 on unknown id, queue, return 200 | `api.py` (`fetch_job`, `fetch_notification`) |
| Re-sending `create_award` "just to be sure" — the idempotency contract | `remotebackend.py` (`check_added_allocation`) |
| Pending treated as benign and retried; rejection treated as terminal | `tasks.py` (`sync_remote_allocation_*`), `remote_project_service.py` |
| Usage and storage synchronised independently | `tasks.py` (`sync_remote_allocation_usage`, `sync_remote_allocation_storage`) |
| The 30-second caller timeout | `remoteclient.py` (`RemoteOpenPortalClient.run`) |
| The error taxonomy before it moved into `openportal` | `op.py` |
