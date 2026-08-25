<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# Slurm requeue accounting: counting the attempts we never saw

Status: **implemented**, as described below, with the deviations noted in §10.
The measurement in §1 was done on one production account-day, and the JSON
fields the design relies on are confirmed present on the production Slurm
(§2.4).

The contract in §3 is the part that matters: `total_usage()` is byte-for-byte
what it always was, the previously invisible consumption is carried alongside it
in new `serde(default)` fields, and nothing deployed has to change to keep
working. The charging policy can then be settled on evidence, rather than having
to be settled before the measurement could be fixed.

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

### 1.1 This is not only a billing question

`set_limit` configures each account's `GrpTRESMins` through `sacctmgr`, and
Slurm enforces that limit against *its own* accumulated usage - which counts
every requeued attempt, because `slurmdbd` records them all. Our reports do
not. So the two disagree, and Slurm is the one that is right.

The visible symptom is an attempt that fails with reason
`AssocGrpCPUMinutesLimit`: Slurm killed the job because the account had
exhausted a limit which, according to the figures we were reporting, was
nowhere near exhausted. This is precisely the confusion that prompted the
investigation - a project believing it had allocation remaining while Slurm
disagreed.

Fixing the measurement therefore reconciles our reporting with the enforcement
mechanism we ourselves configure. That is a correctness argument for the
change, independent of the policy question in §3 about what should be charged.

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

### 2.4 There is a per-record submit time: `time.submission`

Confirmed on the production Slurm. The field is `time.submission`, not
`time.submit`, and it is populated on every record - the earlier null was my
jq asking for a key that does not exist. `submission` plus `job_id` is
`slurmdbd`'s own primary key for a job record, which resolves the job-id-reset
concern outright: grouping on `(job_id, submission)` cannot conflate two
unrelated jobs that happen to share a JobID, because their submit times
differ.

It also gives an independent ordering of attempts. On a sampled requeued job
the ordering by `submission` and by `restart_cnt` agree, and a requeued
attempt's `submission` equals the previous attempt's `end` - Slurm resubmits
at the moment of the requeue. The design uses `submission` as the ordering key
and treats disagreement with `restart_cnt` ordering as a condition to log,
since `submission` is the database key while `restart_cnt` is a counter
maintained alongside it.

Two other fields in the record are worth knowing about: `failed_node` (see §6)
and `array`/`het`, which confirm the fixture cases in §7 are obtainable.

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

There is deliberately no stored scalar for the requeue usage total.
`total_usage()` is computed from its map rather than stored, and
`total_requeue_usage()` mirrors it - a shadow total that cannot drift beats one
that has to be checked.

`scale_total` must scale `requeue_reports` alongside
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
- **An event is counted for every superseded record, with no condition on where
  the attempt started.** This is the opposite of the job count above, and
  getting it wrong is the one mistake this design has already made in
  production - see §4.2.
- `total_wait_seconds` keeps its meaning - the queue wait of `base` records
  only - so the existing mean-wait figure is untouched.

Three wait figures then fall out for consumers:

