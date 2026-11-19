<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# Award spend profiles, and asking a portal what it supports

Status: **implemented.**

Two additions to greatwestern, made together because the second is what makes
the first safe to rely on:

1. **`spend_profile`**, a new optional field on `AwardDetails`, through which an
   awarding portal tells a site how an award's allocation should be spent over
   time - for example a twelve-month award that must spend at least 10%, 10%,
   30% and 40% of its allocation in each quarter, or may spend at most that
   much, or both.
2. **`is_feature_supported`** and **`get_supported_features`**, two new
   instructions through which one portal asks another what it supports, so an
   awarding portal can find out whether a site understands spend profiles before
   depending on one.

## 1. The principle: OpenPortal carries, the endpoints decide

Both additions follow one rule, and every decision below comes from it:

> **OpenPortal communicates the information. What it means is business logic,
> and belongs to the portals at either end.**

A spend profile is an agreement between an awarding portal and a site. The site
sees the real-time spend, and it alone knows how to enforce a minimum - by
warning users, adjusting priorities, or forfeiting what was not used. The
awarding portal knows what it awarded. OpenPortal's job is to get the profile
from one to the other intact and unambiguous, and nothing more. It does not
compute shortfalls, forfeit allocation, or decide whether a profile is sensible.

The same goes for features. Whether a site "supports spend profiles" is a
property of its portal software, which no OpenPortal agent can see. So the
portal software answers, and OpenPortal routes the question and the answer.

If a site does not support spend profiles, the two portals agree some other
mechanism between themselves - for example, the awarding portal sends the award
in parts. That needs nothing from OpenPortal at all.

## 2. `spend_profile`

### 2.1 The shape

```json
{
  "allocation": "1000 NHR",
  "start_date": "2027-01-01",
  "end_date": "2027-12-31",
  "spend_profile": [
    {"from": "2027-01-01", "minimum": "100 NHR", "maximum": "100 NHR"},
    {"from": "2027-04-01", "minimum": "100 NHR"},
    {"from": "2027-07-01", "maximum": "300 NHR"},
    {"from": "2027-10-01"}
  ]
}
```

