<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# Slurm requeue accounting: counting the attempts we never saw

Status: **proposed**. Nothing below is implemented. The measurement in §1 has
been done on one production account-day; the field-availability question in
§2.4 is still open.

## 1. The problem

`op-slurm` builds its usage reports from `sacct`, and calls it without
`--duplicates`. Default `sacct` returns **one record per JobID** - the most
recent one. A requeued job has one record per attempt in `slurmdbd`, each
carrying only its own `elapsed`; the final record does not roll up the earlier
attempts. So every attempt before the last is invisible to us, and the node
seconds it consumed are absent from every figure we report.

This is not a rounding error. On a production account measured over a single
day, adding `--duplicates` raised the summed `elapsed` by half again - about a
third of that account's real consumption had never been reported. The pattern
that causes it is a long job near its wall-clock limit being requeued: each
lost record is worth many times the mean record, so a handful of missing rows
moves the total enormously. It appears to affect a few percent of projects,
and to affect them badly rather than marginally. It was noticed because a
project believed it had considerably more allocation remaining than Slurm
thought it did.

There is a worse case than under-reporting. Consider a job whose earlier
attempt ran for hours and was requeued, and whose final attempt was cancelled
or is still pending, with `elapsed` of zero. Default `sacct` hands us only that
final record; `SlurmJob::get_consumers` (`slurm/src/slurm.rs`) drops
zero-duration records; and so we record **no usage at all** for a job that
consumed a large amount of real resource. In the measured sample this applied
to a quarter of the requeued jobs.

There is also an existing inconsistency worth fixing while we are here.
`sacct`'s de-duplication applies to the records matching the query, so it
depends on the query window. `get_daily_report` asks for a day; when that
query times out we fall back to `get_hourly_report`, which asks 24 times for
an hour. Two attempts of one job in the same day but different hours are
de-duplicated by the daily query and both retained by the hourly one. Today's
reported usage for a day therefore depends on whether a `sacct` call happened
to time out, which is not reproducible.

## 2. What `sacct --duplicates` actually gives us

### 2.1 One record per attempt

Each record is an independent accounting row: its own `start`, `end`,
`eligible`, `elapsed`, `state` and TRES allocation. Summing all records of a
job gives the job's true total consumption, with no double counting - the
attempts are disjoint in time. Our existing usage accumulation in
`sacctmgr.rs` sums unconditionally over every returned record, so **the usage
arithmetic needs no change at all**; the records simply arrive where before
they did not.

### 2.2 `restart_cnt` is an ordering key, not a flag

Records carry `restart_cnt`, which increments with each requeue. It is
tempting to treat `restart_cnt == 0` as "the original attempt", but this is
wrong: `restart_cnt` counts from the job's own beginning, not from the query
window, so a job whose first attempts fell before the window returns records
whose lowest `restart_cnt` is greater than zero. Observed in practice.

What `restart_cnt` is good for is **ordering the attempts returned within one
query**. `max(restart_cnt)` over a job's returned records identifies the last
attempt in that window, which is exactly the record default `sacct` would have
handed us. That equivalence is the foundation of §3.

### 2.3 `state.current` is a list, and we discard most of it

A requeued attempt reports `state.current` as something like
`["PENDING", "REQUEUED"]`. `SlurmJob::construct` handles an array by taking
`.first()`, so we currently record such an attempt as `PENDING` and throw the
`REQUEUED` away. Other terminal states seen on requeued attempts include
`NODE_FAIL`, `PREEMPTED`, `FAILED`, `CANCELLED` and `COMPLETED`. We need the
whole set, both for §6 and because "PENDING" is an actively misleading thing
to have recorded about an attempt that ran for hours.

### 2.4 Open: is there a per-record submit time?

`.time.submit` came back null in the sample, so it is either absent from this
Slurm version's JSON or unpopulated on this path. We do not need it for
ordering - `restart_cnt` serves - but without some submit time or database
index we cannot distinguish two unrelated jobs that share a JobID after a
`slurmctld` job-id reset. That is rare and time-window-bounded, but it is the
one failure mode that silently corrupts the split rather than failing loudly.
Resolve before implementation: if the field exists under another name, or
`JobIDRaw` gives a usable per-record key, group on it as well as `job_id`.

