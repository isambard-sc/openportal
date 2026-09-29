<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# Slurm requeue charging: billing the user's own requeues, and the limit that follows

Status: **implemented**, as described below, with the deviations noted in §11.
The reporting half has been exercised at cluster scale on a production machine,
through `get_requeue_report --cluster-wide` over a full month; the limit
correction of §4 has not yet run in production.

This is the sequel to `slurm-requeue-accounting-design.md`, which made the
previously invisible consumption of requeued attempts *measurable* and
deliberately declined to decide what should be charged for it. That decision
has now been taken, and this document describes it and the one thing it breaks.

The short version. A requeue the user asked for is charged like any other
usage; a requeue caused by the site is not. Because Slurm's own limit
enforcement counts every attempt regardless, the usage we decline to charge
has to be added back onto the Slurm limit, or we hold a project's jobs against
an allocation it was never given the chance to spend.

## 1. The decision

Requeues we cause are absorbed; requeues the user chose are charged.

The evidence for telling them apart was reviewed against the production logs:
there is **no unambiguous record of who issued a requeue**. `scontrol requeue`
leaves nothing in `slurmdbd` naming the uid, and `state.reason` on an
accounting record is the pending reason (`None`, `BeginTime`), not a cause. A
user checkpointing with `--requeue` plus a self-issued `scontrol requeue` is
indistinguishable in the accounting data from an administrator doing the same
thing.

What *is* reliable, and was confirmed against the logs, is the other
direction: **every fault the site causes is correctly classified** —
`NODE_FAIL`, `PREEMPTED`, `BOOT_FAIL` and the rest land in their own terminal
states and never in the bare `REQUEUED` bucket. So the rule is stated
negatively, which is the direction the data supports:

> Charge the `REQUEUED` bucket. Absorb everything else.

`terminal_state()` already makes this work, because `TERMINAL_STATES` is an
ordered precedence list and `NODE_FAIL` and `PREEMPTED` outrank `REQUEUED`
(`slurm/src/slurm.rs`). A record reporting both a node failure and a requeue
buckets as `NODE_FAIL` and is absorbed - which is the whole point of that
ordering, written before there was a policy to serve.

`OTHER` - the bucket for a state a future Slurm reports that we do not
recognise - is **not** charged. We do not bill for a cause we cannot name.

### 1.1 What this is worth

On the real month in `greatwestern/tests/data/project-usage-report.json`, of
12 requeue events and 5,547,234 discarded node-seconds:

| bucket | events | node-seconds | under this policy |
| --- | --- | --- | --- |
| `REQUEUED` | 8 | 4,366,354 (78.7%) | charged |
| `NODE_FAIL` | 4 | 1,180,880 (21.3%) | absorbed |

So the policy recovers about four fifths of what is currently discarded, and
the limit correction of §4 has to carry only the remaining fifth. That ratio is
the reason this is worth doing in this order: charging first makes the
compensating machinery much smaller than it would have been.

## 2. Where "which states are ours" lives

In exactly one place, because two copies of that list is how a report ends up
telling a project it absorbed usage the limit correction charged them for.

op-slurm gains a config option, read in `slurm/src/main.rs` alongside
`slurm-default-node` and the rest, and pushed into the cache at startup:

```toml
requeue-policy = "charge_requeue_state_only"
```

Spelled `requeue-policy` rather than `requeue_policy` to match every other
option in that file; the *values* keep their underscores because they are
policy names rather than keys.

| value | charged terminal states |
| --- | --- |
| `charge_requeue_state_only` (default) | `REQUEUED` |
| `no_charge` | none - today's behaviour |

The option resolves to one function, `RequeuePolicy::charges(state: &str) ->
bool`, and **every** decision - the report's charged/absorbed split, the limit
correction, the printed summaries - goes through it. An unrecognised value in
the config file is a startup error, not a silent fall back to the default: a
typo that quietly bills a site's node failures to its users is not a failure
mode worth having.

Two policies is enough for what is wanted now. The shape is an enum with a
state list behind it so that a third - charging `PREEMPTED` at a discount, say,
or absorbing `REQUEUED` for one partition - is a variant rather than a
redesign.

### 2.1 The default changes behaviour

`charge_requeue_state_only` as the default means an existing deployment that
upgrades without touching its config starts charging for user requeues. This
was chosen deliberately, over a default of `no_charge` that every site would
have to remember to override: the cost of an administrator forgetting is that a
site silently keeps under-billing, which is the bug this work exists to fix.