Each entry sets the controls on spending from its date: a `minimum` ("use it or
lose it"), a `maximum` (a cap), both, or neither. Both are optional. Reading the
example: exactly 100 NHR in the first quarter, at least 100 NHR in the second,
at most 300 NHR in the third, and no controls at all from October.

Each entry is an object rather than a tuple. An object says what each value is,
reads naturally from TypeScript and Java, lets either control be left out, and
leaves room to add fields to an entry later without breaking anything that reads
the current ones.

`minimum` and `maximum` are `Allocation`s, the same type as `allocation` itself,
so they parse the same way (`"100 NHR"`) and inherit that type's validation: a
size that is negative, `NaN` or infinite is refused.

One profile carries both kinds of control, rather than a profile of minimums and
a separate list of caps, so that a period's floor and cap always share one set
of period boundaries and cannot drift out of line with each other.

### 2.2 The intended meaning

This is the convention the portals are expected to share. It is documented here
so they can, but **OpenPortal does not interpret it**:

- Entry *i* applies from its `from` date up to, but not including, the `from`
  date of entry *i + 1*. The last entry applies until the award's `end_date`.
- **Each entry describes only its own period. Nothing carries over from one
  entry to the next.** A control an entry does not set does not apply in its
  period: an entry with a `maximum` and no `minimum` has no minimum, whatever
  the entry before it said. A cap that should hold for four quarters is written
  in all four entries.
- So an entry that sets neither control - `{"from": "2027-10-01"}` - means **no
  controls from that date**. It is how a profile ends its controls before the
  award ends. It reads oddly as JSON, which is why every implementation prints
  it in words (§2.4).
- The days before the first entry have no controls either.
- Each `minimum` is the least that should be spent in its period. Each
  `maximum` is the most that may be spent in it. A `minimum` equal to the
  `maximum` is an exact spend; a `maximum` of zero is a period with no spending.
  Neither needs to add up to the allocation.

This reading is part of the data format, not business logic: without it, two
portals could read the same profile differently. Everything else - whether
spending above a minimum in one period counts towards the next, whether an
unspent cap carries forward, whether a cap is a hard stop or a warning, what
happens to a shortfall, whether the units must match the allocation's, whether
the dates must fall inside the award - is for the two portals to agree.

### 2.3 What OpenPortal validates

Only what it takes for the data to be well formed and to mean one thing:

| Check | Why it is OpenPortal's business |
|---|---|
| Every `from` is a valid date | Otherwise the data cannot be read at all |
| Every `minimum` and `maximum` is a valid `Allocation` | The same |
| Neither control is an empty allocation (`"none"`) | As a `maximum`, "none" could mean "no cap" or "nothing may be spent". An unset control is spelled by leaving it out |
| When both are set, they are in the same units | OpenPortal does not convert units, so a floor and a cap in different units cannot be compared - and a period whose floor and cap cannot be compared does not say one thing |
| When both are set, `maximum >= minimum` | A floor above the cap cannot be met. Equal is allowed |
| An entry has no fields but `from`, `minimum` and `maximum` | An entry that sets no controls means "no controls", so a misspelled `"maximun"` that was quietly ignored would turn a cap into no cap. A field added to entries later comes with a feature name of its own (§3), and a sender only uses it with a site that supports it - so this does not stop the format growing |
| Entries are held in date order | So the same profile always serialises the same way, and comparing two profiles compares their content rather than the order someone happened to write them in |
| No two entries share a `from` date | Two entries from the same day do not say which applies. That is ambiguity in the data itself, not a question of policy |

Deliberately **not** validated, because each is a policy question for the
portals: that the units match the allocation's, or are the same from one entry
to the next; that the dates fall within `start_date` and `end_date`; that the
controls do not exceed the allocation; that `end_date` is set at all.

Nor is anything tidied. Entries arriving out of order are sorted, but otherwise
a profile is kept exactly as sent: two "no controls" entries in a row, or a
profile made only of them, are redundant but not ambiguous, and are carried as
they are. Duplicate dates are refused, with an error naming the date.

### 2.4 Printing an entry

An entry is printed in words, the same in Rust, Python and Java, so that a
person reading a log or a debugger sees what it means:

| Entry | Printed |
|---|---|
| `{"from": "2027-01-01", "minimum": "100 NHR", "maximum": "100 NHR"}` | `from 2027-01-01: spend exactly 100 NHR` |
| `{"from": "2027-01-01", "minimum": "100 NHR", "maximum": "300 NHR"}` | `from 2027-01-01: spend at least 100 NHR and at most 300 NHR` |
| `{"from": "2027-01-01", "minimum": "100 NHR"}` | `from 2027-01-01: spend at least 100 NHR` |
| `{"from": "2027-01-01", "maximum": "300 NHR"}` | `from 2027-01-01: spend at most 300 NHR` |
| `{"from": "2027-01-01"}` | `from 2027-01-01: no spend controls` |

### 2.5 Absent, empty, and a list

The field follows the same three-way convention as `allowed_domains`:

| On the wire | Means |
|---|---|
| absent | No profile. Under a legacy partial `update_award` (§2.6), keep whatever is held |
| `[]` | Explicitly no profile - the way to remove one |
| a non-empty list | This profile, replacing any held before |

The distinction between absent and `[]` matters because of partial updates. The
site portal API says an `update_award` carries the whole of `AwardDetails`, but
it still honours older senders whose partial updates leave absent fields
unchanged. Under that rule, "absent" cannot also mean "remove the profile", so
removal is spelled `[]`.

### 2.6 Updates

A spend profile in an `update_award` replaces the one held, wholesale, exactly as
`members` and `allowed_domains` do. A change to the allocation does not rescale
the profile - its amounts are absolute, which is the point of using them - so an
awarding portal that wants a rescaled profile sends one.

### 2.7 Compatibility, and why discovery matters

`AwardDetails` does not deny unknown fields, so an older peer reading an award
with a `spend_profile` will not fail - it will ignore the field. That is the
right behaviour for a reader, but it has a consequence worth stating plainly:

> **An agent that predates `spend_profile`, and re-serialises the award while
> forwarding it, drops the profile without saying so.**

`op-portal` does exactly that when it passes an award to its bridge. So a profile
arrives intact only if every agent on the path that parses greatwestern is new
enough to know the field. That is the strongest reason for §3: an awarding portal
should ask before it relies on a profile arriving. And because an agent old
enough to drop the field is also too old to know `is_feature_supported`, it
answers the question with an error - which reads correctly as "no".

## 3. Asking what a portal supports

### 3.1 The instructions

```
allocator.site.cluster1 is_feature_supported award_spend_maximums
allocator.site.cluster1 get_supported_features
```

| Instruction | Arguments | Returns |
|---|---|---|
| `is_feature_supported` | `<feature>` | `bool` |
| `get_supported_features` | none | `Vec<String>` |

Both are addressed to an **offering** and answered by the site's **portal
software**, through its bridge, like every other award instruction. Addressing
the offering gives per-resource answers for free: `cluster1` may support spend
profiles where `cluster2` does not.

`is_feature_supported` is the easier of the two to implement correctly, since a
portal only has to recognise the names it supports. `get_supported_features`
lets a caller discover everything at once. A portal should answer both
consistently: a feature is supported if and only if it is in the list.

### 3.2 Feature names are opaque strings

Feature names are **not** an enum. They are strings, defined by convention and
ultimately by the portals at either end:

- An enum would mean every new feature needed every agent that parses
  greatwestern upgraded before it could even be *asked about* - which defeats
  the purpose of an instruction for discovering what the other end can do.
- The endpoints are external portals, which may define features of their own
  that OpenPortal never needs to know about.

OpenPortal validates only the shape of a name: one to 64 characters, lowercase
ASCII letters, digits, `_`, `-` and `.`, starting with a letter. That keeps a
name a single token in the instruction grammar, keeps it bounded, and makes
`Award_Spend_Profiles` an error rather than a silent near-miss. Names outside
that shape are refused, not normalised.

Well-known names are listed in the
[site portal API](../specifications/site-portal-api.md), starting with:

| Feature | Means the site |
|---|---|
| `award_spend_minimums` | Understands the `minimum`s in `AwardDetails.spend_profile`, and acts on them as agreed with the awarding portal |
| `award_spend_maximums` | Understands the `maximum`s in `AwardDetails.spend_profile`, and acts on them as agreed with the awarding portal |

These are two names rather than one because enforcing a floor and enforcing a
cap are different business logic, and a site may well do one and not the other:
a cap maps naturally onto the scheduler limits a site already has, while
forfeiting what was not spent is a process. An awarding portal asks about
exactly what it is about to rely on.

### 3.3 Three outcomes, not two

A caller has to keep "no" and "could not tell" apart. This is the same rule the
repository applies to name lookups: a lookup that fails to answer is not a name
that does not exist.

| What comes back | Means |
|---|---|
| `true` | Supported |
| `false` | Not supported |
| `OpenPortalUnsupportedCommandError` | Not supported - the site's portal software is new enough to receive the instruction but does not implement it |
| An error from an agent that cannot parse the instruction | Not supported - the site is too old to know about feature discovery, and so about anything it could report |
| A timeout, or the job expiring | **Unknown.** Ask again later. Treating this as "no" would let a brief outage permanently downgrade a site |

### 3.4 Answers are snapshots

Sites upgrade. A caller should re-ask - before each `create_award`, or by caching
an answer with an expiry - rather than remember an answer indefinitely.

### 3.5 Why not the capability flags agents already exchange

Agents already announce capabilities to each other when they connect, in
`Register`. Those flags describe *protocol* behaviour between two *neighbouring*
agents. Feature discovery is about the portal software at the far end of the
path, which no agent in between can see. They are different layers, so this is a
separate instruction rather than another flag.

### 3.6 Routing and authorisation

Two agents on the site's side keep an allow-list, and both gain the two new
instructions. `op-portal` only forwards instructions it lists to an offering's
virtual agent, and forwards these to its bridge with `forwarded_for` set,
exactly as the award instructions are. `op-bridge` only puts instructions it
lists on the board its portal software reads, and passes these through
unchanged.

`op-portal` checks the shape of every name in a `get_supported_features`
answer before returning it, and refuses the whole answer if one is malformed -
the portal developer should see the mistake, not a feature that silently never
matches.

Neither instruction names a portal's projects or users, so the ownership rule -
a command naming a portal's identifiers must enter through that portal - has
nothing to check. That is safe: the answer is capability information, not
anything about a project, and only a portal that can already route to the
offering can ask.

## 4. Rollout

| To... | Needs |
|---|---|
| **Send** a spend profile | The awarding portal's agents new enough to know the field |
| **Receive** a spend profile intact | Every parsing agent on the site's side new enough too, *and* the site's portal software reading it |
| **Ask** about features | The awarding portal's agents new enough to know the instructions |
| **Answer** | The site's `op-portal` and `op-bridge` new enough to forward them, and its portal software implementing them |

Until a site upgrades, every one of these degrades to "no": the profile is
dropped, and the question is answered with an error.

## 5. Testing

- `spend_profile` round-trips through JSON; entries are sorted; duplicate dates
  are refused; minimum-only, maximum-only, both, and no-controls entries all
  round-trip and print as words, and nothing carries between entries; an empty
  allocation as a control, mismatched units, a maximum below the minimum and an
  unknown field are refused; absent, `[]` and a list stay distinct through a round trip; an
  older reader's JSON (no field) still parses; the TypeScript binding is
  regenerated.
- Feature names: valid shapes parse, invalid ones (uppercase, spaces, empty, too
  long, leading digit) are refused; both instructions round-trip through parse
  and `to_string`, and appear in the exhaustive parser sweep.
- The Python module and the Java client read and write `spend_profile`, and
  answer `bool` and `Vec<String>` results.
- Both site portal examples answer the two new instructions and carry a spend
  profile through `get_award`.
- End to end, through the site portal example: ask both questions across the
  portal-to-portal link, and send an award with a profile and read it back.

## 6. Out of scope

Everything that gives a spend profile its consequences: computing shortfalls,
forfeiting allocation, carrying an overspend forward, checking units or totals,
and enforcing anything. Those belong to the portals - which is the point.
