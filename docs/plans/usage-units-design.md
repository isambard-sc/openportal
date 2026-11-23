<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# Usage reports that say which unit they are in

Status: **implemented.**

A usage report's figures are integers on the wire, `{"seconds": n}`, and until
now nothing said what those integers counted. Two conventions coexisted:

- **HPC sites** report in the award's unit at 3600 counts per hour, so 50 GPU
  hours goes out as 180000 and the awarding portal divides by 3600
  ([site-portal-api.md §4.3](../specifications/site-portal-api.md)).
- **Cloud sites** report money as micro-units, so £3.20 goes out as 3200000. An
  earlier design for a cloud agent chose that resolution, and noted that
  anything displaying it must know to divide by a million.

An awarding portal could only tell the two apart by knowing, out of band, which
kind of site it was talking to. A report read under the wrong convention is out
by a factor of about 278, and both are perfectly well-formed reports. This
change makes a report say what its integers are.

## 1. The shape

One optional field at the top of `ProjectUsageReport`, and one for the
components:

```json
{
  "project": "myaward1.allocator",
  "usage_unit": {"name": "GBP", "counts_per_unit": 1000000},
  "component_units": {"gpu": {"name": "GPUHR", "counts_per_unit": 3600}},
  "reports": {"2027-03-31": {"reports": {"alice": {"seconds": 3200000}}}},
  "users": {"alice.myaward1.allocator": "alice"}
}
```

- **`usage_unit`** is the unit of every *consumption* figure in the report:
  `reports` (including unattributed usage), and the requeue, charged-requeue and
  reservation figures. These are exactly the figures `scale_total` scales.
  `counts_per_unit` integers make one unit, so a figure of 3200000 is £3.20.
- **`component_units`** gives a component its own unit. A component not listed
  inherits `usage_unit`. So a cloud site can bill in GBP and still say how many
  GPU hours were used.
- **Billing and limits use `usage_unit` alone.** Components are information.
- **What stays as it was:** queue waits, runtimes and the other job statistics
  are still seconds of real time. Limits (`set_limit` / `get_limit`) are still
  Slurm time.

### Why not something else

| Alternative | Why not |
|---|---|
| Rename `seconds` on the wire | Breaks every existing reader. The field name stays, and the APIs let callers work in amounts rather than seconds |
| A unit on every `Usage` | Bloats every entry, allows mixing within one report, and changes what old readers see |
| A unit per portal-level `UsageReport` | Two awards on one project may be in different units, so the unit belongs to each project's report |
| Document "always 3600 per unit" | Needs no code, but gives £ only 1/3600 resolution and leaves existing micro-pound reports ambiguous |

## 2. Units

`UsageUnit` is a canonicalised name and a positive integer `counts_per_unit`.

| Kind | Names | Default `counts_per_unit` |
|---|---|---|
| Time | `NHR`, `GPUHR`, `CPUHR`, `COREHR`, `GBHR`, `MBHR`, `BHR`, in any spelling `Allocation` accepts ("node hours", `nhr`) | 3600: a count is a unit-second, which is what usage has always been |
| Currency | Every ISO 4217 code, case-insensitive (except `XTS` and `XXX`), plus `£` → `GBP` and `€` → `EUR` | 1000000: a count is a micro-unit |
| Your own | Anything else | None: give it explicitly, e.g. `UsageUnit::new("CREDITS", 100)` |

- **Two constructors.** `UsageUnit::parse(name)` accepts only the well-known
  units, at their defaults, and refuses anything else rather than guessing a
  resolution. `UsageUnit::new(name, counts_per_unit)` accepts any unit, at any
  resolution.
- **`$` is not accepted.** It names a dozen currencies; write `USD`.
- **`Allocation` canonicalises the same names.** An award of `5000 gbp` and a
  report in `GBP` are therefore one unit, and can be checked against each other.
- **`MBHR` is new to `Allocation`.** It's there because Slurm counts memory in
  megabytes.
- **Headroom:** a `u64` of micro-pounds holds 18 trillion pounds.

## 3. Amounts, and integers

Accounting stays in integers. An amount is turned into counts once, and summed
exactly from then on:

- **Exact input.** `usage_from_decimal("3.20")` is exactly 3200000 counts of GBP,
  with no floating point. An amount finer than one count (`"0.0000001"` GBP) is
  refused rather than rounded.
- **Float input.** `usage(3.2)` rounds to the nearest count, ties to even, so a
  sum of many conversions carries no bias. A negative, NaN, infinite or
  too-large amount is an error, never a saturated figure.
- **Scaling now rounds too.** `Usage * f64` and the `from_hours` family used to
  truncate, which always rounds down and on money is a systematic loss. They now
  round to the nearest count, ties to even, in Rust and Java alike.

