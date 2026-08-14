<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# Project Portal API Specification

**Status:** Normative for the `greatwestern` domain
**Audience:** Anyone connecting a *project portal* — the portal software that
owns projects and their members (e.g. Waldur) — to an OpenPortal network.

---

## 0. What this document is

The `op-bridge` agent gives portal software an HTTP API. Most of that API is
about the portal *asking* OpenPortal to do something. This document specifies
the other direction: the requests OpenPortal sends **to** the portal, which the
portal must answer.

Those requests are how another portal — an *awarding portal* running its own
OpenPortal agents — creates and queries awards on your infrastructure. If you
implement everything in §4, an awarding portal can create an award on your
portal, ask what happened to it, and collect usage and storage reports, without
either side sharing a database or a credential.

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
| Doing it in Python | [python-api.md](python-api.md) |

---

## 1. The shape of a portal-to-portal exchange

Two portals, each with their own OpenPortal agents:

* **`ukri`** — the *awarding portal*. Makes awards; wants them provisioned
  elsewhere.
* **`aip1`** — the *project portal* (yours). Owns projects, members, and the
  infrastructure behind them.

`aip1` advertises one or more **offerings** — named things `ukri` is allowed to
address. An offering is written `<offering>.<local-portal>.<remote-portal>`,
e.g. `isambard-ai.aip1.ukri`: "the resource `isambard-ai`, offered by `aip1`,
to `ukri`".

A request then flows:

```
 1.  ukri's portal software submits, through its own bridge:
         ukri.aip1.isambard-ai create_award myproj.ukri {…AwardDetails…}

 2.  ukri's portal agent  ──────────►  aip1's portal agent
         (the destination's first hop is the sending portal itself;
          the last hop names the offering)

 3.  aip1's portal agent recognises the offering and re-issues the request
     to its own bridge, tagged with where it came from:
         aip1.<bridge>.isambard-ai create_project myproj.ukri {…}
         forwarded_for = ukri.aip1.isambard-ai

 4.  aip1's bridge puts the Job on its board and signals your portal:
         GET <signal_url>?job_id=<uuid>

 5.  YOUR PORTAL: fetch the job, do the work, post the result.        ← §3

 6.  The result travels back the way it came, to ukri.
```

Everything above step 5 is handled by the agents. Steps 0 and 5 are yours.

### 1.1 Step 0: you must advertise offerings first

Until an offering exists, requests from `ukri` have nowhere to land — they are
held and only delivered once the offering is registered. On startup (and
whenever the set changes) call `POST /sync_offerings` with the complete list:

```json
["isambard-ai.aip1.ukri", "isambard-p1.aip1.ukri"]
```

`sync_offerings` is a *replace*, not a merge — anything absent is withdrawn.
Use `add_offerings` / `remove_offerings` for incremental changes, and
`get_offerings` to read the current set.

The middle element must be your own portal's agent name, or the offering is
rejected.

### 1.2 Telling portal-to-portal requests apart

Two things distinguish an awarding portal's request from a local one:

**`forwarded_for`** carries the original destination, e.g.
`ukri.aip1.isambard-ai`. Its **first** element is the portal that asked; its
**last** element is the offering they came in through. This is the field to
authorise against — it is set by your own portal agent, not by the caller.

**Identifiers name the awarding portal.** A project created by `ukri` is
`myproj.ukri`, not `myproj.aip1`, and its members are
`alice.myproj.ukri`. Key your records on the full identifier: the same project
name may exist under two different awarding portals, and they are different
projects.

A locally-originated request has `forwarded_for` absent or naming only your own
portal.

---

## 2. The `award` vocabulary

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

A fetched job looks like:

```json
{
  "id":            "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "created":       1700000000,
  "changed":       1700000005,
  "expires":       1700000120,
  "version":       2,
  "command":       "aip1.bridge.isambard-ai get_award myproj.ukri",
  "state":         "pending",
  "result":        null,
  "result_type":   null,
  "forwarded_for": "ukri.aip1.isambard-ai"
}
```

