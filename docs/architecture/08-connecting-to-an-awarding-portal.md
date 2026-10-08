<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# 8. Connecting to an awarding portal

What a site portal's operator does, in the order they meet it: connect to an
awarding portal, advertise a resource, receive awards, and report usage back.
[Portal to portal](03-portal-to-portal.md) explains the concepts; this page is
the walkthrough.

The example is the same as everywhere in this guide: the awarding portal
`allocator` awards time on `cluster1`, a resource offered by the site portal
`site`. Each operator runs their own portal agent and bridge, and nothing else
is shared.

```
allocator.site.cluster1 create_award myaward1.allocator {...}
```

## 1. Connecting the two portals

The awarding portal issues the invitation and listens; the site accepts it and
dials out. One awarding portal serves many sites, so only it needs an open port,
and none of the sites do.

### On `allocator`: create the invitation

```
$ op-portal client --add site \
    --ip 198.51.100.0/24 --type portal \
    --zone "allocator>site"
```

This writes `invite_allocator_allocator>site.toml`, to be sent to the site by
any secure out-of-band route:

```toml
name = "allocator"
url = "wss://allocator.example.org:8040"
zone = "allocator>site"
inner_key = "9f3c...e41a"
outer_key = "5b07...c2d8"
type = "portal"
```

and adds the matching client to `allocator`'s own config:

```toml
[[clients]]
name = "site"
ip = "198.51.100.0/24"
zone = "allocator>site"
inner_key = "9f3c...e41a"
outer_key = "5b07...c2d8"
type = "portal"
```

Two flags matter:

- **`--type portal`** pins the type `site` must present itself as. Anything else
  is refused.
- **`--zone "allocator>site"`** puts the link in the portal-to-portal zone - the
  awarding portal first, then the site. Offerings are registered as virtual
  agents in that zone, so an award sent in any other zone has nowhere to land.
  Quote it, or the shell treats `>` as a redirect.

### On `site`: accept it

```
$ op-portal server --add "invite_allocator_allocator>site.toml"
```

`site`'s config gains the matching server:

```toml
[[servers]]
name = "allocator"
url = "wss://allocator.example.org:8040"
zone = "allocator>site"
inner_key = "9f3c...e41a"
outer_key = "5b07...c2d8"
type = "portal"
```