## 3. The contract: a base figure and a requeue figure

Which figure a project should be charged is a policy question, and not one
this change should pre-empt. A requeued attempt is not simply wasted: a job
that checkpoints does real work on every attempt, which is precisely why its
final attempt is short. Equally, an attempt killed by a node failure is not
the user's fault. The change therefore reports **both** figures and leaves the
choice to whoever consumes the report.

Every returned record is classified into exactly one of two buckets:

- **base** - the record with the highest `restart_cnt` among that job's
  records returned by this query.
- **requeue** - every other record for that job.

Three properties follow, and all three should be documented for consumers:

1. **`base + requeue` is the true total, and is window-independent.** Every
   record lands in exactly one bucket, so the sum over any set of days is
   exact. This is the figure that is *correct*.
2. **`base` alone is what we report today, unchanged.** Default `sacct`'s
   de-duplication keeps the latest attempt, and `max(restart_cnt)` selects the
   same record. Existing consumers see the number they have always seen. This
   is the figure that is *continuous*.
3. **The split between the two buckets is window-local**, and this is the
   honest caveat. A job requeued across midnight has its earlier attempt
   classified as `base` by the earlier day's query - it is that job's last
   attempt *within that window* - and its later attempt classified as `base`
   by the following day's query. Summed over a range, `base` therefore
   double-counts such a job. That is not a new defect: it is exactly what we
   report today, for exactly this reason. Only the total is exact.

### 3.1 Why not a job-global "final attempt"

Defining `base` as the attempt that ended the job would remove the window
dependence of the split, and was rejected for two reasons.

It cannot be computed. A windowed query returns only the attempts overlapping
the window; the job's real final attempt may lie outside it.

Worse, it is not stable. A job whose last returned record is still `PENDING`
will run again, so a classification made today would need retroactive
revision tomorrow. Daily reports are cached and marked complete - `set_report`
in `slurm/src/cache.rs` refuses to cache an incomplete report - so a figure
that can change after the day closes has nowhere to live.

Note also that "the attempt that ran to completion" is frequently not a thing
that exists: in the measured sample, most requeued jobs ended in a failure
state rather than `COMPLETED`, and in several the last attempt was a short
failure following a long interrupted run. The window-local rule is defended by
its continuity with today's reporting, not by a claim about which attempt
mattered.

## 4. Report schema

The requeue figures are **not** added to the existing `components` map. That
map is a set of dimensions of one run (`cpu`, `memory`, `gpu`, `billing`),
whereas this is a partition of the record set, and three concrete things break
if they are conflated:

- `ProjectUsageReport::scale_total` deliberately scales `reports` and leaves
  `components` alone, because the main total may be in credits while the
  components are in physical units. A requeue value living in `components`
  would end up in different units from the total it is meant to be subtracted
  from. `Mul`/`Div` scale both, so the two paths would disagree.
- `components()` is surfaced to clients as a bare string list (via
  `python/src/lib.rs`), and a consumer rendering a per-resource breakdown, or
  summing across components, would silently absorb a non-resource entry.
- Requeue usage needs its *own* per-resource breakdown - requeued GPU seconds
  are a more interesting number than requeued node seconds - which a single
  extra component cannot express.

So `DailyProjectUsageReport` (`greatwestern/src/usagereport.rs`) gains a
parallel set of fields, mirroring the shape of what is already there. All are
`#[serde(default)]`, exactly as `user_job_counts` and `num_jobs` already are,
so an older peer's JSON and any cached report deserialise unchanged and an
older peer ignores what it does not know:

