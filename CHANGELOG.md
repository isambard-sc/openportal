# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

### Fixed

- **Slurm usage reports missed everything a requeued job consumed before its
  final attempt.** `op-slurm` called `sacct` without `--duplicates`, which
  returns only the most recent accounting record for each job id. A requeued job
  has one record per attempt, each carrying only its own elapsed time, so every
  attempt before the last was invisible. On a production account measured over a
  single day this hid about a third of the account's real consumption; jobs whose
  final attempt was cancelled before it ran were reported as having used nothing
  at all, because the one record we saw had zero elapsed time and was discarded
  as a non-consumer.

  This also put our reports at odds with Slurm's own enforcement. `set_limit`
  configures `GrpTRESMins`, and Slurm counts every attempt against it, so a job
  could be held for exhausting a limit that our figures said was nowhere near
  exhausted.

### Added

- **Requeue accounting.** `DailyProjectUsageReport` now carries the consumption
  of superseded attempts separately from the usage it has always reported, so
  the two can be told apart rather than merged:

  - `total_requeue_usage()` and `total_usage_including_requeues()`, with per-user
    and per-component breakdowns to match the existing ones;
  - `num_requeue_events()` - requeue *events*, not jobs requeued, so the figure
    is additive over any date range; counted in the single window where the
    requeue happened, which is not in general the window the interrupted attempt
    started in - with `requeue_wait_seconds()`,
    `average_requeue_wait_seconds()` and
    `average_wait_seconds_including_requeues()`;
  - `requeue_states()` and `requeue_usage_in_state()`, bucketing events and usage
    by the terminal state of the superseded attempt, since `NODE_FAIL` (the
    site lost the work), `PREEMPTED` (site policy) and `CANCELLED` are different
    arguments about who should pay.

  - `requeue_report()` on a project or portal report - a readable dump of what
    was reported, what was discarded, what Slurm considers the true total, and
    the breakdown by interrupting state, by day and by user. A project with no
    requeues gets a single line saying so rather than a page of zeroes.

  The per-day printout now carries its own requeue line, and a day whose
  consumption was *entirely* discarded by requeues is no longer skipped as
  having no usage - it has none of the usage we report, which is precisely why
  it is worth showing.

  All of it is exposed through the Python bindings, and every new field is
  `#[serde(default)]`, so a report from an instance that predates them
  deserialises as "no requeues seen" and an older peer ignores what it does not
  know. **`total_usage()` is unchanged**: it still counts only each job's final
  attempt, which is the record `sacct` used to return. Whether a project should
  be charged for the superseded attempts is a policy question - a job that
  checkpoints does real work on every attempt, and an attempt killed by a node
  failure is not the user's fault - so both figures are reported and the choice
  is left to the portal. See
  [docs/plans/slurm-requeue-accounting-design.md](docs/plans/slurm-requeue-accounting-design.md).
- **Expansion factor.** Usage reports now record each project's total wall-clock
  runtime and the expansion factor of its jobs - queue time over runtime - so
  that a project waiting a long time for a little work can be spotted. The
  particular pattern of a job that queues for hours and then exits in seconds,
  repeatedly, is what a user fighting a job that will not run looks like from
  the outside.

  Two forms are reported, because they fail in opposite directions:
  `average_expansion_factor` is the mean of the per-job ratios, which one short
  job that waited a long time moves a long way, and
  `aggregate_expansion_factor` is total wait over total runtime, which no single
  job moves much. The divergence between them is itself the signal - a mean far
  above the aggregate means a few short jobs waited forever - and
  `expansion_factor_for_user()` then says who. Both appear in the printed report
  alongside the job count.

  The convention is the classical one used by `sreport` and the literature -
  `(wait + run) / run` - so **1.0 is the ideal** and the figure rises with every
  second spent queueing. `0.0` means there were no jobs, not a perfect score.

  This could not be derived from what was already collected: the denominator has
  to be runtime, and the nearest existing figure is usage, which weights each
  second by the fraction of a node a job held.
- **Mean job size.** Reports now record the cores and GPUs each job was
  allocated, giving `average_cpus_per_job()` and `average_gpus_per_job()` (and
  per-user variants) - many small jobs against a few large ones. Usage cannot
  answer this: the same core-seconds come from one job on a hundred cores or a
  hundred jobs on one core, which is exactly the distinction being drawn.

  Each job counts once however long it ran, since the question is what shape the
  jobs were rather than what the machine was occupied by. Note that the
  project-wide mean describes a mixed population badly - four 512-core jobs
  beside a hundred 2-core ones average to about 20 - so the per-user figures are
  the ones to read.
- **`expansion_factor_report()`** on a project or portal report - a readable
  summary of how well a project's jobs were served and what shape they were, by
  user and by day. The per-user table is the point of it rather than a
  refinement: both figures are distribution questions being asked of a single
  number, and a project-wide mean job size of twenty cores can be four 512-core
  jobs beside a hundred 2-core ones, describing neither.

  It also names which end of the distribution did the waiting, since the gap
  between the two expansion figures is the most useful thing in the report - a
  mean far above the overall figure means short jobs waited a long time, and the
  other way round means the waiting fell on the long ones and is probably just
  contention. Nothing is said when both are close to the ideal.
- **Reservation accounting.** Usage reports now record which Slurm reservation a
  job ran under, so that what a project put into a reservation can be seen at
  all:

  - `reservations()`, `reservation_usage()`, `reservation_jobs()`,
    `total_reservation_usage()` and `usage_outside_reservations()` on a daily,
    project or portal report, with `reservation_summary()` giving jobs, usage and
    discarded share per reservation, busiest first;
  - `reservation_report()` - a readable dump by reservation, day and user;
  - the per-day printout names each reservation the day's jobs ran in.

  These figures count **every** attempt, superseded ones included: a requeued
  attempt held the reservation's nodes exactly as its replacement did, and for
  occupancy that is what matters. The discarded share is carried separately so
  the two can still be told apart. Reservation usage is therefore a subset of
  `total_usage_including_requeues()`, not of `total_usage()`.

  Reports key on the reservation's **name**, not its id. Slurm gives every
  instance of a reservation its own id, so a recurring or on-demand reservation
  is one name across many ids - a single production account-day showed
  `interactive` under seventeen of them - and keying on the name both merges the
  instances and keeps the key space bounded.

  What this does **not** give is a reservation's utilisation. What a reservation
  held - its node count and duration, and so how fully it was used - is a
  property of the reservation rather than of any one project, and is not in the
  job records; a reservation shared between projects cannot be assessed from any
  single project's report at all. The shares reported are shares of the
  project's own consumption, and the report says so.
- **Usage reports only write what they have to say.** Every counter and map
  added by the work above is omitted from the JSON when it is empty or zero,
  rather than written as `{}` or `0`, and read back through the `serde(default)`
  that each already carried. A day on which nothing was requeued and nothing ran
  in a reservation no longer carries eight empty objects saying so.

  Three fields are still always written - a daily report's `reports` and
  `is_complete`, and a project report's `users` - because release 0.92.0 has no
  `serde(default)` on those and omitting them would make a peer of that version
  fail outright. They now carry one, so a later release can stop writing them
  once no 0.92.0 agents remain.
- Reports that predate any of these statistics print a dash, or omit the line,
  rather than showing a zero. Zero is the "not recorded" value for all of them,
  and on a scale whose ideal is 1.00 an expansion factor of 0.00 would read as
  better than perfect; a mean job size of 0.0 cores would say jobs ran on no
  cores. A number is only printed where a number was measured.
- **Node failures are logged at `error`**, naming the node Slurm blamed, so site
  monitoring picks them up. A node failure destroys a user's work, and on a
  requeued job it is the difference between "the project spent this" and "the
  site lost this".

## [0.92.0] - 2026-08-21

### Added

- **An example site portal** ([python/examples/site_portal/](python/examples/site_portal/)):
  a complete, small, heavily commented implementation of
  [site-portal-api.md](docs/specifications/site-portal-api.md) - every
  instruction as one function, the approval path, the retry contract, and the
  answer-everything guarantee - behind a FastAPI application, with a test suite
  that drives every handler without a bridge, an agent or a network.

  It is written to be **read, not deployed**, and its README is explicit about
  what it deliberately lacks: no authentication on its operator API, no real
  state storage, no durability. The point is the shape, and in particular the
  five things that are easy to get wrong and hard to discover - failing being a
  normal answer and *which* failure mattering, idempotency under retries, never
  leaving a job unanswered, a thirty-second budget rather than two minutes, and
  that a portal may implement as much or as little of the contract as it wants.

  Nothing in it is Python-specific except the convenience of the module, so an
  equivalent in another language belongs alongside it.
- **`AwardDetails()` with no arguments** gives an empty award to fill in with the
  setters, which is what code building one from scratch wants.
  `AwardDetails(json)` is unchanged - the default argument is exactly the `"{}"`
  that produced an empty award before, so no existing caller behaves differently.
- **Structured errors on the wire.** A job's failure was a `String`, so every
  agent that wanted to *act* on one rather than log it had to parse prose - and
  crossing an agent boundary flattened whatever the failing agent had known. A
  failed job now also carries `templemeads::joberror::JobError`: a stable,
  machine-readable `kind` beside the message.

  `kind` is an open string rather than an enum, because templemeads is
  domain-agnostic and cannot own a vocabulary of award decisions. It defines the
  transport kinds (`expired`, `unroutable`, `unsupported`, `invalid`, `run`,
  `unknown`); a `Domain` contributes its own through the new
  `Domain::error_kind_for`, with `greatwestern` supplying `award_pending`,
  `award_rejected` and `award_permission`. A routing hop relays a kind it has
  never heard of without needing to understand it.

  **Nothing deployed has to change.** The prose in `result` is byte-for-byte
  what it always was, including the portal agent's `RuntimeError{…}` wrapper, so
  a peer that reads only that is unaffected. The structured field is additive
  and optional, and a failure arriving without one has its kind reconstructed
  from the message by `Job::error_or_infer`. `Register` gains
  `supports_structured_errors` alongside `supports_portal_routes` - not needed
  for correctness, but it separates "this peer could not have sent a kind" from
  "this failure genuinely had none".

  `Job::errored(message)` keeps its signature and infers a kind, so every
  existing call site acquired one without being rewritten; `Job::errored_with`
  is the explicit path for code that already knows. The portal agent now carries
  a downstream failure's kind through its wrapping instead of discarding it.

  A `JobError` may also record the `origin` agent, for diagnostics. It does not
  leave the agent network: `bridge_server::outbound` is the single funnel every
  job served to a connected portal passes through, and it strips the origin
  there, so internal topology is not volunteered to software outside. The kind
  and message both survive, so nothing a portal acts on is lost.

  In Python the exception class is now chosen from the kind rather than from the
  message, with prose-parsing kept only as the fallback for an older peer, and
  `job.error_kind` exposes the raw kind for anything the class hierarchy does
  not cover.
- **`ProjectStorageReport.to_storage_report()`**, the mirror of
  `ProjectUsageReport.to_usage_report()`. A portal answering
  `get_storage_reports` builds one project report at a time and has to lift each
  into a portal-level `StorageReport` before combining them; without this the
  path raised `AttributeError`.
- **A typed error hierarchy in the `openportal` Python module**, replacing the
  hand-rolled classes and string parser that every portal implementation had to
  write for itself: `OpenPortalError` (deriving from `OSError`, so existing
  `except OSError` code is unaffected), `OpenPortalOtherError`,
  `OpenPortalUnsupportedCommandError`, `ManagedProjectPermissionError`, and its
  two subclasses `ManagedProjectPendingError` and `ManagedProjectRejectedError`.

  The distinction the hierarchy exists to carry is that **pending is not a
  failure**. An award waiting on human approval has no `ProjectMapping` to
  return, so it answers with an error — and the awarding portal must retry that
  one while treating a rejection as final. Losing the class loses that
  difference.

  A job carries one error string, so the class rides inside it as
  `"<ClassName>: <message>"`. `job.errored(exc)` encodes it, `job.error` decodes
  it back to the same class, `job.result` and `job.raise_for_error()` raise it,
  and `openportal.error_from_message()` converts a raw message you already hold.
  Decoding fixes two faults in the implementation it replaces: the wrapper is
  removed by prefix rather than by trimming a character set (which ate the start
  of any message beginning with those letters), and the message is no longer
  off-by-one for `OpenPortalError`.
- **A specification of what a connected site portal must implement**
  ([docs/specifications/site-portal-api.md](docs/specifications/site-portal-api.md)):
  the requests that arrive on the bridge board, the exact result type each one must
  return, the two-minute answering deadline, and how portal-to-portal working hangs
  together - offerings, the `forwarded_for` tag that identifies the awarding portal,
  and the fact that identifiers name that portal rather than the local one. Written
  to be handed to someone connecting a new portal; `bridge-api.md` continues to
  specify the HTTP transport itself.
- `remove_award` is accepted as a synonym for `remove_project`, completing the
  `*_award` spellings alongside `create_award` and `update_award`.
- `freeipa-write-server`, `freeipa-replication-window` and
  `freeipa-concurrent-writes` options for `op-freeipa`,
  and [scripts/check-replication-conflicts.sh](scripts/check-replication-conflicts.sh)
  to find LDAP replication conflicts that already exist in a directory. Both are part
  of the fix below.

### Changed

- **`op-localaccount` now disables an account on removal rather than deleting it**, as
  `op-freeipa` already did. `userdel` freed the account's uid, so a later re-add could
  allocate a different one and leave every file the user owned - including the home
  directory `op-filesystem` had recycled rather than deleted - belonging to a uid its
  owner no longer had, or to whoever the old uid was issued to next. Removal now adds
  the user to a `{managed-group}.removed` group, strips their supplementary groups, and
  locks *and* expires the account; `add_user` re-enables an account it finds in that
  state. Being locked and expired matters: `usermod -L` alone only stops password
  authentication and leaves SSH keys working.

  The removed group is separate from the blocked group, so a blocked user stays blocked
  across a remove and re-add, and an operator can tell why an account is disabled. The
  supplementary groups are stripped because `sync_groups` appends and never removes, so
  a re-enabled user would otherwise get back the access they had before rather than what
  they are entitled to now - the same reasoning `op-freeipa` applies. The `userdel`
  configuration option is gone, being unused.
- **`op-localaccount` no longer creates the home directory** (`useradd -m` is dropped).
  Home directories belong to `op-filesystem`, which creates them and recycles rather
  than deletes them, and this matches `op-freeipa`, whose `user_add` likewise only
  records the attribute. The empty home that `useradd -m` created was enough to stop the
  recycled one being restored - see below.

### Removed

- **`op-cloudaccount` and `op-cloudportal`.** Both were prototypes written to give
  cloud operators something to work against while they had no portal software of
  their own, and both held state - project/user assignment, Award approval - inside
  an agent, which is not where OpenPortal state belongs. The same need is met
  without either agent: the operators run a stock `op-portal` and `op-bridge` and
  put their own software behind the bridge, holding that state on their side of it.
  [site-portal-api.md](docs/specifications/site-portal-api.md) specifies what
  that software has to implement. The archived design documents are kept, marked
  withdrawn, as a record of the reasoning.

  Nothing that came in alongside them is withdrawn: `templemeads::portal::run()`'s
  one-shot mode, `instance::run_delegated`, and `UserMapping`'s acceptance of an
  email address as the local user are all general-purpose and remain.

### Fixed

- A job failure whose `kind` has no exception class of its own no longer loses
  the class its message names. `OpenPortalError` raised by a portal came back as
  `OpenPortalOtherError`, because the kind-first path flattened everything it
  could not place; it now defers to the message in that case, which is what the
  older prose-only path always did.