`site` now dials out to `allocator`, the [four connection checks](04-paddington.md#four-checks-before-a-single-message)
pass, and the two portals are connected.

The config file is plain TOML, and everything the link needs is in it - who to
connect to and where, the zone, the two keys, and the type the peer must
present. You can read it, and edit it by hand if you must (a new URL or address
range, say), though the CLI is the recommended way to add, remove and rotate
peers. Add an `[encryption]` section to protect the keys in the file at rest.
[Agent configuration](../specifications/agent-configuration.md) has the details.

*The address range, URL and keys above are illustrative, and the keys are
shortened.*

## 2. Advertising a resource

The site's portal software tells its bridge what it offers:

```
POST /sync_offerings
["cluster1.site.allocator"]
```

That creates the `cluster1` virtual agent, which `allocator` then addresses as
`allocator.site.cluster1`.

- **Offer first.** A request for a resource that is not offered yet is held, not
  refused, and delivered once it is.
- **Read it both ways.** The site writes *resource.site.allocator*; the awarding
  portal writes *allocator.site.resource*.
- **Replace, not merge.** `sync_offerings` sets the complete list. `add_offerings`
  and `remove_offerings` make incremental changes.

The middle element must be the site's own agent name, or the offering is
rejected. The portal software should call `sync_offerings` on startup and
whenever the set changes.

## 3. Handling awards

All award instructions are addressed to the offering:

| Instruction | Arguments | Asks the site to |
|---|---|---|
| `create_award` | `<award> <AwardDetails>` | Attach an award to a project - usually a new one, but an existing project is fine |
| `update_award` | `<award> <AwardDetails>` | Apply new dates, a new allocation or new members |
| `remove_award` | `<award>` | Detach the award; the project stays intact |
| `get_award` | `<award>` | Return the award as the site holds it |
| `get_awards` | `<portal>` | Return every award from that portal |

The `*_award` names are exact synonyms of the `*_project` forms
(`create_award` is `create_project`), so a site portal should accept both.
Every instruction is retried until it gets an answer, and a Job expires after
two minutes, so **every handler must be safe to repeat**.

### The award details

`AwardDetails` is the one JSON argument, and always comes last:

```json
{
  "name": "Protein folding at scale",
  "members": {
    "lead@example.org": "pi",
    "postdoc@example.org": "member"
  },
  "start_date": "2026-11-01",
  "end_date": "2027-01-31",
  "allocation": "10000 GPUHR",
  "award": { "id": "AWARD-2026-123", "url": "https://..." }
}
```

- **Who:** `members` maps each email address to a role. The site decides the
  local accounts.
- **When:** `start_date` and `end_date`, as ISO dates.
- **How much:** `allocation` carries a size and a unit, and `breakdown` can split
  it further.
- **Where from:** `award`, `call` and `project_link` point back to the awarding
  portal's own records.

Every field is optional. There are more - `description`, `template`, `key`,
`notes`, `earliest_approve`, `membership_control` and others - all in
[the JSON types](../specifications/json-types.md).

### Awaiting approval

Most sites review awards by hand, so the first answer to `create_award` is
usually "not yet":

1. **`create_award` arrives**, and the site queues the award for review.
2. **The answer is `ManagedProjectPendingError`.**
3. **The awarding portal asks again**, each cycle, until the answer changes.
4. **An administrator approves it**, and the next request gets the project
   mapping.

| Answer | What the awarding portal does |
|---|---|
| `ManagedProjectPendingError` | Benign: logs it quietly, and tries again later |
| `ManagedProjectRejectedError` | Terminal: marks the award as errored, and stops |

Pending is expected, not a fault - an award that waits a week for review
produces it every cycle for a week. Use `ManagedProjectRejectedError` only when
asking again can never help: an unknown template, an end date in the past, an
allocation you will never grant. Getting the two the wrong way round is costly
either way.

### The project mapping

```
myaward1.allocator:myproject1.site
└───────┬────────┘ └──────┬──────┘
 the awarding portal's    the site's name for the
 name for the award       project it created
```

- **Returned once approved.** It is the successful answer to `create_award`, so
  there is no mapping while an award is pending.
- **One award, one project.** A project holds at most one award at a time, and
  the site can move an award by returning a new mapping.
- **The join for usage.** The site records usage against its own name, and the
  awarding portal asks using its own. The mapping connects them.

## 4. Reporting usage

The awarding portal asks for usage and for storage on separate schedules, and
neither waits for the other.

| Instruction, after `allocator.site.cluster1` | Returns |
|---|---|
| `get_usage_report myaward1.allocator last_month` | Usage for one award, per day |
| `get_usage_reports allocator this_month` | Usage for every award from `allocator` |
| `get_storage_report myaward1.allocator` | Storage used and quotas, as the latest snapshot |
| `get_storage_reports allocator` | Storage for every award from `allocator` |

Date ranges are `today`, `yesterday`, `this_week`, `last_week`, `this_month`,
`last_month`, `this_year`, `last_year`, or an explicit range; a usage report
with none covers `this_week`.

Three rules for the answers:

- **Report in the awarding portal's terms.** Build the report against your own
  project, then translate it with the mapping, so it names the awarding portal's
  award. Report figures in the awarding portal's units - agree the conversion
  factors in advance.
- **An award that is not on this resource gets an empty report, not an error.**
  That is what lets an awarding portal safely sweep every offering to find one.
- **Usage that cannot be tied to a named user** is reported under `unknown`.

A usage report is keyed by date, then by local user name, and its `users` map
translates local names back to OpenPortal identifiers. A storage report carries
the latest snapshot at the top level and keeps older ones in `daily_reports`,
one per date. Both formats are backwards compatible: every newer field has a
default when absent, so a report can be as simple or as detailed as a site
needs - component breakdowns, job counts, queue waits, reservations, requeue
accounting and expansion factors are all optional.
[The JSON types](../specifications/json-types.md) have the full formats, and
[the site portal API](../specifications/site-portal-api.md) is the contract a
site portal must meet.

## Try it

The [site portal example](../../python/examples/site_portal) does all of this on
a laptop - two portals, two bridges and an example site portal in Python or
Java, with every peering done for you:

```
cd python/examples/site_portal
pip install -r requirements.txt
python example.py start              # the Python site portal
python example.py start --app java   # or the Java one
```

It binds to `127.0.0.1` only and its configs are unencrypted: it is for learning,
not for deployment. Its README walks through all ten steps, including updates,
removal and finalised reports.