```rust
/// Usage from attempts superseded by a requeue, per user.
#[serde(default)]
requeue_reports: HashMap<String, Usage>,
/// The same, broken down by resource component.
#[serde(default)]
requeue_components: HashMap<String, HashMap<String, Usage>>,
/// Scalar shadow total - equals the sum of `requeue_reports`.
#[serde(default)]
requeue_usage: Usage,

/// Number of requeue *events* - an attempt superseded by a later one.
#[serde(default)]
num_requeue_events: u64,
#[serde(default)]
user_requeue_events: HashMap<String, u64>,
/// Queue wait accumulated by requeued attempts (eligible -> start).
#[serde(default)]
requeue_wait_seconds: u64,
#[serde(default)]
user_requeue_wait_seconds: HashMap<String, u64>,

/// Requeue events by terminal state - see §6.
#[serde(default)]
requeue_states: HashMap<String, u64>,
/// Usage by terminal state of the superseded attempt - see §6.
#[serde(default)]
requeue_state_usage: HashMap<String, Usage>,
```

`scale_total` must scale `requeue_reports` and `requeue_usage` alongside
`reports`, or the subtraction a consumer wants to perform is between two
different units. `Mul`, `Div`, `MulAssign`, `DivAssign`, the `+=` merge paths
and `get_component` all need the same treatment - anywhere the existing maps
are walked, the new ones must be walked too.

### 4.1 Counting semantics

- `num_jobs` keeps its meaning: distinct jobs whose `base` record started in
  the window. Since `base` is the record default `sacct` would have returned,
  this figure is unchanged from today.
- `num_requeue_events` counts *events*, not jobs. A job requeued four times
  contributes four. This is deliberate: "how many distinct jobs were affected"
  needs cross-window grouping and reintroduces the problem of §3.1, whereas an
  event count is additive over any range. Name it so nobody reads it as a job
  count.
- `total_wait_seconds` keeps its meaning - the queue wait of `base` records
  only - so the existing mean-wait figure is untouched.

Three wait figures then fall out for consumers:

