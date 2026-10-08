<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# op-slurm

`op-slurm` is the *scheduler* agent: the only agent with Slurm admin rights. It
only ever receives `*_local_*` instructions carrying mappings - never
portal-level identifiers - so it never has to decide a name for itself.

## What it does

It creates and removes Slurm accounts and user associations, sets and reads
limits, and reports usage:

`add_local_project`, `add_local_user`, `remove_local_project`,
`remove_local_user`, `get_local_usage_report`, `get_local_limit`,
`set_local_limit`, and the `is_local_*` state-verification checks.

It works in one of two modes:

- **Command line**, through `sacctmgr`, `sacct`, `scontrol` and `scancel`, each
  of whose paths can be configured.
- **REST**, through `slurmrestd`. Setting `slurm-server` switches to this mode,
  authenticating with a JWT printed by `token-command`.

## Safe by design

- **It only changes accounts in the OpenPortal-managed organisation.** Any other
  account on the cluster is refused, whatever a peer asks for.
- **Removal is clean.** Removing a user or a project cancels their pending jobs.
- **It is gentle on Slurm.** `max-slurm-runners` caps how many Slurm calls are in
  flight at once.

## Accounting

Usage comes from `sacct`, costed against the configured node type. Reports are
per day and per user, and can carry component breakdowns, queue waits, requeue
accounting, reservations and expansion factors.

At scale - thousands of projects and users, millions of jobs - `op-slurm` can
refresh the accounting for every project every ten to fifteen minutes, without
overloading `slurmctld`:

| | |
|---|---|
| **In parallel** | Each day of a report is fetched as its own task, so many projects and many days are collected at once. |
| **Two runner pools** | Creating accounts and users, setting limits and cancelling jobs use a separate *priority* pool, so they never wait behind an accounting sweep. |
| **Completed days are cached** | A finished day is kept for up to 80 days per project. Only unfinished days are read again. |
| **It narrows when it must** | A day `sacct` cannot answer in one query - because it timed out, ran out of memory, or returned truncated output - is fetched an hour at a time instead. An hour that still fails is skipped and counted, and a day with a gap is never cached, so it is read again on the next pass. |
| **It stays accurate** | A day stays incomplete while any job in it is still running, and is read again later. |
| **Evicting, not flushing** | When a cache is full, single old entries are evicted rather than the whole cache flushed, because re-fetching usage is expensive. |

### Requeued jobs

A requeued job has one accounting record per attempt. `op-slurm` reads them all,
so the usage of attempts superseded by a requeue is never lost, and the
`requeue-policy` option decides which of them a project is charged for:

| `requeue-policy` | Charged | Absorbed by the site |
|---|---|---|
| `charge_requeue_state_only` (default) | Attempts the user requeued themselves (a bare `REQUEUED`) | Node failures, preemptions, and anything else |
| `no_charge` | None | Every requeued attempt |

Slurm enforces limits against its own accumulated usage, which counts every
attempt. So `op-slurm` raises each project's Slurm limit by the requeue usage
the site absorbed that month, keeping the limit the portal set in step with the
usage it was told about.
[The requeue charging design](../../plans/slurm-requeue-charging-design.md)
describes both in full.

### Operator tools

Two tools run the agent's own accounting code directly against `sacct`, so they
always agree with it:

- `get_reservation_report` - which projects used a named reservation;
- `get_requeue_report` - what requeueing cost one project, or the whole cluster.

## Configuration

| Option | Meaning |
|---|---|
| `slurm-default-node` | **Required.** JSON describing the default node, used to cost jobs |
| `slurm-cluster` | The Slurm cluster name, for multi-cluster deployments |
| `slurm-partition` | Restrict accounting queries to one partition |
| `parent-account` | The account managed accounts are created under (default `root`) |
| `max-slurm-runners` | The most Slurm calls in flight at once, in each pool (default 5) |
| `requeue-policy` | Which requeued attempts are charged (default `charge_requeue_state_only`) |
| `sacct`, `sacctmgr`, `scontrol`, `scancel` | The commands to run, in command-line mode |
| `slurm-server` | The `slurmrestd` URL; setting it selects REST mode |
| `slurm-user`, `token-command`, `token-lifespan` | REST authentication (`token-lifespan` defaults to 1800 seconds) |

[Agent configuration](../../specifications/agent-configuration.md) is the
reference for every option.