The agent logs the active policy at `info` on startup, whether or not it came
from the config file, so the change is visible in the log of any deployment
that upgrades.

## 3. Report schema: charged and absorbed are separate values

Today `total_usage()` is the base attempts alone and `requeue_*` holds every
superseded attempt. Let

- **B** = base usage (the last attempt in the window),
- **C** = charged requeue usage (superseded attempts whose state the policy
  charges),
- **A** = absorbed requeue usage (every other superseded attempt).

The new meanings:

| quantity | before | after |
| --- | --- | --- |
| `total_usage()` | B | **B + C** |
| `total_requeue_usage()`, `requeue_*` maps | C + A | **A** |
| `total_usage_including_requeues()` | B + C + A | B + C + A (unchanged) |
| `total_charged_requeue_usage()`, `charged_requeue_*` | — | **C** (new) |

So `requeue_*` keeps meaning *the usage we discarded*, and narrows to the part
we still discard. The new `charged_requeue_*` maps mirror the existing ones -
per user, per component, per state, events, wait - and are **informational**:
the usage they describe is already inside `reports`/`components`. That is what
makes "X% of requeue was charged, Y% was not" a fold over two maps that cannot
disagree with the totals they describe.

### 3.1 The consistency checks have to change with it

`requeues_are_consistent` (`greatwestern/src/usagereport.rs:1296`) asserts that
`requeue_states` sums to `num_requeue_events` and `requeue_state_usage` sums to
`total_requeue_usage()`. Those hold unchanged once both sides mean A. The new
maps need their own, and a different one:

- `charged_requeue_states` sums to `num_charged_requeue_events`, and
  `charged_requeue_state_usage` sums to `total_charged_requeue_usage()` -
  equalities, as for the absorbed maps;
- `total_charged_requeue_usage() <= total_usage()` - a **bound**, not an
  equality, because C is a subset of what `reports` already holds. This is the
  same shape as the existing reservation check, and for the same reason.
- No charged state may be one the policy does not charge. Cheap, and it is the
  check that catches a report built under one policy being merged with one built
  under another.

Everything stays `#[serde(default)]` and every check stays conditional on the
map being populated, so a legacy report with empty charged maps still passes.

### 3.2 Wire compatibility, and why it is fine

An older peer reading a new report sums the maps it knows: it gets B + C as
usage - which is what we now intend to charge - and A as discarded, and derives
B + C + A as the true total. It is *correct*, it simply cannot see that part of
the usage came from requeues. A newer agent reading an older report sees empty
charged maps and a `requeue_*` that still means C + A; its derived true total is
still right, only the split differs.

Neither case needs handling beyond what `serde(default)` already does, because
of §7: the switch lands on a month boundary with the previous month closed, so
the two conventions never have to be summed together in an invoice.

## 4. The limit, and why it needs a correction

`set_limit` writes `GrpTRESMins` on the account, and Slurm enforces it against
its own accumulated usage - which counts every attempt, including the ones we
have just decided to absorb. So after this change:

- Slurm's counter climbs by B + C + A;
- the portal's picture of the project climbs by B + C;
- the project is held at the limit having spent, as far as the portal is
  concerned, only part of it.

The gap is exactly **A**, the absorbed requeue usage, and that is the only
quantity the correction carries. C needs no correction at all now that it is in
the reported usage - which is why §1.1's ratio matters.

The compensation:

```
applied = requested + correction          (written to GrpTRESMins)
requested                                  (returned by get_limit)
correction = A for the current month
```

### 4.1 Three values, not two

The single most important structural point, and the one that makes the
straightforward implementation wrong. `get_limit` today compares the Slurm
limit against the cached `account.limit()` and, when they differ, **adopts
Slurm's value into the cache** (`slurm/src/sacctmgr.rs`, the
`actual_slurm_limit` block). Under this design Slurm's value is deliberately
`requested + correction`, so an unmodified `get_limit` would take the inflated
figure as the new requested limit, and the next correction would be added on
top of it. The limit would ratchet upward by A on every cycle.

So op-slurm must hold **requested**, **applied** and **correction** as three
distinct values per account, and the reconciliation must compare Slurm against
`applied`, never against `requested`.