- **`AwardDetails.set_allowed_domains([])` meant the opposite of what it said.**
  The setter normalised an empty list to `None`, so the strictest setting a
  caller could ask for - permit nobody - silently became the most permissive
  one, permit everybody. The failure was invisible and it widened access:

  ```python
  d.allowed_domains = []  # intent: nobody may join
  d.is_domain_allowed("evil.com")  # -> True
  ```

  `allowed_domains` has three distinct states and both `json-types.md` and
  `python-api.md` have always specified them, so this was the setter
  contradicting the type rather than a policy. It now stores what it is given;
  `clear_allowed_domains()` and passing `None` remain the ways to reach "no
  restriction". `from_json`, `to_json` and `merge` already preserved the empty
  list, so the setter was the only path that lost it.
- **`update_award` could widen an allow-list but never narrow it.** `merge`
  took the union of the two lists, so a domain once granted could not be
  withdrawn and an empty list sent to a project that already had entries was a
  no-op - which made the state the fix above restores undeliverable over the
  wire.

  It now replaces the list wholesale, as `members` and `membership_control`
  already did: `allowed_domains` is a definitive set decided by the awarding
  portal, so an update naming fewer domains means fewer. Omitting the field
  still changes nothing. The fields that accumulate on merge are `notes` (an
  audit trail) and `breakdown`, and they still do; `add_allowed_domain` remains
  the incremental path for a portal building a list up locally.
- Documentation: `python-api.md` listed a `Status.expired()` that does not
  exist - expiry is not one of the six job states, and is read from
  `job.is_expired` - and omitted `Status.created()`, which does.
- **The portal agent sent malformed error sentinels.** `ExpirationError{{}}` and
  `UnknownError{{}}` were written as plain string literals, where `{{` is not an
  escape — only `format!` treats it as one, which is why the neighbouring
  `RuntimeError{…}` was correct. Both reached the portal with doubled braces, so
  a portal matching the documented `ExpirationError{}` never matched. They now
  say what they are documented to say.
- **OpenPortal was creating LDAP replication conflicts in multi-master FreeIPA
  topologies.** A site reported 67 `namingConflict` entries accumulated over 11
  months - 29 project groups, 19 users and their 19 server-generated private groups -
  every one on an object created by OpenPortal, and two of the affected accounts had
  home directories owned by the UID of the copy replication later discarded.

  The write paths were already idempotent (FreeIPA's `DuplicateEntry` is treated as
  "it exists", not as a reason to retry). The cause was that `get_connected_server`
  chose a server at random for *every* call, so an existence check and the add that
  depended on it were served by the same master only 1/n of the time - and a master
  that has not yet received a recent add reports that the user does not exist. Three
  changes:

  - Writes, and the reads that decide whether to write, now all go to one server
    (`freeipa-write-server`, defaulting to the first configured). Failover happens
    only once that server is *confirmed* down - a refused connection, a rejected or
    timed-out login, or a run of unanswered calls, but never one timeout on its own,
    since that is indistinguishable from a write that landed and whose response was
    lost - and not until `freeipa-replication-window` (30s) has passed, so anything it
    accepted has had time to reach whichever master takes over.
    Failover elects a single replacement in configuration order rather than spreading
    writes over what is left, and reverts only once the original has been up again for
    a full window: a server that has just come back may not have caught up with what
    stood in for it, which would create the same conflict from the other direction.
  - Before concluding that a user or group does not exist, every configured master is
    asked, not just the one the pool happened to hand us. This also covers the case
    the report described, where an add times out but has in fact landed.
  - Group creation takes a per-group mutex, mirroring the existing per-user one. Two
    `add_user` jobs for different users in one project both need that project's group
    and are not duplicates of each other to the job Board, so they raced.

  Writes have connections of their own - `freeipa-concurrent-writes`, default 2, on
  whichever server currently holds the role - so write concurrency follows the write
  server across a failover and can be raised without also multiplying the connections
  reads share. Concurrency against a single master is safe; it is only two masters
  accepting the same add that cannot be reconciled.

  Every `freeipa-server` entry must name an individual master for this to hold: a VIP
  or a round-robin DNS alias is several masters behind one name.
- **A 401 from FreeIPA could hang a job until its deadline.** The replay path
  reconnected - possibly to a different server - but reused the URL built from the
  original one, so it posted the new server's session cookie to the old server, which
  401s again. The URL is now rebuilt from the server actually being addressed, and the
  replay is bounded.
- **An empty home directory stopped a recycled one from being restored.** `create_dir`
  treated any existing directory as the finished article, so an account agent that
  creates a home when it creates the account left `op-filesystem` looking at an empty
  directory and declining to restore the recycled one holding the user's real files.

  An existing directory no longer wins automatically: if a recycled copy is waiting and
  what is here holds nothing real, the recycled one is preferred. "Nothing real" is
  strict - any non-hidden entry, or any hidden entry that is not a regular file, and the
  existing directory is kept and the recycled copy left alone. Only hidden regular files
  (the `/etc/skel` copies) are removed, one at a time, each logged, before a
  non-recursive `remove_dir`; `EXPECTED_SKEL_FILES` names the unsurprising ones so
  anything else is logged loudly rather than passing silently. Nothing here can remove a
  subtree even if those checks are ever wrong.
- **A directory restored from `.recycle` kept its old ownership.** Restoring moved the
  directory back and stopped there, so a user volume restored for an account that had
  been deleted and recreated came back owned by the *old* uid - which that user no
  longer has, and which may since have been reassigned to somebody else. This is a
  `op-localaccount` pairing in particular: it runs `userdel` where `op-freeipa`
  disables the account, so the uid is freed and a later `useradd` need not get it back.

  A restore now checks the ownership of what it restored and, if it is wrong, warns
  loudly and corrects it - on a file descriptor opened `O_NOFOLLOW`, as directory
  creation already did (finding R33). A restored *symlink* is reported and left alone
  rather than chowned, since chowning it would transfer ownership of its target. Only
  the directory itself is corrected: its contents still carry the old ownership, and
  walking a tree of unbounded size does not belong inside a job with an answering
  deadline, so the warning names both id pairs and says plainly that a recursive chown
  may still be needed.
- **`op-filesystem` intermittently failed to resolve users and groups that exist.**
  Jobs failed with `Could not find a group called <name>` or `Could not search for
  group <name>: EIO: I/O error` for groups that `getent group` on the same node
  resolved correctly seconds earlier.

  Both messages came from resolving names through libc (`nix::unistd::User::from_name`
  and `Group::from_name`, i.e. `getpwnam_r`/`getgrnam_r`). Release binaries are
  statically linked against **musl**, which has no NSS implementation: those calls read
  `/etc/passwd` and `/etc/group` and, on a miss, make a single attempt over musl's own
  minimal `nscd`-protocol client. There is no `nsswitch.conf`, no `sss` module and no
  fallback, so a directory-backed group was invisible whenever `nscd` was not running -
  musl reports a failed `connect()` as *not found* rather than as an error - and
  reported `EIO` whenever the `nscd` exchange did not complete cleanly, which a
  saturated `nscd` thread pool produces. Neither case ever reached SSSD, which is why
  its logs showed nothing during a failure.

  All name resolution now goes through the host's `getent` (`filesystem/src/nameservice.rs`),
  a glibc-dynamic binary that consults every source in `nsswitch.conf` whether or not
  `nscd` is healthy. Being a `tokio::process` call, it also no longer performs blocking
  FFI on a Tokio worker thread. Dynamic linking would not have fixed this: the
  limitation is musl's, not the linker's.

  Which `getent` is used is decided once, on first use, and logged: `/usr/bin/getent`
  if it exists, otherwise the first one found on the absolute entries of `PATH`, saved
  as an absolute path. That one is then used for the life of the process. If it stops
  being runnable the agent says so and lookups fail as indeterminate until it returns,
  rather than silently resolving names through some other program - a `getent`
  appearing or disappearing under a running agent means something is wrong with the
  host, not that a different binary should be picked up.
- **A name that could not be looked up was reported as a name that does not exist.**
  The two are now distinguished. A genuine absence - every source on the host was asked
  and none knows the name - fails immediately and says so. An indeterminate lookup
  (`getent` timing out, being killed, exiting non-zero for any reason other than
  "key not found", or returning something unparseable) is retried with a short backoff
  and then reported as a temporary failure that can be retried, rather than as a
  missing user or group. Lookups also carry a timeout, so an unresponsive name service
  can no longer pin a task indefinitely as the libc call it replaces could.

  `op-filesystem`'s Lustre quota engine had a second, separate copy of this logic
  (`id -u` and `getent group`, with a `/etc/group` fallback that treated a local miss
  as authoritative). It now shares the one implementation.
- **A portal could not report its members.** `get_users` returns each member's email
  address as the `UserMapping` local user - the portal-level equivalent of a Unix
  username - but mapping validation rejected `@`, so every such mapping failed to
  parse and the whole response errored. `local_user` is now a `LocalUser`, parsed as
  either a Unix account name (unchanged rules) or an email address (a deliberately
  conservative grammar), with the form chosen by whether an `@` is present.

  The two are kept apart at the type level rather than by widening the shared
  validator: `local_user()` hands back a `LocalUser`, so a consumer must call
  `unix()` - which fails on an address - to obtain a name for a path, an operand, or
  an RPC parameter. Nothing that reaches a Unix account, Slurm, FreeIPA or a
  filesystem path accepts the wider charset. `local_group` is unchanged: it names a
  Unix group at every layer.
- Documentation errors in [docs/specifications/json-types.md](docs/specifications/json-types.md):
  `get_projects` returns `Vec<ProjectMapping>` (not `Vec<ProjectDetails>`), `get_users`
  returns `Vec<UserMapping>` (not `Vec<UserIdentifier>`), and `get_project` returns
  `AwardDetails` (not `ProjectMapping`, as the summary table in
  `instruction-protocol.md` claimed). The bridge board instruction list in
  `bridge-api.md` was missing `get_users`, `get_storage_report` and
  `get_storage_reports`.

## [0.91.0] - 2026-08-04

### Added

- **A second security review** ([docs/specifications/security-review-2.md](docs/specifications/security-review-2.md)),
  auditing the whole workspace as seven independent areas, together with a companion
  **record of fixes** ([docs/specifications/security-review-2-fixes.md](docs/specifications/security-review-2-fixes.md)).
  The review re-tested and confirmed the first review's cryptographic conclusions -
  including a differential test of the anti-replay window against a reference model -
  and raised 34 findings concentrated in the agent framework's authorization logic, an
  area the first review had not substantially examined. **All 34 are now resolved.**

  The fixes document holds the rationale for each change, the seven recommendations
  deliberately *not* followed and why, and the five places the review itself was
  wrong. The entries below record only what changed.
- Bridge API **signature version 2**, selected with `X-OpenPortal-Signature-Version: 2`.
  Length-prefixed and fixed-arity, so the presence of a nonce is authenticated.
  Version 1 remains accepted, so existing clients need no change.
- `client --add --type <agent-type>` declares the agent type a client must present
  itself as. The reverse expectation travels in the invite, so no config hand-editing
  is needed. Omitting it leaves the peer unchecked, as before.
- **Portal route discovery**: agents derive the route to each portal from their peers
  and refuse traffic that does not match. Enforcement activates only where a peer is
  declared `type = "portal"`, and is scoped to that peer's zone.
- `secret --value-file <path>` (and bare stdin) so a secret need not appear in `ps`.
  `--value` still works but warns.
- `op-proxy init --name`, so an agent can use two proxies in one zone.
- `op-cloudaccount`, `op-cloudportal`, `op-localaccount` and `op-proxy` are now
  published as release binaries for `x86_64` and `aarch64`, each with an attested
  SBOM. No OCI image or Helm chart is built for them. Note `op-localaccount` is a
  **testing** agent - it manages Unix accounts directly rather than through a managed
  directory service, and warns on every startup.
- Seed unit tests for the privileged agents, which previously had almost none. 298
  tests now pass (from 209).

### Fixed

- **Remote process abort from any authenticated peer.** Three arms of
  `Instruction::parse` indexed a slice without a length check, inside `Command`'s
  `Deserialize` - so a ~200-byte message terminated the receiving agent. Every
  panicking index in the workspace is now a checked form.
- **A routine restart locked an agent out of every long-running peer.** The handshake
  nonce counter and replay window shared process lifetime; a restart reset one but not
  the peer's view of it. Needed no attacker - any deploy triggered it.
- **A Job's claimed route was not bound to the peer that delivered it**, nor was an
  agent's type, nor was an instruction bound to the portal whose authority it claimed.
- **`op-slurm` applied no managed-object guard to any mutation**, so a peer-chosen
  `local_group` naming any real Slurm account had its limits rewritten.
- **`op-localaccount` could add an account to a privileged system group** (the
  `docker.system` → `docker` collision).
- **`op-filesystem` followed symlinks when creating and chowning directories**, so a
  writable path component could redirect a root-owned operation outside the managed
  tree. Paths are now verified to resolve inside a configured volume root at operation
  time, and ownership is applied to a file descriptor opened `O_NOFOLLOW`.
- **Relay envelopes were not bound to the connection they arrived on**, letting any
  direct peer churn a relayed pair's session.
- **IPv4/IPv6 truncation defeated every IP allow-list**, and the bridge's rate-limit
  bypass was still open (the left-most `X-Forwarded-For` entry is client-supplied).
- **Unchecked `u64` arithmetic** in `Usage`/`StorageSize` could silently corrupt a
  billed total; `Allocation` accepted `"NaN"` and `"inf"`. Release builds now set
  `overflow-checks = true`.
- Boards, jobs, caches, nonce stores, connection slots and message sizes are all now
  bounded - previously a peer could grow each without limit. The Slurm caches evict
  individual entries rather than flushing, so one entry reaching the cap no longer
  forces every project to re-query `slurmctld`; the FreeIPA caches deliberately do
  flush wholesale, since a miss there is a cheap re-query.
- A stalled handshake held its connection slot indefinitely; there was no WebSocket
  message size limit; the message-exchange overload recovery was dead code.
- Mapping targets permitted whitespace and separators; `PortalIdentifier` never
  received the identifier allow-list; date parsing and arithmetic were unbounded.
- Config and invite files containing key material are written atomically and
  owner-only, including the bridge invite that a previous fix missed. Key and salt
  lengths are validated at import, and `Key`'s `Debug` no longer prints its bytes.
- `trusted_proxy` and client `ip` now also accept the plain comma-separated string
  syntax the documentation describes, and a malformed value reports the specific error
  rather than "did not match any variant of untagged enum".

### Changed

- Lints are declared once in `[workspace.lints]` and inherited by all crates.
  `clippy::indexing_slicing` and `dbg_macro` join `unwrap_used`/`expect_used` as
  denied; `unsafe_code` is forbidden. `impl Index<usize> for Destinations` is removed -
  use `Destinations::get`.
- `make test` no longer passes `--lib`, which silently skipped every test in the agent
  binary crates. CI gained `cargo audit`, fails on clippy warnings, and runs
  `scripts/check-secret-writes.sh`.
- `templemeads::agent::instance::run` re-checks portal ownership on receipt. An
  Instance whose Jobs are delegated by another agent should use the new
  `instance::run_delegated` - `op-cloudaccount` does.
- Identifier validation moved to a shared `templemeads::validate` module, as did the
  single `OPENPORTAL_ALLOW_INVALID_SSL_CERTS` rule.
- `docs/specifications/bridge-api.md` gains **§0**, stating normatively that the
  bridge must not be internet-facing and why that is a design choice.
- API changes: `ServiceConfig::add_client`/`add_relayed_client` and
  `ClientConfig::new`/`new_relayed` take an expected agent type; `Invite` gained
  `with_agent_type`/`agent_type`; `paddington::config::save` takes `&Path`;
  `op-filesystem`'s `create_dir`/`create_link`/`remove_link`/`recycle_dir` take the
  configured volume roots; `RelayEnvelope` carries a `kind` tag (a wire change - the
  relay has no production deployments).

