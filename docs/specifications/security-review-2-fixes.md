<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# Security review 2 — record of fixes

This document records **what changed and why** in response to
[security-review-2.md](security-review-2.md), which found 34 issues (R1–R34) across
the workspace.

It exists so the CHANGELOG can stay a changelog. A reader who wants to know *what
changed* should read the CHANGELOG; a reader who wants to know *why a control is
shaped the way it is* — including why several recommendations were deliberately not
followed — should read this.

- Each finding's own **Fix applied** note lives in
  [security-review-2.md](security-review-2.md), next to the finding it answers.
- This document groups the same work by **subsystem**, so the rationale for a
  subsystem's controls reads as a whole.
- [§9](#9-deliberately-not-done) lists what was declined, with reasons. That section
  is the one most worth reading if you are auditing this work rather than maintaining
  it.

**Outcome:** all 34 findings resolved — 28 fixed in code, 2 fixed in part and re-rated,
1 falsified on re-check, 1 confirmed not to be a bug, 1 closed as a documentation
change, and 1 whose panic was fixed but whose further validation was declined. Verified by 298 unit tests, a
clean `cargo clippy --all-targets --all-features -- -D warnings`, a clean release
build with `overflow-checks` newly enabled, and live integration testing against
running agents.

---

## 1. The theme

Round 1 established that OpenPortal's cryptography and key topology are sound, and
round 2 re-tested that with stronger methods and agreed. What round 2 found instead
was a **single recurring shape** in the layer above:

> paddington establishes an authenticated transport identity correctly, and
> templemeads then made authorization decisions from **wire data** rather than from
> that identity.

A Job's asserted origin was never compared with the peer that delivered it
([R4](security-review-2.md#r4)). An agent's *type* — which decides whether it may
submit jobs, restart peers or be selected as the account agent — was whatever the
peer claimed ([R3](security-review-2.md#r3)). Nothing bound a command to the portal
whose authority it invoked ([R34](security-review-2.md#r34)).

The fixes therefore cluster into: **bind decisions to locally-trusted state**, and
**bound anything a peer can grow**.

---

## 2. Binding authority to local state

### Sender adjacency ([R4](security-review-2.md#r4))

`Destination::position` required only that the sender's name appear *somewhere* in an
attacker-supplied path. It now requires the sender to be the **immediately adjacent**
hop — `previous_index + 1 == agent_index` travelling downstream, or the reverse
travelling upstream. Anything else is `Position::Error`.

This is the check that makes a Job's claimed route mean something: the sender is
stamped by paddington from the authenticated connection, so adjacency ties the route
to a peer that actually holds keys for that position.

### Declared peer types ([R3](security-review-2.md#r3))

A `[[clients]]`/`[[servers]]` entry may declare `type = "..."`, and a peer whose
`Register` claims a different type is refused. Unset means unchecked, so an existing
config keeps working and the check is adopted per peer.

Two follow-ups matter more than the original change:

- **The declaration is no longer hand-edited.** `client --add --type bridge` records
  what a client must be, validated against the nine known types *at add time* so a
  typo fails while an operator is watching rather than being written to disk and
  silently discarded at startup. The reverse direction travels in the invite: the
  issuer declares its own type, so `server --add` picks it up with no manual step.
- **The invite never carries the client's expected type.** That would invert the
  direction of trust. Trusting the issuer's *own* type is sound for the same reason
  trusting the invite's `name` and keys is — the file arrives by deliberate operator
  action, not over the wire. What the finding distrusts is the role a peer *claims at
  registration*, which is now checked against it.

An invite written by an older version has no `type` field, imports cleanly, and
leaves the peer unchecked.

### Portal ownership, and why it is zone-scoped ([R34](security-review-2.md#r34))

The rule "an instruction naming portal X may only arrive via a destination rooted at
X" was enforced only at the system's entry points. It now also runs at the agent that
*acts* — the Job's terminal hop — decided entirely from locally-trusted state
(`Domain::owning_portal`, plus the agent's own declared type).

**This finding's premise was wrong, and the correction is the interesting part.** The
rule was an entry-point validation *because the invariant does not hold internally*:
`op-freeipa` legitimately builds `freeipa.shared get_local_home_dir john.proj.brics`
while adding a user — naming a portal on a destination rooted at itself — and passes
`check_portal = false` because the rule does not apply to a trusted internal
delegate. A receiver cannot distinguish such a Job from an injected one by inspecting
the Job.

So the missing control was never "re-check the same rule downstream" but "distinguish
portal-originated from delegate-originated traffic". The **zone** is the mechanism
this architecture already had for that, and the derived portal-route table already
encodes it. The check therefore applies **only in a zone where a portal route is
known** — an instance's upstream zone, never the internal zone holding the agents it
delegates to. See
[portal-route-discovery-design.md](../plans/portal-route-discovery-design.md) §4.7.

A consequence worth stating plainly: **declaring `type = "portal"` on a peer is what
activates this family of controls.** An estate that has not done so has no
portal-ownership enforcement on the wire.

### What remains, and is accepted

These controls are **positional, not cryptographic**. Together they move the
requirement from "compromise one agent" to "compromise one agent *and* have a peer
provisioned under a chosen name, in a chosen topology position, with a chosen role".
Command signing was designed, costed and consciously deferred — the reasoning is in
[security-review-2.md §4.1](security-review-2.md).

---

## 3. Panics and arithmetic

### The reachable abort ([R1](security-review-2.md#r1))

Three arms of `Instruction::parse` indexed their argument list without a length
check, and that parser runs inside `serde`'s `Deserialize` for `Command`. With
`panic = "abort"` in the release profile, a ~200-byte message from any authenticated
peer terminated the process. Proven by execution.

The fix went well beyond the three sites: **every** panicking index and slice
operation in the workspace's own code is now a checked form, and
`clippy::indexing_slicing` is denied workspace-wide so the class is closed
structurally rather than one site at a time. Enabling it turned up exactly one
non-test violation — `impl Index<usize> for Destinations`, whose only purpose was to
*offer* a panicking accessor, and which had no callers.

### Checked arithmetic, then `overflow-checks` ([R33](security-review-2.md#r33))

Release builds now set `overflow-checks = true`. That is only safe because the
arithmetic reachable from the wire saturates explicitly first — with
`panic = "abort"`, a newly-checked overflow would be a remote process kill:

- `Usage` and `StorageSize`: every operator saturates; `Div`/`DivAssign` use
  `checked_div`, since division by zero panics regardless of the flag and a zero
  divisor is reachable from a report file.
- `Usage`'s `-` and `-=` had **disagreed** — one clamped at zero, the other wrapped to
  near `u64::MAX`.
- `Allocation` accepted `"NaN"`, `"inf"` and `"infinity"`, because `f64::from_str`
  parses them and the existing `size < 0.0` test is *false* for NaN. Both then
  saturated to `u64::MAX` downstream. This was a latent hole that only surfaced while
  making the saturation explicit.

### Bounded date handling ([R25](security-review-2.md#r25))

Year range, span cap, checked arithmetic, and terminating `months`/`years` loops. The
`weeks()` Monday roll-back panicked at `NaiveDate::MAX` — found by a test written for
this finding, not by the finding itself.

---

## 4. Bounding what a peer can grow

Every one of these is a peer-supplied quantity that nothing capped.

| What | Bound | Note |
|---|---|---|
| Job `expires` ([R31](security-review-2.md#r31)) | ≤ 1 hour after creation | Reaping is the *only* thing bounding a board, so a peer-chosen far-future expiry meant a Job was never reaped. Clamped in `Board::add`, the single point every wire Job passes. |
| Jobs per board | 10,000 | Enforced only for Jobs not already held, so an update always gets through and a full board can drain. |
| Boards per process | 1,000 | Checked *before* the `or_insert`, so an attacker-chosen destination name no longer leaves a permanent board behind. |
| Queued commands per board | 1,000 | These accumulate while a peer is unreachable. |
| `Command::Sync` payload | 10,000 Jobs | Re-injected into the inbound channel, so an oversized one is both a large allocation and a large amount of work. |
| WebSocket frames ([R22](security-review-2.md#r22)) | 2 MiB, both directions | |
| Handshake ([R21](security-review-2.md#r21)) | 30 s pre-auth deadline | The finding's own residual said slots "free eventually"; they never did, because there was no timeout at all. |
| Bridge request body ([R24](security-review-2.md#r24)) | 1 MiB (was 2 MiB) | This is what bounds the **pre-authentication** HMAC-SHA512 and the second ~2 MiB `String` copy `sign_api_call` used to format. |
| Bridge concurrency | 512, fail-fast 503 | Mirrors paddington's `MAX_UNAUTHENTICATED_CONNECTIONS`. |
| Bridge request deadline | 30 s | Also bounds the slow-*body* half of a slowloris, since the extractor runs inside the handler. |
| Rate-limit table | 8,192 addresses | Pruned deterministically inside the lock it already holds. The old sweep claimed 1% but was 1.17%, and ran an O(n) `retain` on the pre-auth path. |
| Nonce store | 100,000, evict oldest | Correctness comes from the per-entry TTL check, so purging is now lazy rather than an O(n) scan per request under one global mutex. At the cap it **evicts** rather than 503s — the old behaviour failed *only nonced* requests, pushing clients towards the unprotected mode. |
| Health / diagnostics caches ([R20](security-review-2.md#r20)) | 1,024 / 256 | Least-recently-received eviction. |
| Pending relay bootstraps ([R28](security-review-2.md#r28)) | 256 | |
| Slurm/FreeIPA caches ([R33](security-review-2.md#r33)) | see below | |

### The Slurm and FreeIPA caches deserve their own note

Fetching usage data taxes `slurmctld`, so **dropping a cache entry is expensive here
in a way it is not for most caches**. The caps are therefore set for a large national
facility rather than a small one, and the policy is **evict, never flush**:

| Map | Cap |
|---|---|
| Slurm accounts | 10,000 |
| Slurm users | 100,000 |
| Slurm usage-report projects | 10,000 |
| Days of usage per project | 100 (≈1,000,000 daily reports overall) |
| FreeIPA users | 100,000 |
| FreeIPA groups, group memberships | 100,000 |
| FreeIPA instance groups | 1,000 |
| Mutex maps | 100,000 |

Three things make this correct rather than merely bounded:

- **Date-keyed maps drop the *oldest* date**, which is the natural policy for a time
  series and keeps the recent end that queries want.
- **Identifier-keyed maps drop one arbitrary entry**, because `HashMap` offers no
  access order. Losing one project costs one re-fetch; flushing cost everything.
- **The mutex maps are handled differently.** They are the identity of a lock, not a
  cache, so dropping one while a task holds its `Arc` would hand the next caller a
  *different* mutex for the same user and silently lose mutual exclusion. Only
  entries nobody holds are dropped — `strong_count == 1` means the map is the sole
  owner.

Two properties make the caps effectively unreachable by an attacker, and both were
undocumented and load-bearing, so they are now stated in the code:

1. **There is no negative caching.** `cache::add_account`/`add_user` are called only
   after a fetch that returned `Some`, so a query for a project that does not exist
   in Slurm caches nothing. The cache can only ever hold as many entries as there are
   real Slurm objects. A future change that added negative caching would reopen the
   eviction-amplification attack this closes.
2. **Concurrent Slurm work is already bounded** by the fixed-size `SLURM_RUNNERS` /
   `PRIORITY_RUNNERS` pools, each behind a `Mutex`, with `runner()` polling until one
   frees.

Separately, a cache/Slurm discrepancy used to call `cache::clear()` — a **wholesale
flush** of exactly the expensive cache. All twelve sites now evict only the named
account or user, via `cache::remove_account`/`remove_user`: a discrepancy about one
account says nothing about any other, and flushing meant every project re-queried
`slurmctld` at once.

---

## 5. Transport and relay

### The restart lockout ([R10](security-review-2.md#r10))

The handshake anti-replay design kept the outgoing nonce *counter* and the incoming
*replay window* in the same process-lifetime structure. A restart reset the counter
while the peer's window remembered where it had got to — so a **routine restart**
locked an agent out of every long-running peer for a period proportional to that
peer's accumulated reconnect count. Any deploy triggered it.

Fixed with a **random per-process epoch** and one replay window per sender
incarnation. Monotonic epochs were considered and ruled out: several processes share
one `name@zone` identity for HA, and agents cannot write state to local disk, so
there is nothing to make monotonic. Validated against real agents through repeated
disconnect/reconnect cycles, directly connected and via a proxy.

### Relay envelopes are bound to their connection ([R16](security-review-2.md#r16))

`envelope.from` is a wire field. F7 bound it to the authenticated sender *on the
proxy*; there was no receive-side counterpart. Any direct peer — a portal, a second
proxy — could inject `{from:"A",to:"us",…}` for a relayed pair it had no part in, and
the receiver would emit a genuine key-signed `SessionUnknown` to A for every packet,
churning their session.

The receiver now requires the envelope to have arrived over the connection to the
relay its own config names for that peer. Note this is **not** covered by the
adjacency check: adjacency applies to a Job's `Destination`, while a relay envelope is
unwrapped inside paddington before any templemeads layer sees it.

The check is a pure `arrived_over_configured_relay`, tested including the case a naive
implementation gets wrong — `relay_zone` (the direct connection to the proxy) is
usually *not* `zone` (the relayed relationship).

### `SessionUnknown` amplification ([R28](security-review-2.md#r28))

A hostile relay could drop every `Start` to keep a peer session-less, then inject junk
envelopes; each one made the peer sign a *real* `SessionUnknown`, which the far side
accepted (the nonces are genuinely fresh, so the replay window is no defence),
dropping its session and spawning a bootstrap task. Steady state was rate × 30 s live
tasks.

Three bounds: `SessionUnknown` debounced to one per peer per 5 s; the re-bootstrap
single-flight per peer rather than one task per message; `PENDING_BOOTSTRAPS` capped.
R16's fix removes the injection step entirely for anything that is not the proxy.

### The replay TOCTOU ([R33](security-review-2.md#r33))

Ongoing traffic is decrypted against a *clone* of the session taken under a read
lock, then the nonce is recorded against the stored session under a write lock. A
re-bootstrap landing between the two installed a **fresh** window, which accepted a
replayed old-session nonce by first-nonce initialisation.

Sessions are now stamped with a generation id, and the nonce is only recorded if the
stored session is still the incarnation the message was decrypted with. The `None`
arm — session vanished mid-flight — now **rejects**; it previously accepted, on the
reasoning "nothing to check against", which is precisely the window a replay wants.

### Other relay hardening

- **All-zero session keys refused** at both relay receive points. F15 added this to
  the two *direct* handshake paths but not the three relay ones, and `Key::derive`
  HKDFs happily from all-zero material, so it silently worked.
- **`RelayEnvelope` is positively identified** — a `kind: "openportal-relay-v1"` tag
  plus `deny_unknown_fields`. Classification was purely structural, so any future
  payload with those four fields would have been misread as an envelope. *This is a
  wire change*, acceptable only because no production deployment uses the relay yet.
- **`PENDING_BOOTSTRAPS` keyed `(peer, magic)`** rather than magic alone. Not
  exploitable — magic is 32 CSPRNG bytes inside the ciphertext — but binding a
  response to the peer it was sent to is free.
- **Duplicate relayed names refused**, on both sides: `configure()` on the agents,
  and `configure_proxy()` plus `client --add` on the proxy. See
  [§9](#9-deliberately-not-done) for why the alternative was declined.

### Salts and keys are validated at import ([R33](security-review-2.md#r33))

`Key` and `Salt` accepted **any** hex length. A truncated key was accepted at import
and failed opaquely at connect time, and `Key::derive` sizes its HKDF output from the
*input* length. Worse, an **absent** salt header decoded to an empty salt: orion's
HMAC accepts an empty key, so the connection succeeded with the per-connection salt
defence silently gone, with no error and no log.

Both now require exactly `KEY_SIZE`/`SALT_SIZE` bytes, validated through a repr
struct so the wire format is unchanged. A missing salt header is reported distinctly
from a malformed one. The legacy XOR-masked salt format is unaffected — un-masking
happens on an already-parsed salt, and both operands are now guaranteed full length
(`Salt::xor` zips, so a short salt used to silently shorten the result too).

### `supports_nonce` is enforced ([R33](security-review-2.md#r33))

The flag was negotiated and then never checked, so an honest upgraded peer gained
nothing by advertising it. A nonce-less payload from a peer that advertised support is
now **rejected**.

This is safe in a mixed fleet because the combination cannot arise between honest
peers: `NoncedPayload::for_peer` omits the nonce *only* when the sender believes the
**recipient** is legacy, and that belief is fixed from `PeerDetails` during the
handshake, before any message is sent. A receiver seeing "this peer advertised
support" together with "this payload has no nonce" is seeing a contradiction. A
genuinely old peer never advertises, so its payloads take the untouched path.

It also closes a downgrade: "advertise support, then omit the nonce" is no longer a
way to skip the replay window.

---

## 6. Secrets at rest

### The missed writer ([R9](security-review-2.md#r9))

F9 restricted config and invite files to `0600` via a shared helper — but the helper
was `pub(crate)` to paddington, so the **bridge invite**, which holds the HMAC API
key, could not reach it and kept using a bare `std::fs::write`, landing at the process
umask.

The helper is now public, and `scripts/check-secret-writes.sh` asserts structurally
that no `fs::write` appears outside a small annotated allow-list. Two rounds of review
found the same regression independently, which is why it is now checked by CI rather
than by reviewers.

### Atomic, symlink-safe writes ([R33](security-review-2.md#r33))

`write_secret_file` now writes to a random-suffixed temporary file opened with
`create_new` and renames it over the target. That gives three things at once:

- **Atomicity** — a concurrent reader sees the whole old file or the whole new one,
  never a partial secret.
- **Symlink safety without `O_NOFOLLOW`.** `create_new` is `O_CREAT|O_EXCL`, which
  POSIX requires to fail if the path exists *at all* — including as a symlink — so the
  temporary file cannot be redirected. And `rename` **replaces** a symlink at the
  destination rather than writing through it, which matters because several invite
  paths default to the current working directory. `O_NOFOLLOW`'s value is
  platform-specific and would have meant a new direct dependency; this needs neither.
- **Durability** — `sync_all` before the rename, so a crash cannot leave an empty file
  where a valid secret used to be.

### Zeroizing, and what `SecretBox` did not cover ([R33](security-review-2.md#r33))

Round 1 §3.7's claim that key material is zeroized is true of the *canonical* copies
only. `get_password` returned a bare `String`, and both invite loaders read the
plaintext TOML — carrying both peer keys in hex — into a plain `String`. All now use
`Zeroizing`, reached through `secrecy`'s re-export so no new dependency.

A bare `Key`'s derived `Debug` also printed its raw bytes; the doc comment claiming
`[REDACTED]` described `SecretBox`, not `Key`. `Debug` is now hand-written.

**That change immediately paid for itself.** Three tests failed once `Debug` stopped
leaking — they were comparing session keys via `format!("{:?}", key)`, which now
compares two identical `[REDACTED]` strings. The finding noted "two tests rely on it,
proving the vector is live"; in fact those assertions were silently *vacuous*.
`Key::equals` (constant-time) was added and the tests rewired.

### Secrets no longer need to appear in `ps` ([R33](security-review-2.md#r33))

`secret --value` puts the secret in argv, visible to every local user while the
command runs. It is kept for compatibility but warns, and `--value-file` (with `-` for
stdin) and bare stdin were added.

### Legacy v0 secrets are announced ([R33](security-review-2.md#r33))

A prefix-less secret stays on the weak fixed-salt derivation indefinitely with nothing
prompting a re-encrypt. Decrypting one now warns once per process, naming the fix. No
downgrade is possible — a v0 ciphertext is pure hex and can never carry the v1 prefix
— so this is operational rather than a live weakness.

Changing the encryption scheme also now **lists the secrets that will stop
decrypting**, since nothing re-encrypts them.

---

## 7. The bridge HTTP API

### Client IP resolution ([R11](security-review-2.md#r11))

F3 added a `trusted_proxy` gate, but the code then took the **left-most**
`X-Forwarded-For` entry — which is client-supplied, since the list is appended to by
each hop. So F3's own stated attack still worked in F3's own recommended deployment: a
client sending `X-Forwarded-For: 1.2.3.4` through a Cloudflare tunnel chose its own
rate-limit bucket.

Resolution now walks **right-to-left** and takes the first entry that is not itself a
trusted proxy, stopping on an unparseable entry rather than skipping past it. A round-1
test had locked in the old behaviour and was replaced.

A missing resolved-IP header now **fails the request** rather than defaulting to
`127.0.0.1`, which silently merged such requests into the loopback bucket.

### Signature version 2 ([R29](security-review-2.md#r29))

The v1 canonical string is `\n`-joined with no length prefixes and no field count, in
one of four un-tagged shapes. Consequently, for a POST:

```
…\n<function>\n<body>\n<nonce>   ==   …\n<function>\n<body ‖ "\n" ‖ nonce>
```

These are the *same bytes*, so the **presence of a nonce is not authenticated**.

Version 2 signs a seven-field string with every field length-prefixed and always
present, led by a length-prefixed `openportal-sig-v2` tag — so no field's content can
be read as a boundary, the arity is fixed regardless of empty bodies, and a v2 string
cannot collide with a v1 one.

**Backwards compatibility was the whole design constraint**, because the bridge's
clients include portal software this project does not control:

- An **absent** `X-OpenPortal-Signature-Version` header means v1, still verified
  byte-for-byte as before.
- An **unrecognised** value is a 400, never a fallback to v1 — otherwise mangling one
  header would downgrade a v2 client.
- Every v1 verification logs at debug level naming the header to add, so the remaining
  v1 clients are discoverable rather than invisible.

Validated live against a running `op-bridge` on both a GET and a POST path, using the
project's own signer: v1-with-no-header 200, v2-with-header 200, v1-with-`1` 200,
v2-without-header 401, v1-with-`2` 401, unrecognised 400.

v1 should be refused once every client is known to send `2`.

### The trust boundary is now documented ([R32](security-review-2.md#r32))

[bridge-api.md](bridge-api.md) gains a **§0** stating that the bridge must run on a
trusted network or behind a TLS-terminating proxy, and *why this is a design choice*:
`op-portal` holds the single internet-facing surface — one WebSocket endpoint speaking
the authenticated, encrypted protocol — while `op-bridge` holds the HTTP control
surface on the private side, so the portal never needs both.

It then states the three consequences plainly: bodies and responses are cleartext with
the HMAC covering the request direction only; there is no pre-header read timeout
(`axum::serve` does not expose one); and the API key authenticates the *portal
software*, not individual users.

Round 1's F12 is cross-referenced in both directions, since its "an on-path attacker
cannot read message content" applies to the paddington wire protocol and **not** to
this API.

---

## 8. Privileged agents

### Slurm ([R5](security-review-2.md#r5), [R14](security-review-2.md#r14), [R27](security-review-2.md#r27))

Round 1 credited Slurm with refusing to act on unmanaged objects. That was true only
on the *create* path, and even there the check ran against a locally-constructed
object whose `organization` is a hard-coded constant — so it could never fail. **No
mutation path checked the organization of the account that actually existed in
Slurm**, so a peer-chosen `local_group` naming any real account on the cluster had its
`GrpTRESMins` rewritten. `set_limit` and both `cancel_pending_*_jobs` now check.

Mapping targets got an allow-list (they had a *deny*-list permitting whitespace and
commas), and names are percent-encoded into REST paths — F5's note that identifier
validation "neutralises the URL-path-injection concern" was inaccurate for mapping
fields, which are interpolated unencoded.

The API-version probe no longer indexes element 2 unconditionally, and increments the
server-supplied patch component with `checked_add`. See
[§9](#9-deliberately-not-done) for why no *validation* of the version string was
added.

The token command's raw stdout no longer appears in an error. That error travels back
up the Job chain, making it the one place credential-bearing output could escape to a
requesting peer — F15 fixed the *logging*, not this.

### FreeIPA ([R33](security-review-2.md#r33))

`is_project_group()` admitted `portal == "openportal"`, so
`remove_project openportal.openportal` passed the guard and resolved to the *managed*
group. The blast radius was zero only by accident — an unrelated filter two layers
down skips every user — which is not something a guard should rely on. It now rejects
all three internal portal names, which also subsumed and removed
`is_system_group()`.

The `OPENPORTAL_ALLOW_INVALID_SSL_CERTS` toggle is announced once per process. This is
a **legitimate operator decision**, not a misconfiguration: FreeIPA is commonly
deployed with a local-only CA, and whether to trust such a certificate is the
operator's call. It is stated rather than warned about because it was previously
silent, so an operator who set it in a development shell had no way to see it was
still set.

### `op-localaccount` ([R13](security-review-2.md#r13))

F13 added managed-object guards to the *remove* paths only. **The add path still had
the exact `docker.system` → group `docker` collision F13 documented**, and none of the
guard: `identifier_to_projectid` returns the bare project component for the internal
portals, and `ensure_group_exists` returned `Ok(())` whenever `getent group` succeeded
without checking who owned the group. Fixed on the add path and in `update_homedir`,
including validation of the peer-supplied home directory.

### `op-filesystem` ([R33](security-review-2.md#r33))

`clean_and_check_path` rejected relative paths and `..`, then checked a **deny-list**
of sensitive locations — but canonicalised only when `check_exists: true`, which none
of the three callers that *create* things pass. So the deny-list checked the
*unresolved* string, and a symlinked component silently relocated the operation. Since
`chown` and `set_permissions` follow symlinks, that was a route to root handing
ownership of a directory outside the tree to an unprivileged user.

The fix has two layers, and they are complementary rather than redundant:

- **A runtime containment check.** Every path is verified to resolve inside one of the
  configured volume roots, canonicalising both **at the time of the operation**. Doing
  it live rather than from roots canonicalised at startup is what avoids the
  automounter problem: the volume must be mounted for the operation to succeed anyway,
  so there is nothing to race and no stale pre-mount resolution.
- **File-descriptor-based ownership.** The created directory is opened with
  `O_NOFOLLOW | O_DIRECTORY` and `fchown`/`fchmod` applied to the **fd**, which cannot
  be redirected at all — stronger than a nofollow *path* operation, which still
  resolves the path once more. Remote mode uses `chown -h`.

The containment check catches a symlink already in place; the fd catches one planted
between the check and the operation.

`create_dir` also gained a comment stating that it **must not** become
`create_dir_all`. Non-recursive creation failing `EEXIST` on a pre-planted symlink is
what currently protects the final component, and making it recursive — an easy-looking
fix for "parent doesn't exist" — would silently reopen the escalation. That safety was
emergent and asserted nowhere.

Three things were got wrong on the way, all found by integration testing rather than
by unit tests, and all worth recording because they are the same class of mistake:
canonicalising **locally** when `exec-prefix` means the paths live on another system;
passing **one** root to `create_link`, whose two paths legitimately live under
different roots; and **substituting** the resolved path for the caller's, which for a
path that *is* a symlink returned its target — so restoring a recycled project tried
to link a directory to itself. The validation function now returns `()`, making
substitution unrepresentable rather than merely corrected.

---

## 9. Deliberately not done

This section is the point of the document. Each of these is a recommendation from
[security-review-2.md](security-review-2.md) that was considered and declined, with
the reasoning, so it does not read as an unfixed gap.

### Command signing ([R3](security-review-2.md#r3)/[R4](security-review-2.md#r4)/[R34](security-review-2.md#r34))

Would make portal authority cryptographic rather than positional. Declined for now:
`orion` is symmetric-only, so it would mean introducing asymmetric crypto; every
endpoint agent would need the portal's public key, which means either a published URL
proxied into private networks or another provisioning channel; and any signature
would be one-way, so it authenticates the portal to the agent but nothing back.
Recorded as accepted residual in
[security-review-2.md §4.1](security-review-2.md) rather than as planned work.

### Restricting health, diagnostics and restart ([R2](security-review-2.md#r2)/[R19](security-review-2.md#r19))

The review proposed gating these on a config-declared control principal, restricting
them to the downstream direction, and omitting `recent_logs` and instruction text from
reports crossing an agent boundary. **All three declined.**

Whole-deployment visibility *from the portal downwards, deliberately ignoring zone*,
is a required capability: agents are routinely deployed where the operators have no
other access — inside customer private networks, behind NAT, on clusters they do not
administer — and this is the only control plane reaching them. That includes reading
remote logs, because that is what makes an unreachable agent diagnosable.

The boundary that matters is **portal to portal**, because a different estate is run
by a different team, and that filter already exists and holds.

The severities were also wrong, and were re-rated. No unmodified agent binary can
*originate* any of the three commands: there are five construction sites, all either
an HMAC-authenticated bridge endpoint or a forward of an inbound message; no
`Instruction` maps to one; and account/filesystem/scheduler agents register
`cascade_health = false`, so they never reach the cascade and refuse to forward. The
real precondition is host-level read of an agent's config file followed by an
attacker-authored client — far above "a peer misbehaves".

What *was* fixed: the resource monitor's accidental fleet-wide cascade, diagnostics
freshness judged on the peer's clock, unbounded response caches, and two ordering bugs
in `Restart` (an empty destination from a remote sender; the leaf-node check running
after the target decision).

### Slurm version-string validation ([R27](security-review-2.md#r27))

The *panic* is fixed. Treating an unexpected version string as a hard login error is
declined: the operator of `op-slurm` is almost certainly the operator of the Slurm
cluster, so a stricter check risks breaking something that is not broken, and baking
in a version-string expectation invites failure on a future Slurm release or a
vendor-modified build.

The hostile-`slurmrestd` attacker this contemplates is on the other side of a boundary
the operator already controls end to end — if `slurmrestd` is compromised, refusing to
parse its version string is not what saves the cluster. Tolerate-and-warn is the
intended behaviour. The only hard requirement is that no input can panic, and that is
tested.

### Trial-decryption for duplicate relayed names ([R33](security-review-2.md#r33))

A relayed peer is identified on the wire by `envelope.from` alone, and R15 established
that the envelope's own `zone` must not be trusted — so the same name in two zones is
genuinely ambiguous on the inbound path. Supporting it properly would mean
trial-decrypting against every candidate key, which is a *capability* addition in the
crypto path, not hardening.

Instead the configuration is refused, on both sides. The operational rule is that a
proxy may not connect two peers with the same name even in different zones, which
keeps the name unambiguous end-to-end.

### A startup allow-list of volume roots ([R33](security-review-2.md#r33))

Superseded by the runtime check described in [§8](#8-privileged-agents), which avoids
the automounter problem entirely.

### A per-server FreeIPA TLS flag ([R33](security-review-2.md#r33))

Declined: the FreeIPA servers are a redundant set with the same certificate, so
per-server granularity buys nothing. The genuine future improvement is a **custom-CA
option** — trusting a specific CA rather than disabling verification — which would
keep a local-CA deployment working *and* keep verification on. Not implemented.

### `Hour: TryFrom<NaiveDateTime>` ([R33](security-review-2.md#r33))

`from_chrono` rejects a non-zero minute or second, but the `From` impl accepted one
silently. Making it fallible would be a public API break for no security benefit, so
it **truncates to the top of the hour** instead — the invariant becomes unconditional
rather than checked on one of two paths.

### A bounded inbound channel ([R31](security-review-2.md#r31))

Replacing paddington's unbounded inbound channel with a bounded one, so overload is
expressed as backpressure rather than growth, remains the better fix and is **still
open**. The per-map bounds in [§4](#4-bounding-what-a-peer-can-grow) reduce the
consequences but do not address the channel itself.

---

## 10. Process changes

These are not fixes to findings but to how findings were missed
([security-review-2.md §5](security-review-2.md)).

- **`clippy::indexing_slicing` denied workspace-wide**, alongside `unwrap_used`,
  `expect_used`, `dbg_macro`, `unsafe_code = "forbid"` and
  `unused_crate_dependencies`. Lints moved from per-crate attributes into a single
  `[workspace.lints]` table inherited by all 19 crates; `clippy.toml` exempts tests.
- **`overflow-checks = true`** in release, safe only after the arithmetic work in
  [§3](#3-panics-and-arithmetic).
- **`make test` no longer passes `--lib`**, which had silently skipped every test in
  the agent binary crates — the ones guarding a previous finding.
- **CI runs `cargo audit --deny warnings`**, and its clippy step is
  `--all-targets --all-features -- -D warnings` so lints fail the build rather than
  being advisory.
- **`scripts/check-secret-writes.sh`** asserts no bare `fs::write` outside an
  annotated allow-list, run by `make lint` and CI.
- **Seed tests in the privileged agents**, which had almost none between them,
  targeting the findings that lived there.
- **Shared implementations replaced duplicates** where a fix established an invariant:
  identifier validation in `templemeads::validate`, and one
  `OPENPORTAL_ALLOW_INVALID_SSL_CERTS` rule instead of a copy per agent.

---

## 11. Where the review itself was wrong

Recorded because a review that cannot correct itself should not be trusted, and
because [security-review-2.md §7](security-review-2.md)'s verification grades earned
their keep.

- **[R12](security-review-2.md#r12) — falsified entirely.** Its premise, that a peer
  can write to a third peer's board, is not true: all three inbound board writes take
  the authenticated sender, and those are the only such call sites in the workspace.
  The reported attack chain also fails independently, because duplicate detection
  scans within one board and boards are per-peer.
- **[R7](security-review-2.md#r7) — not a bug.** The human approval gate covers award
  *creation*, not each membership change; automatic member provisioning is intended.
  The finding measured the code against a requirement that does not exist.
- **[R20](security-review-2.md#r20) — two of three claims wrong**, and its recommended
  fix ("only cache a response whose name equals the authenticated sender") is
  architecturally impossible: responses relay back hop by hop and legitimately
  describe an agent several hops away. Health already stamped a local timestamp, and
  the "deep clone per check" happens once per collection, not per poll.
- **[R2](security-review-2.md#r2)/[R19](security-review-2.md#r19) — attacker
  capability overstated**, as described in [§9](#9-deliberately-not-done).
- **[R34](security-review-2.md#r34) — right conclusion, wrong diagnosis.** See
  [§2](#2-binding-authority-to-local-state); "fixing" it as written broke user
  creation in a live estate.

Three of these were graded *reported* — cited with `file:line` but not independently
re-confirmed. Any finding at that grade should be re-verified before code is changed
on its account.