`command` is the destination path followed by the instruction. Parse the
instruction rather than the whole string: in Python, `job.instruction.command`
gives the verb (`get_award`) and `job.instruction.arguments` the argument list
(`["myproj.ukri"]`).

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
"result": "\"myproj.ukri:grp001\"",
"result_type": "ProjectMapping"
```

### 3.3 Failing

Return a failure as a completed exchange, not by staying silent: set the state
to `error` with a message (`job.errored("no such project")` in Python) and post
it. The awarding portal receives it as `RuntimeError{no such project}`.

Silence is the one thing to avoid — it turns a clear failure into a timeout for
whoever is waiting.

### 3.4 Deadlines

**Jobs expire two minutes after creation.** After that the awarding portal
receives `ExpirationError{}` and your late result is discarded.

That budget covers the whole round trip, so treat it as roughly 90 seconds of
processing time. If you cannot answer in that window — a report that takes
minutes to compute — answer immediately from cache and compute out of band, or
have the awarding portal ask again later.

Consequences worth designing around:

* **Poll frequently** if you rely on `/fetch_jobs` rather than `signal_url`. A
  60-second poll interval leaves no margin.
* **Your `signal_url` endpoint should return quickly** — 2xx as soon as the job
  is queued internally, not after it has been processed. The bridge retries a
  failed signal 5 times at 2-second intervals and then *removes the job from
  the board and errors it*, so a slow or flapping signal endpoint fails
  requests outright.

---

## 4. The contract

For each instruction the portal may receive: the arguments it carries, and what
a successful result must contain. Types are as specified in
[json-types.md](json-types.md).

### 4.1 Awards

| Instruction | Arguments | Must return | Wire form |
|-------------|-----------|-------------|-----------|
| `create_project` / `create_award` | `<project_id> <AwardDetails JSON>` | `ProjectMapping` | `"myproj.ukri:grp001"` |
| `update_project` / `update_award` | `<project_id> <AwardDetails JSON>` | `ProjectMapping` | `"myproj.ukri:grp001"` |
| `remove_project` / `remove_award` | `<project_id>` | `ProjectMapping` | `"myproj.ukri:grp001"` |
| `get_project` | `<project_id>` | `AwardDetails` | object |
| `get_award` | `<project_id>` | `AwardDetails` | object |
| `get_awards` / `list_awards` | `<portal_id>` | `Vec<AwardDetails>` | array of objects |
| `get_projects` | `<portal_id>` | `Vec<ProjectMapping>` | array of **strings** |
| `get_project_mapping` | `<project_id>` | `ProjectMapping` | `"myproj.ukri:grp001"` |

Notes:

* **`ProjectMapping`** is `<project_id>:<local_group>` — the identifier the
  awarding portal used, paired with whatever you call that project locally. It
  is a string, not an object.
* **`create_*` returns a mapping, not a status.** Returning it means "recorded",
  which is not the same as "provisioned" — a portal that queues awards for
  human approval still returns the mapping immediately and reflects the real
  state in `get_award` afterwards.
* **`update_*` is a merge.** Only the fields present in the supplied
  `AwardDetails` change; absent fields keep their current values. `members`,
  when present, replaces the member set wholesale.
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
`alice.myproj.ukri:alice@example.ac.uk:myproj.ukri`.

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
* **Storage reports are point-in-time.** The top-level fields of a
  `ProjectStorageReport` are the *latest* snapshot; `daily_reports` holds older
  ones, at most one per date. The date range therefore selects history, not the
  current figure.
* **Empty is better than absent.** For storage, an empty report for a project
  with no storage is the expected answer — a caller typically asks for usage and
  storage together and an error fails both.

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
2. Expose a `signal_url` endpoint that queues the job id and returns 2xx
   immediately (§3.1, §3.4).
3. Fetch with `POST /fetch_job`; also run a slower `GET /fetch_jobs` sweep to
   catch anything a missed signal left behind.
4. Dispatch on the canonical instruction name — the `*_award` spellings arrive
   as their `*_project` equivalents (§2).
5. Authorise against `forwarded_for`, and store records under the full
   awarding-portal identifier (§1.2).
6. Return the exact type in §4 for each instruction, or an explicit error.
   Never let a job go unanswered (§3.3).
7. Answer within the two-minute expiry; serve slow reports from cache (§3.4).
8. Handle `membership_control` and reject unknown `template` values (§4.1).
9. Fetch and acknowledge notifications (§5).

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
| A worked portal implementation | `cloudportal/src/main.rs` |