## [0.90.0] - 2026-07-24

### Added

- ** Separated out the grammar of the Job commands and the Notifications
  into a new Domain** - Now paddington and templemeads do not know about or
  force any particular command or notification grammar onto the agents,
  and could, in theory, be used to create agents that work in any number
  of domains. The previous HPC domain commands and notifications have
  been broken out into the `greatwestern` domain,
  and the `op-provider` agent has been updated to be a
  multi-domain router.
- **`greatwestern` — `Domain::name()`/`version()`/`assume_legacy_domain_version()`**
  — `Hpc` now reports itself as `"greatwestern"` plus its crate version
  (used by the connection-level check), and treats any peer whose
  reported engine version predates the `templemeads`/`greatwestern` crate
  split (`<= 0.32.2`) as implicitly speaking `greatwestern 0.32.2` - those
  older agents have no way to report a domain of their own, since the
  split didn't exist yet when they were built.
- **Domain-oblivious multi-domain routing (`Erased`)** — `templemeads`
  gains an `Erased` domain for building router/proxy agents that forward
  Jobs and Notifications between other agents without needing to
  understand their instruction vocabulary at all. `Job`/`Notification` now
  carry their originating domain's name and version through every hop,
  surviving serialization through any number of `Erased`-typed relays
  completely unchanged. New opt-in compatibility checks,
  `ensure_domain_matches` (connection-level) and
  `ensure_job_domain_matches`/`ensure_notification_domain_matches`
  (per-message), let an agent refuse to talk to a peer speaking a
  different domain - a peer identifying as `Erased` is always accepted at
  the connection level, since routers are meant to carry any domain.
  `op-provider` now uses `Erased` as its domain, making it a genuine
  multi-domain router rather than being hardcoded to `greatwestern`. See
  `docs/plans/archive/multi-domain-routing-design.md`.
- **`op-cloudaccount` — new agent representing a single cloud account** —
  lets a project be assigned to a cloud account (e.g. one AWS account) the
  same way a project is assigned to an `op-cluster` instance. Deliberately
  a rough prototype, built alongside cloud operators who are still
  developing their own side of the integration: there is no cloud-side
  API yet to record project/user assignment, so this agent is the source
  of truth for that (one JSON file per project, atomic writes, in-memory
  cache); usage reports are reconstructed by parsing whatever cost-report
  JSON files the operators drop into a directory, diffing consecutive
  cumulative reports and spreading the delta evenly across the calendar
  days each pair of reports spans. `Usage` (normally compute-seconds
  elsewhere in OpenPortal) is reinterpreted here as micro-currency-units
  (1 second = 1e-6 of the configured currency) - the same
  "reinterpret-the-base-unit" move `op-slurm` already makes for
  node-seconds, just for cost instead of time. See
  `docs/plans/archive/op-cloudaccount-design.md`.
- **`op-cloudportal` — new agent representing a self-contained "cloud"
  portal** — a `Portal` agent for the other side of a portal-to-portal
  Award relationship (e.g. a central portal creating Awards on it), with
  no real portal management software (no Waldur) behind it. Also a rough
  prototype: it stores Award state itself as plain JSON files, read fresh
  from disk on every instruction rather than cached, since the separate
  CLI approval step described below can edit the same files while the
  server process is running; `AwardDetails.template` picks which cloud
  provider an Award targets, mapped via config to a specific
  `op-cloudaccount` peer. Award creation and infrastructure provisioning
  are deliberately decoupled
  behind a human-in-the-loop approval step: `list-pending`/`approve`/
  `reject` CLI subcommands (pure state-file edits, no network calls)
  alongside a background poller inside the running `run` process that
  makes the actual `add_project`/`add_user` calls once an Award is
  approved, retrying automatically on its next cycle if a previous
  attempt partially failed. An earlier design based on `op-portal`'s
  virtual-resource/offering mechanism was abandoned after tracing
  `templemeads::virtual_agent::send()` showed it only ever routes jobs
  within the same process, never between genuinely separate peers -
  `airr`/`cloud` are just an ordinary direct portal-to-portal connection
  instead. See `docs/plans/archive/op-cloudportal-design.md`.
- **One-shot CLI support for `Portal` agents** — `templemeads::portal::run()`
  gained the `run --one-shot "instruction args"` mode (synthesize a local
  Job, run it through the real instruction handler, print the JSON
  result, exit) already available to Account/Filesystem/Scheduler agents,
  mirroring `account.rs` exactly. Added to support debugging/inspecting
  `op-cloudportal` state without a live network peer, but applies to any
  `Portal` agent, `op-portal` included.
- **Multiple IPs/ranges per client allowlist entry** — a `[[clients]]`
  entry's `ip` now accepts a comma-separated list of addresses and/or
  CIDR ranges (IPv4, IPv6, or a mix of both), any one of which is allowed
  to match, e.g. `client --add new_agent --ip
  127.0.0.1,10.0.0.0/24,2001:db8::/32`. A single entry (no comma) behaves
  exactly as before - no change to how existing configs are stored or
  read. Implemented as a new `IpOrRange::List` variant rather than
  changing `ClientConfig.ip`'s type, so it composes with the existing
  single-address/CIDR-range/IPv4-IPv6 logic instead of duplicating it.
- **IPv6 support for IP allowlisting and server binding** — `paddington`'s
  IP-based connection authentication (`IpOrRange`) and a server's own
  listen address now both work correctly for IPv6, not just IPv4. A
  single allowed IP already worked for either family; CIDR *ranges* were
  previously hard-coded to IPv4 only (`iptools::iprange::IPv4`) and are
  now tried against IPv6 too, with identical config syntax either way
  (`ip = "2001:db8::/32"`, just like `ip = "10.0.0.0/24"`). Separately, a
  server's own bind address was built via a formatted string that never
  bracketed an IPv6 address correctly (`TcpListener::bind` requires
  `[::1]:8080`, not `::1:8080`) - now built as a typed `SocketAddr`
  instead, matching the pattern the health-check listener already used.
  Dual-stack listening (one socket accepting both families) remains
  outside OpenPortal's control - it's an OS-level socket option this
  layer doesn't expose - see `docs/plans/ipv6-support-design.md`.
- **`op-proxy` — blind relay proxy for outbound-only agents** — a new agent
  that relays encrypted traffic between two agents that can each only make
  outbound connections (neither can open a port the other can reach),
  without ever being able to decrypt what it forwards. Agents opt in
  explicitly via a `proxy` field in their paddington config, and the proxy
  operator must separately `allow` each `(agent, agent)` pair before it
  will relay between them (default-deny). Every `templemeads`-based agent
  (`op-portal`, `op-provider`, `op-cluster`, etc.) can act as one of the
  two relayed peers: `client --add <name> --proxy <relay-name>` introduces
  a relayed peer, and the resulting invite file is self-describing, so the
  importing side's ordinary `server --add` picks up the relay
  automatically with no extra flag. Validated end-to-end with real
  `op-proxy`/`op-portal`/`op-cloudportal` processes. See
  `docs/plans/archive/blind-relay-proxy-design.md`.
- **Replay protection for ongoing message traffic** — every ongoing
  message (Jobs, Notifications, keepalives) now carries a per-sender
  nonce, checked against a receiver-side sliding window before being
  processed: the standard IPsec/WireGuard-style anti-replay scheme (a
  high-water-mark plus a fixed-size bitmap of recently-accepted values),
  chosen deliberately over a bespoke design. Without this, a captured,
  validly-encrypted message - by an attacker, or the blind relay proxy
  itself - could be resent later to re-trigger its effect; encryption
  alone never prevented that. The nonce lives inside the encrypted,
  authenticated content, so the proxy can no more forge or strip it than
  it can read the payload, and the same protection applies uniformly to
  direct and relayed connections. Rollout is negotiated, not a
  coordinated flag-day: each peer advertises `supports_nonce` in its
  `PeerDetails` (or relayed bootstrap message), and a sender only wraps
  outgoing traffic with a nonce once the specific peer it's talking to
  has confirmed support - an upgraded server therefore gains full
  protection against every already-upgraded peer immediately, while
  continuing to interoperate, unprotected but functioning, with peers
  that haven't been upgraded yet. See
  `docs/plans/replay-protection-design.md` §5.