`Amount` (a `Usage` plus its `UsageUnit`) is what reports hand back for display:

```rust
format!("{}", total)    // "3.200000 GBP" - full resolution, exact for a power of ten
format!("{:.2}", total) // "3.20 GBP"
```

A bare `Usage` now prints as its raw count, since it has no unit. Use
`as_duration()` or `in_hours()` for figures that really are time.

## 4. Reports from before units

A report with no `usage_unit` is a report from before units. Every such report
was time, 3600 counts per hour of the award's unit, except a cloud site's, which
were micro-units.

- **They still read, total, print and pass on.** Printing shows a duration, as
  before.
- **Nothing can be added to one until its unit is declared.** `add_report`,
  `set_report` and `convert` refuse. This forces new code to state a unit, and
  forces a cached legacy report to be declared before it is extended.
- **`set_unit(unit)`** declares the unit. It is allowed only while the report has
  none, so a unit is set once. On a report that already has figures it declares
  what they mean, and changes none of them. To change a unit, use `convert`,
  which rescales.
- **`set_time_unit(name)`** is `set_unit(UsageUnit::parse(name))` for a time
  unit, and refuses a currency or a unit of your own. A migration of cached
  reports calls it with each award's `allocation.units()`: a GPUHR award's
  reports become GPUHR, not NHR. A cloud site's cached reports were
  micro-pounds, so they are declared with `set_unit(UsageUnit::parse("GBP"))`
  instead. `set_time_unit` refusing GBP keeps a migration from getting this
  wrong silently.

## 5. Combining, converting, components

| Operation | Rule |
|---|---|
| `try_add` / `combine` / `+` | The two units must match, or both be absent (so legacy reports from mixed-version deployments still combine). A report with no figures takes the other's unit, which is what lets `combine` start from an empty report. A component named by both must be in one unit. `try_add` and `combine` return the error; `+=` warns and leaves the report unchanged; Python and Java raise. |
| `convert(to, factor)` | Multiplies every consumption figure by `factor` (how many of `to` one of the current unit is worth) and moves it to `to`'s resolution, rounding once. Every component that was inheriting the old unit is first given it explicitly, so converting a GBP report to GPUHR does not relabel its GBP components. |
| `set_component_unit` | Refused if the component already has figures in a different unit, since that would relabel them. An override equal to the report's unit is not kept. |
| `get_component(c)` | A report in the component's own unit. |
| `UsageReport::total` | A portal-wide total exists only when every project shares one unit (`common_unit`); printing says so otherwise. |
| `scale_total` / `*` | Unchanged in meaning: they scale counts and leave the unit alone. Use `convert` to change a unit. |

**A bug fixed on the way.** `scale_total` kept its own list of consumption
figures, and that list had missed the charged-requeue ones, which describe a
subset of `reports` and so must scale with it. It now delegates to the daily
reports' single list.

The Java client had a related gap. Its table of report fields did not know the
charged-requeue fields at all, so `plus`, `times` and user remapping silently
skipped them. That table now includes them.

## 6. Agents

- **`op-slurm`** writes `NHR` at 3600, which is what node-seconds always were.
  Its components now say what they are in: `cpu` → `CPUHR`, `gpu` → `GPUHR`,
  `billing` → `BHR`, and `memory` → `MBHR`, because Slurm counts memory in
  megabytes. The breakdown was always in these units; now it says so.
- **`op-cluster`** passes the scheduler's report through, and writes `NHR` for
  the empty report it builds when the scheduler returns nothing.
- **Both example site portals** report in the award's unit (`unit_for` /
  `unitFor`). A unit the two portals agreed between themselves, such as
  `CREDITS`, is reported at micro-unit resolution.
- **Every place that printed a `Usage`** now either uses its report's unit or
  prints an explicit duration (Slurm limits, queue waits). The audit was done by
  removing `Usage`'s `Display` and compiling.

## 7. Compatibility and rollout

| Reader / writer | Behaviour |
|---|---|
| An old reader of a new report | Ignores `usage_unit` and `component_units`, and reads the counts exactly as today: as before, it must know the unit out of band |
| A new reader of an old report | Sees no unit: a legacy report, readable as before and declarable with `set_unit` / `set_time_unit` |
| An old agent re-serialising a new report | Drops the fields, as with any field it predates. The reader then sees a legacy report, which is no worse than today, because the unit was already agreed out of band |
| Library code (Rust, Python, Java) | Breaking: `add_report` / `set_report` now return a `Result` (or raise) and refuse a report with no unit; `str(Usage)` is the raw count; scaling rounds. Construct reports with a unit |

An awarding portal (Waldur) should read a report's figures as
`counts / counts_per_unit` in `usage_unit.name`, and fall back to today's
convention only when the field is absent.