It must also *re-apply* rather than adopt. op-slurm has full authority over
these accounts; nobody sets them by hand. A `GrpTRESMins` that disagrees with
`applied` is therefore something that changed behind our back, and the response
is to log it at `error` and write `applied` back, not to believe it. This is a
behaviour change in `get_limit` - today it believes Slurm - and it is
deliberate.

### 4.2 Recovering `requested` after a restart

The cache is in-memory only, and `SlurmAccount::construct` sets `limit:
Usage::default()` - the limit is never read from the account itself, only from
the association query in `get_limit`. So after a restart the only thing
observable is `applied`, and decomposing it needs the correction.

That resolves itself, because the correction is **recomputed from Slurm's own
records**, not recovered from cache: once a usage report for the current month
has run, A is known, and `requested = applied - A`. Usage reports run for every
project roughly every ten minutes, so the window is short.

During that window `get_limit` does not know `requested`. It says so - returns
the applied figure with a warning that the correction is not yet known - rather
than returning a figure that is wrong by A with no indication. See §5.1 for the
more important half of this rule.

## 5. The rules the correction obeys

**5.1 Unknown is not zero.** An absent correction means "not yet computed",
never "zero". The dangerous case is not the start of a month - there the
correction genuinely is zero - but a restart mid-month, where treating unknown
as zero would push `requested + 0` and silently withdraw headroom the project
is already relying on. So: **the applier** never raises a limit on a correction
it has not computed, and never lowers one at all.

A `set_limit` is different, and §11 records why: it is an instruction rather
than a guess, so it is honoured at once with whatever correction is known, and
the applier adds the rest within the hour.

**5.2 The correction only ever increases, within a month.** The base/requeue
split is window-local by design (`slurm-requeue-accounting-design.md` §3, §5.2)
- a record can be `Base` in one window and `Requeued` in the next - so a
recomputed month can come out *lower* than the one before it. Acting on that
would lower the Slurm limit, which holds a project's jobs for a reclassification
rather than for anything it did. The applied correction therefore ratchets
upward and never down, and a computed value below the applied one is logged at
`warn` with both figures. The cost is a small over-grant that the next month's
reset clears.

**5.3 Zero stays zero.** When the caller sets a limit of zero it is stopping the
project - it has overspent, and the portal has decided. Adding a correction to
zero would hand back an allowance precisely when the intent was to withdraw it.
A `requested` of zero applies as zero, with no correction, under every policy.

**5.4 No limit means no limit.** An account with `GrpTRESMins` unset is
unlimited. A correction must never *create* a limit where there was none: with
no limit to correct, every path here is a no-op.

**5.5 The arithmetic saturates.** `requested + correction` is saturating
addition, and the minute conversion is done once and rounded up rather than
recomputed and re-truncated on every pass - limits round-trip through minutes
(`set_limit` truncates `(node.cpus() as f64 * limit.minutes()) as u64`;
`SlurmLimit::construct` reads back `count * 60`), so a value recomputed
repeatedly drifts downward a minute per TRES at a time. Release builds are
`panic = "abort"` with `overflow-checks = true`: an overflow here is a remote
process kill, and there are no panics in this path.

## 6. Who writes, and when

A usage report must not write to the cluster. The reporting path therefore
**only updates the cached correction** for the project and month it just
computed; nothing in `get_usage_report` touches `sacctmgr`.

A background task, spawned at startup in the manner of
`templemeads::systeminfo::spawn_monitor`, wakes **hourly**, walks the accounts
whose correction has changed since it last looked, and applies the difference.
An hour is ample: the site's Slurm policy *holds* over-spending jobs rather than
killing them, so the worst case of a late correction is a job held slightly
longer, or one that starts with insufficient credit and overspends a little.
Neither is worth a tighter loop against `slurmctld`.

The applier takes the project's existing mutex (`cache.project_mutexes`) and
writes only when `requested + correction` actually differs from `applied`, so a
sweep over every project is not a burst of `sacctmgr modify` calls. It carries
its own expiry rather than inheriting a job's.

## 7. The month boundary belongs to the caller

op-slurm does not know when the accounting month turns over, and this design
does not teach it. The caller calculates the limit monthly, after the previous
month's usage has been received and invoiced, and sets it as the starting credit
for the new month.

All op-slurm has to guarantee is that **the correction covers only the current
calendar month's jobs**. Then the caller's reset lands on a correction that is
starting from zero again, and the two stay in step without either knowing the
other's schedule. There is some leakage across the boundary - a job spanning
midnight on the 1st - and it corrects itself within the month; a little usage
beyond the limit is acceptable by policy.