- **Replay protection for handshake/bootstrap messages** — `Handshake`/
  `PeerDetails` (direct connections) and `StartRelayedConnection`/
  `RelayedConnectionAccepted`/`SessionUnknown` (relayed bootstrap) now
  carry a nonce too, checked against a per-peer window that - unlike the
  ongoing-traffic window above - persists across reconnects rather than
  resetting, since these messages are encrypted under the *permanent*
  pre-shared key pair, which doesn't change across reconnects. Tracing
  through the actual threat showed `StartRelayedConnection` and
  `SessionUnknown` are where this closes a real, repeatable disruption (a
  single captured message could otherwise reset a peer's live session, or
  force endless re-bootstrap churn, indefinitely); `Handshake`/
  `PeerDetails` get it too for defense-in-depth, though a session hijack
  via their replay was never actually possible (session keys are freshly
  random per connection). For direct connections this needed no
  capability negotiation at all - `nonce` is an additive optional field on
  messages that were already structured objects, so an old peer's message
  is simply read with `nonce: None` and the check is skipped, with no
  wire-shape change to gate on. For relayed bootstrap, no backward
  compatibility was needed at all (`op-proxy` isn't deployed yet), so
  `nonce` there is a plain required field. See
  `docs/plans/replay-protection-design.md` §10.

### Security

- **New `docs/specifications/security-review.md`** — an independent,
  code-level security assessment complementing the existing
  `security-model.md`. Where the model document describes how security is
  *intended* to work, the review *evaluates* it for security professionals:
  the threat model, verified strengths (bounded trust topology, sound
  transport crypto with no nonce reuse, a correct anti-replay window, the
  genuinely-blind relay proxy, no-shell privileged agents), and graded
  findings (F1-F15) with `file:line` references and remediation, cited from
  `security-model.md` and cross-linked from the specifications index. It
  records which findings were fixed while writing it (below) and which
  remain open.
- **Fixed: arbitrary absolute-path file write in `op-cloudaccount` /
  `op-cloudportal`** (review finding F1) — a crafted `ProjectIdentifier`
  whose project component began with `/` escaped the configured state
  directory, because `Path::join` discards its base on an absolute
  argument. Closed both at the source (identifier validation, below) and at
  the write path, which now rejects any filename that is not a single plain
  path component.
- **Fixed: strict identifier validation** (F5) — `UserIdentifier` /
  `ProjectIdentifier` components are now restricted to `[A-Za-z0-9_-]` with
  no leading `-` and a length cap, and mapping targets reject `/`, a
  leading `-`, and control characters. This closes argument (flag)
  injection into the privileged tools agents spawn (`useradd`, `sacctmgr`,
  …) and the path-escape enabling F1, at the point identifiers enter the
  system.
- **Fixed: reachable panic decoding wire frames** (F4) —
  `deenvelope_message` sliced an attacker-controlled text frame at fixed
  byte offsets, which panics (aborting the process) if a multi-byte UTF-8
  character straddles a boundary. It now slices with `str::get`, returning
  a clean error instead.
- **Fixed: proxy now binds the relayed `from` to the authenticated sender**
  (F7) — `op-proxy` dropped any envelope whose claimed `from` did not match
  the peer identity the connection authenticated as, so `RelayPolicy` no
  longer rests on attacker-supplied labels.
- **Fixed: secrets no longer leak into error messages** (F10) —
  `Key::from_password` and `ServiceConfig::get_key` no longer interpolate
  the password (or a secret environment variable's *value*) into error
  context that can reach logs.
- **Fixed: config and invite files are written `0600`** (F9) — files
  holding plaintext pre-shared keys are now owner-only on Unix, rather than
  landing at the process umask.
- **Fixed: strong, versioned config-secret encryption** (F2) — secrets
  stored in a config's `extras` (e.g. FreeIPA/Slurm credentials) are now
  encrypted with a fresh per-secret random salt and a realistic Argon2 cost
  (19 MiB / 3 passes) via `Key::from_password_with_salt`, in a versioned
  `op-secret-v1:` format. Previously the derivation used orion's minimum
  cost (8 KiB) with a hardcoded salt, making it deterministic and cheap to
  brute-force. Legacy (v0) secrets still decrypt, and re-running the
  `secret` command upgrades a value to the strong format. The `Simple`
  encryption scheme is now documented as obfuscation only (its "password"
  is the public agent name) - use `Environment` in production.
- **Fixed: forwarded client IPs are only trusted from a configured proxy**
  (F3, F6) — a new `trusted_proxy` config field / `--trusted-proxy` init
  flag (IP or CIDR list) gates all trust in `proxy_header` /
  `X-Forwarded-For`. On the agent (paddington) side, a forwarded client
  address is honoured only when the real TCP peer matches `trusted_proxy`,
  else the header is ignored (fail-closed), closing IP-allow-list spoofing.
  On the bridge HTTP side, a new middleware resolves the client IP from the
  real TCP peer (`ConnectInfo`), honouring `X-Forwarded-For`/`X-Real-IP`
  only from a trusted peer, so rate limiting can no longer be bypassed by a
  spoofed header. Works with a Cloudflare tunnel / in-cluster proxy on an
  internal address (e.g. `--trusted-proxy 127.0.0.0/8`).
- **Fixed: pre-authentication resource-exhaustion (DoS) hardening** (F11) —
  the agent (paddington) accept loop now fail-fasts any inbound connection
  whose source address matches neither a configured client IP nor the
  `trusted_proxy` range, dropping it before any WebSocket-upgrade or crypto
  work; and it bounds concurrent *unauthenticated* connections with a
  process-wide semaphore (limit 2048, released the moment a peer
  authenticates, so long-lived authenticated peers never occupy the pool).
  On the bridge, `verify_headers` now verifies the request signature
  **before** reading or growing the nonce store (so only authenticated
  callers can touch it), and the store has a hard size cap.
- **Fixed: `op-localaccount` now only removes accounts/groups it manages**
  (F13) — `remove_user` applies the same managed-group guard as
  `block_user`/`unblock_user` (a pre-existing system account is never
  `userdel`'d), and `remove_project` refuses to `groupdel` any group with a
  system GID (`< 1000`) or a configured system/managed group name — closing
  the case where a crafted project identifier (e.g. `docker.system`) mapped
  to a bare system group name. `op-localaccount` is a **testing agent**
  (for a containerised test Slurm cluster; use `op-freeipa` in production)
  and now logs a warning to that effect on every startup, with matching
  notes added to its docs.
- **Noted (by design): the bridge's `X-Nonce` is optional for backward
  compatibility** (F8) — the official Python client always sends a fresh
  per-request nonce (included in the signature), so current clients get
  full replay protection; the optional path exists only for older clients,
  mirroring the negotiated nonce rollout on the agent side.
- **Documented: TLS is an external concern by design** (F12) — the wire
  protocol is confidential and authenticated over plain HTTP/`ws` on its own
  (double-envelope AEAD; HMAC on the bridge), so terminating TLS is left to
  the operator's infrastructure. Operators who also want the residual
  metadata (salts, IPs, sizes, timing) protected layer on HTTPS/`wss` with
  standard tooling (nginx, ingress, Cloudflare tunnel) and point
  `trusted_proxy` at the terminator. Clarified in the security review and
  agent-configuration docs as a deliberate design decision, not a gap.
- **Documented: no forward secrecy, by design** (F14) — session keys are
  freshly random per connection but key-transported (encrypted) under the
  permanent pre-shared keys, never negotiated in-band; OpenPortal
  deliberately provides no in-band key-agreement route (not even
  Diffie-Hellman). The permanent keys only ever encrypt high-entropy random
  session keys, so there is no crib to attack them from the wire; security
  rests on out-of-band permanent-key secrecy plus the `rotate` path.
  Corrected inaccurate "forward secrecy" wording in `wire-protocol.md`,
  `highavailability.md`, `security-model.md` (new §2.5), and the relay
  source comments.
- **Fixed: lower-severity hardening (F15)** — replaced the bridge's
  hand-rolled constant-time compare with `paddington::constant_time_eq`
  (orion `secure_cmp`); the bridge no longer echoes internal `Debug` error
  detail to clients (logged server-side, generic message returned);
  `op-slurm` no longer logs the token-fetch command (may embed a
  credential); FreeIPA login now uses `reqwest`'s `.form()` for correct
  URL-encoding; both handshake paths reject an all-zero session key;
  `clean_and_check_path` now rejects relative paths and `..` components; and
  `op-localaccount`'s shadow-utils mutation commands gained a `--`
  end-of-options separator. The handshake now sends HKDF salts **in the
  clear** (they are public by design) instead of XOR-masking them, negotiated
  via a new `openportal-salt-format: plain` header so an upgraded server still
  reads legacy (XOR) clients — upgrade servers before clients, since a client
  commits to its salt encoding in the first message (see
  `wire-protocol.md` §4.1). AEAD/MAC key domain separation was assessed and
  deliberately not changed: no code path uses one key for both (wire = AEAD
  only, bridge = MAC only, config = AEAD only), so it would be a breaking
  change for zero benefit; the invariant is instead documented on `Key::sign`.
  The auth-layer timing distinction and the healthcheck worker count are left
  as-is (not exploitable / useful monitoring signal).

### Fixed

- **Slurm hourly usage-report fallback silently dropped per-component
  usage** — `sacctmgr::get_hourly_report()` (used when a project has too
  many jobs for the daily `sacct` query to complete in time) only
  accumulated total usage, job counts, and wait times; unlike the normal
  daily path, it never called `add_component_usage`, so any day that fell
  back to hourly reporting kept a correct overall total but lost its
  cpu/memory/gpu/billing breakdown entirely, in both the cached-hour and
  freshly-fetched branches. Fixed by adding the same per-job component
  accumulation the daily path already does.
- **Invalid IP ranges no longer silently break connections or panic** — a
  hand-edited or mistyped range (e.g. `0.0.0.0/0.0.0.0`) is now rejected
  with a clear error when the config is loaded, instead of loading
  successfully and only failing later, silently, at connection time. The
  canonical "match everything" CIDR range `0.0.0.0/0` is now handled
  correctly and no longer triggers an integer-overflow panic in the
  underlying `iptools` dependency. `client --add` without `--ip` now also
  errors clearly instead of falling back to an invalid default range.
- **`server --remove` / `client --remove` silently doing nothing** —
  removal used to match on name *and* zone together, defaulting to the
  `"default"` zone when `--zone` wasn't given; if the peer had actually
  been added under a different zone, the command reported success but left
  the peer list unchanged. Removal by name now succeeds without needing
  `--zone` at all as long as the name is unambiguous, and errors clearly
  (rather than silently doing nothing) if the name doesn't exist or exists
  in more than one zone.

## [0.32.2] - 2026-06-03

### Fixed

- **Python bindings — `AwardDetails` setters now accept `None`** — The
  `name`, `key`, `description`, `project_template`, and `members` setters
  previously rejected Python `None` even though the underlying Rust fields are
  `Option<T>`. Assigning `None` now clears the field, consistent with the
  other optional setters (`start_date`, `end_date`, `allocation`, `award`,
  etc.).

### Changed

- **GitHub Actions updated to Node.js 24-compatible versions** —
  `actions/checkout` (v6), `actions/upload-artifact` (v7),
  `actions/download-artifact` (v8), `actions/attest-build-provenance` (v4),
  `actions/attest-sbom` (v4), `softprops/action-gh-release` (v3),
  `actions/setup-python` (v6), and `PyO3/maturin-action` (v1, pinned from
  `main`).

## [0.32.1] - 2026-05-29

### Added

- **Static binaries as GitHub release assets** — Each release now attaches
  plain statically-linked binaries for all eight agents (`op-bridge`,
  `op-cluster`, `op-clusters`, `op-filesystem`, `op-freeipa`, `op-portal`,
  `op-provider`, `op-slurm`) directly to the GitHub release, enabling simple
  `curl`/`wget` downloads and automatable upgrades without requiring a
  container runtime. Both `x86_64-unknown-linux-musl` (named `op-*`) and
  `aarch64-unknown-linux-musl` (named `op-*-aarch64`) builds are provided.

### Changed

- **Python bindings — string comparison and hashability for wrapper types** —
  All Python-wrapped types that are thin string wrappers now support equality
  and inequality comparison directly against plain Python strings (`x == "value"`,
  `x != "value"`), in addition to comparing against objects of the same type.
  Types that did not already support hashing have gained `__hash__`, making them
  usable as `dict` keys and in `set`s. Affected types: `Status`, `MembershipControl`,
  `QuotaLimit`, `DomainPattern`, `Uuid`, `Destination`, `Instruction`,
  `UserIdentifier`, `ProjectIdentifier`, `PortalIdentifier`, `ProjectTemplate`,
  and `Volume`. `MembershipControl` also gains a `from_string` static constructor
  (accepted values: `"open"`, `"members_only"`, `"roles_only"`, `"locked"`).

## [0.32.0] - 2026-05-22

### Added

- **`AwardDetails` member validation and atomic batch operations** —
  `add_member` now validates that the supplied address is a well-formed email
  and is permitted by the project's `allowed_domains` list; it returns
  `Result<(), Error>` (Python: raises `OSError`) instead of silently ignoring
  bad input. Two new batch operations provide atomic semantics: `add_members`
  adds multiple members without replacing existing ones, and `set_members`
  replaces all members; both validate every entry before making any change so
  callers never need to roll back a partial update. The Python bindings expose
  all three methods.
- **Email addresses in `allowed_domains`** — `DomainPattern` now accepts exact
  email addresses (e.g. `"collaborator@gmail.com"`) alongside domain patterns
  (`"example.com"`, `"*.university.ac.uk"`). The new `is_email_allowed(email)`
  method on `AwardDetails` checks an email against both email-pattern entries
  (exact, case-insensitive) and domain-pattern entries (matched against the
  domain part). `is_domain_allowed` is unchanged and ignores email entries.
  The Python binding exposes `is_email_allowed`.

## [0.31.0] - 2026-05-18

### Added

- **`notification::send` — shared notification helper** — `templemeads` now
  provides `notification::send(destination, event)` that routes a notification
  to the next hop in `destination`, or delivers it locally if this agent is the
  final destination (via `invoke_notify_runner`). Award notifications in the
  portal and user/project notifications in the cluster both use this function;
  the old per-crate `send_notification` / `send_award_notification` helpers are
  removed. `Destination::reverse()` is also new, returning a copy of the path
  with agents in reversed order.
- **`forwarded_for` field on `Job`** — bridge-board jobs created by the portal's
  virtual resource runner now carry the original job destination (e.g.
  `remove.local.resource`) in a `forwarded_for` field. Web-portal code can read
  this via `job.forwarded_for` (Python `Destination | None`) instead of
  reconstructing the originating portal from the bridge destination. The field is
  absent (`null`) on all other jobs and is backwards-compatible with older agents
  that do not set it.

## [0.30.1] - 2026-05-15

### Added

- **`fetch_notification` Python function** — `openportal.fetch_notification(uuid)`
  fetches a pending notification from the bridge by UUID. Called from the web
  portal's `notification_url` handler after the bridge sends its GET signal.
  Accepts a UUID string, `Uuid` object, or `Notification` object. Raises
  `OSError` if the UUID is not found. Complements the existing `fetch_job` /
  `fetch_jobs` pattern.

## [0.30.0] - 2026-05-15

### Added

- **New notification system** — notifications now flow end-to-end between
  infrastructure agents and the web portal via the bridge. Infrastructure
  agents can fire events (e.g. `user_added`) that are notified to the
  web portal via a `notification_url`. The web portal can also
  send notifications into the agent network (including to peer portals) via the
  new `POST /notify` bridge endpoint and the `openportal.notify()` Python
  function. A new `Notification` Python class provides `event_type` and
  `event_argument` properties for straightforward dispatch in portal code. See
  [notification-protocol.md](docs/specifications/notification-protocol.md),
  [bridge-api.md](docs/specifications/bridge-api.md), and
  [python-api.md](docs/specifications/python-api.md) for full details.
- **TypeScript bindings for `templemeads` types** — the `templemeads` crate
  now derives TypeScript type definitions from its Rust structs and enums via
  [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Running
  `cargo test -p templemeads export_ts_bindings` regenerates
  `templemeads/bindings/` with one `.ts` file per exported type. Exported
  types cover jobs (`Job`, `Status`), diagnostics (`DiagnosticsReport` and
  all sub-entries), health (`HealthInfo`), storage (`Quota`, `Volume`,
  `StorageReport`, `ProjectStorageReport`), usage (`UsageReport`,
  `ProjectUsageReport`, `DailyProjectUsageReport`, `Usage`), and award
  details (`AwardDetails`, `Link`, `Note`, `MembershipControl`).
- **`templemeads/bindings/identifiers.ts`** — hand-written TypeScript utility
  providing parse and stringify helpers for the five string-encoded identifier
  types (`PortalIdentifier`, `ProjectIdentifier`, `UserIdentifier`,
  `ProjectMapping`, `UserMapping`). Allows React components to decompose
  e.g. `"alice.myproject.brics"` into `{ username, project, portal }` and
  reassemble when sending instructions back to OpenPortal.
- **`docs/specifications/typescript-bindings.md`** — specification document
  covering the bindings: how to generate them, the full table of exported
  types, serialisation notes (timestamp formats, HashMap key conventions,
  custom-format strings), and instructions for adding new exported types.

## [0.29.0] - 2026-05-05

### Added

- **`block_user` / `unblock_user` / `is_blocked_user` instructions** — new
  account-level instructions to suspend and reinstate users without touching the
  filesystem or Slurm scheduler:
  - `block_user <user>` — adds the user to the `openportal.blocked` group and
    disables their account (FreeIPA: `user_disable`; localaccount: `usermod -L`).
    Idempotent; no-ops if the user is already blocked or is protected.
  - `unblock_user <user>` — removes the user from `openportal.blocked` and
    re-enables their account. Idempotent; no-ops if the user is not blocked or
    is protected.
  - `is_blocked_user <user>` — returns `true` if the user is currently in the
    `openportal.blocked` group.
- **`block_project` / `unblock_project` / `is_blocked_project` instructions** —
  cluster-level convenience instructions that operate on all users in a project:
  - `block_project` / `unblock_project` call `block_user` / `unblock_user` for
    each member. Per-user errors are logged but do not abort the fan-out. Note:
    `unblock_project` unblocks **all** users in the project, including any that
    were individually blocked beforehand.
  - `is_blocked_project` returns `true` if the project has at least one member
    **and** every member is currently blocked; `false` if the project is empty or
    any member is unblocked.
- **`openportal.blocked` group** — the `op-freeipa` and `op-localaccount` agents
  use a `{managed-group}.blocked` group (default: `openportal.blocked`) as the
  canonical source of truth for blocked status. This lets blocked users be
  distinguished from users removed for other reasons — both are account-disabled,
  but only blocked users appear in the group.
- **`gpasswd` configurable command in `op-localaccount`** — needed to remove a
  user from a single supplementary group cleanly during `unblock_user`. Defaults
  to `gpasswd`; can be overridden in the agent config (e.g.
  `gpasswd = "docker exec slurmctld gpasswd"`).

### Changed

- **`add_user` will not re-enable a blocked user** — if a user is in the
  `openportal.blocked` group at the time `add_user` is called, the call returns
  the existing mapping immediately without re-enabling the account. Only
  `unblock_user` can reinstate a blocked user. A corresponding guard was added
  at the cluster level to avoid unnecessary downstream agent calls.
- **`is_existing_user` returns `true` for blocked users** — a blocked user is
  still a managed, existing user and should continue to appear in queries.
  Previously, disabled accounts that were not protected were treated as
  non-existent.
- **`remove_user` can remove blocked users** — a blocked user can be fully
  removed by `remove_user`; the early-exit guard for disabled accounts now skips
  only users who are disabled but *not* blocked.

## [0.28.0] - 2026-05-01

### Added

- **Log ring buffer in diagnostics** — agents now capture tracing log messages
  into an in-memory ring buffer (up to 500 entries) via a custom
  `tracing_subscriber::Layer` installed at startup. The full buffer is included
  in every `DiagnosticsReport` as `recent_logs` (newest-first in JSON; field is
  `serde(default)` so old responses deserialise cleanly).
- **`DiagnosticsReport.logs()` / `Diagnostics.logs()` (Python)** — retrieve log
  entries in chronological order (oldest first) with optional filtering:
  - `max` (default `0` = all) — limits entries returned, applied *after* any
    level/search filter, so `.logs(50, level="ERROR")` gives the 50 most recent
    errors.
  - `level` — exact match (`"INFO"`) or minimum-level threshold (`"INFO+"`);
    accepts `"WARN"` and `"WARNING"` interchangeably. Levels from lowest to
    highest: `TRACE`, `DEBUG`, `INFO`, `WARN`, `ERROR`.
  - `search` — case-insensitive substring match against the message text.
  - All filters are ANDed.

  ```python
  d = openportal.diagnostics("brics")
  d.logs()  # all entries, oldest first
  d.logs(level="WARN+")  # warnings and errors
  d.logs(50, level="WARN+", search="timeout")  # last 50 matching entries
  ```
- **`LogEntry` Python class** — properties: `timestamp` (UTC `datetime`),
  `level` (`str`), `target` (Rust module path, e.g. `"templemeads::agent"`),
  `message` (`str`).

## [0.27.1] - 2026-04-01

### Fixed

- Watchdog false disconnections (paddington) — after removing `register_activity()`
  from `send_message()` to fix zombie connection detection, the client-side
  (`make_connection`) receive path was missing a `register_activity()` call.
  Client-initiated connections had no activity tracking at all and were
  disconnected by the watchdog after 300 seconds even when healthy. Activity
  is now registered on every successfully received message on both the
  server-side (`handle_connection`) and client-side (`make_connection`) paths.

## [0.27.0] - 2026-04-01

### Added

- **`Link` and `Note` types** — new reusable types in `templemeads::grammar`:
  - `Link { id: Option<String>, url: Option<String> }` — a reference to an
    external resource with an optional human-readable ID and an optional validated
    URL. Used for all the link fields on `AwardDetails`.
  - `Note { timestamp: DateTime<Utc>, author: String, text: String }` — a
    timestamped message. `Note::new(author, text)` stamps with `Utc::now()`;
    `Note::with_timestamp(dt, author, text)` accepts an explicit timestamp.
    `Display` format: `[YYYY-MM-DD HH:MM UTC — Author] text`.
- **New fields on `AwardDetails`**:
  - `award: Option<Link>` — link to the award record on the funding body's system
    (e.g. UKRI GtR). Replaces the previous flat `award_id` / `award_url` fields.
  - `call: Option<Link>` — link to the funding call that produced the award.
  - `project_link: Option<Link>` — link to the project page on the remote/awarding
    portal, so local users can navigate there.
  - `renewal: Option<Link>` — link to the renewal / more-time application page.
  - `notes: Vec<Note>` — append-only list of timestamped messages from the awarder.
    Serialises as `[]`-omitted (field absent when empty); deserialises with
    `#[serde(default)]` so old JSON is backward compatible.
  - `earliest_approve: Option<DateTime<Utc>>` — RFC 3339 UTC timestamp before
    which the receiving portal must not approve or provision the award. Lets the
    awarder make corrections in the window between creating the award and it being
    acted on (e.g. set to one hour in the future on creation).
  - `breakdown: BTreeMap<String, String>` — a free-form map of portal-defined
    allocation component names to human-readable values (e.g.
    `"project_storage" → "5 TB"`). OpenPortal carries this map transparently and
    does not interpret its contents. Keys and values are both arbitrary strings;
    ordering is deterministic (alphabetical by key). On merge, entries from the
    incoming `AwardDetails` overwrite or add to existing entries (no keys are
    deleted). Absent from serialised JSON when empty.
- **Python bindings**: `openportal.Link` and `openportal.Note` classes exported.
  `AwardDetails` gains `award`, `call`, `project_link`, `renewal`, `notes`,
  `add_note`, `clear_notes`, `earliest_approve`, `set_earliest_approve`,
  `clear_earliest_approve`, `breakdown` (getter/setter), `set_breakdown_entry`,
  `remove_breakdown_entry`, and `clear_breakdown`. `earliest_approve` is exposed
  as a UTC-aware `datetime.datetime`.
- **`MembershipControl` enum and `membership_control` field on `AwardDetails`** —
  the sending portal can now declare a policy that constrains whether the receiving
  portal may independently modify project membership or roles:
  - `open` (default, field absent) — receiving portal manages membership and roles
    freely.
  - `members_only` — receiving portal may add/remove members; roles are
    authoritative in `AwardDetails` updates from the sender.
  - `roles_only` — receiving portal may change the role of existing members; it
    must not add or remove members.
  - `locked` — receiving portal must not change membership or roles; both are
    authoritative in `AwardDetails` updates from the sender.
  On merge, the incoming value overwrites the existing value if present; absent
  is treated as `open` at runtime (field is omitted from serialised JSON).
  Python: `award.membership_control` getter/setter, `award.clear_membership_control()`,
  and convenience methods `award.can_change_membership()` / `award.can_change_roles()`.
  `openportal.MembershipControl` is exported as a Python class with `Open`,
  `MembersOnly`, `RolesOnly`, and `Locked` attributes, each of which also exposes
  `can_change_membership()` and `can_change_roles()` directly.
- **`get_users` instruction at portal level** — the portal and bridge agents now
  handle `get_users <project_id>`, forwarding it to the connected web portal via
  the bridge. The response is `Vec<UserMapping>` where `local_user` = email
  address (the portal-level equivalent of a Unix username). The Python bindings
  already supported `Vec<UserMapping>` so no Python changes were required.
- **`get_award` and `get_awards` instructions** — new portal-level instructions
  to retrieve award details. `get_award <project_id>` returns the `AwardDetails`
  for a single project; `get_awards <portal_id>` (also accepted as `list_awards`)
  returns all award details for a portal as a `Vec<AwardDetails>`. Both are
  handled by the portal agent and forwarded to the bridge.

### Fixed

- Zombie connection prevents reconnection (paddington) — when a network
  fault silently dropped a websocket connection without closing the TCP socket,
  the server retained the stale connection in its registry. Any reconnection
  attempt from that peer was incorrectly classified as a secondary (standby)
  connection and never promoted to primary, causing all messages to that peer
  to be lost indefinitely until the server was restarted.
  - Root cause: `send_message()` was calling `register_activity()` on every
    outgoing send, so the watchdog's 300-second inactivity timer was
    continuously reset by keepalive sends to the zombie connection. Only
    received messages now count as activity.
  - Extra safeguard: `check_standby()` now proactively runs the watchdog
    check on the existing connection whenever a new connection arrives for
    the same peer. If the existing connection is stale it is disconnected
    immediately, allowing the new connection to become primary without
    waiting for the background watchdog loop.

### Changed

- **`ProjectDetails` renamed to `AwardDetails`** — the data structure that
  carries project/award metadata is now called `AwardDetails`, reflecting that
  it describes a funding award (with its members, dates, allocation, etc.) from
  which a project is created. `ProjectDetails` remains as a type alias so
  existing Rust code compiles without changes.
  - Wire protocol unchanged: `NamedType::type_name()` still returns
    `"ProjectDetails"` for `AwardDetails`, so result-type tags in Job payloads
    are unchanged. `create_project` and `update_project` remain the canonical
    command names (Display still emits the old names). `create_award` and
    `update_award` are accepted as synonyms by the parser.
  - Python: the class is now `openportal.AwardDetails`. `openportal.ProjectDetails`
    is registered as an alias pointing to the same class object
    (`openportal.ProjectDetails is openportal.AwardDetails` is `True`).
- **`members` field migrated to `BTreeMap`** — `AwardDetails::members` was
  `Option<HashMap<String, String>>` serialised through a custom `ordered_map`
  helper that sorted keys on every serialise. It is now
  `Option<BTreeMap<String, String>>`, which provides the same deterministic
  alphabetical key ordering without the helper. The change is fully
  backward-compatible: existing JSON deserialises identically and the wire
  representation is unchanged. The `ordered_map` helper function has been
  removed from `templemeads::grammar`.

## [0.26.0] - 2026-03-25

### Added

- **`filter(range: DateRange)`** on all four report types — returns a copy of
  the report restricted to days that fall within the given date range
  (inclusive on both ends):
  - `ProjectUsageReport::filter` — keeps only the daily-report entries whose
    date is within `range`; the `users` map is preserved.
  - `UsageReport::filter` — delegates to `ProjectUsageReport::filter` for each
    contained project report.
  - `ProjectStorageReport::filter` — keeps only the `daily_reports` entries
    whose date is within `range`; the top-level (current) snapshot fields
    (`generated_at`, `project_quotas`, `user_quotas`, `users`) are preserved
    unchanged.
  - `StorageReport::filter` — delegates to `ProjectStorageReport::filter` for
    each contained project report.
  - Python bindings added for all four types.
- **Date-range support for storage report instructions** — `get_storage_report`,
  `get_storage_reports`, and `get_local_storage_report` now accept an optional
  `<date_range>` argument (default: `today`). The filesystem agent enforces that
  the range must be `today`; any other range causes the job to fail with an
  error. This mirrors the existing `[<date_range>]` argument on the usage report
  instructions.

### Fixed

- **`ProjectUsageReport` `+` / `+=` now correctly merges `users` maps** — both
  operators previously merged the daily-report data but silently discarded the
  `users` map from the right-hand operand. Both now perform a union: existing
  entries in `self` are preserved and any missing entries are filled from the
  other report.

### Changed

- **`ProjectStorageReport::daily_reports` key type changed from `NaiveDate` to
  `Date`** — the `HashMap<NaiveDate, DailyStorageReport>` field is now
  `HashMap<Date, DailyStorageReport>`, consistent with `ProjectUsageReport`.
  The wire/JSON format is unchanged (`Date` serialises as the same `YYYY-MM-DD`
  string). `ProjectStorageReport::get_report` now takes `&Date` instead of
  `&NaiveDate`; the Python binding is unchanged (still accepts
  `datetime.date`).

## [0.25.0] - 2026-03-25

### Added

- **Report remapping** — all four report types (`ProjectUsageReport`,
  `UsageReport`, `ProjectStorageReport`, `StorageReport`) now support
  remapping identifiers, enabling reports to be translated from one portal
  to another before merging or publishing:

  - `remap_project(&new_project: ProjectIdentifier)` on
    `ProjectUsageReport` / `ProjectStorageReport` — replaces the top-level
    project identifier and rebuilds all `UserIdentifier` keys in the users
    map so that `username.old_project.old_portal` becomes
    `username.new_project.new_portal`. For `ProjectStorageReport`, also
    rebuilds the `user_quotas` keys and updates historical snapshots in
    `daily_reports`.
  - `remap_portal(&new_portal: PortalIdentifier)` on all four types —
    convenience wrapper that keeps each project's name unchanged and only
    swaps the portal, e.g. `aiproject.brics` → `aiproject.ukri`.
  - `remap_project(old: &ProjectIdentifier, new: &ProjectIdentifier)` on
    `UsageReport` / `StorageReport` — remaps a single contained project
    report from `old` to `new`. Does nothing if `old` is not present.
  - `remap_portal(&new_portal: PortalIdentifier)` on `UsageReport` /
    `StorageReport` — bulk-remaps every contained project to the new portal
    and updates `self.portal`.
  - `remap_users(&new_usermapping: HashMap<UserIdentifier, String>)` on all
    four types — updates the local-username strings for the specified users.
    Returns an error if the remapping would cause two distinct users to share
    the same local username. For `ProjectUsageReport`, also propagates the
    rename into all daily-report `HashMap<String, Usage>` entries (including
    component breakdowns, per-user job counts, and per-user wait seconds).
- **`user_mapping()`** added to all four report types — returns the full
  `HashMap<UserIdentifier, String>` (portal user → local username) for the
  report. For `UsageReport` and `StorageReport`, aggregates mappings across
  all contained project reports.

### Changed

- `ProjectStorageReport::users()` now returns `Vec<UserIdentifier>` (sorted)
  instead of `&HashMap<UserIdentifier, String>`, consistent with
  `ProjectUsageReport::users()`. Use `user_mapping()` to obtain the full
  identifier → local-username map.
- Python: `ProjectStorageReport.users` is now `list[UserIdentifier]`
  (was `dict[UserIdentifier, str]`). Use `user_mapping` for the dict.
- Python bindings added for all of the above on `ProjectUsageReport`,
  `UsageReport`, `ProjectStorageReport`, and `StorageReport`. `remap_users`
  accepts a plain Python `dict[UserIdentifier, str]`.

## [0.24.0] - 2026-03-13

### Added

- `ProjectStorageReport` now supports temporal history alongside its
  point-in-time snapshot, mirroring the `ProjectUsageReport` / `UsageReport`
  API:
  - New internal `DailyStorageReport` type (not exposed publicly) stores
    per-day snapshots inside `ProjectStorageReport`. Callers always receive
    `ProjectStorageReport` even for individual days, so there is no silent
    type change.
  - New `daily_reports: HashMap<NaiveDate, DailyStorageReport>` field on
    `ProjectStorageReport`, serialised as `"daily_reports"` in JSON.
    The field is omitted from JSON when empty (`skip_serializing_if`), so
    existing serialised reports remain valid (fully backwards-compatible).
  - Merge semantics (`+` / `+=`): the newer snapshot (by `generated_at`)
    becomes the top-level state; the older snapshot is stored in
    `daily_reports` under its calendar date. If both snapshots fall on the
    same day, the older is discarded. Historical entries from both sides are
    merged keeping the newest snapshot per date. The current top-level date
    is never duplicated in `daily_reports`.
  - `ProjectStorageReport::daily_reports(with_usage_only)` — returns all
    snapshots (historical + current) sorted by date (oldest first), each as a
    `ProjectStorageReport` with an empty history map. When `with_usage_only`
    is `true`, only snapshots with quota data are returned. When `false`,
    every calendar date in the range [earliest, latest] is included (empty
    reports for missing days), mirroring `ProjectUsageReport::daily_reports`.
  - `ProjectStorageReport::get_report(date)` — returns the snapshot for a
    specific `NaiveDate` as a `ProjectStorageReport`. If the date matches the
    top-level snapshot's date, the top-level data is returned (without nested
    history). Returns an empty report for unknown dates.
  - `ProjectStorageReport::combine(reports)` — combines a slice of reports
    using the merge semantics above.
  - `StorageReport` gains `+` / `+=` operators and a `combine(reports)`
    function, merging per-project history across portal-level reports.
  - Python bindings updated: `ProjectStorageReport.daily_reports(with_usage_only=True)`
    → `list[ProjectStorageReport]`; `ProjectStorageReport.get_report(date)`
    accepts a `datetime.date`; `+` / `+=` / `combine()` on both
    `ProjectStorageReport` and `StorageReport`.

## [0.23.1] - 2026-03-12

### Changed

- Slurm job wait time (`SlurmJob::wait_time()`) is now computed from
  Slurm's `time.eligible` timestamp rather than `time.submission`. The
  eligible time is when the job first became runnable (after any holds,
  `--dependency`, or `--begin` constraints are resolved), which is a more
  accurate measure of scheduler queue wait time than the raw submission
  timestamp. The internal field has been renamed from `submit_time` to
  `eligible_time` accordingly. For jobs with no holds or dependencies the
  two values are identical.

### Fixed

- `op-localaccount`: `userdel` no longer passes `-r`, so the user's home
  directory is not deleted by the account agent. Home directories are managed
  exclusively by the filesystem agent, which recycles them rather than
  permanently deleting them.
- `op-filesystem`: symlink removal in `RemoveLocalProject` now works correctly
  in prefix (remote/container) mode. Previously, `std::fs::remove_file` was
  called against the local filesystem, so symlinks on the remote system were
  silently left in place. The new `filesystem::remove_link` helper dispatches
  to `rm -f` via the exec prefix when a prefix is configured.

## [0.23.0] - 2026-03-11

### Added

- Job accounting improvements for the Slurm agent:
  - Jobs are now counted only on the day they **start**, preventing
    multi-day jobs from being double-counted in daily usage reports.
  - Added `submit_time` and `original_start_time` fields to `SlurmJob` so
    queue wait time can be computed. `original_start_time` records the
    true start time before any day-boundary clamping.
  - `SlurmJob::wait_time()` returns the time between job submission and
    start (clamped to zero for jobs that started immediately).
  - `DailyProjectUsageReport` gains per-user job counts
    (`user_job_counts`) and per-user queue wait time (`user_wait_seconds`),
    alongside the existing scalar totals `num_jobs` and the new
    `total_wait_seconds`. New accessors: `num_jobs_for_user`,
    `wait_seconds_for_user`, `average_wait_seconds_for_user`,
    `average_wait_seconds`, `is_consistent`.
  - `ProjectUsageReport` gains `total_wait_seconds`, `average_wait_seconds`,
    and `daily_reports(with_usage_only)`.
  - All new fields use `#[serde(default)]` so they are fully
    backwards-compatible with JSON produced by older releases.
  - Runtime consistency check in the Slurm sacctmgr path: local shadow
    counters are compared against the report's scalar totals after each
    daily report is built; a warning is logged if they diverge.
- `Usage`, `DailyProjectUsageReport`, and `ProjectUsageReport` now have an
  `in_hours()` method that returns a human-readable string with all values
  expressed in hours — useful for comparing across days with consistent
  units.
- `Usage::Display` now auto-scales to seconds/minutes/hours, limits output
  to 3 decimal places, and uses correct singular/plural forms
  (e.g. `"1 second"`, `"1 job"` instead of `"1 seconds"`, `"1 jobs"`).
- Display output for `DailyProjectUsageReport` and `ProjectUsageReport`
  now includes per-user job counts and average queue wait times inline.
- All new Rust functionality exposed through the Python bindings:
  `DailyProjectUsageReport.total_wait_seconds`, `average_wait_seconds`,
  `num_jobs_for_user()`, `wait_seconds_for_user()`,
  `average_wait_seconds_for_user()`, `is_consistent`, `in_hours()`;
  `ProjectUsageReport.total_wait_seconds`, `average_wait_seconds`,
  `daily_reports()`, `in_hours()`; `Usage.in_hours()`.
- Updated `docs/specifications/json-types.md` and
  `docs/specifications/python-api.md` to document all new fields and
  methods, including backwards-compatibility notes.
- Storage quota reporting: new point-in-time storage report for projects
  and portals, complementing the existing time-ranged usage reports:
  - New `templemeads/src/storagereport.rs` containing `ProjectStorageReport`
    and `StorageReport` types (with `NamedType` implementations for result
    dispatch). `ProjectStorageReport` captures project-level and per-user
    quotas across all volumes, plus the user identifier→local username
    mapping. `StorageReport` is the portal-level aggregate.
  - Three new `Instruction` variants in `templemeads/src/grammar.rs`:
    `GetStorageReport(ProjectIdentifier)`,
    `GetStorageReports(PortalIdentifier)`, and
    `GetLocalStorageReport(ProjectMapping)`.
  - `get_storage_report <project_id>` and `get_storage_reports <portal_id>`
    commands handled by the portal, bridge, and cluster agents.
  - `get_local_storage_report <project_mapping>` is an internal instruction
    sent by the cluster instance agent to the filesystem agent. The
    filesystem agent builds the full `ProjectStorageReport` locally: it
    fetches project and per-user quotas from the filesystem, and calls back
    to the sender (cluster) with `get_users <project_id>` to obtain the
    project member list. This keeps the business logic in the agent that
    owns the quota data and reduces inter-agent round trips.
  - Python bindings in `python/src/lib.rs`: `ProjectStorageReport` and
    `StorageReport` exposed as PyO3 classes with `project`, `generated_at`,
    `project_quotas`, `user_quotas`, `users`, `portal`, `projects`,
    `get_report()`, `is_empty()`, `to_json()`, `from_json()`, and standard
    `__str__`/`__repr__`/`__copy__`/`__deepcopy__` methods.
  - `docs/specifications/instruction-protocol.md`: new "Storage Reporting
    Instructions" section documenting all three commands.
  - `docs/specifications/json-types.md`: full JSON schemas for
    `ProjectStorageReport` and `StorageReport`, plus new entries in the
    `result_type` reference table.
  - `docs/specifications/python-api.md`: `ProjectStorageReport` and
    `StorageReport` class references with property tables and usage example.
- Added `SECURITY.md` with vulnerability reporting contact, supported version
  policy, scope definition, and link to the security model specification.
- Added `CONTRIBUTING.md` covering bug reporting, dev setup, code standards,
  and pull request guidelines.
- Added `CODE_OF_CONDUCT.md` (adapted from Contributor Covenant v2.1).
- Added root `LICENSE` file (MIT) for tool and platform compatibility.
- New `op-localaccount` agent (`localaccount/`) — an Account agent that
  implements the full Account instruction set (`AddUser`, `RemoveUser`,
  `AddProject`, `RemoveProject`, `GetUsers`, `GetProjects`,
  `GetUserMapping`, `GetProjectMapping`, `IsExistingUser`,
  `IsExistingProject`, `IsProtectedUser`, `UpdateHomeDir`) using standard
  Unix commands (`useradd`, `userdel`, `groupadd`, `groupdel`, `usermod`,
  `getent`). All commands are individually configurable so they can be
  prefixed for container execution (e.g.
  `useradd = "docker exec slurmctld useradd"`). Intended for testing
  without a FreeIPA installation.
  - All required groups are created before the user is added: project
    group, managed group (`openportal` by default), an auto-generated
    per-instance group (`op-<instance-name>`), plus any extra groups
    specified via the `system-groups` and `instance-groups` config keys.
  - Home directory is obtained by calling back to the instance agent
    (`get_local_home_dir`), matching the protocol used by `op-freeipa`.
  - Documented in `docs/specifications/agent-configuration.md` §3.6.1.
- `op-filesystem` exec-prefix support: a new optional `exec-prefix` extra
  redirects all filesystem operations (mkdir, chown, chmod, mv, ln -s,
  touch, rm -rf, test, readlink) through an external command prefix
  instead of the native Rust stdlib/nix calls. Setting
  `exec-prefix = "docker exec slurmctld"` lets the filesystem agent run
  on the host while performing all operations inside a container.
  Documented in `docs/specifications/agent-configuration.md` §3.7.
- New `linux` quota engine (`filesystem/src/linuxquotaengine.rs`) for
  `op-filesystem`. Uses the standard `setquota` / `repquota` utilities to
  manage per-user and per-group quotas on any Linux filesystem that
  supports the kernel quota interface (ext4, xfs, etc.). Both commands
  are configurable for container execution. Configure with
  `type = "linux"` in a `[quota_engines.*]` block.
  Documented in `docs/specifications/agent-configuration.md` §3.7.5.
- New `fake` quota engine (`filesystem/src/fakequotaengine.rs`) for
  `op-filesystem`. Designed for local testing on Mac / Docker where real
  quota filesystems are unavailable. Quota limits are persisted as
  plain-text files in a configurable `quota_dir` on the agent host; disk
  usage is measured with `du -sk` (configurable for container execution).
  No quota enforcement happens — it reports the stored limit against real
  `du` usage, which is sufficient to exercise the full portal-to-filesystem
  quota plumbing. Configure with `type = "fake"` in a `[quota_engines.*]`
  block. Documented in `docs/specifications/agent-configuration.md` §3.7.6.

### Fixed

- Added missing parse arms for `get_user_dirs` and `get_local_user_dirs` in
  `Instruction::parse()` (`templemeads/src/grammar.rs`). These instructions
  were already fully implemented (enum variants, `Display`, argument
  serialisation) but could not be parsed from a command string, making them
  unreachable over the wire. Removed the corresponding errata entry from
  `docs/specifications/notes.md`.

### Changed

- Extended documentation: added `docs/bridge/README.md` (bridge and Python
  example), `docs/specifications/python-api.md` (full Python API reference),
  and updated the specifications index with links to both.
- Fixed several issues in the existing docs: corrected a copy-paste error in
  the `cmdline` example (portal config was showing cluster config), replaced
  a stale note about config encryption with current information, fixed a
  dangling link to the (now created) bridge example, corrected several typos,
  added the `slurm` agent to the agent type list in `docs/README.md`, added
  cross-links between the specifications and narrative documentation, and
  expanded the root `README.md` with an agent type table and description.

## [0.22.4] - 2026-03-05

### Changed

- Ensured that any slurm job that would round down to zero usage would be billed
  at least 1 node second. This ensures that super small jobs are still
  billed.
- Added protocol specifications and docs into docs/specifications, with
  links from the README. This should make it easier for
  developers to understand the protocol and to implement new agents.

## [0.22.3] - 2026-03-04

### Changed

- Added `--sender` and `--zone` options to the `--one-shot` commands, so that
  you can set the sender and zone for the command. This is useful for testing
  commands that include the sender information, e.g. those involving
  FreeIPA. Also added `--one-shot` support to the accounting / freeipa
  agent.

### Fixed

- Fixed get_users and get_projects commands so that they only return the
  active users and projects for the specified portal for the specified
  instance. Previously, get_users would return users that may not have
  been fully added (preventing fixing this in case the user addition failed),
  and get_projects returned all projects in FreeIPA related to OpenPortal,
  for that portal, even if they were for different instances.

## [0.22.2] - 2026-01-21

### Fixed

- Fixed Lustre quota parsing failing when usage exceeds the quota limit.
  Lustre's `lfs quota` command appends a `*` suffix to values that exceed
  quota (e.g., `2000*` instead of `2000`). The parser now strips this suffix
  before parsing the numeric value.
- Fixed HA standby-only logic incorrectly triggering for agents that act as
  servers. The standby-only shutdown behavior now only applies to client-only
  agents. Agents that also accept server connections (indicated by calling
  `set_is_server()`) will no longer incorrectly enter standby mode based on
  peer connection status.

### Changed

- Volume configuration subpath templates now accept case-insensitive
  placeholders. `{PROJECT}`, `{Project}`, `{USER}`, `{User}`, and other case
  variants are automatically normalized to lowercase `{project}` and `{user}`
  during validation. This prevents configuration errors from placeholder
  case mismatches.
- Added validation to ensure user and project volume subpaths contain the
  required `{project}` placeholder. Invalid configurations now fail early
  with a descriptive error message.

## [0.22.1] - 2026-01-20

### Fixed

- Fixed a stack overflow bug in the slurm agent caused by infinite recursion
  when logging SlurmJob objects. The Display implementation for SlurmJob
  called billed_node_seconds(), which called billed_node_fraction(), which
  logged self using Display, causing infinite recursion. The fix logs just
  the job ID instead of the full object.

### Changed

- Added separate priority runner pool for time-sensitive slurm commands.
  Commands like adding/removing users, getting/setting limits, and cancelling
  jobs now use a dedicated pool of runners that won't be blocked by
  long-running usage report queries (sacct). This ensures that priority
  operations remain responsive even when multiple usage reports are being
  generated.

## [0.22.0] - 2026-01-14

### Added

- Added billing TRES as a value that can be used when calculating node
  hours. This will be the only calculation method if cpu, gpu and memory
  are not specified when creating the default node object.
- Added components to the usage reports. The cpu, gpu, memory and
  billing components are now also tracked separately and available
  in all of the usage reports.
- Added AwardDetails and DomainPattern types, with Python bindings.
  These are used in ProjectDetails to provide richer information about
  a project's associated award, and about which email domains are
  allowed to be associated with a project.
- Added ability to set, get and clear filesystem volume quotas. Currently
  this is only supported for Lustre, but the engine can be expanded to
  support other filesystems. This adds new commands, e.g.
  `set_project_quota`, `get_project_quota`, `get_project_quotas`,
  `clear_project_quota`, `set_user_quota`, `get_user_quota`, `get_user_quotas`,
  and `clear_user_quota`.
- Added instructions to see if a user or project already exists, namely
  `is_existing_user` and `is_existing_project`. This is used by the cluster
  agent to only remove partially-added users or projects if an
  add operation failed, and the user or project did not already exist
  before the operation started. This adds a level of safety, as it should
  stop the unintential removal of existing users or projects if something
  goes wrong when talking with other agents (e.g. filesystem or scheduler)

### Fixed

- Fixed a bug when signing API calls that incorrectly introduced possible
  serialisation / deserialisation issues when verifying signatures. This
  led to some calls failing signature verification.

## [0.21.1] - 2025-12-02

### Fixed

- Fixed the issue that prevented the results of jobs sent to virtual agents from being returned to the original sender.

## [0.21.0] - 2025-11-21

### Added

- Implemented cascading health checks across the agent network with intelligent timeout handling (500ms or until all peers respond), automatic detection of disconnected peers, circular loop prevention via visited-chain tracking, and configurable cascade blocking for leaf nodes (FreeIPA, Filesystem). Portal-to-portal health queries are blocked to prevent cross-site information leakage. Health checks now report in-flight jobs (those passing through intermediate agents) and queued jobs (waiting for reconnection) separately from detailed job states, which are only shown for source and destination agents.
- Added restart command functionality allowing agents to be remotely restarted via control commands.
- Implemented soft restart functionality. Jobs are error-cancelled during restart, diagnostics data is cleared, and new job submissions are rejected with retry-able errors. Routing of restart requests respects portal and leaf node boundaries.
- Added worker count tracking to health checks. The paddington event loop now tracks active worker tasks, exposed via the health endpoint and included in HealthInfo.
- Implemented system resource monitoring using the sysinfo crate. Agents now track and report process memory usage, CPU usage, total system memory, and CPU core count in health checks.
- Added background monitoring task that refreshes every 10 seconds, warning when CPU usage exceeds 90% or process memory exceeds 80% of system memory. High resource usage triggers detailed health info logging for troubleshooting.
- Implemented job execution timing statistics. Job run times are tracked with min/max/mean/median calculations over a rolling window of 1000 jobs, exposed in health checks for performance monitoring.
- Added diagnostics tracking with all-time counters for completed, failed, expired, and slow jobs (>10s). Historical data includes recent failures, slowest executions, and expired jobs. Diagnostics totals are exposed in health checks and cleared on soft restart.

## [0.20.2] - 2025-11-15

### Fixed

- Updated all dependencies to their latest versions, including fixes to
  compile with the latest PyO3 (for Python 3.14 support). Thanks to
  @livenson for the help :-)

## [0.20.1] - 2025-10-21

### Fixed

- Added automatic retry logic with exponential backoff to the Python bridge client
  (`call_get` and `call_post` functions in `python/src/lib.rs`) to handle rate
  limiting from the bridge server. The client now detects HTTP 429 (Too Many Requests)
  responses and automatically retries with exponential backoff (100ms, 200ms, 400ms,
  800ms, 1600ms) up to 5 times before failing. This prevents the Python client from
  being blocked when calling the bridge server too frequently.

## [0.20.0] - 2025-10-17

### Added

- Added support for multiple home roots in the filesystem agent. The `home-roots`
  configuration option now accepts colon-separated paths (e.g., `/home:/scratch`),
  and the `home-permissions` option accepts corresponding colon-separated permissions
  (e.g., `0755:0755`). When a user is added, home directories are created in all
  configured home roots at `{home-root}/{project}/{user}` with the appropriate
  permissions. The `GetLocalHomeDir` instruction returns the first home directory.
- Implemented non-destructive removal for filesystem operations. The `RemoveLocalUser`
  and `RemoveLocalProject` instructions now move directories to `.recycle` subdirectories
  instead of deleting them. Directory timestamps are updated to the current time when
  recycled, enabling external cleanup processes to remove old recycled directories
  (e.g., after 7 days).
- Added automatic restoration from `.recycle` when creating directories. If a directory
  exists in `.recycle`, it is restored to its original location instead of creating
  a new directory, making the removal and recreation process fully reversible.
- Implemented automatic cancellation of pending Slurm jobs when removing users or projects.
  The `RemoveLocalUser` and `RemoveLocalProject` instructions now use `scancel` to cancel
  all queued (PENDING state) jobs while leaving running jobs to complete. This ensures
  that removed users cannot submit new jobs while preserving historical accounting data.
  The `scancel` command is configurable via the `scancel` configuration option (default:
  `scancel`)

### Fixed

- Fixed error in `sacctmgr` command related to setting limits.

## [0.19.1] - 2025-10-13

### Added

- Added support for multiple connections to the slurm REST API server,
  matching the behaviour and code of the freeipa agent, and similar
  to the sacctmgr connection.

### Fixed

- Made the number of jobs optional in the JSON (defaults to zero) so
  that this doesn't break backwards compatibility with agents that
  don't send this field.

## [0.19.0] - 2025-10-12

### Added

- Fixed timeouts of slurm accounting when large numbers of jobs are
  run per project per day. The code automatically switches over to
  doing houly accounting if the daily accounting takes too long.
  In addition, accounting is now done in parallel, with multiple
  sacct calls now allowed to be made (controlled by the
  `max-slurm-runners` config option, default 5).
- Added counting of the number of jobs run per day for slurm accounting,
  and exposed the number of jobs (`num_jobs`) in `UsageReport` and
  `ProjectUsageReport` for better tracking.
- Added an `Hour` object that can be used to represents a specific hour
  in a day, to make it easier to request hourly accounting data.
  The start and end times of an hour form a half-open interval,
  i.e. the start time is included, but the end time is not. This
  matches the expected behaviour of the slurm `sacct` command.
- Added the ability to run commands directly from the command line,
  simplifying debugging and testing. This can be done using the
  new `--one-shot` command line option, which takes an OpenPortal
  command without a destination. For example,
  `op-slurm run --one-shot "get_local_usage_report brics.aiproject:brics.aiproject this_month"`
  would get the usage report for the `brics.aiproject` project for
  the current month. Note that multiple `--one-shot` commands can
  be added, and they will be run in sequence. There is also
  a `--repeat N` option that can be used to repeat the
  commands N times.
- Added in a Claude.md file that is used by Claude code to help
  maintain and improve the codebase.

### Fixed

- Fixed the `Date` start and end time to also use the same half-open
  interval as the `Hour` object. This means that the start time
  is included, but the end time is not. This matches the expected
  behaviour of the slurm `sacct` command.
- Fixed a number of minor cybersecurity issues discovered by Claude code.
  These are adding a counter/nonce to the Bridge API server, reducing
  the time tolerance for the API server signed time check to 5 seconds,
  adding in rate limiting for calls to the Bridge server API,
  adding constant-time comparison of keys to prevent timing attacks,
  and using a command builder pattern to construct slurm commands
  to prevent any possible injection attacks. Note that none of these
  issues are exploitable in practice, but these changes make the
  system more robust and secure.
- Reduced logging verbosity when the system is under load to make logs
  more readable.

## [0.18.0] - 2025-09-29

### Added

- Added a `merge` function to `ProjectDetails` to simplify merges.
- Added a `get_portal` function so that it is easier for the portal
  connected to a python bridge to be determined.

## [0.17.0] - 2025-09-23

### Added

- Added support for "virtual agents" which can be used to provide
  additional agents that represent extra resources offered by a portal,
  without the need to create full agents for those resources. This is
  particularly useful when using remote portals that offer
  classes of offerings under a single virtual identifier.
- Added commands related to creation of new offerings, e.g.
  `add_offerings`, `sync_offerings`, `remove_offerings`, and
  `get_offerings`. Made these accessible to Python, and connected
  them to the virtual agent model. This allows Python-based portals
  to communicate new offerings to OpenPortal, which are spun up
  as virtual agents. This should make it easier to manage
  offerings that are provided by portals for portal-to-portal
  requests, without needing to spin up full agents.

## [0.16.3] - 2025-09-13

### Added

- Added support for connecting to multiple redundant FreeIPA servers,
  and allowed parallelising requests across multiple servers (
  including multiple requests to the same server). This should improve
  the overall reliability and performance of the system.

### Fixed

- Optimised the algorithm for increasing and decreasing the number of
  tokio workers to make the agents more responsive to changes in load.
  The number of workers can grow and shrink more quickly, and are no
  longer capped to 10 parallel workers. This removes the bottlenecks
  observed when the message load is high.

## [0.16.2] - 2025-08-12

### Fixed

- Fixed incorrect return type of the portal `get_usage_report` function.

## [0.16.1] - 2025-08-11

### Added

- Added more functions to support UsageReport creation in Python,
  plus exposed the `DailyProjectUsageReport` class.

## [0.16.0] - 2025-08-08

### Added

- Added extra commands that can be run by the portal: `get_projects`,
  `remove_project` and `get_usage_reports`.
- Added ability to combine `UsageReport` and `ProjectUsageReport` objects
  together using static `combine` functions, plus added in lots of operators
  to add, multiply and divide usage. This should make manipulation
  of usage reports much easier.

### Fixed

- Cleaned up the way that python objects are extracted in the `job.completed`
  function.

## [0.15.1] - 2025-08-04

### Fixed

- Fixed an issue where the order of serialisation of the members field
  in the ProjectDetails object was not deterministic, which led to
  failed signature validation for the bridge API calls. Now, the
  members are always serialisaed in a sorted, deterministic order.

## [0.15.0] - 2025-07-22

### Added

- Added an Allocation type that can be used to describe an allocation in
  arbitrary units (e.g. node hours, GPU hours etc.). Also added a Node type
  that can provide metadata about a node, so that we can interconvert between
  different allocation units.
- Updated the ProjectDetails object to use Allocation rather than Usage
  as the allocation type. This is now under the field `allocation`, with
  the `credits` field now not being used.

## [0.14.0] - 2025-06-10

### Added

- Added automatic de-duplication of jobs. Now, if the board detects
  that a job is added that is the same as one that is already being
  processed, it will automatically mark the new job as a "duplicate",
  and will not process it. Instead, it will copy the result from the
  already-running job and return that result once it is ready. In this
  way, we prevent job storms if a caller continually re-submits the
  same job without waiting for the result. Duplicate jobs are caught
  in the communication chain, so will not be sent on to downstream
  peers. This makes the system more responsive and robust, as
  now only new jobs are processed downstream, with duplicates
  filtered out at a high level.
- Added passing of the job expiry time to the functions called by
  the slurm and freeipa agents. Now, these agents will abort any
  functions calls that take too long and that whose results would
  be ignored anyway as the calling job had expired. This prevents
  resource starvation and denial of service / deadlocks caused
  by floods of long-running jobs blocking the system, and causing
  all new jobs to timeout or run slowly.
- Added a semaphore to the function calls of the slurm and freeipa
  agents. This semaphore ensures that only 10 jobs can be processed
  at the same time. This reduces contention pressure on the
  (serialised) access to the freeipa / slurm REST APIs, or to
  running saact/mgr commands. This prevents a deadlock situation
  where a single function calls the API or runs the command
  one after the other, but gets blocked by a storm of new
  jobs that hold that resource in the first call to the API
  or command. In this case, all of the jobs would be blocking
  each other on the first call, preventing any from making
  subsequent calls, and thus the jobs expire (but the function
  call would keep going). Now, only 10 function calls can be
  made in parallel, which will reduce contention and ensure
  that they can complete before job expiry. This, combined with
  checking the job expiry time, should prevent flooding
  of the system, and creating of long chains / queues of jobs
  that never complete.
- Added timeouts to REST API calls and for running external
  commands. These timeouts (60 seconds) ensure that if any
  command or REST call takes too long, then they will be terminated
  and an error returned. This is important, as calling a REST
  API or running a command is serialised (held behind a mutex)
  to ensure that OpenPortal doesn't flood downstream services
  (OpenPortal only makes one FreeIPA rest call, or one SLURM
  call at a time). Previously, a failure of, e.g. FreeIPA,
  could cause the freeipa agent to hang indefinitely, as the
  REST API call would never return. Now, if the call takes
  longer than 60 seconds, it will be aborted, and an error
  returned. This, combined with all of the changes described
  above should make the whole OpenPortal more robust
  and resilient to errors and job storms.

## [0.12.1] - 2025-06-04

### Added

- Added a "signal_url" that can be called by the bridge to signal
  the connected web-portal that a new job has been submitted and
  is awaiting processing. The Job ID is submitted as a query
  parameter, providing an effective shared secret that the
  connected web-portal can use to fetch the job from the bridge.
- Added support for more instructions that can be sent to the
  connected web-portal. These are `get_project_mapping`
  and `get_usage_report`. Added in the python wrapping for
  `DateRange.parse` so that we can easily go from the string
  representation to the `DataRange` object.

## [0.12.0] - 2025-06-03

### Added

- Added new commands to support the creation and updating of projects in
  attached portals. These are `create_project`, `update_project`, and
  `get_project`, all of which use new `ProjectDetails` and `ProjectClass`
  objects to describe the project in more detail.
- Added the ability for portals to send commands to other portals. This allows
  a "higher level" portal to send `create_project` and `update_project`
  commands to a "lower level" portal, which will create the project there.
  Then, other commands (such as `get_usage_report` and `get_project`)
  can be used to query and get data about projects owned by the
  "higher level" portal.
- Added the ability for the bridge agent connected to a portal to also
  send `create_project`, `update_project` and `get_project` commands
  to its attached portal. This allows the Python interface created
  via OpenPortal to be used to create and manage projects directly
  in the portal, without needing to use the web portal's own API.
  This should simplify automation of project creation and management.
- Added a bridge-side job board, so that jobs sent from a portal
  to a bridge (so that the attached web portal can access and process
  them) can now be accessed from Python, processed, and then the
  results sent back to the portal. The web-portal, via a Python interface,
  can now call `fetch_job`, `fetch_jobs` and `send_result` to
  get any jobs sent to the bridge, and send back the results.
- Added more functions to the Python API and made it easier to use.
  Can now properly use the `Job` class from Python, have more
  detail about the job status, and the `Instruction` class now
  has functions to get the job command and arguments. This should
  make it much easier to interface web-portals with OpenPortal
  via the Python API, and to write Python scripts that automate
  project creation and management.

### Fixed

- Fixed a bug where `job.wait()` could wake up even if the job
  has not yet completed. This left the `result` as empty,
  which was surprising behaviour. Now, `job.wait()` will
  keep waiting until the job has completed - with a safeguard
  that if it re-awakens more than 10 times it will return
  an error. This should prevent an infinite loop. This fixes
  issue #12.
- Fixed a bug where FreeIPA was allowed to create user accounts
  when the home directory was not set. Now, this will raise an
  error. This fixes issue #13.

## [0.11.0] - 2025-05-21

### Added

- Added support for high availability (HA) for client OpenPortal agents.
  This allows for client agents to be run on multiple nodes, with only
  one node being active at a time, and automatic failover to other nodes
  if the active node fails. The failover also ensures that all client
  connections are failed over to the same node. Note that HA for
  server agents is not yet supported - including agents that act
  as both servers and clients. There should still only be a single
  instance of such agents in a network. HA for server agents
  in planned.
- Added command line options to support rotating of client and server
  keys. Use the `client --rotate name --zone zone` on a server to
  rotate the keys for the specified client in the specified zone. This
  will write out a key rotation file, which can be passed to the
  client via the `server --rotate filename` option.

## [0.10.0] - 2025-04-01

### Added

- Added ability to specify the partition used for accounting for a slurm
  account. This is useful if different services use different partitions
  to separate out the accounting.
- Added getting and setting of slurm limits, updating the set_limit
  command to access a unit designator (e.g. "hours"). Defaults to
  seconds if not specified.
- Cleaned up the log messages so the agents are less chatty at the
  "INFO" level, and the flow of jobs through agents is easier to follow.

## [0.9.6] - 2025-03-05

### Added

- Added ability to control the parent account of slurm accounts that
  are added via openportal. This defaults to "root" (the default), but
  can be changed by setting the `parent_account` option in the
  `op-slurm` configuration file. Note that the parent account must
  exist already - if it doesn't, then the account creation will fail.

## [0.9.5] - 2025-02-20

### Added

- Added an environment variable to turn on checking of the user
  class in FreeIPA. This is the double-check that isn't really needed
  and gets in the way now. The default is to not check the user
  class is "openportal". Setting the environment variable
  `OPENPORTAL_REQUIRE_MANAGED_CLASS` to `true` will turn on the check.

### Fixed

- Made the logic for modifying users in FreeIPA more robust - now always
  re-fetch if the user is in the openportal group so that this info
  is always up to date.
- Cleaned up the logic for removal - a user will be removed even if
  they aren't in any of the resource instance groups. This removed an
  edge case where they were not in a resource instance group, but were
  still active, but openportal would not remove them.

## [0.9.4] - 2025-02-20

### Fixed

- Made sure that RUST_LOG_FORMAT is configurable from the helm chart.

## [0.9.3] - 2025-02-20

### Added

- Added configurable logging - output now respects the value of the
  `RUST_LOG` environment variable, using the standard `env_logger` crate.
- Added json logging, which is controlled by the `RUST_LOG_FORMAT` environment
  variable. If this is set to `json`, then logs will be output in JSON format.
- Fixed a communications flood caused by a connection not detecting if
  multiple watchdog messages are already in flight. Now only a single
  watchdog message is pending send, using the same mechanism as the
  keepalive messages.

## [0.9.2] - 2025-02-19

### Fixed

- Fixed incorrect handling of the `cluster` field in slurm that meant
  that race conditions prevented users and accounts from being properly
  added to multiple clusters within the same slurmd instance.

## [0.9.1] - 2025-02-18

### Added

- Added a command to force a disconnect of an open connection. Changed
  the keepalive logic so that, if a keepalive message can't be sent,
  then the connection is automatically disconnected and remade. This
  should prevent hangs caused by one half of a connection being down.
- Added a "last activity" tracker to the connections, and a periodic
  watchdog that checks for connections that have been inactive for
  more than 5 minutes (much greater than the keepalive period).
  This will automatically disconnect the connection, and log a warning,
  with the connection automatically remade. This should prevent
  connections getting stuck in a stuck half-open state.
- Updated to support the latest version of rust, plus to use the latest
  version of all dependencies. This includes upgrading to the new
  secrecy 0.10 from 0.8, which required internal code changes. This
  doesn't impact anything external.

## [0.9.0] - 2025-02-10

### Added

- Added instructions to ask for the home and project directories for a
  user and project.
- Changed the order of creating a user account, so that now `op-freeipa`
  will ask `op-filesystem` for the expected home account details before
  actually creating the account. This way, the home directory can be
  part of the account creation process, preventing FreeIPA from triggering
  the creation of home directories in the wrong place.
- Added FreeIPA groups that record which OpenPortal instances a user is
  a member of. This lets OpenPortal know if a user is a member of multiple
  instances, thus preventing removing a user from one instance from
  removing them from all instances. This also adds some additional layers
  of protection against accidental removal of users from instances.
- Added mutex locking around adding / removing individual users in
  `op-freeipa` and `op-slurm`, and around each directory creation
  operation in `op-filesystem`. This removes the possibility of many
  race conditions, and that we aren't going to accidentally try to
  add and remove a single user at the same time. New processes try to
  get the lock for 10 seconds, and if they can't, they will return an
  error.

## [0.8.3] - 2025-02-06

### Fixed

- Improved logging to reduce chattiness and improve clarity
- Reduced timeout values so that missing agents won't cause the system
  to get too stuck in loops

## [0.8.2] - 2025-02-06

### Fixed

- Extra protections to ensure that agents are connected to the cluster
  before it attempts anything, and to return valid results if existing
  protected users exist

## [0.8.1] - 2025-02-06

### Fixed

- Stopped the freeipa agent from removing groups! This can lead to GID
  information being lost, and is not what we want. Instead, we now
  remove the user from the group, and leave the group alone. Now, if the
  group with the same name is recreated, it will recover its previous
  GID.

## [0.8.0] - 2025-02-05

### Added

- Added a "is_protected_user" instruction, to allow querying for user accounts
  that should not be managed by OpenPortal. This is useful for accounts that
  exist and are managed by other systems, but which need to be seen by
  portals interfacing via OpenPortal

## [0.7.0] - 2025-02-04

### Added

- Added in convenience functions to the Python API to make it easier
  to query dates.

## [0.6.2] - 2025-02-04

### Fixed

- General bugfixes in how the slurm accounting evaluated job consumption data.
- General bugfixes related to how agents handle mulitple slurm clusters.

## [0.6.1] - 2025-02-03

### Added

- Added support for legacy BriCS accounts and projects

## [0.6.0] - 2025-01-27

### Added

- Added commands to get and set usage limits. These are recorded, but
  not yet translated into slurm (that will be for a future release - currently
  they are just used to link with Waldur).
- Added lots of convenience functions and converters for date ranges,
  to make requesting of older reports easer.
- Added lots of converters for usage quantities, plus converters for
  constructors. Prettier print output too.

## [0.5.0] - 2025-01-23

### Added

- Added full accounting support. Can now get accounting data from slurm
  and return this as `UsageReport` and `ProjectUsageReport` objects
  that are also accessible from Python.
- Cleaned up the logging so the output is cleaner and easier to follow
- Made the FreeIPA interface even more robust, handling even more errors
  and edge cases.

## [0.4.0] - 2025-01-03

### Added

- Added per-message encryption keys, using a per-connection pair of
  random salts and randomly generated additional infos per message.
  This is a breaking change in the communication format, so agents
  older that this release will not be able to communicate with
  newer agents.
- Added the ability to construct most of the python-exposed objects
  in Python by mapping the parse functions to Python constructors.
  This will make it easier to save objects to strings, and then
  reconstruct as needed.
- Added the ability to ignore invalid SSL certificates when connecting
  to a FreeIPA server, if the environment variable
  `OPENPORTAL_ALLOW_INVALID_SSL_CERTS` is equal to `true`. The default
  is `false`, so that invalid certificates are not allowed.
  This should only be used in development or debugging, as use
  in production is a security risk.
- Added a check so we can't query projects from the wrong portal.

## [0.3.0] - 2024-12-23

### Added

- Added a `PortalIdentifier` so that we are clean in how we identify
  the three parts; User, Project and Portal
- Added parse pattern for all identifiers - they can now only
  be parsed, and will always be valid if created.
- Added functions to list projects and users, so that we can now
  fully integrate with Waldur. These cache the results from FreeIPA,
  so shouldn't hit the server too hard.
- Added functions to remove users and projects, which are fully
  functional for FreeIPA and stubbed for slurm and filesystem.
  Removed users are disabled in FreeIPA, and are re-enabled
  if they are re-added. This ensures that their stats plus their
  UIDs etc are preserved. Removing a project will remove all
  of the users in the project.
- Added new Python return types, namely Vector/List versions of all
  of the base types (`String`, `UserIdentifier`, `ProjectIdentifier`, etc),
  plus the new `PortalIdentifier` and `Vec<PortalIdentifier>`.
  This triggered the bumped minor version as the API has changed.

## [0.2.0] - 2024-12-17

### Added

- Added some extra functions to the Python layer to make it easier to
  integrate OpenPortal with, e.g. Waldur. These include `is_config_loaded`
  to check if the config has been loaded, and `get` to get the
  Job that matches the passed ID.
- Added automatic building of Python Linux aarch64 binaries, so that
  the Python module can be used on ARM64 systems.
- Cleaned up the Python API and added in lots of convenience functions.
  Objects are now correctly returned from the `run` function, so that you
  don't need to parse anything. Also added in the ability to default
  wait for a command to run
- Added in extra commands to add and remove projects, list users in a
  project, and list projects in a portal. Some of these are still stubbed.
- Added in `ProjectIdentifier` and `ProjectMapping` to mirror the
  equivalent `User` classes. Also cleaned up the concept of local
  users and groups, so that a `UserMapping` maps a user to a local
  unix username and unix group, while the `ProjectMapping` maps a
  project to a local unix group.

## [0.1.1] - 2024-12-02

### Added

- Added `instance_groups` to the FreeIPA agent, so that is is possible to
  specify additional groups that a user should be added to when they are
  added from a specific instance. This is useful when multiple instances
  share the same freeipa agent, and you want to add them to different groups.

## [0.1.0] - 2024-11-26

### Added

- Added full recovery support, so that agents can restore their boards
  after they restart. Also added a queue, so that messages are queued
  if the agent is down. Plus added a wait when looking for agents, so that
  time is given for an agent to first connect and identify itself. All of
  this makes the system more robust and reliable, as most jobs are now
  tolerant of individual agents going down.
- Added a better handshake so that agents communicate both their comms
  engine details (e.g. paddington version 0.0.25) and their agent
  engine details (e.g. templemeads version 0.0.25). This will future proof
  us if we make any future changes to the protocols. Note that this
  is BREAKING, so agents cannot commnunicate with older versions of
  openportal
- Added an expiry to jobs, default to 1 minute, that means that both
  jobs are now cleaned automatically from boards once expired (by a
  quiet background tokio task), and that putter of jobs can get a signal
  that the job has expired, and thus return an error, if the job gets
  lost in the system. This is a breaking change, as the job expiry
  is a new field. It again significantly improves the robustness of the
  system, both stopping putters getting stuck indefinitely, and also
  preventing memory exhaustion by jobs that are never cleaned up. Have
  set the bridge agent to put jobs with a expiry of 60 minutes, so that
  there is plenty of time for the web portal to fetch the results without
  worrying about them being expired.
- Added a command line support for the slurm agent, so that it can use
  `sacctmgr` to create accounts on slurm in addition to the REST API.
  You choose the command line option by not setting the `slurm-server`
  value in the config file.

### Fixed

- General bug fixes and cleaning of output logging to improve resilience
  and make it easier to debug issues.

## [0.0.25] - 2024-11-20

### Fixed

- Fixed attestation issue for slurm container

## [0.0.24] - 2024-11-20

### Added

- Added control over the lifetime of the slurm JWT token, plus a check
  to automatically refresh the token before it expires.

### Fixed

- Fixed the lack of op-slurm containers and helm charts - these are now
  built automatically by GH Actions

## [0.0.23] - 2024-11-19

### Added

- Added in a slurm agent as an example of an accounting agent. This can
  now create accounting accounts on slurm when a user is added to
  a cluster. The slurm account is created with the mapped username
  and project name via the `add_local_user` command, in a similar
  way to how the filesystem agent works. This uses the slurm REST
  API to create and manage the account, using JWT tokens for
  authentication.

## [0.0.22] - 2024-11-13

### Added

- Finished the "AddLocalUser" command for the filesystem agent. User home
  dirs and project dirs are now created, following admin settings. This
  includes multiple project dirs, plus links between dirs. Multiple checks
  ensure that directories are only created if they don't exist, and that
  they aren't created if the user or group don't exist. Also, checks to
  ensure that they aren't written to anywhere sensitive on the filesystem.

## [0.0.21] - 2024-11-12

### Added

- Moved all command and grammar parsing fully over to the parse pattern.
  You cannot now create any commands that aren't valid. Added lots of
  extra tests of validity, e.g. that commands that impact users must
  come from the portal that manages that user.
- Separated out the bridge so that it communicates via the portal in a
  different zone. Added a "submit" command that is only used by the
  bridge to submit instructions to the portal. Added lots of strict
  validation to ensure the bridge<=>portal connection is verified and
  all comamnds are sane, and pass all of the about parse tests.
- Related to the above, changed commands so that you now don't specify
  the bridge<=>portal connection when submitting commands via python.
  You would now do "portal.provider.platform.instance add_user user.project.portal",
  rather than "bridge.portal.provider.plaform.instance ...". It is a small
  change, but it is easier to understand, and now the bridge is just
  an invisible bridge between the "normal" work and the OpenPortal world.

## [0.0.20] - 2024-11-08

### Added

- Added the concept of zones. Agents can now only send messages along chains
  within the same zone. This increases security, and makes it easier to
  segment the agent peer network into different zones (with some agents
  acting as bridges between multiple zones).

## [0.0.19] - 2024-11-07

### Fixed

- Made the code more robust to freeipa being cleared / having groups removed
  behind our back. Also better way to handle errors.

## [0.0.18] - 2024-11-05

### Fixed

- Specified default TLS provider so that containerised services can run without
  panicing.

## [0.0.17] - 2024-11-01

### Fixed

- Fixed issues with attestations that depended on releases. Need to release
  each agent separately, which this release now does.

## [0.0.16] - 2024-11-01

### Fixed

- Fixed issue with attestation of OCI images

## [0.0.15] - 2024-11-01

### Fixed

- Fixed issues with the helm charts and OCI images (removed `op-platform` as it
  doesn't exist!)

## [0.0.14] - 2024-11-01

### Added

- Changed the names of the cluster instance and platform agents to `cluster` and `clusters`,
  as they don't need to be named after slurm (and would cause confusion with the slurm agent).
- Added OCI images and helm charts for all agents
- Added instructions on how to configure the freeipa agent

## [0.0.12] - 2024-10-28

### Added

- Added support for keepalive messages so that connections are kept open

## [0.0.11] - 2024-10-28

### Added

- Fixed bug in handling of client proxy IP - need to use IP not port ;-)

## [0.0.10] - 2024-10-25

### Added

- Fixed bug in parsing header proxy IP address

## [0.0.9] - 2024-10-25

### Added

- Fixed bug in parsing command line options for bridge
- Added support for getting the client IP address from a proxy header (e.g. `X-Forwarded-For`)
- Cleaned up port handling, so URLs with default ports don't have the ports specified

## [0.0.8] - 2024-10-24

### Added

- Added names for the ports in the helm charts

## [0.0.7] - 2024-10-24

### Added

- Added a healthcheck server to simplify pod healthchecks
- Updated helm charts to use the healthcheck server, plus expose the bridge server port

## [0.0.6] - 2024-10-23

### Added

- Separated out build artefacts so that they can be picked up by the rest of the build

## [0.0.5] - 2024-10-23

### Added

- Fixing generation and attestation of SBOMs for container images (finally!)

## [0.0.4] - 2024-10-23

### Added

- Fixing release issues, and beginning work on the workflow for the Python module

## [0.0.3] - 2024-10-23

### Added

- Fixing the attestations so that SBOMs are correctly generated for container images.

## [0.0.2] - 2024-10-23

### Added

- Fixing the helm charts so that they version numbers are correctly set.

## [0.0.1] - 2024-10-23

### Changed

- Initial release
  This is an initial alpha release of the OpenPortal project. It is not yet feature complete and is not recommended for production use.

[0.92.0]: https://github.com/isambard-sc/openportal/releases/tag/0.92.0
[0.91.0]: https://github.com/isambard-sc/openportal/releases/tag/0.91.0
[0.90.0]: https://github.com/isambard-sc/openportal/releases/tag/0.90.0
[0.32.2]: https://github.com/isambard-sc/openportal/releases/tag/0.32.2
[0.32.1]: https://github.com/isambard-sc/openportal/releases/tag/0.32.1
[0.32.0]: https://github.com/isambard-sc/openportal/releases/tag/0.32.0
[0.31.0]: https://github.com/isambard-sc/openportal/releases/tag/0.31.0
[0.30.1]: https://github.com/isambard-sc/openportal/releases/tag/0.30.1
[0.30.0]: https://github.com/isambard-sc/openportal/releases/tag/0.30.0
[0.29.0]: https://github.com/isambard-sc/openportal/releases/tag/0.29.0
[0.28.0]: https://github.com/isambard-sc/openportal/releases/tag/0.28.0
[0.27.1]: https://github.com/isambard-sc/openportal/releases/tag/0.27.1
[0.27.0]: https://github.com/isambard-sc/openportal/releases/tag/0.27.0
[0.26.0]: https://github.com/isambard-sc/openportal/releases/tag/0.26.0
[0.25.0]: https://github.com/isambard-sc/openportal/releases/tag/0.25.0
[0.24.0]: https://github.com/isambard-sc/openportal/releases/tag/0.24.0
[0.23.1]: https://github.com/isambard-sc/openportal/releases/tag/0.23.1
[0.23.0]: https://github.com/isambard-sc/openportal/releases/tag/0.23.0
[0.22.4]: https://github.com/isambard-sc/openportal/releases/tag/0.22.4
[0.22.3]: https://github.com/isambard-sc/openportal/releases/tag/0.22.3
[0.22.2]: https://github.com/isambard-sc/openportal/releases/tag/0.22.2
[0.22.1]: https://github.com/isambard-sc/openportal/releases/tag/0.22.1
[0.22.0]: https://github.com/isambard-sc/openportal/releases/tag/0.22.0
[0.21.1]: https://github.com/isambard-sc/openportal/releases/tag/0.21.1
[0.21.0]: https://github.com/isambard-sc/openportal/releases/tag/0.21.0
[0.20.2]: https://github.com/isambard-sc/openportal/releases/tag/0.20.2
[0.20.1]: https://github.com/isambard-sc/openportal/releases/tag/0.20.1
[0.20.0]: https://github.com/isambard-sc/openportal/releases/tag/0.20.0
[0.19.1]: https://github.com/isambard-sc/openportal/releases/tag/0.19.1
[0.19.0]: https://github.com/isambard-sc/openportal/releases/tag/0.19.0
[0.18.0]: https://github.com/isambard-sc/openportal/releases/tag/0.18.0
[0.17.0]: https://github.com/isambard-sc/openportal/releases/tag/0.17.0
[0.16.3]: https://github.com/isambard-sc/openportal/releases/tag/0.16.3
[0.16.2]: https://github.com/isambard-sc/openportal/releases/tag/0.16.2
[0.16.1]: https://github.com/isambard-sc/openportal/releases/tag/0.16.1
[0.16.0]: https://github.com/isambard-sc/openportal/releases/tag/0.16.0
[0.15.1]: https://github.com/isambard-sc/openportal/releases/tag/0.15.1
[0.15.0]: https://github.com/isambard-sc/openportal/releases/tag/0.15.0
[0.14.0]: https://github.com/isambard-sc/openportal/releases/tag/0.14.0
[0.12.1]: https://github.com/isambard-sc/openportal/releases/tag/0.12.1
[0.12.0]: https://github.com/isambard-sc/openportal/releases/tag/0.12.0
[0.11.0]: https://github.com/isambard-sc/openportal/releases/tag/0.11.0
[0.10.0]: https://github.com/isambard-sc/openportal/releases/tag/0.10.0
[0.9.6]: https://github.com/isambard-sc/openportal/releases/tag/0.9.6
[0.9.5]: https://github.com/isambard-sc/openportal/releases/tag/0.9.5
[0.9.4]: https://github.com/isambard-sc/openportal/releases/tag/0.9.4
[0.9.3]: https://github.com/isambard-sc/openportal/releases/tag/0.9.3
[0.9.2]: https://github.com/isambard-sc/openportal/releases/tag/0.9.2
[0.9.1]: https://github.com/isambard-sc/openportal/releases/tag/0.9.1
[0.9.0]: https://github.com/isambard-sc/openportal/releases/tag/0.9.0
[0.8.3]: https://github.com/isambard-sc/openportal/releases/tag/0.8.3
[0.8.2]: https://github.com/isambard-sc/openportal/releases/tag/0.8.2
[0.8.1]: https://github.com/isambard-sc/openportal/releases/tag/0.8.1
[0.8.0]: https://github.com/isambard-sc/openportal/releases/tag/0.8.0
[0.7.0]: https://github.com/isambard-sc/openportal/releases/tag/0.7.0
[0.6.2]: https://github.com/isambard-sc/openportal/releases/tag/0.6.2
[0.6.1]: https://github.com/isambard-sc/openportal/releases/tag/0.6.1
[0.6.0]: https://github.com/isambard-sc/openportal/releases/tag/0.6.0
[0.5.0]: https://github.com/isambard-sc/openportal/releases/tag/0.5.0
[0.4.0]: https://github.com/isambard-sc/openportal/releases/tag/0.4.0
[0.3.0]: https://github.com/isambard-sc/openportal/releases/tag/0.3.0
[0.2.0]: https://github.com/isambard-sc/openportal/releases/tag/0.2.0
[0.1.1]: https://github.com/isambard-sc/openportal/releases/tag/0.1.1
[0.1.0]: https://github.com/isambard-sc/openportal/releases/tag/0.1.0
[0.0.25]: https://github.com/isambard-sc/openportal/releases/tag/0.0.25
[0.0.24]: https://github.com/isambard-sc/openportal/releases/tag/0.0.24
[0.0.23]: https://github.com/isambard-sc/openportal/releases/tag/0.0.23
[0.0.22]: https://github.com/isambard-sc/openportal/releases/tag/0.0.22
[0.0.21]: https://github.com/isambard-sc/openportal/releases/tag/0.0.21
[0.0.20]: https://github.com/isambard-sc/openportal/releases/tag/0.0.20
[0.0.19]: https://github.com/isambard-sc/openportal/releases/tag/0.0.19
[0.0.18]: https://github.com/isambard-sc/openportal/releases/tag/0.0.18
[0.0.17]: https://github.com/isambard-sc/openportal/releases/tag/0.0.17
[0.0.16]: https://github.com/isambard-sc/openportal/releases/tag/0.0.16
[0.0.15]: https://github.com/isambard-sc/openportal/releases/tag/0.0.15
[0.0.14]: https://github.com/isambard-sc/openportal/releases/tag/0.0.14
[0.0.12]: https://github.com/isambard-sc/openportal/releases/tag/0.0.12
[0.0.11]: https://github.com/isambard-sc/openportal/releases/tag/0.0.11
[0.0.10]: https://github.com/isambard-sc/openportal/releases/tag/0.0.10
[0.0.9]: https://github.com/isambard-sc/openportal/releases/tag/0.0.9
[0.0.8]: https://github.com/isambard-sc/openportal/releases/tag/0.0.8
[0.0.7]: https://github.com/isambard-sc/openportal/releases/tag/0.0.7
[0.0.6]: https://github.com/isambard-sc/openportal/releases/tag/0.0.6
[0.0.5]: https://github.com/isambard-sc/openportal/releases/tag/0.0.5
[0.0.4]: https://github.com/isambard-sc/openportal/releases/tag/0.0.4
[0.0.3]: https://github.com/isambard-sc/openportal/releases/tag/0.0.3
[0.0.2]: https://github.com/isambard-sc/openportal/releases/tag/0.0.2
[0.0.1]: https://github.com/isambard-sc/openportal/releases/tag/0.0.1