| figure | expression |
| --- | --- |
| mean wait per job, excluding requeues (today's) | `total_wait_seconds / num_jobs` |
| mean wait per requeue | `requeue_wait_seconds / num_requeue_events` |
| mean total wait per job, including requeues | `(total_wait_seconds + requeue_wait_seconds) / num_jobs` |

One caveat for whoever reads the middle row: Slurm imposes a begin-time hold
after a requeue, so requeue wait includes an enforced delay and is not a
measure of contention.

## 5. Where the code changes

### 5.1 Both `sacct` call sites gain `--duplicates`

`get_hourly_report` and `get_daily_report` in `slurm/src/sacctmgr.rs` build
the same argument list. `--allocations` stays - it keeps steps out, which is
unrelated to and unaffected by this change.

The daily query will return more rows and is more likely to hit its 20-second
timeout, pushing more days onto the hourly path. That path is correct and
already exercised, so this is a cost rather than a risk, but it is worth
watching after deployment on accounts with heavy preemption.

### 5.2 `get_consumers` classifies before it filters

`SlurmJob` gains `restart_cnt` and a classification - an enum
(`Attempt::Base` / `Attempt::Requeued`) is clearer than a bool. The cache
holds `SlurmJob` in memory only, so adding fields costs nothing.

The order of operations in `get_consumers` is load-bearing and easy to get
wrong:

1. Construct every record, with no filtering.
2. Group by `job_id` (and any disambiguating key from §2.4) and mark the
   highest `restart_cnt` in each group as `Base`, the rest as `Requeued`.
3. Only then clip `start_time`/`end_time` to the query window.
4. Only then drop zero-duration records.

Classifying after step 4 silently changes the legacy figure. Take the case
from §1, where the last attempt has zero elapsed: drop it first and the
*earlier* attempt becomes the highest surviving `restart_cnt`, gets classified
as `Base`, and hours of previously-unreported usage appear in the figure that
was supposed to stay continuous - for precisely the jobs where continuity
matters most.

A record spanning a window boundary may be `Base` in one window and
`Requeued` in the next, if a later attempt appears alongside it there. Its
portions are then counted in different buckets on different days. This is
harmless - the total is unaffected - but it should not come as a surprise when
reading the numbers.

### 5.3 State parsing

`SlurmJob::construct` should keep the whole `state.current` set rather than
`.first()`. Serialising `state` as a `Vec<String>` changes the cached
representation, which is fine in memory but note that `state()` has callers.

### 5.4 Consistency checks

Both report paths in `sacctmgr.rs` maintain local shadow counters and warn
when they disagree with the report's own totals, and
`DailyProjectUsageReport::is_consistent` cross-checks the per-user maps
against the scalars. The new counters need the same treatment, or they become
the one part of the report with no runtime cross-check.

## 6. Requeue events by terminal state

With the full state set available, counting requeue events - and their usage -
by the terminal state of the superseded attempt is nearly free, and answers a
question the single requeue total cannot. `NODE_FAIL` time is a site problem;
`PREEMPTED` time is a policy the project opted into; `CANCELLED` may be the
user's own doing. Those are different conversations with different answers,
and the flat requeue figure lumps them together.

Both maps are keyed by a normalised terminal-state string. The key must be
derived deterministically from the state set (a requeued attempt reports both
a current state and `REQUEUED`), and unrecognised states must map to a
catch-all rather than being dropped, so the state maps always sum to
`num_requeue_events` and `requeue_usage` respectively - which the consistency
check in §5.4 should assert.

## 7. Testing

`get_consumers` has no test coverage at all today, which is the main reason
this change needs fixtures rather than unit tests over hand-built structs.
The plan is to capture real `sacct --json --duplicates` output, anonymise it,
and commit it.

Cases the fixture set should cover:

- a job requeued once, both attempts within the window;
- a job requeued more than once;
- a job whose lowest returned `restart_cnt` is greater than zero, i.e. earlier
  attempts fell outside the window (§2.2);
- a job whose final attempt has zero elapsed, so the whole job's usage is in
  the requeue bucket and the base figure is zero (§5.2);
- attempts with each terminal state we intend to key on (§6);
- an attempt spanning the window boundary, to pin the clipping arithmetic;
- a job array and a heterogeneous job, neither of which is covered today.

Assertions worth making explicit, since they are the contract of §3:
`base + requeue` equals the sum over all records; `base` alone equals what the
same fixture yields with the duplicate records removed (a direct test of
continuity); each job contributes exactly one to `num_jobs`; the state maps sum
to the flat requeue totals.

**Anonymisation, one trap.** `nodes` is a string looked up in `SlurmNodes`,
and a miss falls back silently to a default node - which changes
`node_fraction` and therefore every usage number derived from it. Rewritten
node names need a consistent mapping plus a matching `SlurmNodes` fixture, or
the expected values will be quietly wrong in a way that still looks
plausible. Fields that must survive anonymisation intact: `job_id`,
`restart_cnt`, all of `time`, `state`, `tres.allocated`, `tres.requested`,
`qos`, `cluster`. `user` and `account` can be renamed freely.

## 8. Compatibility and rollout

Nothing on the wire breaks. Every new field is `#[serde(default)]`, so an
older peer's report deserialises with zeroes and an older peer ignores the new
fields in ours. The cache is in-memory only (`Lazy<RwLock<Database>>`), so
there is no persisted state to migrate - a restart is sufficient, though note
that a long-running process holding days already marked complete will keep the
old values for those days until it restarts.

The figure a consumer sees for `reports` does not change, by construction.
That is the point of §3: no flag day, and the policy decision about what to
charge can be taken on evidence, afterwards, with both numbers in hand.

That said, the base figure is now known to understate real consumption
substantially on affected projects. Offering it indefinitely means offering a
number we know to be wrong. It should be presented as a migration window with
an end, not a standing choice.

## 9. Open questions

1. §2.4 - is there a per-record submit time or database index available, and
   should we group on it to survive a job-id reset?
2. Should `requeue_states` key on the raw Slurm state string, or on a smaller
   normalised set (`node_fail` / `preempted` / `requeued` / `cancelled` /
   `other`)? The latter is friendlier to consumers and hides Slurm version
   differences; the former loses nothing.
3. Do we want a distinct-jobs-requeued count as well as the event count?
   Deferred rather than rejected - it needs the cross-window grouping of §3.1
   to be meaningful.
4. Is the wait time of a requeued attempt worth attributing per user, or is
   the project-level total enough? The schema in §4 includes the per-user map
   for symmetry; it may not be worth the width.