## 8. Testing

The arithmetic is the easy half and the state machine is the hard one.

On the report:

- `total_usage()` over a fixture equals base plus exactly the charged states,
  under `charge_requeue_state_only`, and equals the old base figure under
  `no_charge` - the direct test that the policy switch is the only thing that
  moves it;
- `B + C + A` equals the sum over every record, under both policies - the
  invariant that must not depend on the policy at all;
- a record that is both `NODE_FAIL` and `REQUEUED` is absorbed, from the
  fixture that already has one;
- `OTHER` is absorbed;
- splitting a month into days and summing them back reproduces both splits, as
  the existing month fixture already checks for the totals.

On the limit, each as its own case because each is a way to lose a project's
allocation:

- a correction is applied once and does not compound over repeated
  `get_limit`/report cycles - the §4.1 ratchet, which is the regression test
  this whole design turns on;
- `requested` survives a `get_limit` that sees an inflated Slurm limit;
- a drifted `GrpTRESMins` is re-applied, not adopted;
- an unknown correction never lowers an applied limit;
- a computed correction below the applied one does not lower it, and warns;
- a `requested` of zero applies as zero;
- an account with no limit stays without one;
- `u64::MAX` in either term does not panic.

## 9. Rollout

The switch is thrown at the start of an accounting month, with the previous
month closed and invoiced, so no invoice ever spans the two conventions. The
cache is in-memory, so the restart that picks up the new default also clears
every daily report computed under the old one - there is no mixed-convention
state to migrate, provided the restart happens at the boundary rather than
mid-month.

This is the end of the migration window that
`slurm-requeue-accounting-design.md` §8 asked for: `total_usage()` stops being
the figure we have always reported and becomes the figure we intend to charge.
Affected projects' bills rise at that moment, by up to the charged share of
their requeue usage - four fifths of the discarded total, on the sample in
§1.1. That is the correction the whole exercise was for, but it should be
communicated before it appears on an invoice rather than after.

## 10. Deliberately not doing

- **Attributing a requeue to the uid that issued it.** The data does not
  support it (§1), and harvesting `slurmctld` logs to recover it is a different
  piece of work with a different lifetime - the logs rotate long before an
  invoice is disputed.
- **Reconciling against Slurm's `GrpTRESMins` usage counter.** Rejected in
  `slurm-requeue-accounting-design.md` §9.4 and still rejected: it puts the
  portal's monthly-reset policy inside op-slurm. The correction here is not
  that - it is a mechanical consequence of *our own* accounting choice, made in
  the only place that knows the choice was made.
- **Per-partition or per-QoS policies.** The enum admits them; nothing asks for
  them.

## 11. Deviations from the design as built

- **`set_limit` writes immediately, with whatever correction is known.** §5.1
  first said the applied figure should be left alone until the correction was
  known. That is wrong for the case that matters most: the caller zeroes a
  limit when a project overspends, and holding that back would leave the
  project running on an allowance it has already exhausted. A `set_limit` is an
  instruction, not an inference, so it is applied at once - with the correction
  if one is known, without it if not, and the hourly applier makes up the
  difference. The rule it was protecting still holds where it belongs: the
  applier only ever raises.

- **`get_limit` recovers the requested limit rather than giving up on it.**
  §4.2 described the window after a restart as one where `get_limit` cannot say
  what was requested. It can, once a usage report has run:
  `requested = observed - applied correction`, which it records. Only before
  that does it return the Slurm figure with a warning.

- **"Slurm holds no limit" means no association row *or* an association with no
  `GrpTRESMins`.** The second is the case that actually occurs, and
  `SlurmLimit::has_any_limit` is what distinguishes it. Both are unlimited, and
  the applier leaves both alone.

- **The charged-state check lives in op-slurm, not in the report.** §3.1 listed
  "no charged state may be one the policy does not charge" with the report's own
  consistency checks. It cannot live there: a report travels between agents, and
  the policy belongs to the agent reading it rather than to the report. It is
  checked in `check_counter_consistency`, which is where the policy is in scope.

- **A charged requeue still counts towards a reservation's discarded share.**
  `reservation_requeue_usage` records what a reservation's occupancy owed to
  requeued attempts, which is a question about occupancy rather than about
  charging, so both kinds belong in it.
