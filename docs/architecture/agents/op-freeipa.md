<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# op-freeipa

`op-freeipa` is the *account* agent: the only agent with FreeIPA admin
credentials. Instance agents send it portal-level user and project instructions,
and it returns the Unix mappings that every other agent then works from.

## What it does

It creates and removes users and project groups, blocks and unblocks users, and
reports mappings and home directories:

`add_user`, `remove_user`, `block_user`, `unblock_user`, `add_project`,
`remove_project`, `get_user_mapping`, and the `is_*` state-verification checks.

It is the agent that **decides local names**. When an instance asks it to add a
user, it chooses the Unix user and group and returns them as a mapping; the
instance then passes that mapping to `filesystem` and `slurm`, which never have
to work a name out for themselves. See
[the three tiers of identifiers](../06-greatwestern.md#three-tiers-of-identifiers).

One `op-freeipa` can serve several instance agents - which is how a site-wide
directory gives a user the same account on every cluster
([sharing is wiring](../02-the-agent-network.md#sharing-is-wiring-not-code)).
The `instance-groups` option adds a user to extra groups depending on which
instance asked.

## Safe by design

- **It only touches users it manages.** Everyone else in the directory is
  protected from it.
- **Blocking is reversible.** A blocked user is disabled and added to the
  `openportal.blocked` group, keeping their account and files.
- **Its credential is protected at rest.** The admin password is held as a
  secret in the encrypted part of its config.

## Multi-master aware

FreeIPA replicates between several servers, and its replication cannot
reconcile two independent creations of the same entry. So `op-freeipa`:

- **sends every write to one server at a time**, and moves writes to another
  server only once the first has been confirmed down for longer than the
  replication window;
- **spreads reads across every server** - and before concluding that a user does
  not exist, checks all of them, logging any risk it finds as `REPLICATION-RISK`.

`scripts/check-replication-conflicts.sh` finds any conflict entries that already
exist.

## Under load, and when a server fails

| | |
|---|---|
| **Limited connections** | A fixed pool of connections per server - one per time it is listed in `freeipa-server`, so listing a server twice gives it two. Writes are limited to `freeipa-concurrent-writes` at once (default 2). |
| **Heavy caching** | Users, groups and memberships are held in memory, so most lookups never reach FreeIPA. Every cache is bounded. |
| **Sessions reused** | It logs in once per connection and reuses the session. After a timeout the session is dropped, so the next login doubles as a health check. |
| **Backs off** | After three failed reconnects to a server, it waits twenty seconds per failure before trying that server again. |
| **Bounded calls** | Every call is capped at twenty seconds, or the Job's deadline if that is sooner. Failed calls are retried after reconnecting. |
| **Safe concurrency** | Changes to the same user or group are serialised by per-entry locks; different users are handled in parallel. |

A server counts as down after a refused connection, a failed login, or three
consecutive unanswered calls - never because of a single slow call.

The result is fast for OpenPortal, gentle on FreeIPA, and still working when a
server goes away.

## Configuration

| Option | Meaning |
|---|---|
| `freeipa-server` | The FreeIPA servers, comma-separated - each an individual server, not a load-balanced address. List one more than once to allow it more concurrent connections. |
| `freeipa-user` | The admin user (default `admin`) |
| `freeipa-password` | The admin password, as a secret |
| `freeipa-write-server` | The server that takes all writes (default: the first in `freeipa-server`) |
| `freeipa-replication-window` | How long a write server must be down before writes move, in seconds (default 30) |
| `freeipa-concurrent-writes` | How many writes may be in flight at once (default 2) |
| `system-groups` | Groups every managed user is added to |
| `instance-groups` | Extra groups, per instance |

[Agent configuration](../../specifications/agent-configuration.md) is the
reference for every option.

---

[← 8. Connecting to an awarding portal](../08-connecting-to-an-awarding-portal.md) · [Contents](../README.md) · [op-slurm →](op-slurm.md)