| figure | expression |
| --- | --- |
| mean wait per job, excluding requeues (today's) | `total_wait_seconds / num_jobs` |
| mean wait per requeue | `requeue_wait_seconds / num_requeue_events` |
| mean total wait per job, including requeues | `(total_wait_seconds + requeue_wait_seconds) / num_jobs` |

A note on the middle row. Slurm imposes
a begin-time hold after a requeue, but it advances `eligible` past that hold,
and `wait_time()` measures `eligible -> start`. So the hold is *excluded* from
requeue wait, and the figure is genuine time spent waiting to be scheduled.
The hold itself - `submission -> eligible`, a couple of minutes on the sampled
job - is not captured by any field in this design. That seems right: it is a
policy delay rather than contention, and conflating the two would make the
requeue wait figure less useful, not more.

### 4.2 Why a requeue event needs no window guard, and why one broke it

A job's count needs a window guard. A record that is still running when the
window closes is returned again for the next window, so without
`started_in_window` a long job would be counted once per window it touched.

A requeue event needs the opposite treatment, and the first version of this
design applied the guard to both. The result was a count of 1 where there were
several, on the same data whose requeue *usage* was correct - the figures
disagreed with each other because they were being gated on different things.

A superseded record is classified `Requeued` in **at most one** window, so
counting every one of them counts each event exactly once:

- a record is only returned for windows it overlaps, so it can be seen at all
  only up to the window holding its end;
- it is only classified `Requeued` when a later attempt is in the same
  response, and a later attempt cannot start before this one ended - so the
  window must also reach the successor's start, at or after this record's end;
- the two conditions meet in exactly one window: the one holding the end, which
  is the instant of the requeue.

Requiring the record to have started in that window as well asked for something
almost no real requeue can satisfy. The attempts that get requeued are the long
ones - a job near its wall-clock limit - so the requeue lands on the day *after*
the attempt began and the two conditions cannot both hold. The usage was right
throughout, because usage is attributed per window to the part consumed in it;
only the event count fell in the gap.

The one case still missed is a requeue within seconds of a window boundary,
where the successor is submitted on the far side and the two records never
appear in one response. The count is a lower bound to that extent, and never
counts anything twice.

This is also why `wait_time()` measures from the *unclipped* start. How long an
attempt queued is a property of the attempt, not of the window it is reported
in. With the guard in place the distinction never showed - a counted attempt had
started inside the window, so its clipped and real starts were identical - but a
requeue is counted in the window where it happened, which is not where the
attempt began, and the clipped start would have reported it as having waited
until the window opened.

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

`SlurmJob` gains `submission`, `restart_cnt` and a classification - an enum
(`Attempt::Base` / `Attempt::Requeued`) is clearer than a bool. The cache
holds `SlurmJob` in memory only, so adding fields costs nothing.

The order of operations in `get_consumers` is load-bearing and easy to get
wrong:

1. Construct every record, with no filtering.
2. Group by `job_id`, keyed within the group on `time.submission` (§2.4), and
   mark the attempt with the latest `submission` as `Base`, the rest as
   `Requeued`.
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

The record's `failed_node` field is worth capturing alongside this. For a
`NODE_FAIL` attempt it names the node responsible, which turns "the site lost
this work" into something actionable - correlating repeated failures against
particular nodes, and evidencing the case when a project disputes a charge.
It does not belong in the usage report itself; logging it at `info` from the
slurm agent is enough.

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
- a job array and a heterogeneous job, neither of which is covered today;
- **an attempt spanning midnight whose replacement never runs** - the shape
  almost every real requeue has, and the one that caught §4.2. It has to be
  checked over two consecutive windows, with the records filtered by overlap as
  `sacct` would filter them, or the case disappears: the whole point is that
  day one cannot see the successor and day two cannot see the start.

Assertions worth making explicit, since they are the contract of §3:
`base + requeue` equals the sum over all records; `base` alone equals what the
same fixture yields with the duplicate records removed (a direct test of
continuity); each job contributes exactly one to `num_jobs`; the state maps sum
to the flat requeue totals.

A test that hands `get_consumers` every record in the fixture is not testing
much. Whether a superseded attempt can be *recognised* as one depends entirely
on which of a job's other attempts the query returned, so the tests filter the
fixture by overlap with the window first, exactly as `--starttime`/`--endtime`
would.

### 7.5 A real month, as a fixture

Synthetic fixtures can only check that the arithmetic agrees with itself.
`greatwestern/tests/data/project-usage-report.json` is a real month of one
project's usage as `op-slurm` produced it, with the project and usernames
anonymised and nothing else touched: 24 days, 357 jobs, eight users of very
different habits, requeues in both states, an `interactive` reservation on some
days and not others, and one day still incomplete.

What it is worth testing against, beyond the totals:

- **Every day is internally consistent** - the per-user maps sum to the scalars,
  the per-state maps account for every requeue event and second, no reservation
  claims more than the day consumed. Real data satisfying the invariants is
  worth more than any number of hand-built cases that were written to.
- **Splitting the month into days and summing them back** reproduces every
  total, every component breakdown and every derived ratio. This is not an
  abstract property: it is exactly what `op-slurm` does to build a month.
- **A date range and its complement partition every total.**
- **The project mean is not the mean of the daily means** - on this month the
  two expansion figures differ by more than two hundred, which is the case for
  computing a project figure over every job rather than over every day.
- **Scaling a month agrees with scaling its days**, to the second. Not because
  `Usage` does not truncate - it does, and thirty-two seconds vanish from this
  month when it is halved - but because usage is only ever stored per user per
  day, so both paths scale the same stored values and nothing coarser exists to
  lose a fraction of. That is what makes a monthly invoice reconcilable against
  a daily breakdown, and it is worth a test because it would stop being true the
  moment a total were stored rather than derived.

Two things that are *not* properties, both found by asserting them and being
wrong: re-serialising a report is not byte-identical, because these are
`HashMap`s and serde emits their keys in whatever order the map iterates - the
document is stable, the bytes are not; and halving a total then doubling it does
not recover it, for the truncation reason above.

**Anonymisation, one trap.** `nodes` is a string looked up in `SlurmNodes`,
and a miss falls back silently to a default node - which changes
`node_fraction` and therefore every usage number derived from it. Rewritten
node names need a consistent mapping plus a matching `SlurmNodes` fixture, or
the expected values will be quietly wrong in a way that still looks
plausible. Fields that must survive anonymisation intact: `job_id`,
`restart_cnt`, all of `time`, `state`, `tres.allocated`, `tres.requested`,
`qos`, `cluster`. `user` and `account` can be renamed freely.

## 7.1 Reporting it to a human

The figures are only useful if someone can read them, and the first thing asked
of them in practice was "how many requeues, on which day, and what interrupted
them" - which the flat totals could not answer at a glance.

`ProjectUsageReport::requeue_report()` (and the same on `UsageReport`, for every
project that has any) returns a plain-text summary: the reported, discarded and
true totals with the discarded share as a percentage, the event count and the
queue wait it threw away, then breakdowns by interrupting state, by day and by
user. Everything in it is in hours - `Usage`'s own formatting rescales itself
per value, which is right for a single figure and unreadable in a column.

Two smaller things belong with it. The per-day printout of a project report now
carries its own requeue line: the daily line existed on
`DailyProjectUsageReport`'s own `Display`, but a project report renders its days
itself, so it never appeared where anyone was reading it. And a day whose
consumption was entirely discarded by requeues is no longer skipped: the daily
listing dropped any day whose `total_usage()` was zero, which is exactly what
such a day has - none of the usage we *report* - while still having plenty to
say.

## 7.2 Reservations, and the half of utilisation we cannot supply

Job records carry the reservation a job ran under - `reservation` in the same
`sacct` response requeue accounting already fetches, so capturing it costs
nothing - and reports now record usage and job counts per reservation, with a
`reservation_report()` dump alongside the requeue one.

Three decisions worth writing down.

**Reports key on the reservation's name, not its id.** Slurm reports the
reservation as `{"id": N, "name": "..."}` - a job outside one has id 0 and an
empty name - and it gives every *instance* of a reservation its own id. A
recurring or on-demand reservation is therefore one name across many ids: a
single production account-day showed `interactive` under seventeen of them.
Keying on the name merges the instances, which is both the figure anyone asking
about a named reservation means and the only one whose key space stays bounded -
keying on the id would mint a fresh key every time a reservation was recreated,
in a map that travels between agents.

The consequence to be aware of when reading the output: a name like
`interactive`, if a site creates one per user on demand, is a family of
short-lived reservations rather than a block of capacity someone booked, and its
row means something quite different from a row for a named benchmarking or
maintenance window. The reports cannot tell the two apart, because nothing in a
job record can. Per-instance detail belongs with the reservation metadata a
future `add_reservation` instruction would carry.

**Reservation figures count every attempt, superseded ones included.** A
requeued attempt held the reservation's nodes exactly as its replacement did,
and for occupancy that is the whole point. This makes reservation usage a subset
of `total_usage_including_requeues()` rather than of `total_usage()`, which is
the opposite of the split everything else in this design follows - so the
discarded share is recorded per reservation as well, and the two can be
separated by anyone who wants the other convention.

**These are not utilisation figures, and are deliberately not called that.**
Utilisation is work done over capacity held, and a per-project usage report
cannot supply the denominator:

- a reservation's capacity is its node count multiplied by its duration, and the
  job records carry neither;
- a reservation is normally shared between projects, so no single project's
  report can see the whole numerator either.

What the reports give is the numerator, per project. The shares they print are
shares of that project's own consumption, and both the API docs and the printed
report say so, because "64.5%" next to a reservation name invites exactly the
wrong reading.

Supplying the denominator is deliberately out of scope here, and not because it
is hard: it is a question about the *cluster* rather than about a project, and it
is answered above OpenPortal, by combining these per-project numerators across
every project. Nothing in `op-slurm` needs to know a reservation's capacity.

The natural time to revisit it is when OpenPortal grows instructions for
managing reservations - `add_reservation`, assigning projects to a reservation -
since a portal that creates a reservation already knows what it booked, and
instructions to read back a reservation's definition and the projects entitled to
use it belong in that vocabulary rather than being bolted onto a usage report.
That is also where per-instance identity would live, if it is ever wanted.

## 7.3 Expansion factor

Turnaround over runtime, per job - `(wait + run) / run`, the classical
definition. **1.0 is the ideal**, a job that ran the instant it became eligible,
and the figure rises with every second spent queueing; 2.0 means jobs spent as
long waiting as running. `0.0` is the no-jobs sentinel rather than a score,
which cannot be confused with a real value since no job can score below 1.0. A project whose jobs wait a long time for a
little work is being poorly served, or is doing something odd; a rising figure
is worth a look, and the particular pattern of a job that queues for hours and
then exits in seconds, repeatedly, is what a user fighting a job that will not
run looks like from the outside.

It cannot be derived from what was already collected. The numerator was there -
`total_wait_seconds` - but the denominator has to be runtime, and the only thing
resembling it in a report is *usage*, which is `node_fraction × duration`. For
anything but a whole-node job those differ, so total runtime is now recorded
alongside, in seconds. It is a genuinely new quantity, not a rearrangement of
existing ones.

**Two forms are reported, because they fail in opposite directions.**

- `average_expansion_factor` is the mean of the per-job ratios. One job that
  queued for hours and exited in seconds moves it a long way, which is the whole
  point.
- `aggregate_expansion_factor` is total wait over total runtime - the project as
  one job. No single job moves it much, and equally it will not show a handful of
  short jobs that waited forever.

Carrying both is cheap, and the *divergence between them* turns out to be the
most useful signal of the two: a mean far above the aggregate says a few short
jobs waited a long time, which is exactly the case worth chasing. Per-user
figures then say who. Neither can be reconstructed from the other, so storing
one would have been a choice about which question to allow.

**The form is `(wait + run) / run`, matching `sreport` and the literature.** The
`wait / run` variant differs by exactly one and loses nothing, but a shared
convention matters more than a marginally more direct expression: a figure
compared against Slurm's own reporting has to be on Slurm's scale.

**The ratios are accumulated as thousandths, not as floats.** Float addition is
not associative, so summing the same daily reports in a different order would
give a different total - and these reports are merged out of `HashMap`s whose
iteration order is arbitrary, which would make the shadow-counter checks in §5.4
fail for no reason and the tests order-dependent. Thousandths of an expansion
factor is far finer than anyone reads.

Two smaller things follow from the definition. The population is exactly the one
`num_jobs` counts - one job, once, in the window it started in - so the mean has
a denominator it agrees with; a superseded attempt's wait is already covered by
the requeue figures. And both halves of the ratio are the job's own, not the
window's, so a job running past midnight contributes its whole runtime rather
than the part that fell inside the day: the alternative would inflate the factor
for precisely the long jobs it ought to reassure about. This is the same reason
`wait_time()` measures from the unclipped start (§4.2).

A project-level figure is computed from the summed thousandths over the summed
job count, never by averaging each day's average - a day with four jobs must not
weigh as heavily as a day with four hundred.

Finally, every scaling operation leaves these figures alone. A credit conversion
rescales usage; it does not change how many jobs ran, how long they queued, or a
dimensionless ratio of the two.

## 7.4 Mean job size

The cores and GPUs each job was allocated, summed, so that dividing by the job
count gives the mean size of a job - many small jobs against a few large ones.

Like the expansion factor this cannot be recovered from usage, and for a sharper
reason: usage is core-seconds, and the same core-seconds come from one job on a
hundred cores or a hundred jobs on one core. That is precisely the distinction
being drawn, so the numerator has to be counted separately.

**Deliberately unweighted by runtime.** Each job contributes once however long it
ran, because the question is what shape the jobs were. The other question - what
the machine was actually occupied by - is time-weighted, and is roughly answered
already by the `cpu` component's usage over the runtime of §7.3. Mixing the two
would produce a figure that answers neither.

The project-wide mean is a weak statistic on its own: a project running four
512-core jobs alongside a hundred 2-core jobs reports about 20 cores per job,
which describes neither population. It is the per-user figures that answer the
question, and both are exposed. The same is true of the expansion factor, and for
the same reason - these are distribution questions being asked of a single
number, so the per-user breakdown is not a refinement but the point.

The same argument applies to the expansion factor itself, which is why every row
carries both forms of it. A mean of ratios alone cannot distinguish a user whose
jobs queued a long time and then exited almost at once from a user who simply
waits, and those want different responses. Reading the pair settles it: on the
real fixture one user shows 7290.14 against 3.49 - 85% of that mean comes from
three days, and on the worst of them the day's own totals leave only one
possibility, a job that queued for about three days and then ran for one second,
so a single job out of ninety-two is 40% of the figure. Another user shows 5.83
against 12.52, the opposite case, where the waiting fell on the long jobs. A
third has one job, so the two are the same arithmetic and agree exactly.

Reporting only the aggregate would have hidden the three-day, one-second job
completely, which is the thing worth finding; reporting only the mean leaves an
operator unable to tell whether a large number matters.

The rows are ordered by the aggregate, and the aggregate column comes first,
because that is the figure that answers "who was worst served". Ranking on the
mean instead put the user with one freak job at the top - on this fixture that
is a user whose overall figure is an unremarkable 3.49, above one who genuinely
queued nine and a half hours for a job that ran thirty-four seconds. The mean
earns its place in the row next to it, not in the ordering.

A wait also means nothing without a runtime beside it, so the report's tables
carry both: nine hours of queueing for a job that runs for a day is a busy
queue, and nine hours for a job that runs for thirty seconds is somebody
fighting a job that will not start. The expansion factor already separates those
two - the real fixture has one user at 2.91 and another at 1004 - but the two
columns are what make the figure legible rather than mysterious.

Which is why `expansion_factor_report()` leads with the per-user table rather
than the totals, and shows each day underneath so that a change over time is
visible - when the trouble started being about as useful as who caused it. It
also names which end of the distribution did the waiting, comparing the two
expansion figures as **excesses over 1.0** rather than as raw values: on this
scale 1.0 means "waited not at all", so all the signal is in the part above it,
and a ratio of the raw values calls 1.02 and 1.97 similar when one project waited
fifty times as much as the other. When both excesses are small it says nothing at
all, because then there is nothing to explain.

## 8. Compatibility and rollout

Nothing on the wire breaks. Every new field is `#[serde(default)]`, so an
older peer's report deserialises with zeroes and an older peer ignores the new
fields in ours. The cache is in-memory only (`Lazy<RwLock<Database>>`), so
there is no persisted state to migrate - a restart is sufficient, though note
that a long-running process holding days already marked complete will keep the
old values for those days until it restarts.

One thing compatibility does *not* cover on its own: a report that predates a
statistic has no data for it, and every one of these figures uses zero as its
"not recorded" value. For usage that is harmless - nothing consumed nothing - but
an expansion factor of 0.00 reads as better than perfect on a scale whose floor
is 1.00, and a mean job size of 0.0 cores says jobs ran on no cores. So the
printed output omits a line it has no data for, `expansion_factor_report()` says
outright that the report predates those statistics, and a per-day or per-user row
missing them shows a dash. A number is only printed where a number was measured.

The figure a consumer sees for `reports` does not change, by construction.
That is the point of §3: no flag day, and the policy decision about what to
charge can be taken on evidence, afterwards, with both numbers in hand.

That said, the base figure is now known to understate real consumption
substantially on affected projects. Offering it indefinitely means offering a
number we know to be wrong. It should be presented as a migration window with
an end, not a standing choice.

## 9. Resolved questions

1. **Per-state keys** (§6) use Slurm's own spelling for the states we know, and
   bucket anything else as `OTHER`. Keeping the raw string loses nothing for
   known states, and the allowlist bounds the key space - these keys come from
   Slurm's JSON, and a map keyed on unbounded external strings is the growth
   problem of `security-review-2.md` (finding R33). An unknown state is bucketed
   rather than dropped, so the per-state counts still account for every event.
2. **A distinct-jobs-requeued count** is not implemented. The event count is
   additive over any date range; counting affected jobs needs the cross-window
   grouping ruled out in §3.1. Deferred, not rejected.
3. **Per-user requeue wait** is implemented, for symmetry with the base figures.
4. **A reconciliation check against `GrpTRESMins`** is rejected as designed.
   Slurm's own accounting is reset at the start of each month and a fresh limit
   sent based on the state of the accounts at that point, and a job already
   running is allowed to finish rather than being killed when a limit is
   reached - so an account legitimately runs slightly over. Reproducing that
   business logic inside `op-slurm`, to decide whether a divergence is real,
   would put the portal's policy in the wrong place entirely.

   A better shape for the same idea, worth doing separately: have `get_limit`
   return the account's usage as Slurm sees it alongside the limit, and let the
   portal - which knows about monthly resets and about its own recorded usage -
   decide whether the two have diverged. That changes the `Instruction`'s return
   type, so it is its own piece of work.

## 10. Deviations from the design as built

- **No stored scalar for requeue usage** - see §4.
- **`get_component` does not carry the per-state maps.** They account for the
  whole report's events and usage, and there is no way to apportion them to a
  single component; copying them would leave a component report whose state
  breakdown claimed more usage than the report itself contained.
  `is_consistent` therefore checks each map only when it is populated - which is
  also what makes it tolerate legacy data.
- **Job-id reuse is handled by chain-splitting, not by the grouping key.** §2.4
  proposed grouping on `(job_id, submission)`, but that is the key of a
  *record*, not of a job - grouping on it would put every attempt in a group of
  its own. Instead the records for one id are ordered by submission time and
  split wherever the restart count fails to increase, since no single job's
  attempts can do that. Each resulting chain gets its own base attempt, and the
  split is logged with the submission times so an operator can see which two
  jobs shared the id.
- **`state` became `states`.** `SlurmJob` keeps the whole state set, and the
  single-state accessor was removed rather than kept alongside it: its only
  caller was `Display`, and `terminal_state()` is what that wanted.
- **The requeue event count is not gated on the window** - see §4.2. The first
  version was, which made it near-useless in production while the usage it
  counted was correct. Fixed after testing on real data, with the regression
  test sitting on the accumulation function where the mistake was rather than on
  the classification, which was never wrong.
- **`wait_time()` measures from the unclipped start** - see §4.2.
- **Node failures are logged at `error`**, naming the node and the states Slurm
  reported, from the fresh-fetch paths only - see §6.
