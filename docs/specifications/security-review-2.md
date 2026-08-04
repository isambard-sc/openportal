<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# OpenPortal Security Review — Round 2

This is a second, independent code-level security assessment of OpenPortal,
carried out after the remediation described in
[security-review.md](security-review.md) (round 1) was complete. It is written
to be read *alongside* that document, not instead of it: round 1 remains the
record of what was found and fixed in that pass, and its threat model (§1) and
strengths analysis (§3) are reused here rather than restated.

- **Review date:** 2026-07-29
- **Version reviewed:** 0.90.0 (branch `feature_greatwestern`, including the
  uncommitted `IpOrRange` deserialisation change)
- **Baseline at review time:** `cargo test --offline` — 209 tests passing, 22 test
  binaries, 0 failures. Clean build, no warnings. (**Now 285**, after the fixes and
  the process work in [§5](#5-process-and-tooling-observations).)
- **Scope:** the full workspace, audited as seven independent areas — the
  cryptographic core; connection establishment and the listening server; the
  blind-relay proxy and config-at-rest; **the templemeads agent framework**;
  the `op-bridge` HTTP boundary and its Python client; the privileged agents
  (`freeipa`, `slurm`, `filesystem`, `localaccount`); and the domain grammar
  plus the prototype/orchestration agents.

> **Why this round found what round 1 did not.** Round 1 concentrated on the
> cryptographic core, transport authentication, config-at-rest, the bridge
> boundary, and the privileged agents' command/path handling. It did not
> substantially examine **`templemeads`' own authorization logic** — how an
> agent decides whether the peer that just sent it a Job was entitled to send
> it. That is where the most serious findings in this round live. Round 1's
> conclusions about the cryptography were re-tested in this round and **hold
> up**; see [§3](#3-what-round-1-got-right).

---

## 1. Executive summary

**Status (2026-08-04): all 34 findings are resolved** — 28 fixed, 2 fixed in part and
re-rated after re-checking their premises ([R2](#r2), [R19](#r19)), 1 falsified
([R12](#r12)), 1 confirmed not to be a bug ([R7](#r7)), 1 closed as a documentation
change ([R32](#r32)), and 1 whose panic was fixed but whose additional validation was
declined ([R27](#r27)). All six process and tooling items in
[§5](#5-process-and-tooling-observations) are done.

Seven of the review's recommendations were considered and **deliberately not
followed** - command signing, restricting the operator control plane, Slurm
version-string validation, trial-decryption for duplicate relayed names, a startup
allow-list of volume roots, a per-server TLS flag, and making `Hour`'s conversion
fallible. Each is recorded with its reasoning in
[security-review-2-fixes.md §9](security-review-2-fixes.md), so none of them reads as
an unfixed gap. One item remains genuinely open and is named there: replacing
paddington's unbounded inbound channel with a bounded one
([R31](#r31)).

**The rationale for every fix is in
[security-review-2-fixes.md](security-review-2-fixes.md)**, grouped by subsystem. This
document remains the record of what was *found*; that one records what was *done*, and
why.

**Nothing found in this review is now exploitable without an existing route into the
agent network.** The one remaining open item ([R31](#r31)'s unbounded inbound channel)
requires a valid pre-shared peer key, and the residual risks recorded in
[§4.1](#41-accepted-trade-off--portal-authority-is-positional-not-cryptographic)-[§4.3](#43-scope-note--the-cloud-prototype-agents)
require either a peer key or local access to an agent host.

Three findings *were* reachable before authentication, which is why they were fixed
first, and all three are fixed:

- **[R21](#r21)** (no handshake deadline) and **[R22](#r22)** (no WebSocket
  frame-size limit) were on the paddington listener, so they were reachable by
  anything able to open a socket to it — including, for the one internet-facing
  `op-portal`, from the internet. Neither disclosed anything; both were resource
  exhaustion. Now bounded by a 30 s pre-authentication deadline and a 2 MiB frame
  limit.
- **[R11](#r11)** (client-IP spoofing via `X-Forwarded-For`) affected rate-limit
  keying on the bridge, which is required to be on a private network and never
  internet-facing (see [bridge-api.md §0](bridge-api.md)); acting on the API itself
  additionally requires the HMAC key. Now resolved right-to-left against the
  configured trusted proxies.

Everything else required a peer key, host access, or the bridge's HMAC key from the
outset.

**No confidentiality or integrity break was found in the wire protocol,** and this
round tested that harder than round 1 did rather than taking it on trust. A
differential test of the anti-replay window against a perfect-memory reference model
(400 seeds × 20,000 operations concentrated at the window edge, plus exhaustive
enumeration of every length-5 nonce sequence across the word and window boundaries)
found **zero mismatches**. HKDF argument roles, Argon2 parameter ordering, AEAD
nonce generation, the absence of key/MAC reuse, and round 1's F4 fix were each
re-verified against the `orion` source. Without pre-shared keys an attacker still
cannot read or forge traffic, and the "no god key" property — every peer
relationship using independent keys, with no master credential anywhere — is real
and holds.

### What this round actually found, and what changed

The findings clustered in the **agent framework's authorization logic**, an area
round 1 had not substantially examined. The theme was that paddington establishes an
authenticated transport identity correctly, and templemeads then made authorization
decisions from *wire data* instead of from that identity. Concretely, and all now
fixed:

- A Job's asserted origin was never compared with the authenticated identity of the
  peer that delivered it — `position()` required only that the sender's name appear
  *somewhere* in an attacker-supplied path. It now requires the sender to be the
  **immediately adjacent** hop ([R4](#r4)).
- An agent's *type* (Portal, Bridge, Account…) was whatever the peer claimed, because
  the per-peer config had no role to check it against. A peer may now be declared
  `type = "..."`, checked at registration, and the declaration propagates through the
  normal peer-introduction flow rather than needing to be hand-edited ([R3](#r3)).
- Nothing bound a command to the portal whose authority it claimed. Agents now derive
  and enforce portal routes, scoped to a zone
  ([R34](#r34), `docs/plans/portal-route-discovery-design.md`).

Together these move the requirement from "compromise one agent" to "compromise one
agent **and** have a peer provisioned under a chosen name, in a chosen topology
position, with a chosen role".
[§4.1](#41-accepted-trade-off--portal-authority-is-positional-not-cryptographic)
records what remains after that as an explicit, reasoned trade-off — these controls
are positional rather than cryptographic, and command signing was designed,
costed and consciously deferred.

**One genuinely serious bug was found and is fixed.** Three arms of
`Instruction::parse` indexed a slice without a length guard, and that parser runs
inside `serde`'s `Deserialize` for `Command`. With `panic = "abort"` in the release
profile, a ~200-byte message from **any authenticated peer** terminated the process.
It was proven by execution, not inferred. The fix went beyond the three sites: every
panicking index and slice operation in the workspace's own code is now a checked
form, and `clippy::indexing_slicing` is denied workspace-wide so the class is closed
structurally rather than site by site ([R1](#r1),
[§5](#5-process-and-tooling-observations)).

**One availability defect needed no attacker at all**, and is the reason this round
was worth doing even setting security aside: the handshake anti-replay design kept
the outgoing nonce counter and the incoming replay window in the same
process-lifetime structure, so a *routine restart* locked an agent out of every
long-running peer. Any deploy triggered it. Fixed with a per-process random epoch and
one replay window per sender incarnation, and validated against real agents through
repeated disconnect/reconnect cycles, both directly connected and via a proxy
([R10](#r10)).

**Several round-1 fixes were incomplete rather than wrong** — applied at the sites
that review enumerated, with equivalent sites elsewhere missed. F9 missed a third
secret writer (the one holding the bridge HMAC key); F13 hardened
`op-localaccount`'s *remove* paths but not its *add* path; F5 never reached mapping
targets or `PortalIdentifier`; F3 read the wrong end of `X-Forwarded-For`. All are
fixed, and where a fix establishes an invariant it is now enforced structurally — a
CI grep-assertion for bare secret writes, a shared validator, a single shared
TLS-verification rule — so the next site cannot be missed. Round 1 carries an inline
correction note at each affected claim.

### How to read this document

- **Severities are this review's own assessment**, assigned adversarially and before
  the deployment context was fully accounted for. Several were **lowered on
  re-check** once the required attacker capability was traced precisely — see
  [R2](#r2), [R19](#r19) and [R20](#r20), where the original framing assumed any
  peer could originate a control message that no unmodified agent binary can
  construct.
- **[§7](#7-method-and-verification-standard) grades every finding** as `proven`
  (confirmed by executing code), `source` (the cited code read directly) or
  `reported` (cited but not independently re-confirmed). That last grade earned its
  keep: three `reported` findings were substantially wrong on re-check, one of them
  ([R12](#r12)) entirely, and one ([R20](#r20)) recommended a fix that is
  architecturally impossible. They are left in the document with their corrections
  rather than quietly deleted.
- **Deliberate decisions are recorded as such**, not left looking like gaps:
  [§4.1](#41-accepted-trade-off--portal-authority-is-positional-not-cryptographic)
  (positional authority),
  [§4.2](#42-accepted-trade-off--the-operator-control-plane-spans-the-whole-deployment)
  (whole-deployment health/diagnostics/restart is a required capability for agents in
  private networks operators cannot otherwise reach),
  [§4.3](#43-scope-note--the-cloud-prototype-agents) (the two prototype cloud
  agents), and [R27](#r27) (Slurm version strings are deliberately tolerated rather
  than validated, so an upstream or vendor change cannot break a working cluster).

This was a self-commissioned audit held to a deliberately hostile standard, and the
finding count reflects the depth of the search rather than the state of the system.
The document is kept as a working record — including the parts where the review
itself was wrong — because that is what makes it useful the next time.

### Findings at a glance

Severity is this review's assessment. **Verification** distinguishes findings
confirmed by executing code (`proven`), by reading the cited source directly
(`source`), or reported with a file:line citation but not independently
re-confirmed (`reported`).

| ID | Sev | Verif. | Status | Finding |
|----|-----|--------|--------|---------|
| [R1](#r1) | Critical | **proven** | **Fixed** | Unguarded slice index in `Instruction::parse`, reachable from `Deserialize` → remote process abort |
| [R2](#r2) | ~~High~~ Low–Medium | source | Re-rated, partly fixed | `Command::Restart` cascades along an attacker-supplied path — but no unmodified binary can originate one |
| [R3](#r3) | High | source | **Fixed** | Agent type is self-declared over the wire; no config to check it against |
| [R4](#r4) | High | source | **Fixed** | Job provenance never bound to the authenticated sender → lateral pivot |
| [R5](#r5) | High | source | **Fixed** | `op-slurm` applies no managed-object guard to any mutation |
| [R6](#r6) | High | source | **Fixed** | Attacker-controlled `version`/`changed` drive an unbounded loop under the board write lock |
| [R7](#r7) | ~~High~~ n/a | source | **Not a bug** | `op-cloudportal`'s human approval gate is bypassable after approval |
| [R8](#r8) | High | **proven** | **Fixed** | IPv4/IPv6 truncation in `IpOrRange::matches` defeats every IP allow-list |
| [R9](#r9) | High | source | **Fixed** | Bridge invite — the HMAC API key — written world-readable (F9 missed this writer) |
| [R10](#r10) | High | source | **Fixed** | Handshake nonce counter resets on restart while the peer's window persists → prolonged lockout |
| [R11](#r11) | Medium–High | source | **Fixed** | F3's rate-limit bypass persists: left-most `X-Forwarded-For` entry is client-seeded |
| [R12](#r12) | ~~Medium~~ n/a | reported | **Falsified** | Peer can write to a third peer's Board → forged job results, suppressed real jobs |
| [R13](#r13) | Medium | source | **Fixed** | `op-localaccount` add-path group collision (F13 covered only removal); `update_homedir` unguarded |
| [R14](#r14) | Medium | source | **Fixed** | Mapping local names permit whitespace/commas → argument injection into OpenPortal's own grammar |
| [R15](#r15) | Medium | reported | **Fixed** | Relayed `envelope.zone` is unauthenticated but is half the peer identity |
| [R16](#r16) | Medium | reported | **Fixed** | Relay envelopes accepted over any authenticated connection (no receive-side `from` binding) |
| [R17](#r17) | Medium | source | **Fixed** | `owning_portal` omits 10 identifier-bearing instructions → cross-portal operations |
| [R18](#r18) | Medium | source | **Fixed** | `PortalIdentifier::parse` never received F5's allow-list |
| [R19](#r19) | ~~Medium~~ Low | reported | Re-rated; exposure is intended | Diagnostics/health carry other tenants' data — but whole-deployment visibility is a required capability, and no unmodified binary can originate a request |
| [R20](#r20) | ~~Medium~~ Low | reported | Re-rated, **fixed** | Diagnostics freshness judged on the peer's clock; both caches unbounded. Two of the finding's three claims were wrong |
| [R21](#r21) | Medium | source | **Fixed** | No handshake timeout: pre-auth semaphore permits held indefinitely |
| [R22](#r22) | Medium | source | **Fixed** | No WebSocket message-size limit; work amplified per candidate config |
| [R23](#r23) | Medium | **proven** | **Fixed** | `exchange.rs` overload recovery is dead code (inverted time comparisons) |
| [R24](#r24) | Medium | reported | **Fixed** | Bridge listener has no connection cap, no timeouts, and HMACs 2 MB pre-auth |
| [R25](#r25) | Medium | reported | **Fixed** | Date/`DateRange` parsing accepts the full chrono range → OOM, panics, infinite loops |
| [R26](#r26) | Medium | reported | **Fixed** | One hostile/mistyped cost report OOM-kills `op-cloudaccount` |
| [R27](#r27) | ~~Medium~~ n/a | source | **Panic fixed; validation declined** | `version_numbers[2]` index panic from a hostile Slurm REST response |
| [R28](#r28) | Medium | reported | **Fixed** | Malicious proxy induces genuine `SessionUnknown` storms → unbounded bootstrap tasks |
| [R29](#r29) | Low–Med | source | **Fixed** (V1 until clients migrate) | HMAC canonicalization is ambiguous: the nonce can be folded into the body |
| [R30](#r30) | Low–Med | reported | **Fixed** | `op-cloudaccount` answers usage/limit queries for projects never assigned to it |
| [R31](#r31) | Low–Med | reported | **Fixed** | Unbounded board/job growth: no `expires` cap, arbitrary board creation |
| [R32](#r32) | Info | reported | **Documented** | Bridge responses are neither authenticated nor encrypted — F12's claim overstates the bridge hop |
| [R33](#r33) | Low | mixed | **Fixed** | Lower-severity hardening cluster (35 items) |
| [R34](#r34) | High | **proven** | **Fixed** | The portal-ownership check never runs on the wire path (`check_portal = false` on every deserialisation, and no backend re-checks) |

### Remediation status (as of 2026-08-04)

**All thirty-four findings are resolved**, in six passes. Each fix is
described in its finding under **Fix applied**, and the whole set is covered by
**298 passing unit tests** (up from 209 at the start of this round), a clean
`cargo clippy --all-targets --all-features -- -D warnings`, `cargo fmt`, and a clean
release build with `overflow-checks` newly enabled.

| Pass | Findings | Theme |
|---|---|---|
| 1 | [R1](#r1), [R8](#r8), [R9](#r9), [R10](#r10), [R11](#r11), [R21](#r21), [R22](#r22), [R23](#r23) | Everything reachable *without* holding a peer key, plus R9 (any local user on the bridge host) and R10 (needs no attacker at all) |
| 2 | [R5](#r5), [R6](#r6), [R13](#r13), [R14](#r14), [R15](#r15), [R17](#r17), [R18](#r18), [R25](#r25) | Mechanical fixes whose reach requires an already-compromised peer key or a hostile external service |
| 3 | [R3](#r3), [R4](#r4), [R34](#r34) | The positional authority controls, with the accepted residual recorded in [§4.1](#41-accepted-trade-off--portal-authority-is-positional-not-cryptographic) |
| 4 | [§5](#5-process-and-tooling-observations) (all six) | `indexing_slicing` denied workspace-wide, `overflow-checks` enabled, `cargo audit` in CI, a structural guard against bare secret writes, `make test` no longer skipping binary crates, seed tests in the privileged agents |
| 5 | [R2](#r2), [R19](#r19), [R20](#r20) | Re-checked, re-rated, and the real bugs within them fixed - see [§4.2](#42-accepted-trade-off--the-operator-control-plane-spans-the-whole-deployment) |
| 6 | [R16](#r16), [R24](#r24), [R26](#r26), [R28](#r28), [R29](#r29), [R30](#r30), [R31](#r31), [R32](#r32) | The remaining relay, bridge, board and cloud-agent findings |
| 7 | [R33](#r33) (35 items) | The hardening cluster, across paddington, templemeads, greatwestern and every privileged agent |

**Resolved without a code change, deliberately:**

- **[R7](#r7) — not a bug.** The human-in-the-loop approval gates the *creation* of
  an award (and an increase past a threshold, which the web portal detects), not each
  subsequent membership change; automatic member provisioning is intended, and the
  gate lives in the portal software. The finding measured the code against a
  requirement that does not exist.
- **[R12](#r12) — falsified.** Its premise, that a peer can write to a third peer's
  board, is not true of the code: all three inbound board writes take the
  authenticated sender, and those are the only such call sites in the workspace.
- **[R27](#r27) — declined.** An unexpected Slurm version string is deliberately
  tolerated-and-warned rather than treated as a hard error, so that a future Slurm
  release or a vendor-modified build cannot break a working cluster. The *panic* it
  reported is fixed and tested; only the additional validation is declined.
- **[R32](#r32) — documented.** The bridge's trust boundary and the reason it is a
  design choice are now stated in [bridge-api.md §0](bridge-api.md).

**All findings are now resolved.** [R33](#r33)'s 35 items were completed on
2026-08-04, seven of them by a deliberate decision not to make the suggested change.

**One sub-item remains genuinely open**, and is named rather than buried: replacing
paddington's unbounded inbound channel with a bounded one, so overload is expressed as
backpressure rather than growth ([R31](#r31)). The per-map bounds now in place reduce
the consequences but do not address the channel itself.

**Deployment note on the bridge findings.** [R24](#r24), [R29](#r29) and
[R32](#r32) all concern the `op-bridge` HTTP surface. The bridge is **not**
internet-reachable and must not be: in the reference deployment it runs inside the
same Kubernetes cluster as the portal software it serves, with istio providing mTLS
on that hop, and only `op-portal` is exposed externally (behind a Cloudflare tunnel).
That materially lowers the practical severity of all three - an attacker must already
be inside the cluster network. This requirement is now stated normatively in
[bridge-api.md §0](bridge-api.md) rather than being implicit; any deployment that
*does* expose the bridge should treat these as live.

---

## 2. Round 1 claims this round falsifies

These are called out explicitly so the earlier document is not relied upon
where it is now known to be wrong. Each links to the finding that supersedes
it.

As of 2026-08-03 every row below is also annotated **into** round 1 itself, as an
inline "Round 2 correction" note at the affected claim, so a reader who opens that
document first cannot miss it.

| Round 1 statement | Status now |
|---|---|
| §3.6: "FreeIPA **and Slurm** additionally refuse to act on objects not in the OpenPortal-managed org/group" | **False for Slurm.** True only on the *create* path, and even there the check is against a locally-constructed object whose organization is a constant. No mutation path checks the organization of the account that actually exists in Slurm. → [R5](#r5) |
| §3.7: "no payload-reachable panic was found in the agents" | **False.** → [R1](#r1) (any Job payload), [R27](#r27) (external service response), [R25](#r25) (date arguments) |
| §3.1 / §7: compromise of one link's keys does not reach beyond that relationship | **True of key material, false of authority.** → [R3](#r3), [R4](#r4), [R12](#r12). ([R2](#r2) and [R19](#r19) were originally listed here; on re-check they need an attacker-authored client, not just keys — see their re-ratings.) |
| §3.3: "Handshake/bootstrap nonces correctly persist across reconnects" | **Only half the state persisted.** The window did; the send counter did not. → [R10](#r10), now fixed by keeping one window per sender incarnation |
| F3 **[Fixed]**: rate limiter now keyed on a real, non-forgeable address | **Bypass remains** in the deployment F3 recommends, because the left-most `X-Forwarded-For` entry is client-supplied. → [R11](#r11) |
| F9 **[Fixed]**: config and invite files restricted to `0600` | **One writer missed** — the bridge invite, which holds the HMAC API key. → [R9](#r9) |
| F13 **[Fixed]**: `op-localaccount` given the managed-object guard | **Only the remove paths.** The add path still has the exact `docker.system` → `docker` collision F13 documented. → [R13](#r13) |
| F5 **[Fixed]**: identifiers validated against a strict allow-list | **Two gaps:** mapping targets got a deny-list that permits whitespace and commas ([R14](#r14)), and `PortalIdentifier` got nothing ([R18](#r18)) |
| F5 note: "neutralises the URL-path-injection concern for Slurm REST calls (such characters can no longer appear in an identifier)" | **Inaccurate for mapping fields**, which are interpolated unencoded into REST paths. → [R14](#r14) |
| F15: "Both handshake paths now reject an all-zero session key" | **The three relay bootstrap paths do not.** → [R33](#r33) |
| F11 residual: an attacker "could still deny *new* connections **until slots free**" | **Slots never free.** There is no handshake timeout anywhere, so permits are held for as long as the attacker keeps sockets open. → [R21](#r21) |
| F12: "the wire protocol is confidential and authenticated over plain HTTP … an on-path attacker cannot read message content, forge or tamper with a message" | **Not true of the bridge hop.** Bridge payloads are cleartext content (not metadata), and the HMAC is request-direction only — responses are unauthenticated. → [R32](#r32) |
| F8: "all current clients get full nonce-based replay protection" because the nonce is signed | **The nonce's *presence* is not bound**, so the signed string is ambiguous. → [R29](#r29) |
| §6: the one remaining action is live end-to-end validation | Still outstanding, and [R10](#r10) is a strong reason to do it: the restart-lockout behaviour is exactly what an offline unit test cannot show. |

---

## 3. What round 1 got right

Re-tested in this round with stronger methods, and confirmed:

- **The anti-replay window is correct.** Differential-tested against a
  perfect-memory reference model, compiled with `overflow-checks=off` to match
  the release profile: 400 seeds × 20,000 operations biased to the window edge,
  hand-built extremes (`0, u64::MAX, 0, u64::MAX`; `u64::MAX-1024±`), and
  exhaustive enumeration of all 8⁵ length-5 sequences over
  `{0,1,63,64,65,1023,1024,1025}`. **Zero mismatches.** `shift_left`'s
  high→low word iteration, the absence of a shift-by-64, and the
  `age >= WINDOW_BITS` guard bounding the bitmap index to ≤15 were each
  verified by hand as well. The defect in [R10](#r10) is in how the *counter*
  is initialised, not in the window.
- **Key/MAC domain separation (F15) holds.** Confirmed that
  `orion::auth::SecretKey` and `orion::aead::SecretKey` are *the same type*, so
  there is no type-level protection and the documented invariant is the only
  guard — then traced every caller. `Key::sign` is reached only with the bridge
  config key; `encrypt`/`decrypt` only with session/permanent/at-rest keys.
  No key reaches both. F15's conclusion is correct.
- **KDF and AEAD usage is correct.** HKDF `salt`/`ikm`/`info` roles are not
  swapped; Argon2's `(password, salt, iterations, memory, length)` arguments
  are in the right slots (19 MiB / 3 passes, the OWASP floor), and the
  derivation genuinely runs once at startup, not on a hot path; `orion` draws a
  fresh 192-bit AEAD nonce per `seal` and length-checks before slicing in
  `open`.
- **Authentication ordering (§3.4) is correct.** Key possession precedes
  identity; `peer_name`/`peer_zone` come from the matched `ClientConfig`, never
  from a wire field; every subsequent comparison is
  `wire_value != config_value`. `supports_nonce`, `version` and all nonces ride
  inside the AEAD in both directions, so a keyless attacker cannot downgrade
  any of them.
- **The legacy XOR salt masking never leaked key material.** The salt is 32
  fresh CSPRNG bytes — the same length as the key — and no salt is ever
  observed in both masked and plain form, so the mask is a genuine one-time
  pad. F15's change was correct hygiene, not a vulnerability fix.
- **The blind relay is blind for *content*.** Every cross-pair splice,
  reflection, `Start` replay, `Accepted` replay, stale-magic acceptance, and
  session-half substitution was enumerated and is blocked. F7's `from` binding
  is complete *on the proxy side*. What breaks is the routing label and
  availability — [R15](#r15), [R16](#r16), [R28](#r28) — not confidentiality.
- **`RelayPolicy` is genuinely default-deny**, with no network path to mutate
  it and fail-closed startup on a malformed policy.
- **The uncommitted `IpOrRange` untagged-deserialise change is safe** — it
  admits exactly what the CLI parser already admitted; empty, whitespace and
  trailing-comma inputs are still rejected, and the `/0` panic guards still
  fire. (The pre-existing flaw it sits next to is [R8](#r8).)
- **F1 (`Path::join` swallowing an absolute component) is fully fixed** on
  every read, write, delete and enumeration path in both prototypes, including
  the temp-file/rename pair.
- **Identifier `Deserialize` does route through validation.** Every identifier
  type has a hand-written `Deserialize` that calls its own `parse`; none uses a
  bare derive. (The exceptions are `Volume` and `AwardDetails` — see
  [R33](#r33) and [R7](#r7).)
- **No shell is ever invoked.** Re-confirmed across all privileged crates: ten
  process-spawn sites, zero shells, and no credential passed via a child's
  environment.
- **TLS verification is on by default** everywhere, with one env-gated
  dev switch; container images are distroless and run as UID 65534; all 409
  dependencies are current with no known advisories (`orion 0.17.14`,
  `shlex 1.3.0`, `ring 0.17.14`, `rustls 0.23.40`).

---

## 4. Findings

<a name="r1"></a>
### R1 — Unguarded slice index in `Instruction::parse`, reachable from `Deserialize` · Critical · **proven**

> **Status: fixed** (2026-07-30) - see **Fix applied** at the end of this finding.

**Location:** `greatwestern/src/grammar.rs:2948` (`submit`), `:2982`
(`create_project`/`create_award`), `:2988`/`:2992` (`&parts[3..]`), `:3004`
(`update_project`/`update_award`). Reached via `templemeads/src/job.rs:189`
(`impl Deserialize for Command<L>` → `Command::parse` → `L::parse_instruction`).
Release profile `panic = "abort"` at `Cargo.toml:32`.

**What it is.** `Instruction::parse` splits on `' '` and matches `parts[0]`.
Every arm from `:3255` onward checks `parts.len()` first. Three arms do not —
they index `parts[1]` directly, and two also slice `&parts[3..]` on a vector
that may hold two elements. `parts[0]` is safe (`split` always yields at least
one element); the rest are not.

Critically, this fallible text parser runs **inside `serde`'s `Deserialize`**
for `Command<L>`, which is a field of `Job<L>`. So it executes on every Job
arriving from any peer, before any business logic sees it.

**Attacker path.** Any peer holding one link's keys sends
`Command::Put { job }` where the job's `command` string is `"a.b submit"`.
Deserialisation panics; `panic = "abort"` means no unwind and no
`catch_unwind` anywhere on the path — the process dies. Equivalent payloads:
`"a.b create_project"`, `"a.b update_award"`, `"a.b create_project p.portal"`.
The same panic is reachable unauthenticated-by-OpenPortal-standards via
`POST /run` on the bridge (`templemeads/src/bridge.rs:99`).

**Amplification.** `op-provider` routes with the `Erased` domain, whose
`parse_instruction` never fails and forwards the string verbatim — so a peer on
a link to a *router* can post a poisoned Job that the router forwards to a
real-domain agent, which then dies. The crash reaches one hop beyond the
compromised link.

**Verification.** Proven by execution against the real crates:

```
Instruction::parse("submit")                 → PANIC grammar.rs:2948  index out of bounds: the len is 1 but the index is 1
Instruction::parse("create_project")         → PANIC grammar.rs:2982
Instruction::parse("update_project")         → PANIC grammar.rs:3004
Instruction::parse("create_project p.portal")→ PANIC grammar.rs:2992  range start index 3 out of range for slice of length 2
serde_json::from_str::<Job<Hpc>>({… "command":"a.b submit" …})
                                             → PANIC inside Deserialize
```

**Fix.** Add `if parts.len() < 2 { return Err(...) }` to the three arms, matching
the style already used at `:3255`; replace `&parts[3..]` with
`parts.get(3..).unwrap_or_default()`. Structurally: retire the `parts[N]` idiom
for a `parts.get(N).ok_or(...)` helper, enable `clippy::indexing_slicing` for
`greatwestern`, and add a property test asserting `Instruction::parse` never
panics for arbitrary input. Consider whether a domain text grammar should run
inside `Deserialize` at all.

**Fix applied.** Every panicking index and slice operation in the workspace's
own code is now a checked form - `get`, `first`, `split_first`, `split_once`,
`strip_prefix`, or a slice pattern with `let ... else`. In
`Instruction::parse` this is done with two local accessors (`arg(n)` and
`rest(n)`) that yield an empty string for a missing argument, which every
sub-parser already rejects, so each arm's existing error handling takes over
unchanged; 323 index expressions in that function were converted. Two latent
panics of the same class were found and fixed on the way: a **char** index used
as a **byte** offset when splitting a Lustre quota expression
(`filesystem/src/lustreengine.rs`, which panics on any multi-byte character),
and `permissions`/`links` in a volume config being indexed with a `roots` index
when those independently-configured lists may be shorter
(`filesystem/src/volumeconfig.rs`). The one remaining panicking index is
`Index<usize> for Destinations`, which is required to panic by its `std`
contract, has no callers, and is now documented alongside a non-panicking
`Destinations::get`. Locked in by
`test_instruction_parse_never_panics_on_missing_arguments`, which asserts a
clean error for each truncated form and sweeps every recognised keyword with
0-3 arguments.

---

<a name="r2"></a>
### R2 — `Command::Restart` is unauthenticated and cascades · ~~High~~ → Low–Medium · source

> **Status: partially fixed and re-rated** (2026-08-03). The severity below was
> wrong: it assumed any authenticated peer could *originate* a `Restart`, which no
> unmodified agent binary can. See **Re-rated** and **Fix applied** at the end of
> this finding.

**Location:** `templemeads/src/handler.rs:499-504`;
`templemeads/src/restart.rs:172-352` (exit at `:247`, `:254`; forwarding at
`:261-338`).

**What it is.** The only guard is "a Portal will not accept a restart from a
peer that *self-declared* itself a Portal" (`restart.rs:182-196`) — which
depends on [R3](#r3), matches peers by name while ignoring zone, and applies to
no other agent type. For every non-Portal agent there is no check at all.
`destination.is_empty()` means "restart yourself", so `restart_type: "hard"`
with an empty destination reaches `std::process::exit(0)`. `"soft"` errors and
removes every job on every board, clears diagnostics, and disconnects every
peer.

**Attacker path.** A peer holding one link's keys sends
`{"Restart":{"restart_type":"hard","destination":""}}` — the neighbour dies
immediately, repeatably, defeating a supervisor. To reach further: send
`HealthCheck` first, which cascades fleet-wide and returns a recursive map of
every agent name (plus per-process memory/CPU/job counts), then address
`Restart` at a multi-hop path. Non-leaf agents forward it, stripping one
component per hop, until a path of length 1 that does not match forwards with
an *empty* destination — which the next agent reads as "restart yourself".
Leaf agents refuse to *forward* but still kill themselves on request, because
`is_target` is evaluated before the cascade check.

**Fix.** Require the sender to be a specific, config-declared control principal
(the bridge or an operator link) before honouring `Restart`; never derive that
from a self-declared type. Restrict forwarding to the intended downstream
direction, and require an explicit self-name match rather than accepting an
empty destination from a remote sender.

**Re-rated (2026-08-03) — the attacker path requires an attacker-authored
client, not merely a peer.** The finding's severity rested on "a peer holding one
link's keys sends `{"Restart":...}`". That elides *how*. There are exactly two
sites in the workspace that construct a `Restart`:

| Site | Reachable from |
|---|---|
| `bridge_server.rs:707` | `POST /restart`, HMAC-authenticated |
| `restart.rs:336` | a *forward* of an inbound `Restart` only |

There is no third. Nor can a Job induce one: `greatwestern`'s `Instruction` enum
has no restart variant, so the ordinary traffic every agent sends cannot become a
`Restart` at any hop — there is no confused-deputy route.

Decisively, `op-freeipa`, `op-slurm` and `op-filesystem` register with
`cascade_health = false` (`account.rs:37`, `scheduler.rs:36`,
`filesystem.rs:37`), which makes `handle_restart_request` return an error before
reaching the forward, and they run no bridge. So an unmodified leaf binary has
**no code path that emits a `Restart` at all**. The same holds for
`HealthCheck`/`DiagnosticsRequest` — see [R19](#r19).

The real precondition is therefore *host-level read of an agent's config file*
(the keys are in it), after which the attacker writes their own client. That is
a much lower bar than exploiting Rust — it is exactly what
[F9](security-review.md)/[R9](#r9) protect — but it is far above "a peer
misbehaves", and it means this is **post-host-compromise escalation, not a
peer-reachable vulnerability**. Combined with the operational facts that every
agent runs under systemd or Kubernetes and is restarted automatically, and that
OpenPortal jobs are idempotent and re-submitted by the portal software, the
residual is availability-only: a compromised host can bounce agents it could not
otherwise reach, including the portal.

Rated **Low–Medium**. Recorded here rather than deleted so a future round does
not re-raise it at High on the same reasoning.

**Fix applied (2026-08-03).** The two ordering bugs are closed, since they cost
almost nothing and both were wrong independently of any attacker:

- An **empty destination** no longer means "restart whoever received this" when
  the request arrives from a remote peer. It remains valid for a locally
  originated one, because the bridge's `POST /restart` with an empty destination
  injects the command into its own queue via `send_to(self)`, so `sender` is our
  own name.
- The **leaf-node check now precedes the target decision**. It used to run
  afterwards, so an agent that refused to *forward* a restart would still kill
  *itself* on request from the very peer it would not relay for.

Both now live in a pure `decide_restart`, with tests covering each case,
including the bridge's self-restart and a `name@zone` target hop.

**Not done, deliberately.** Gating `Restart` on a config-declared control
principal, or on direction. Both were considered and rejected: the attacker they
defend against already holds a host and writes their own client, and the
operator's control plane (bridge → portal → any agent downward, ignoring zone)
is a relied-upon feature for agents in private networks that operators cannot
otherwise reach. See the note in [§4.1](#41-accepted-trade-off--portal-authority-is-positional-not-cryptographic).

---

<a name="r3"></a>
### R3 — Agent type is self-declared over the wire · High · source

> **Status: fixed** (2026-07-30) - see **Fix applied** at the end of this finding.

**Location:** `templemeads/src/handler.rs:157-192` (`Command::Register`
handling); `templemeads/src/agent.rs:201-244` (`register_peer`).
`paddington/src/config.rs:161-176` — `ServerConfig`/`ClientConfig` carry
`name`, `url`, `proxy`, `zone` and keys, and **no role field**.

**What it is.** `Command::Register { agent: AgentType, … }` is accepted verbatim
from the peer and stored as that peer's authoritative type. There is nothing in
the provisioned configuration to check it against, so every authorization
decision that keys on agent type is decided by the party being authorized.

**Consequences, per guard:**

| Guard | Location | Bypass |
|---|---|---|
| Portal accepts `Submit` only from a Bridge | `portal/src/main.rs:202` | A compromised downstream peer registers as `Bridge`, then `Submit`s — the portal parses and puts the job southbound **in the portal's name** |
| Bridge accepts instructions only from virtual agents | `bridge/src/main.rs:130` | Register as `Virtual` on the first `Register` (the first one wins) |
| Portals don't share diagnostics with other portals | `handler.rs:517-527` | Register as `Instance`; guard silently skipped |
| Portals don't restart other portals | `restart.rs:182-196` | Same, and the check ignores zone |
| Instance selects "the" account/filesystem/scheduler agent | `agent.rs:294-316` | A peer registering as `Account` can become the agent an instance routes account operations to |

**Fix.** Add the expected `AgentType` (and expected `Domain`) to each peer's
config entry; reject and disconnect a `Register` whose claimed type disagrees;
fail closed for peers with no declared role.

**Fix applied.** A `[[clients]]`/`[[servers]]` entry may now declare
`type = "..."` (one of the nine agent types). On `Register`, a peer claiming any
other type is refused and the mismatch logged as an error.

The field is **optional, and unset means unchecked**, so an existing config keeps
working and the check is adopted one peer at a time. That is a deliberate
trade-off: it means the hole stays open for any peer not yet declared. To make
the remaining gaps discoverable rather than invisible, an undeclared peer's
claimed type is logged at debug level naming the exact value to add, and an
*unrecognised* value is logged as an error and treated as unset rather than
rejecting the peer.

Note what this does and does not do. It stops a *provisioned* peer claiming more
authority than it was provisioned with. It does not stop an attacker who has
compromised an agent from adding new peers with whatever types they like, because
adding a peer is a local config operation on a machine they already control -
see [§4.1](#41-accepted-trade-off--portal-authority-is-positional-not-cryptographic).

**Follow-up (2026-08-03) - the declaration is no longer hand-edited.** As first
implemented, `type` had to be added to the config by hand on both sides, which is
how a peer ends up undeclared: live validation of the portal-route work needed
exactly this manual step before enforcement switched on. The two directions are
now populated by the normal peer-introduction flow, and remain deliberately
asymmetric:

- **What the client must be** is declared on the issuing side, with
  `client --add --type bridge`. It is never derived from anything the client
  sends; the two operators exchange it out-of-band, which is the whole point.
  The value is validated against the nine agent types *at add time*, so a typo is
  an error while there is still an operator there to read it, rather than being
  written to disk and then silently discarded at startup as unrecognised.
  Omitting `--type` still means "unchecked", and now logs a warning saying so.
- **What the server is** travels in the invite, as a new optional `type` field.
  The issuer knows its own type for certain, so it is always declared, and the
  importing side picks it up with no manual step. Trusting it is sound for the
  same reason trusting the invite's `name`, `url` and keys is: the file reaches
  the client by deliberate operator action, not over the wire. What this finding
  distrusts is the role a peer *claims at registration* - which is now checked
  against the invited value.

The invite never carries the *client's* expected type; there would be nobody to
tell it to, and it would invert the direction of trust. An invite written by an
older version has no `type` field, imports cleanly, and yields "not declared",
so nothing breaks. `op-proxy` records no type in either direction: a blind relay
has no `agent::Type` of its own, and its authorization is the explicit `allow`
pair list rather than anything about the roles it relays between.

---

<a name="r4"></a>
### R4 — Job provenance is never bound to the authenticated sender · High · source

> **Status: fixed** (2026-07-30) - see **Fix applied** at the end of this finding.

**Location:** `templemeads/src/handler.rs:207-232` (Update), `:273-288` (Put),
`:430-455` (Delete); `templemeads/src/destination.rs:36-56` (`parse`),
`:62-85` and `:105-113` (`position`).

**What it is.** The transport identity is trustworthy — `sender`/`zone` are
stamped by paddington from the authenticated connection
(`paddington/src/connection.rs:1106`, `:1825`), `recipient` is stamped locally,
and `set_sender` is never reachable from an inbound path. But the Job's
asserted origin is never checked against it. The entire routing decision is
`job.destination().position(recipient, sender)`, and `position` requires only
that the sender's name appear **somewhere** in the attacker-supplied path:

```rust
pub fn position(&self, agent: &str, previous: &str) -> Position {
    match self.agents.contains(&previous.to_string()) {   // ← "somewhere", not "adjacent"
        false => Position::Error,
        true => self.position_internal(agent, previous),
    }
}
```

`Destination` is a bare `Vec<String>` from a dot-split with **no** charset,
length, count or hierarchy validation, and components may repeat. Nothing
anywhere compares `sender` with `job.destination()`'s adjacency.

**Attacker path.** A is a compromised account or filesystem agent whose only
legitimate peer is instance B; B also peers with C, a link A holds no keys for.
A sends B a `Put` with destination `A.B.C`. B computes `Downstream`, and
forwards to C **over B's own authenticated link, bearing B's identity**. C sees
`Position::Destination` and executes. Verified that C's runner performs no
sender authorization: `cluster`, `clusters`, `provider`, `slurm` and
`cloudaccount` mains contain no `agent_type`/`envelope.sender` reference at
all, and `provider`/`platform` register `runner = None`, making them pure
wire-driven routers. The same shape works sideways to any other peer of B.

**Not affected.** Infinite relay is genuinely impossible — first-occurrence
index monotonicity means Downstream strictly increases the index and Upstream
strictly decreases it, direction never flips back, and fan-out is 1 per hop.
That safety property is *emergent* rather than explicit, so an explicit hop
counter is still worth having.

**Fix.** At each hop require `job.destination().previous(recipient) == sender` —
the sender must be exactly the immediately preceding hop — and gate both
forwarding and execution on the sender's *configured* (not self-declared,
cf. [R3](#r3)) role being upstream of this agent. Validate `Destination`
components against the identifier allow-list and cap their count.

**Fix applied.** `Destination::position` now requires the sender to be the
**immediate** neighbour of the recipient in the claimed route - `previous_index +
1 == agent_index` travelling downstream, or `agent_index + 1 == previous_index`
travelling upstream - instead of merely appearing somewhere in it. Reaching the
last agent no longer returns `Destination` without looking at the sender.

The reason this is worth more than it looks: the sender is stamped by paddington
from the authenticated connection's own `ClientConfig`, so it cannot be forged
without that link's pre-shared key, whereas the route is an unvalidated
`Vec<String>` off the wire. Binding one to the other means an agent can only
claim a position in the path for which it holds the key.

Before landing it, every destination shape the codebase actually builds was
enumerated from its construction site and asserted to still route correctly:
`portal.provider.platform.instance`, the minimal `portal.instance`,
`bridge.portal`, `instance.backend`, `cloudportal.cloudaccount`, and the
`resource.local-portal.remote-portal` offering shape. A pre-existing unit test
asserted the *vulnerable* behaviour (`position("c", "a") == Destination`) and has
been updated with a comment saying so - the same situation as the
`X-Forwarded-For` test under [R11](#r11).

---

<a name="r5"></a>
### R5 — `op-slurm` applies no managed-object guard to any mutation · High · source

> **Status: fixed** (2026-07-30) - see **Fix applied** at the end of this finding.

**Location:** `slurm/src/sacctmgr.rs:1669-1749` (`set_limit`), `:1752-1794`
(`cancel_pending_user_jobs`), `:1796-1841` (`cancel_pending_project_jobs`);
reached from `slurm/src/main.rs:155,169,182,259,273,287`. The unused guard is
`SlurmAccount::is_managed()` at `slurm/src/slurm.rs:1817`.

**What it is.** Round 1 §3.6 credits Slurm with refusing to act on unmanaged
objects. That is true only on the *create* path, and even there the check is
applied to `SlurmAccount::from_mapping`, whose `organization` field is
hard-wired to the managed org (`slurm.rs:1667`) — so it can never fail. No
mutation path checks the organization of the account that actually exists in
Slurm:

- `set_limit` calls `get_account(account.name(), …)`, which applies no
  organization filter, then runs
  `sacctmgr --immediate modify account <name> set GrpTRESMins=… where cluster=…`
  with no `is_managed()` test.
- `cancel_pending_project_jobs` / `cancel_pending_user_jobs` run
  `scancel --account=<x>` / `--user=<x>` with **no lookup and no guard at all**.

**Attacker path.** The account name is `ProjectMapping::local_group()`, entirely
peer-chosen (see [R14](#r14)). Send
`set_local_limit myproj.myportal:<victim_account> 0` — the agent finds the
victim's real account (organization e.g. `physics`) and zeroes its
`GrpTRESMins`, blocking every job of another tenant. Any value can be set, up
or down. Send `remove_local_project x.y:<victim_account>` and all pending jobs
of an arbitrary account are cancelled; `remove_local_user` does the same for an
arbitrary user.

**Contrast.** `op-freeipa`'s guards are solid: `is_managed()` is an exact
membership test against `memberof_group`, all authorising lookups use `cn` as a
kwarg (exact match), the two substring `group_find` uses are non-authorising
enumeration whose results are intersected with an exact member set, and every
guard fails closed on error.

**Fix.** In `set_limit`, after `get_account`, return an error unless
`account.is_managed()`. Give both `cancel_pending_*_jobs` the same treatment:
resolve the account/user first and refuse if it is not in the managed
organization.

**Fix applied.** `set_limit` now calls the `is_managed()` that already existed
and refuses an account in any other organization. Both `cancel_pending_*_jobs`
resolve their target first - `get_account` for the project form, `get_user`
plus an association walk for the user form, since `SlurmUser` carries no
organization of its own and "managed" is therefore defined by being associated
with at least one managed account - and refuse anything unmanaged. A target
that does not exist is a logged no-op rather than an error, so removal stays
idempotent.

---

<a name="r6"></a>
### R6 — Wire-supplied `version`/`changed` drive an unbounded loop under the board write lock · High · source

> **Status: fixed** (2026-07-30) - see **Fix applied** at the end of this finding.

**Location:** `templemeads/src/board.rs:256-271`; `templemeads/src/job.rs:381`
(`increment_version`), `:200-206` (both fields are plain `Deserialize` inputs).

```rust
else if job.changed() > j.changed() {
    let newer_version = j.version();
    *j = job.clone();
    while j.version() <= newer_version {
        *j = j.increment_version();       // self.version + 1
    }
```

**Attacker path.** Two messages. First: `Update` with id `U`, `version: 2^40`,
`changed: T1`, far-future `expires`. Second: same id, `version: 0`,
`changed: T2 > T1`. `0 > 2^40` is false but `T2 > T1` is true, so
`newer_version = 2^40` and the loop runs ~10¹² iterations, each performing a
full deep clone of the Job. With `version: u64::MAX` in the first message the
loop **never terminates**: the release profile sets no `overflow-checks`, so
`self.version + 1` wraps to 0 and `0 <= u64::MAX` holds forever.

**Why it is worse than one hung task.** The loop is synchronous, with no
`.await`, executed while holding `board.write()`. So the tokio worker never
yields and cannot be rescheduled; that board's lock is held forever; the global
`clean_boards` task blocks on it and **no board on the agent is ever cleaned
again**; and `aggregate_job_stats` blocks, so every subsequent `HealthCheck`
spawns a task that never completes. An attacker holding *k* links pegs *k*
workers, and tokio's default worker count is the core count.

**Fix.** Reject or clamp wire-supplied `version`; replace the loop with
`j.version = newer_version.saturating_add(1)`; never hold the board lock across
unbounded work.

**Fix applied.** Two changes. The loop is gone: `Board::add` now jumps straight
to `newer_version.saturating_add(1)` via a new `Job::with_version`, instead of
calling `increment_version()` (a full deep clone) until it passed the stored
value. And `increment_version` itself now uses `saturating_add`, so it cannot
wrap silently in a release build.

On top of that, and at the maintainer's suggestion, `Board::add` rejects any
Job whose `version` exceeds `MAX_PLAUSIBLE_JOB_VERSION` (2^60). A real Job's
version counts single increments from zero, so a value near this is a bug or a
peer probing the version handling - it is logged and refused rather than acted
on, which also means the saturating arithmetic below it is never reached in
practice.

---

<a name="r7"></a>
### R7 — ~~`op-cloudportal`'s human approval gate is bypassable after approval~~ · **Not a bug** · source

> **Status: closed as not-a-bug** (2026-08-03), on the maintainer's design
> statement, reaffirmed three times. The human gate was never intended to cover
> project membership: it is a **single review of the initial creation of an award**
> (and of an increase above a threshold, which the web portal detects). All
> subsequent member changes are approved automatically, by design. This finding
> measured the implementation against a requirement that does not exist.
>
> The two sub-observations noted below stand as code-quality items rather than
> security findings, and are scoped by the deployment: `op-cloudportal` and
> `op-cloudaccount` are temporary prototypes on a locked-down host with **no
> inbound network access**. See
> [§4.3](#43-scope-note--the-cloud-prototype-agents).


**Location:** `cloudportal/src/state.rs:217-232` (`update_award`), `:337-343`
(`approved_unprovisioned`); `greatwestern/src/grammar.rs:2659-2661` (`merge`).

**What it is.** `update_award` reads the record, merges peer-supplied details,
and writes it back **without resetting `status`**. `AwardDetails::merge`
replaces the member map wholesale (`if other.members.is_some() { merged.members = other.members.clone() }`).
The background poller then provisions any record with
`status == Approved && !unprovisioned_members().is_empty()`.

**Attacker path.** The upstream portal peer — exactly the party the approval
gate exists to constrain — sends `create_award` with a modest member list; the
operator reviews it and runs `approve`; the attacker then sends `update_award`
with a new member map. Within one poll interval those members are provisioned
on the cloud account with **no second human review**. This is not a race: it
works at any time after approval, indefinitely. The gate only ever gates the
member list that happened to be on disk at the instant the operator typed
`approve`.

**Corollaries.** `AwardDetails.earliest_approve` and `membership_control` are
declared and documented but have **no callers outside `grammar.rs` and the
Python bindings** — they are documented controls that no agent enforces.
`allowed_domains` is likewise unenforced: `AwardDetails` uses a *derived*
`Deserialize`, so the wire path never runs the `validate_member` /
`is_email_allowed` checks that `set_members` would. And `Note` is plain
`Deserialize`, so a peer can forge notes attributed to `"cloud-operator"`,
muddying the approval audit trail.

**Creation is safe:** `create_award` hardcodes `status: Pending` and
`AwardStatus` is not a field of `AwardDetails`, so no payload can create an
already-approved award.

**Fix.** If a merge changes `members` (or `allocation`/`template`/`end_date`) on
an `Approved` record, reset `status` to `Pending` and clear
`provisioned_users`; or reject the update for non-`Pending` records. Record a
hash of the approved details and have the poller refuse to provision if the
current details no longer match. Enforce `earliest_approve` and
`membership_control`. Give `AwardDetails` a `try_from` `Deserialize` that runs
the member validation, and route `merge` through `set_members`.

---

<a name="r8"></a>
### R8 — IPv4/IPv6 truncation in `IpOrRange::matches` defeats every IP allow-list · High · **proven**

> **Status: fixed** (2026-07-30) - see **Fix applied** at the end of this finding.

**Location:** `paddington/src/config.rs:519-528` (`matches`), consumed at
`:991-999` (`may_attempt_connection`), `paddington/src/server.rs:95`,
`paddington/src/connection.rs:1299` (`trusted_proxy`) and `:1337`
(`ClientConfig::matches`).

**What it is.** `matches` delegates to `iptools::iprange::IpRange::<IPv4>::contains`,
which parses an IPv6 argument to a `u128` and compares it as **`addr as u32`** —
a silent truncation to the low 32 bits. Any IPv6 address whose last 32 bits
fall inside an IPv4 CIDR matches that CIDR. The `/0` special cases at
`:369-384` and `:517-518` *do* check the address family, which makes
`0.0.0.0/0` paradoxically **stricter** than `10.0.0.0/8` with respect to IPv6
sources.

**Verification.** Proven through paddington's own public API against
`iptools 0.3.0` as locked:

```
trusted_proxy=127.0.0.0/8    peer=2001:db8::7f00:1     matches => true
trusted_proxy=10.0.0.0/24    peer=2001:db8::a00:5      matches => true
trusted_proxy=127.0.0.0/8    peer=203.0.113.9          matches => false   (control)
```

**Attacker path.** Against an agent reachable over IPv6 (or OS dual-stack —
`ipv6-support-design.md` §2 leaves this to the OS) whose config carries the
documented `trusted_proxy = "127.0.0.0/8"` plus a `proxy_header`: connect from
any controlled prefix whose low 32 bits are `7f00:00xx`. `trusted_proxy` now
matches, so `connection.rs:1299` honours the attacker's own `X-Forwarded-For`
and the attacker becomes any allow-listed client IP. F6's stated "fails closed"
guarantee no longer holds. The F11 fail-fast filter falls the same way, so a
flood again reaches the WebSocket-upgrade and crypto path.

**Fix.** Reject the cross-family case before calling `iptools`:

```rust
IpOrRange::Range(range) => {
    let range_is_v4 = range.contains('.') && !range.contains(':');
    if range_is_v4 != addr.is_ipv4() { return false; }
    …
}
```

Decide deliberately whether `::ffff:a.b.c.d` should canonicalise to
`a.b.c.d` (dual-stack listeners need this to work with IPv4 allow-lists at
all) — but do it with `IpAddr::to_canonical()`, not truncation, and apply it
consistently to `IP`, `Range` and `List`. Add cross-family tests; the existing
suite only covers same-family cases.

**Fix applied.** `IpOrRange::matches` now canonicalises the address with
`IpAddr::to_canonical()` and then only ever compares a range against an address
of its **own** family, discriminated by whether the range string contains a
colon. Canonicalisation is what keeps the legitimate case working: on a
dual-stack listener an IPv4 peer arrives as `::ffff:a.b.c.d`, and an IPv4
allow-list entry should still match it - while `2001:db8::7f00:1` no longer
matches `127.0.0.0/8`. Covered by
`test_ip_range_never_matches_across_address_families` (both directions, plus
lists) and `test_ipv4_mapped_ipv6_address_matches_ipv4_rules`.

---

<a name="r9"></a>
### R9 — Bridge invite (the HMAC API key) written world-readable · High · source

> **Status: fixed** (2026-07-30) - see **Fix applied** at the end of this finding.

**Location:** `templemeads/src/bridge_server.rs:284-308` — plain
`std::fs::write` at `:305`. Reached from
`templemeads/src/agent_bridge.rs:342-352` (`op-bridge bridge --config <file>`).
Contrast `paddington/src/config.rs:81-92` (`write_secret_file`, `0o600`).

**What it is.** F9 routed `paddington::config::save` and both
`paddington::invite::save` functions through `write_secret_file`. This third
invite writer was missed. `std::fs::write` lands at the process umask —
commonly `0644` — permanently, with no later chmod, and `create_dir_all` uses
default `0755`. The file holds `url` plus `key: SecretKey`, the HMAC credential
for the bridge API, and unlike the paddington service config it has **no
at-rest encryption option at all**.

**Attacker path.** Any unprivileged local account on the bridge/portal host
reads the file and can then sign valid bridge requests: `POST /run` to submit
arbitrary instructions as the portal, `POST /restart` to kill any agent whose
dot-path it guesses (see [R2](#r2)), and `POST /diagnostics` to pull recent log
lines from every reachable agent (see [R19](#r19)). This is the
highest-privilege entry point in the system.

**Why it was missed.** `write_secret_file` is `pub(crate)` to `paddington`, so
`templemeads::bridge_server` structurally *cannot* call it — the fix was
applied per-call-site rather than by making the unsafe primitive unavailable.

**Fix.** Export a crate-public secret-file writer from `paddington` and use it
here; create the parent directory `0o700`. Better, make it the only way to
serialise anything containing a `SecretKey`, and add a grep-assertion in CI
that no `fs::write` remains on a key-writing path. Consider having the Python
client warn if the invite is group/other-readable on load.

**Related** (see [R33](#r33)): `write_secret_file` itself restricts permissions
*after* the secret bytes are on disk, does not set the mode at create, and
follows symlinks.

**Fix applied.** `paddington::config::write_secret_file` is now `pub` and is
used by `templemeads::bridge_server::save`, making it the single writer for
every file in the workspace that contains key material. While making it
crate-public, two weaknesses in the helper itself (raised as hardening items
under [R33](#r33)) were also closed: it now sets mode 0600 **at creation** via
`OpenOptions::mode` rather than with a `set_permissions` call afterwards -
which left a window in which the secret was already on disk at the umask, and
did not lower the mode of a pre-existing file at all, since `std::fs::write`
preserves it - and it creates a missing parent directory 0700 rather than at
the umask. `config::save` no longer creates that directory separately (it was
doing so at the umask) and now takes `&Path`.

---

<a name="r10"></a>
### R10 — Handshake nonce counter resets on restart while the peer's window persists · High · source

> **Status: fixed** (2026-07-30) - see **Fix applied** at the end of this finding.

**Location:** `paddington/src/anti_replay.rs:211-227`
(`HandshakeNonceState` — `next_nonce` **and** `replay_window` in one struct,
`Default` gives `next_nonce = 0`), `:70-76` (first-nonce init);
`paddington/src/connection.rs:46-69` (process-lifetime `Lazy` map), `:704`,
`:772`, `:800`, `:865`, `:1474`, `:1503`, `:1541`; the same pattern at
`paddington/src/relay.rs:215-233` (`BOOTSTRAP_NONCE_STATE`).

**What it is.** The state that protects handshake/bootstrap messages correctly
persists across reconnects, because those messages ride under the *permanent*
pre-shared key. But the outgoing **counter** lives in the same memory-only
structure and restarts at 0 on every process start. The two ends therefore stay
consistent only if both processes restart together.
`replay-protection-design.md` §10.6 reasoned about the receiver's window
("a fresh window for every peer … no worse than today") and never about the
sender's counter meeting a *persistent* window.

**Failure path — no attacker required.**
1. Client A and server S run for a while. Each successful connection burns two
   nonces (`Handshake`, `PeerDetails`), so S's window for A reaches
   `highest = H ≈ 2 × reconnects`, with every bit 0..H set.
2. A restarts — deploy, crash, OOM, or a `Restart` from any neighbour
   ([R2](#r2)). A's counter → 0; S keeps its window.
3. A sends `Handshake` nonce 0. For `H < 1024` the bit is set; for `H >= 1024`
   the age exceeds the window. Either way `check_handshake_replay` rejects and
   S closes the socket.
4. `client::run` retries every **5 s**, burning exactly one nonce per attempt.
   A cannot connect until its counter reaches `H + 1`.

**Outage ≈ 5 s × (H + 1)**, i.e. ~10 s per prior reconnect. A hundred prior
reconnects is 17 minutes; ten thousand is 28 hours. The symmetric case (S
restarts, A does not) breaks the client's window instead.

**Attacker amplification (on-path, no keys).** An attacker who simply RSTs the
TCP connection forces a reconnect every ~5 s, advancing `H` by 2 each time —
about 34,500 per day. The next restart then produces roughly *two days of
outage per day of flapping*, and it persists after the attacker stops. This
converts a transient capability into durable denial. There is also a
window-poisoning variant: replay a captured `Handshake` into a freshly
restarted server, whose window is uninitialised and therefore accepts
unconditionally, setting `highest` to the captured nonce.

**The relay path is worse.** Each failed bootstrap costs
`BOOTSTRAP_TIMEOUT` 30 s + `BOOTSTRAP_RETRY_DELAY` 5 s = 35 s. A malicious
proxy can force completed re-bootstraps by dropping the relay hop, advancing
`H` every ~5 s. And `SessionUnknown` — the message that exists specifically to
recover from one side restarting — is sent with nonce 0 and is therefore
*guaranteed* to be rejected after a restart, so the designed recovery path is
the one that cannot work.

**Fix.** Anchor freshness in something that survives a restart, or is
co-invalidated with one. In rough order of preference:
- Make the nonce `(epoch, counter)` where `epoch` is a per-process
  random-or-monotonic value: accept any counter under a strictly-greater epoch,
  reject a repeated pair. Smallest change; fixes both the natural and amplified
  cases.
- Or persist `next_nonce` per peer, fsynced ahead of use, with a generous
  forward skip on load (the IPsec/WireGuard answer).
- Or replace the counter with a receiver-supplied challenge and drop the window
  for handshake/bootstrap entirely.

Additionally, do not initialise a window from a handshake that never completed:
commit `highest` only once the peer has proved session-key possession, which
removes the poisoning variant. Correct §10.6 of the design doc and §3.3 of
round 1.

**Fix applied.** Every handshake and bootstrap message now carries an `epoch`
alongside its nonce - a random 64-bit value generated once per process start -
and the receiver keeps **one replay window per epoch** rather than one per peer,
in a most-recently-used-first list bounded at 8 (`MAX_TRACKED_EPOCHS`).

Three things had to hold simultaneously, and only per-epoch windows achieve all
three:

- *A restart must not wedge the link.* An unseen epoch gets a fresh window, so
  the restarted peer's nonce 0 is accepted immediately.
- *A replay must still be rejected.* The superseded epoch's window is
  **retained**, so a captured message lands on the window that already recorded
  it. Note that the obvious reading of "reset the counter when the epoch
  changes" - discarding the old window - would have been *weaker* than a single
  window, because an attacker could alternate a replay with genuine traffic and
  have the window cleared for them every time.
- *Client HA must keep working.* Several processes legitimately present the same
  `name@zone` ([highavailability.md](highavailability.md) §2), each contributing
  an epoch. This is also why the epoch is **random rather than clock-derived**: a
  monotonic epoch with a strictly-greater rule gives replay resistance but
  rejects the lower-epoch HA replica as a replay, so it could never reach
  standby.

The epoch rides inside the AEAD like the nonce, so it cannot be forged or
stripped by an on-path attacker, and it is `#[serde(default)] Option<u64>` on
all five message types, so a peer built before the field reads as `None`, gets
its own window, and behaves exactly as before - no flag day required. Nothing is
written to disk.

The accepted residual is the eviction bound: at 8 tracked incarnations the
least-recently-used window is dropped, making that (long-superseded)
incarnation's nonces replayable again. Reaching it takes 8 further
restarts/replicas after the capture, and §10.1 of the design doc establishes
that a replayed `Handshake`/`PeerDetails`/`Accepted` buys only wasted work.

Covered by six `anti_replay` tests (restart accepted; superseded-epoch replay
still rejected, including under interleaved genuine traffic; concurrent epochs
not resetting each other; the eviction bound; the `None` legacy slot; epoch
stability) plus backward-compatibility tests asserting each of the five message
types deserialises with `epoch: None` when the field is absent.
`docs/plans/replay-protection-design.md` §10.6 has been corrected and the
reasoning recorded as its §11; `wire-protocol.md` §4.2/§4.3 document the new
field.

**Validated live** (2026-07-30) against real agents, directly connected and via
a real `op-proxy`, across repeated disconnect/reconnect cycles - the path this
finding broke. Not yet validated against a pre-epoch peer binary; see §7.

---

<a name="r11"></a>
### R11 — F3's rate-limit bypass persists: the left-most `X-Forwarded-For` entry is client-seeded · Medium–High · source

> **Status: fixed** (2026-07-30) - see **Fix applied** at the end of this finding.

**Location:** `templemeads/src/bridge_server.rs:331-341` (`forwarded_ip`),
consumed at `:377-381`. The behaviour is enshrined by a unit test at
`:1371-1385`.

**What it is.** When the TCP peer matches `trusted_proxy`, the middleware takes
`X-Forwarded-For.split(',').next()` — the **left-most** entry. Every standard
appending proxy builds XFF as `<client-supplied>, <real peer>`: nginx's
`$proxy_add_x_forwarded_for` is literally `"$http_x_forwarded_for, $remote_addr"`,
and Cloudflare appends the visitor IP to any pre-existing header. So the value
read is the untrusted left edge of a list the client can seed.

**Attacker path.** The operator follows F3's own deployment guidance
(`--trusted-proxy 127.0.0.0/8` for "a Cloudflare tunnel or in-cluster ingress
on loopback"). The attacker sends `X-Forwarded-For: 203.0.113.<random>` with
each request; the origin receives `203.0.113.N, <real IP>`; `forwarded_ip`
returns `203.0.113.N`; the rate limiter allocates a fresh bucket per request.
This is verbatim the attack F3 describes and claims to have closed. It also
poisons every rate-limit warning log with attacker-chosen addresses. Secondary:
with a `/8` trusted range, any local process — including an unprivileged user
hitting `127.0.0.1:3000` directly — is itself "a trusted proxy".

**Note the inconsistency.** Paddington's equivalent
(`connection.rs:1300-1302`) requires the *whole* header value to parse as one
`IpAddr`, so it fails **closed** — but that also means paddington's
`proxy_header` is unusable behind any appending proxy, since `"client, proxy"`
will not parse and the connection is rejected. The two layers should agree.

**Fix.** With a single trusted proxy, take the **right-most** XFF entry; better,
walk the list right-to-left skipping entries that match `trusted_proxy`
(rightmost-untrusted). Prefer `X-Real-IP`/`CF-Connecting-IP` where available.
Update the test at `:1379`, which currently locks in the defect. Align
paddington's parser with whatever is chosen, and warn at startup when
`trusted_proxy` includes loopback while the listener is reachable by local
users.

**Fix applied.** `forwarded_ip` now walks `X-Forwarded-For` from the **right**,
skipping entries that themselves match `trusted_proxy`, and takes the first
untrusted address - the "rightmost untrusted" rule. That address was observed by
a proxy we trust rather than asserted by the client. An unparseable entry stops
the walk rather than sliding further left onto a client-supplied value, and
`X-Real-IP` (a single value, so unambiguous) remains the fallback. The unit test
that asserted the old first-entry behaviour has been replaced by
`test_forwarded_ip_takes_rightmost_untrusted_xff_entry`, and
`test_forwarded_ip_cannot_be_spoofed_by_prepending_entries` asserts the concrete
rotate-a-fake-prefix attack no longer moves the resolved address.

Paddington's own `proxy_header` handling still requires the whole header value
to parse as a single `IpAddr`, so it fails *closed* rather than open - but that
also means it cannot be used behind an appending proxy at all. Aligning the two
layers is left open.

---

<a name="r12"></a>
### R12 — ~~A peer can write to a third peer's Board~~ · **Falsified** · reported

> **Status: falsified on re-check** (2026-08-03). The central mechanism does not
> exist in the code. See **Re-checked** at the end of this finding.

**Location:** `templemeads/src/handler.rs:211-224`;
`templemeads/src/job.rs:892-944` (`update`); `templemeads/src/board.rs:245-426`
(`add`), `:284-364` (duplicate matching).

**What it is.** Boards are per-peer and `Board::add` asserts
`job.assert_is_for_board(&self.peer)` — but `board` is `#[serde(skip)]` and is
set locally immediately before the call, so it is an internal consistency check,
not a provenance check. The normal inbound path (`job.received(&sender)`)
correctly confines a peer to its own board. The handler then breaks that
confinement: on `Update`, the target peer comes from
`job.destination().previous/next(recipient)` — attacker-supplied — so peer A
can insert a Job with an arbitrary UUID, version, state and `result` into the
board B maintains for peer C. `get_waiter` is keyed purely on `(board, uuid)`;
nothing records which agent a result actually came from.

**Reported attack chain** (needs no UUID guessing): A plants a pending job on
C's board at B carrying the instruction B is about to send C; B's real job is
then classified a duplicate (`is_duplicate_of` compares only
`destination().last()` and `instruction()`) and `Job::put` returns early
**without sending anything to C**; A then sends a higher-version `Complete`
with a forged `result`, which `copy_result_from` copies into B's real job and
notifies the waiter. B and everything upstream of it consume A's fabricated
result as if it came from C. `Command::Delete` gives the mirror primitive —
forced failure of someone else's in-flight job.

**Fix.** Never write to a board other than the authenticated sender's. Forward
`Update`/`Delete`/`Put` onward as messages only, and let each hop's own
`received()` be the sole writer of its own board. Key duplicate detection on
the originating agent, and record on each board entry which peer last mutated
it. [R4](#r4)'s adjacency fix also closes the path that reaches this.


**Re-checked (2026-08-03) — the premise is wrong.** This finding says the handler
"breaks that confinement" because "the target peer comes from
`job.destination().previous/next(recipient)` — attacker-supplied". It does not.
All three inbound board writes take the **authenticated sender**:

| Handler | Write |
|---|---|
| `Update` (`handler.rs:631`) | `job.received(&peer)` where `peer = Peer::new(sender, zone)` |
| `Put` (`handler.rs:685`) | same |
| `Delete` (`handler.rs:868`) | `job.deleted(&peer)`, same |

Those are the only three call sites of `received`/`deleted` in the workspace, and
`Job::received` writes to `state::get::<L>(peer)` for exactly the peer it is passed
(`job.rs:831`). `previous(recipient)`/`next(recipient)` are used **only** to choose
which peer to *forward a message to* — and that peer then writes its own board
through its own `received()`, which is precisely the fix this finding recommends
("let each hop's own `received()` be the sole writer of its own board"). It was
already the case.

The reported attack chain fails for a second, independent reason: duplicate
detection scans `self.jobs` **within one board** (`Board::add`), and boards are
per-peer. A Job planted on A's board therefore cannot shadow a Job on C's board, so
`is_duplicate_of` never matches across the two and `Job::put` is not short-circuited.

What *is* real, and is not this finding, is the general question of whether a peer
can get a Job forwarded somewhere it should not — which is what [R4](#r4)'s adjacency
check and the portal-route work address, and what
[§4.1](#41-accepted-trade-off--portal-authority-is-positional-not-cryptographic)
records as the accepted residual. No separate change is scheduled for R12.

This is the third *reported*-rated finding this round to be substantially wrong on
re-check (with [R19](#r19) and [R20](#r20)); see the note in
[§7](#7-method-and-verification-standard).
---

<a name="r13"></a>
### R13 — `op-localaccount`: F13's fix covered only the remove paths · Medium · source

> **Status: fixed** (2026-07-30) - see **Fix applied** at the end of this finding.

**Location:** `localaccount/src/localaccount.rs:158-165`
(`identifier_to_projectid`), `:194-220` (`ensure_group_exists`), `:232-284`
(`sync_groups`, `usermod -aG` at `:273`), `:487-510` (`update_homedir`).

**What it is.** F13 added `is_protected_user`/`is_protected_project` to
`remove_user`/`remove_project`, explicitly citing the bare-group-name collision
(`docker.system` → group `docker`). **The add path still has that collision and
no guard.** `identifier_to_projectid` returns the bare project component for the
internal portals `openportal|system|instance`, and `ensure_group_exists`
returns `Ok(())` whenever `getent group` succeeds — without checking who owns
the group.

**Attacker path.** Send `add_user bob.docker.system`. `local_user` becomes
`bob.docker` and the project group `docker`. `useradd` creates the account,
`ensure_group_exists("docker")` sees the host's real docker group and returns
OK, and `usermod -aG docker,openportal,op_<peer> -- bob.docker` puts the new
account into it — root-equivalent on most hosts. `wheel`, `sudo`, `shadow` and
`adm` work identically. If the account already exists, `useradd` exits 9 and the
code deliberately falls through to `sync_groups`, so an existing dotted account
can be added to the collided group.

`op-freeipa` is **not** vulnerable to the same input: internal-portal group
resolution is restricted to a config-derived whitelist, so `bob.docker.system`
fails closed there.

**Also:** `update_homedir` has no `is_protected_user` guard, unlike its
`remove_user`/`block_user`/`unblock_user` siblings and unlike
`op-freeipa::update_homedir`. The homedir string is never validated anywhere —
it is a bare `String` in the instruction, and on the `AddUser` path it comes
back over the wire from the peer — and is handed to `useradd -d <path> -m`,
which for a non-existent path makes root create the directory and chown it to
the new account. The filesystem agent applies `clean_and_check_path` to every
path it touches; `useradd -m` here gets no equivalent.

**Context.** `op-localaccount` is a testing agent (`op-freeipa` is the
production path) and logs a testing-only warning at startup. This should still
be fixed so a mistaken production deployment fails safe — which was F13's own
stated rationale.

**Fix.** Reject identifiers whose portal is an internal name when they arrive
from a peer; in `ensure_group_exists`/`sync_groups`, refuse any group that
already exists with GID < `MANAGED_GID_MIN` or that is not one this agent owns.
Add `is_protected_user` to `update_homedir`, and validate the homedir with the
same absolute/no-`..`/not-sensitive checks as `clean_and_check_path`.

**Fix applied.** Three changes. `add_project`, `add_user` and `update_homedir`
now refuse an identifier naming an internal portal (`openportal`, `system`,
`instance`), which is what produced the bare group name in the first place -
matching what `op-freeipa` already does in `force_get_user`/`get_users`.
`add_project` and `sync_groups` additionally refuse to adopt a group that
already exists with a system GID, as defence in depth behind that (the project
group is the one entry in the group list derived from a peer-supplied
identifier rather than from configuration). And `update_homedir` gained the
`is_protected_user` guard its siblings already had.

The home directory is now validated too, on both the `update_homedir` and
`add_user` paths, with the same absolute / no-`..` / not-sensitive checks
`op-filesystem::clean_and_check_path` applies - it is a bare `String` on the
wire, is supplied by the *peer* on the add path, and `useradd -m` will create
it as root and chown it to the new account.

---

<a name="r14"></a>
### R14 — Mapping local names permit whitespace and commas · Medium · source

> **Status: fixed** (2026-07-30) - see **Fix applied** at the end of this finding.

**Location:** `greatwestern/src/grammar.rs:267-294` (`ProjectMapping::new`) and
`:373-420` (`UserMapping::new`) — a deny-list, versus
`validate_identifier_component`'s allow-list at `:37-71`.

**What it is.** F5 gave identifiers a strict allow-list but gave mapping targets
only a deny-list: empty, leading/trailing `.`, `/`, leading `-`, control
characters. So `,` `=` `%` `?` `#` `*` `$` `@` and **internal whitespace** are
all accepted. These fields are supplied by the peer inside every `*_local_*`
instruction and are the *identity* half of every privileged operation, with no
length cap.

**Consequence 1 — argument injection into OpenPortal's own grammar.**
Instructions serialise to a space-delimited string and are re-parsed by
`Instruction::parse`, which indexes fixed positional fields. `cluster` rebuilds
instructions by interpolation, e.g.
`format!("{}.{} set_local_limit {} {}", …, mapping, limit.seconds())` at
`cluster/src/main.rs:1352`. A compromised account agent answering
`get_project_mapping` with `proj.portal:grp 999999999` therefore shifts every
later argument: the real limit is dropped and the attacker's value becomes the
Slurm limit. The same shift controls the target `Volume` at `:1395` and
`:1592`, and guarantees a parse failure (DoS) at `:1204`. This lets a
compromised *downstream* agent escalate onto a link it holds no keys for.

**Consequence 2 — query injection into Slurm REST.**
`slurm/src/slurm.rs:1008`/`:1215` interpolate the name into a REST **path**
with no percent-encoding, and `Url::parse_with_params` then *appends* to any
query the injected string introduces. A `local_group` of
`x?with_deleted=true` injects a query parameter. `clean_account_name`/
`clean_user_name` only replace `/` and space and lowercase — enough to stop
path traversal, nothing else. F5's claim that this concern is neutralised does
not hold for mapping fields.

**Consequence 3 — list injection into `sacctmgr`.** A `,` inside a name becomes
a Slurm list separator in `key=value` forms, e.g.
`sacctmgr add account name=a,b`. On read paths this is defanged by an
exact-name `find()` — a lucky accident rather than a guard.

**Fix.** Apply `validate_identifier_component`'s allow-list to mapping targets,
extended with `.` only (`[A-Za-z0-9_.-]`, no leading `-`, no leading/trailing
`.`, length-capped). That one change removes the whitespace, comma, `%`, `?`
and `=` classes at the source. Independently: percent-encode names before URL
interpolation, and stop rebuilding instructions with `format!` in
`cluster/src/main.rs` — construct the `Instruction` value directly.

**Fix applied.** Mapping targets now go through
`templemeads::validate::validate_mapping_target`, an allow-list of
`[A-Za-z0-9_.-]` with no leading `-`, no leading/trailing `.`, no `..`, and the
same 64-character cap as every other component. The interior `.` that made a
deny-list seem necessary is still permitted, so a local account named after
`user.project` is unaffected - but whitespace, `,`, `=`, `%`, `?` and `#` are
not.

That closes the class at its source, which matters most for the injection into
OpenPortal's *own* grammar: `cluster` rebuilds instructions with `format!` into
a space-delimited string that is then re-parsed positionally, so a space in a
mapping shifted every later argument. A round-trip test now asserts that an
accepted mapping survives that path with its arguments intact.

Independently, account and user names are percent-encoded before being
interpolated into a slurmrestd path (`encode_path_segment`), so a `?` cannot
introduce a query parameter even if it reached there from somewhere other than
a mapping - Slurm's own responses, for instance.

**Not done:** the structural suggestion of building `Instruction` values
directly in `cluster/src/main.rs` rather than via `format!`. The allow-list
removes the vector, but the string round trip remains a sharp edge.

---

<a name="r15"></a>
### R15 — Relayed `envelope.zone` is unauthenticated but is half the peer identity · Medium · reported

> **Status: fixed** (2026-07-30) - see **Fix applied** at the end of this finding.

**Location:** `paddington/src/relay.rs:45-51` (`RelayEnvelope`), `:815`, `:856`,
`:989`. Contrast the direct path, which takes the zone from config
(`connection.rs:1435`) and rejects a mismatched claim (`:900`, `:1575`).

**What it is.** `RelayEnvelope { from, to, zone, ciphertext }` is plaintext JSON
outside the AEAD (`Key::encrypt` passes no associated data). `to` is checked
against `my_name`, and `from` is implicitly bound because it selects the
decryption key — but **`zone` is bound to nothing**, and the receiver
propagates the wire value verbatim even though the configured `peer.zone` is
in hand.

**Attacker path.** A malicious proxy rewrites `zone`; or a peer holding the
pair's keys simply sets it. Downstream, templemeads treats `{name}@{zone}` as
the peer identity: boards, job routing, keepalive de-duplication,
`agent::is_virtual`, and peer lookup in `restart.rs`/`diagnostics.rs` all key
on it. Via the synthesised `Connected` event the attacker also chooses the zone
under which the victim registers the peer, so the real board is never synced
and queued jobs for the real `peer@zone` are never delivered. Round 1 §3.5's
claim that a compromised proxy "cannot impersonate either relayed agent to the
other" is true for content and `from`, false for the zone half of the identity.

**Fix.** Use `peer.zone` from the configured `RelayedPeer` and ignore
`envelope.zone` (or reject a mismatch) — one-line changes at `relay.rs:856` and
`:989`. Optionally also thread `from|to|zone` through as AEAD associated data;
`orion`'s hazardous XChaCha20-Poly1305 API accepts it, though the high-level
`aead::seal` hardcodes `None`.

**Open question.** Whether zone is purely a routing label or also a *trust*
label in templemeads/greatwestern was not settled. If any authorization
decision keys on zone, this becomes High.

**Fix applied.** The receive side now uses `peer.zone` - the zone this agent has
configured for that relayed peer - and ignores `envelope.zone` entirely, at
both the bootstrap (`handle_start`) and ongoing-traffic
(`Message::received_from`) call sites. A disagreement between the two is logged
by `warn_on_zone_mismatch`, so a genuine misconfiguration is still visible
rather than silently papered over.

The proxy's own use of `envelope.zone` is unchanged and correct: the proxy is
addressing its *own* connection registry, for which the relayed pair's zone is
the right fallback when `PROXY_CLIENT_ZONES` has no entry.

**Still open:** whether `zone` is a routing label or a *trust* label in
templemeads/greatwestern. That determines whether this finding was Medium or
High, but not the fix - binding to configuration is right either way.

---

<a name="r16"></a>
### R16 — Relay envelopes are accepted over any authenticated connection · Medium · reported

> **Status: fixed** (2026-08-03). Note that [R4](#r4)'s sender-adjacency check
> does *not* cover this path: adjacency applies to a `Job`'s `Destination`, while a
> relay envelope is unwrapped by `paddington` before any templemeads layer sees it,
> so the two are independent. Fixed as defence in depth regardless.

**Location:** `paddington/src/relay.rs:820-832` (`handle_incoming_envelope`),
`:1003-1019` (`relay_dispatch_handler` discards `message.sender()`). Registered
as the top-level handler for **every** templemeads agent, relayed or not.

**What it is.** F7 bound `envelope.from` to the authenticated sender *on the
proxy*. There is no receive-side counterpart: the handler parses the payload as
a `RelayEnvelope` and looks the peer up purely by the wire field
`envelope.from`, never checking that the message arrived over the connection to
the relay the config names for that peer.

**Attacker path.** C is any direct peer of B — B's portal, or a second proxy —
with no keys for and no relationship to the A↔B pair. C sends B
`{"from":"A","to":"B","zone":"…","ciphertext":"00"}`. B accepts it, fails the
PSK decrypt, finds no session for A, and genuinely emits a PSK-signed
`SessionUnknown` to A — which A accepts, dropping its session and
re-bootstrapping. So C can churn a pair's session it has no part in, and inject
arbitrary zone values ([R15](#r15)) without being the proxy.

**Fix.** Pass `message.sender()`/`message.zone()` into
`handle_incoming_envelope` and require them to equal the configured relay name
and `relay_zone` for that peer; drop and log otherwise.


**Fix applied (2026-08-03).** `handle_incoming_envelope` now takes the
authenticated `sender`/`zone` that paddington stamped on the arriving message, and
drops any envelope whose claimed `from` is a peer configured to be relayed through
a *different* relay, or through the same relay on a different connection. The check
is a pure `arrived_over_configured_relay`, tested including the case that a naive
implementation gets wrong: `relay_zone` (the direct connection to the proxy) is
very often not the same as `zone` (the relayed relationship), so comparing the
wrong one would wave the injection through.

This also removes the injection step [R28](#r28) depends on for any attacker that
is not itself the proxy.
---

<a name="r17"></a>
### R17 — `owning_portal` omits 10 identifier-bearing instructions · Medium · source

> **Status: fixed** (2026-07-30) - see **Fix applied** at the end of this finding.

**Location:** `greatwestern/src/grammar.rs:4817-4884`, enforced at
`templemeads/src/job.rs:129-141`.

**What it is.** `Command::parse(…, check_portal = true)` enforces "an
instruction naming portal X may only be issued via a destination whose first
agent is X" — the control that stops one portal's client operating on another
portal's namespace. It is skipped silently wherever `owning_portal` returns
`None`. Verified missing:

```
BlockUser  UnblockUser  IsBlockedUser
BlockProject  UnblockProject  IsBlockedProject
GetStorageReport  GetStorageReports  GetLocalStorageReport
GetAwards
```

while their siblings `AddUser`, `RemoveUser`, `AddProject`, `GetUsageReport`
and the rest of the quota family **are** covered — so this is an oversight in
an otherwise-working control, not an absent design.

**Attacker path.** A bridge client of portal `brics` submits
`brics.provider.cluster block_project victimproj.otherportal`. No owning portal
is found, so no check runs; the portal forwards it and the cluster blocks every
user of another tenant's project.

**Fix.** Add the ten missing arms. Add a test that iterates every `Instruction`
variant and asserts a non-`None` result for each one carrying an identifier, so
future variants cannot silently miss the control.

**Fix applied.** All ten arms added: `BlockUser`/`UnblockUser`/`IsBlockedUser`
to the user match, `BlockProject`/`UnblockProject`/`IsBlockedProject` plus
`GetStorageReport`/`GetLocalStorageReport` to the project match, and
`GetAwards`/`GetStorageReports` to the portal match.

The more important half is the test:
`test_owning_portal_covers_every_identifier_bearing_instruction` enumerates
every identifier-bearing variant explicitly and asserts each resolves to its
portal, and asserts the four genuinely identifier-free variants stay `None`. A
new instruction added without an `owning_portal` arm now fails that test rather
than silently losing the ownership check.

---

<a name="r18"></a>
### R18 — `PortalIdentifier::parse` never received F5's allow-list · Medium · source

> **Status: fixed** (2026-07-30) - see **Fix applied** at the end of this finding.

**Location:** `templemeads/src/portal_identifier.rs:31-51`.

The entire validation is: non-empty, and no space or `.`. No charset
allow-list, no leading-`-` rejection, no length cap — while
`validate_identifier_component` applies `[A-Za-z0-9_-]`, forbids a leading `-`,
and caps at 64 characters. `PortalIdentifier` is deserialised straight off the
wire as the sole argument of `GetProjects`, `GetAwards`, `GetUsageReports` and
`GetStorageReports`.

Today its consumers are comparisons and map keys, so no confirmed
path-or-argv sink was found — but it is the one identifier type the F5 fix
skipped, and it breaks the invariant the rest of the codebase assumes:
`from_validated` is documented as safe *because* "domain crates have already
validated this", which is untrue of the `parse`/`Deserialize` path.

**Fix.** Apply the same allow-list and length cap. It cannot live in
`greatwestern` (templemeads must not depend on it), so lift
`validate_identifier_component` into templemeads and have `greatwestern` call
it. Add tests for `"../x"`, `"-x"`, an embedded NUL, and a 65-character name.

**Fix applied.** `validate_identifier_component` has been lifted out of
`greatwestern::grammar` into a new `templemeads::validate` module - it has to
live in templemeads because `PortalIdentifier` does, and a domain crate cannot
be a dependency of templemeads - and `PortalIdentifier::parse` now calls it.
`greatwestern` calls the same shared implementation, so there is one allow-list
rather than two that can drift.

Tests cover both the `parse` path and the `Deserialize` path, since it is the
latter that the wire uses.

---

<a name="r19"></a>
### R19 — Diagnostics and health are readable and forwardable by any peer · ~~Medium~~ → Low · reported

> **Status: partially fixed and re-rated** (2026-08-03). "By any peer" is not
> true of any unmodified agent binary. See **Re-rated** at the end of this
> finding.

**Location:** `templemeads/src/handler.rs:461-493` (`HealthCheck`), `:505-537`
(`DiagnosticsRequest`); `templemeads/src/diagnostics.rs:63-84` (report
contents), `:564-606` (`RingBufferLayer`), `:782-965`;
`templemeads/src/health.rs:533-640`.

**What it is.** Both commands are honoured for any authenticated peer; the only
filter is the Portal↔Portal rule, itself bypassable via [R3](#r3). Both also
*forward* along an attacker-supplied path, so a compromised leaf can pull
diagnostics from agents several hops away with which it has no relationship —
again exceeding round 1 §3.1's stated bound.

`DiagnosticsReport` is global to the process, not scoped to the requester. It
carries `destination` and `instruction` for up to 200 failed, 200 slow and 200
expired jobs plus all running jobs — i.e. the literal instruction text (user
and project identifiers, mappings, quota values) for **every tenant and every
portal the agent serves** — plus the last 500 tracing events verbatim.
`HealthCheck` returns a recursive map of every agent name in the fleet with
per-process memory/CPU/uptime/job counts, which is the reconnaissance step for
[R2](#r2).

**Log-exfiltration path.** Under `RUST_LOG=debug` — precisely the state an
operator is in when they care about diagnostics — the ring buffer captures
`"Put job: {:?}"`/`"Update job: {:?}"` (full Job Debug), FreeIPA
`"group_find result: {:?}"` (raw directory responses), and Slurm
`"Calling function {} with payload: {:?}"`. Any peer then reads all of it with
one request. No *credential* was found reaching the buffer, so secret
exfiltration is not claimed — but directory contents, project membership and
other tenants' identifiers do.

**Fix.** Restrict both commands, and their forwarding, to a config-declared
operator/bridge principal. Omit instruction text and `recent_logs` from any
report crossing an agent boundary, or scope them to the requesting peer's own
jobs.

**Re-rated (2026-08-03) — "any peer" requires an attacker-authored client.**
`HealthCheck` is constructed at exactly one site (`health.rs:669`, inside
`cascade_health_checks`) and `DiagnosticsRequest` at one (`diagnostics.rs:896`, a
forward of an inbound request). Their only origins are the `HealthCheck` handler,
the `DiagnosticsRequest` handler, `GET /health` and `POST /diagnostics` (both
HMAC-authenticated), and the resource monitor. Account, filesystem and scheduler
agents register `cascade_health = false`, so they never reach the cascade and
refuse to forward — an unmodified leaf emits neither command. As with
[R2](#r2), the precondition is host-level file read followed by a custom client,
not a peer misbehaving. Rated **Low**.

**The exposure is also the intended design, not an oversight.** Whole-deployment
visibility from the portal downwards, *deliberately ignoring zone*, is a required
capability: agents are routinely deployed in private networks the OpenPortal
operators cannot otherwise reach, and health/diagnostics/restart is the only
control plane they have. The Portal↔Portal filter is the boundary that actually
matters, because a different estate is run by a different operator team, and it
is the one case where visibility must not cross. So the recommended fixes above
are explicitly **not** being applied: gating on a control principal would break
the operator path, and omitting `recent_logs` and instruction text would remove
the remote log access that makes unreachable agents diagnosable. Recorded as
accepted in [§4.1](#41-accepted-trade-off--portal-authority-is-positional-not-cryptographic).

**Fix applied (2026-08-03) — one real bug found while checking this.** The
resource monitor called `collect_health("", vec![])` under a comment reading
"without cascading" (`systeminfo.rs:149`, `:173`). `collect_health` cascades
whenever `should_cascade_health()` is true, and an empty `requester` with an empty
`visited` chain filters nothing out — so any instance/platform/provider/portal
agent crossing 90% CPU or 80% memory fanned a full health check to *every* peer in
*every* direction, including upstream, and then discarded the aggregate into a
local log line. Exactly the wrong behaviour at the moment an agent is already
under load, and [R31](#r31)'s unbounded board growth is a plausible way to get
there. There is now a `collect_own_health` that does not cascade, and the monitor
uses it.

Also corrected: `downstream_peers` in `collect_health` is a misnomer — it is *all*
peers minus the requester and the visited chain, with `visited` preventing loops
rather than upward travel. The name is left alone (the behaviour is intended per
the note above) but the comment now says what it does.

---

<a name="r20"></a>
### R20 — Health/diagnostics response caches are keyed on attacker-supplied names · ~~Medium~~ → Low · reported

> **Status: fixed and re-rated** (2026-08-03). Two of this finding's three claims
> were wrong, and its recommended fix is architecturally impossible. See
> **Re-checked** at the end.

**Location:** `templemeads/src/handler.rs:494-498`, `:538-545`;
`templemeads/src/health.rs:494-511`, `:690-712`;
`templemeads/src/diagnostics.rs:704-709`, `:731-767`.

Both response handlers cache unconditionally, keyed on a field *inside the
attacker's payload*, with no check that the response was solicited or that the
name matches the sender. So a peer can inject a fabricated `HealthResponse` for
any agent name, and the operator's health view for that agent becomes the
attacker's fabrication (including forging a peer as connected or
disconnected). For diagnostics, `wait_for_diagnostics_response` accepts any
cached report whose `generated_at` exceeds a baseline — and `generated_at` is
attacker-controlled, so a far-future timestamp returns immediately and wins the
race against the real agent every time. Both maps are unbounded with no
eviction, and `get_cached_health()` deep-clones the whole map on every check.

**Fix.** Only cache a response whose name equals the authenticated sender, and
only while a request to that sender is outstanding. Bound both caches. Use a
locally generated request id and a local monotonic clock rather than the peer's
`generated_at`.

**Re-checked (2026-08-03).** This finding was marked *reported*; on verification
it is partly wrong.

*Wrong — the recommended fix cannot be applied.* "Only cache a response whose name
equals the authenticated sender" contradicts how responses travel. A responding
agent replies to *its own sender* (`handler.rs:931`, `:975` — `send_to(&sender_peer)`),
so the response is relayed back hop by hop and legitimately describes an agent
several hops away. The name inside the payload is the only usable key. Any
implementation of this recommendation would break multi-hop diagnostics entirely.

*Wrong — health does not use an attacker-supplied clock.*
`cache_health_response` stamps `last_updated = Utc::now()` locally on receipt
(`health.rs:503`), and `wait_for_health_updates` compares against that. The
`generated_at` problem is real but **diagnostics-only**.

*Wrong — no per-check deep clone.* `get_cached_health()` does clone the whole
map, but it is called once per `collect_health` (`health.rs:757`), not per poll
iteration; the poll loop reads the map under a read lock without cloning
(`health.rs:832`).

*Right, and fixed.* Two things stand, and neither needs an attacker — which is
why they were worth fixing even after the re-rating:

- **Diagnostics freshness was judged on the peer's clock.**
  `wait_for_diagnostics_response` compared `report.generated_at` against a
  locally-taken baseline. Ordinary clock skew between hosts breaks this in both
  directions: a peer running slightly behind never appears to have answered, and
  one running ahead satisfies a baseline it predates. Reports are now wrapped in a
  `CachedReport { cached_at, report }` stamped on receipt, and freshness is judged
  on `cached_at` — matching what health already did. The "age" log line used
  `generated_at` too, and could print a negative age.
- **Both caches were unbounded with no eviction.** Now capped
  (`MAX_CACHED_HEALTH_ENTRIES = 1024`, `MAX_CACHED_REPORTS = 256`) with
  least-recently-received eviction and a warning when it fires.

Rated **Low**: forging a response still requires a custom client, since an
unmodified agent only ever reports its own `agent::name()`.

---

<a name="r21"></a>
### R21 — No handshake timeout: pre-auth semaphore permits held indefinitely · Medium · source

> **Status: fixed** (2026-07-30) - see **Fix applied** at the end of this finding.

**Location:** `paddington/src/server.rs:86-137` (accept loop,
`try_acquire_owned`); `paddington/src/connection.rs:1169-1178` (permit taken),
`:1271` (`accept_hdr_async`), `:1353`, `:1600` (permit released).

**What it is.** `grep -rn "timeout" paddington/src/` returns two hits, both in
`relay.rs`. There is no `tokio::time::timeout` on any part of
`Connection::handle_connection`, no socket deadline, and the watchdog only
covers post-authentication connections. The F11 permit is released only after
the key/name/zone/version checks succeed.

**Permit release is correct by code path** — all 20 return points either
release explicitly or drop the permit with the frame, and `panic = "abort"`
means there is no unwind to leak on. The leak is by **time**, not by path.

**Attacker path.** Open a TCP connection from any address passing
`may_attempt_connection` — which, in the TLS-terminator deployment round 1
recommends, is every connection, because the TCP peer is always the proxy. Send
nothing: tungstenite's `AttackCheck` only fires on reads that return bytes, so a
silent socket is never rejected and the upgrade future awaits forever.
Repeat 2048 times and the pool is exhausted; every new connection, including
every legitimate peer's, is dropped for as long as the attacker holds the
sockets. Cost: 2048 idle sockets. F11's residual note ("until slots free")
assumed slots free.

Composes badly with [R10](#r10): a listener that cannot accept new connections
plus a lockout that requires many reconnect attempts is a long outage.

**Fix.** Wrap the whole pre-authentication phase in a deadline (10–30 s) around
`accept_hdr_async` and each pre-auth read, or around everything up to the
permit release. Add a per-source-IP concurrent-connection cap so one source
cannot take the whole pool.

**Fix applied.** A 30-second watchdog (`HANDSHAKE_TIMEOUT_SECS`) now covers the
pre-authentication phase. `Connection::handle_connection` takes a
`oneshot::Sender` and fires it at exactly the point it releases the
unauthenticated-connection permit, so the deadline applies to the handshake
only and never to an established connection; the watchdog task selects on that
signal against a sleep, and aborts the connection task if the sleep wins.
Dropping the sender (a failed handshake) also stands the watchdog down. Real
handshakes complete in milliseconds, so 30 seconds is far above any plausible
round-trip while still bounding a slowloris.

---

<a name="r22"></a>
### R22 — No WebSocket message-size limit; work amplified per candidate config · Medium · source

> **Status: fixed** (2026-07-30) - see **Fix applied** at the end of this finding.

**Location:** `paddington/src/connection.rs:1271` (`accept_hdr_async` with no
`WebSocketConfig`), `:688` (`connect_async`, likewise), `:1371-1411`
(peer-selection filter: `message.clone()` plus a full `deenvelope_message`
*per candidate* `ClientConfig`). Confirmed: `grep -rn "WebSocketConfig\|max_message_size\|max_frame_size"`
over the repo returns **no hits**, so tungstenite's defaults apply —
`max_frame_size` 16 MiB, `max_message_size` 64 MiB,
`max_write_buffer_size` `usize::MAX`.

**What it is.** `FrameSocket::read_frame` parses the length header and calls
`in_buffer.reserve(len)` **before any payload arrives**, so ~14 attacker bytes
reserve 16 MiB. And when the payload does arrive, the peer-selection filter
does a full-frame `String` clone, a hex decode and an AEAD open for *each*
candidate config matching the source IP — roughly 3× the frame size in
transient peak, per candidate, all pre-authentication.

**Attacker path.** From any address passing the IP filter (an on-path attacker
spoofing an allow-listed source; a loopback TLS terminator, which makes every
internet host qualify; or a broad CIDR): send one 16–64 MiB frame per
connection, up to the 2048-slot ceiling. Allocation failure aborts the process
under `panic = "abort"` — and then [R10](#r10) turns the crash into a
fleet-wide outage requiring a manual restart of every neighbour. F11 bounded
connection *count*, not per-connection work.

**Fix.** Pass an explicit `WebSocketConfig` to both `accept_hdr_async` and
`connect_async` with `max_message_size`/`max_frame_size` sized to real traffic
(the largest legitimate frame is a `Sync` board dump; 1–2 MiB is generous) and
a finite `max_write_buffer_size`. Restructure the peer-selection loop to
hex-decode once against a borrowed `&str` rather than cloning per candidate.

**Fix applied.** Both directions now pass an explicit `WebSocketConfig` capping
frames and messages at 2 MiB (`MAX_WEBSOCKET_MESSAGE_SIZE`), via
`accept_hdr_async_with_config` on the server and `connect_async_with_config` on
the client, with a bounded write buffer. 2 MiB corresponds to roughly a 512 KiB
plaintext message once the double-hex-plus-JSON envelope's ~4x inflation is
accounted for, which is far above the largest legitimate frame (a board `Sync`
dump). The per-candidate `message.clone()` in the peer-selection loop is
unchanged and remains a smaller multiplier on top of the now-bounded frame
size.

---

<a name="r23"></a>
### R23 — `exchange.rs` overload recovery is dead code · Medium · **proven**

> **Status: fixed** (2026-07-30) - see **Fix applied** at the end of this finding.

**Location:** `paddington/src/exchange.rs:279-288`, `:311-314`, `:326-334`.

**What it is.** `signed_duration_since(rhs)` computes `self - rhs`. All three
overload/logging guards have the operands reversed, so each expression is
negative and monotonically decreasing:

```rust
last_logged_update.signed_duration_since(chrono::Utc::now()).num_seconds() >= 60   // never true
last_update.signed_duration_since(chrono::Utc::now()).num_seconds()        >= 10   // never true
start_reaping.signed_duration_since(chrono::Utc::now()).num_seconds()      >= 300  // never true
```

So the periodic worker-count log never fires, the "still overloaded" warning
never fires, and — the one that matters — the
`workers.abort_all(); workers.detach_all();` recovery is **unreachable**. The
`while workers.len() > 768 { … sleep(100ms) }` loop therefore has no exit other
than workers actually completing, and while it spins, `rx.recv()` is not called,
so the **unbounded** `exchange.tx` channel grows without limit.

**Attacker path.** A peer floods ordinary messages; each spawns a task with no
admission control. If ≥769 workers are stuck on anything long-lived (a hung
FreeIPA/Slurm connection, the 23 s keepalive sleep), the event loop stops
draining the queue — with no timeout, no log line and no abort. The agent goes
silent while the queue accumulates every message from every peer until OOM,
which under `panic = "abort"` is process death and then [R10](#r10).

**Fix.** Correct all five comparisons to
`chrono::Utc::now().signed_duration_since(x)`. Then bound the pipeline properly:
replace `unbounded_channel` with a bounded channel so backpressure is a send
failure rather than unbounded growth, and cap concurrent workers with a
semaphore instead of spawning first and reaping later. Note `abort_all()` as
written would also destroy other peers' in-flight work — a bounded queue is the
better primitive.

**Fix applied.** All five comparisons now read
`chrono::Utc::now().signed_duration_since(then)`, so the periodic worker-count
log, the "still overloaded" warning and the `abort_all` recovery all fire as
intended. Note that this makes `abort_all` *reachable* for the first time, and
it discards other peers' in-flight work as well - replacing the unbounded
inbound channel with a bounded one, so that overload is expressed as
backpressure rather than growth, remains the better fix and is still open.

---

<a name="r24"></a>
### R24 — Bridge listener has no connection cap, no timeouts, and HMACs 2 MB pre-auth · Medium · reported

> **Status: fixed** (2026-08-03), with one part deliberately not done.

**Location:** `templemeads/src/bridge_server.rs:1290-1298`
(`TcpListener::bind` + `axum::serve`; no `TimeoutLayer`, no
`ConcurrencyLimitLayer`, no semaphore, no IP allow-list), `:409-501`
(verification ordering), `:57-95` (`sign_api_call`).

F11 gave paddington a fail-fast source check and a 2048-permit pool. The
bridge — whose entire purpose is being the externally reachable surface — got
neither, and hyper 1.x adds no default header-read or idle timeout. So:
slowloris and fd exhaustion are unbounded; and the verification order is
rate-limit → header presence → `Date` parse → **HMAC-SHA512 over the whole
body**, where the `Bytes` extractor has already buffered up to the 2 MB default
limit before the handler runs and `sign_api_call` then formats a second ~2 MB
`String` copy. At 10,000 requests / 10 s per IP that is ~2 GB/10 s of HMAC plus
as much copying, per source address, for a signed 401 the attacker never had to
authenticate for. F3's "intentionally generous" note describes the expected
client, not a bound on a hostile one.

**Fix.** Add `TimeoutLayer` + `RequestBodyTimeoutLayer` and a hyper header-read
timeout; add a connection semaphore mirroring `MAX_UNAUTHENTICATED_CONNECTIONS`;
lower `DefaultBodyLimit` to what the API actually needs (the largest legitimate
body is tens of KB) so pre-auth HMAC cost is bounded; consider an optional
client IP allow-list as paddington has.


**Fix applied (2026-08-03).** Three bounds, and **no new dependencies** - all of
this is axum built-ins plus `tokio` primitives already in use, which is worth
preferring here since the workspace now gates on `cargo audit` and warns on unused
crate dependencies:

- `DefaultBodyLimit::max(1 MiB)`, down from axum's 2 MiB default. This is the one
  that matters: the `Bytes` extractor buffers the whole body before the handler
  runs, so it directly bounds the pre-authentication HMAC-SHA512 *and* the second
  ~2 MiB `String` copy `sign_api_call` used to format. 1 MiB is well above any
  legitimate call (the largest is a `send_result` carrying a completed Job) and was
  deliberately not tightened to the "tens of KB" this finding suggests without
  measuring real payloads first - a too-small limit rejects real work.
- A 512-permit concurrency semaphore, mirroring paddington's
  `MAX_UNAUTHENTICATED_CONNECTIONS` (round 1 F11). Fail-fast with 503 rather than
  queueing, matching how paddington treats its own pool.
- A 30 s request deadline. Because the `Bytes` extractor runs inside the handler,
  this also bounds the slow-*body* half of a slowloris.

**Not done:** a pre-header read timeout, which would have to be set on hyper's
builder - `axum::serve` does not expose it. That half of slowloris remains
uncovered, and is one more reason the bridge must stay on an internal network. This
is now stated in `bridge-api.md` rather than left implicit.
---

<a name="r25"></a>
### R25 — Date/`DateRange` parsing accepts the full chrono range · Medium · reported

> **Status: fixed** (2026-07-30) - see **Fix applied** at the end of this finding.

**Location:** `greatwestern/src/grammar.rs:719-733` (`Date::parse`),
`:1051-1114` (`DateRange::parse`), `:1139-1148` (`end_time`), `:1151-1240`
(`days`, `weeks`, `months`, `years`), `:637` (`Hour::end_time`).

`Date::parse` uses `%Y`, which accepts an unbounded digit count when signed, so
`+262142-12-31` and `-262143-01-01` both parse, and there is no cap on a
`DateRange`'s *span*. Reported measurements on the full range:
`end_time()`, `days()` and `weeks()` **panic** on chrono `Duration` overflow;
`months()` and `years()` **never return** (at the top of the range
`from_ymd_opt(year+1,1,1)` is `None`, `unwrap_or(start_date)` then yields an
`end_date` earlier than `current`, so the loop never advances while pushing to
a `Vec` each iteration). Independently the span itself is a resource bomb:
191,491,529 days, so `days()` builds a ~766 MB `Vec<Date>` before panicking.

**Attacker path.** Submit
`get_usage_report proj.portal -262143-01-01:+262142-12-31`. The string
round-trips cleanly through `Instruction`'s Display/parse and `DateRange`'s
serde, and `cluster` forwards it verbatim to the scheduler, where
`for day in dates.days()` allocates then panics — killing `op-slurm` under
`panic = "abort"`. `months()`/`years()` are currently reached only via the
Python bindings, so that loop is a library defect rather than a wire-reachable
one — one caller away.

The comments claiming "all the unwraps are safe, as we are always working with
valid dates" are true of the `unwrap_or`s but not of the bare `+`/`-`
`Duration` operators, which are unguarded.

**Fix.** Reject years outside a sane window in `Date::parse` (e.g. 1970–2200) —
one bound that kills the whole class; cap `DateRange` span (say 5 years);
replace every `date + Duration::days(n)` with `checked_add_signed`; fix the
`months()`/`years()` loops to break when `end_date < current`.

**Fix applied.** Four changes, of which the first closes most of the class on
its own:

- `Date::parse` rejects any year outside 1970-2200 (`MIN_YEAR`/`MAX_YEAR`). That
  alone makes the 191-million-day span unconstructible from the wire.
- `DateRange::parse` caps the *span* at `MAX_DATE_RANGE_DAYS` (~5 years),
  bounding how much work one instruction can ask an agent to do, since the
  report types aggregate per day/week/month across it.
- Every `date + Duration` in `days`/`weeks`/`months`/`years`/`end_time` (and
  `Hour::end_time`) is now checked, so a `DateRange` built through
  `from_chrono` - which bypasses `parse` - cannot panic at the edges of the
  representable range.
- `months()` and `years()` terminate. A shared `advance_past` helper requires
  each step to make *forward progress*; without that, at the top of the range
  `from_ymd_opt(year + 1, 1, 1)` returns `None`, the fallback produced an end
  date earlier than the cursor, and the loop spun forever while pushing a
  `DateRange` per iteration.

Writing the regression test surfaced a fifth unchecked add I had missed in
`weeks()` - the roll-back to Monday and roll-forward to Sunday - which panicked
at `NaiveDate::MAX`. That is now checked too.

---

<a name="r26"></a>
### R26 — One hostile or mistyped cost report OOM-kills `op-cloudaccount` · Medium · reported

> **Status: fixed** (2026-08-03).

**Location:** `cloudaccount/src/accounting.rs:280-299` (`days_touched`),
`:314-344` (`spread_across_days`), `:367-433` (`reconstruct`), `:34-51`
(`CostReportFile`), `:257` (`read_to_string`).

`time_period.start` is a plain `NaiveDate` and `generated_at` a plain
`DateTime<Utc>`; chrono's serde accepts the full range for both. For the first
report the window is `period_start .. generated_at` directly, and
`days_touched` returns one entry per calendar day with no cap, after which
`spread_across_days` inserts a `DailyProjectUsageReport` (four `HashMap`s) per
day.

**Attacker path (a cloud operator writing the directory, or a typo).** One file
with `"start":"-262143-01-01"` yields 191,491,529 `NaiveDate`s (~766 MB) and
then tens of GB of daily reports. A plausible typo — `0001-01-01` for
`2001-01-01` — already produces ~739,000 daily reports, which are then
serialised into a Job result and pushed over the wire. Separately there is no
size cap on `read_to_string`, and `read_dir`/`read_to_string` follow symlinks
with no regular-file check, so a multi-GB file or a symlink to `/dev/zero` is
read entirely into memory.

The parser is otherwise commendably tolerant: bad JSON, unparseable projects
and unreadable files are warn-and-skip; `to_usage` rejects non-finite and
non-positive values before the cast; negative cumulative deltas are clamped.
The problem is size and span, not shape.

**Fix.** Clamp the window in `reconstruct` (reject or clip a `period_start`
more than N days before `generated_at`; reject a future `generated_at`); cap
`days_touched`; check `metadata().len()` before reading; skip non-regular
entries via `symlink_metadata`.


**Fix applied (2026-08-03).** Four bounds:

- `clamp_window` pulls a `period_start` more than ~5 years before the window end
  forward to that ceiling, *before* the window reaches `days_touched` - so an
  absurd start yields a short window rather than a truncated giant one.
- `days_touched` is independently capped at the same limit, so it is safe for any
  pair of dates a caller might pass, including `NaiveDate::MIN`/`MAX`.
- Files larger than 8 MiB are skipped rather than read into memory.
- Entries are stat'd with `symlink_metadata` and skipped unless they are regular
  files, so a symlink to `/dev/zero` is not followed.

Tested with `NaiveDate::MIN`, the realistic `0001-01-01`-for-`2001-01-01` typo, and
normal windows to confirm they are untouched.
---

<a name="r27"></a>
### R27 — `version_numbers[2]` index panic from a Slurm REST response · ~~Medium~~ → **panic fixed; validation deliberately declined** · source

> **Status: the panic is fixed; the remaining suggestion is declined**
> (2026-08-03). See **Decision** at the end of this finding.

**Location:** `slurm/src/slurm.rs:282-339` — panic at `:338`.

The API version comes from the server's `openapi.json` `info.version`, is split
on `.` and parsed into a `Vec<u32>` with **no length check**, and then
`version_numbers[2] += 1` indexes element 2 unconditionally.

**Attacker path.** A compromised or hostile `slurmrestd` — or an on-path
attacker when `slurm-server` is configured as `http://`, which is supported and
which F12 makes an external concern — returns `{"info":{"version":"dbv1"}}` and
answers `/ping` with any JSON. `working_version` is set, then the index panics
and `op-slurm` aborts. It also fires on a legitimate two-component version
string.

**Fix.** `if version_numbers.len() < 3 { return Err(...) }`, or use `get_mut(2)`;
bound the probe loop. The sibling `clusters[0]` access at
`slurm/src/sacctmgr.rs:985` needs the same treatment — its REST twin already
checks.


**Decision (2026-08-03) — the panic is gone; no version *validation* will be
added.** The index panic was closed as part of [R1](#r1)'s sweep: `version_numbers`
is read with `get_mut(2)`, a two-component or otherwise unexpected version string
just stops the probe loop with a warning, and the patch component is incremented
with `checked_add` so a reported `u32::MAX` cannot overflow under the release
profile's `overflow-checks`. `parse_api_version` is now a separate, tested function.

What this finding additionally suggested - treating an unexpected version string as
a hard login error - is **deliberately not being done**, on the maintainer's
reasoning:

> The operator of `op-slurm` is almost certainly the operator of the Slurm cluster,
> so a stricter check risks breaking things that are not broken. Slurm can be
> painful, and baking in a version-string expectation invites failure on a future
> Slurm release, or on a vendor-supplied or locally modified build, whose
> `info.version` does not match what we expect.

This is the right trade for this threat model. The hostile-`slurmrestd` attacker
this finding contemplates is on the *other* side of a trust boundary the operator
already controls end to end - if `slurmrestd` is compromised, refusing to parse its
version string is not what saves the cluster. Meanwhile the cost of being wrong is a
production outage on a Slurm upgrade the OpenPortal operator does not control.

Tolerate-and-warn is therefore the intended behaviour, not a gap: an unrecognised
version stops the probe and logs, and the agent proceeds with the lowest version the
server advertised. The only hard requirement is that no input can panic, and that is
tested (`test_api_version_parsing_tolerates_a_hostile_version_string`, including
`u32` overflow and non-numeric components).
---

<a name="r28"></a>
### R28 — Malicious proxy induces genuine `SessionUnknown` storms · Medium · reported

> **Status: fixed** (2026-08-03) - the amplification, not the related session-divergence note.

**Location:** `paddington/src/relay.rs:922-947` (one `notify_session_unknown`
per undecryptable envelope whenever no session exists), `:900-914` (one
**unbounded** `tokio::spawn(bootstrap(...))` per `SessionUnknown`, no
single-flight), `:518`, `:547-562` (`PENDING_BOOTSTRAPS` entries retained until
success or the 30 s timeout).

Round 1 §3.5 and `security-model.md` state that `SessionUnknown` is sealed under
the permanent key "so the proxy cannot forge one to force a peer into spurious
re-bootstraps". The proxy does not forge it — it **induces the legitimate key
holder to produce one, on demand, per injected packet**: drop every `Start` so
B stays session-less for A, then inject junk envelopes `{from:"A",to:"B",…}`
(valid at B per [R16](#r16)). B signs a real `SessionUnknown` for each; A
accepts them all (fresh, monotonically increasing nonces, so the replay window
is no defence), drops its session, and spawns one bootstrap task per message.
Steady state is rate × 30 s live tasks and map entries, exhausting memory on A
and taking down A's other, non-relayed peers with it.

**Fix.** Rate-limit/debounce `notify_session_unknown` per peer; make bootstrap
single-flight per peer and have the `SessionUnknown` handler merely nudge the
existing `maintain_relayed_client` loop rather than spawn; cap
`PENDING_BOOTSTRAPS`. [R16](#r16)'s fix removes the injection step for
non-proxy attackers.

**Related** (reported): concurrent or reordered bootstraps can leave the two
sides holding *different* sessions, after which neither emits `SessionUnknown`
(both hold *a* session) and every message fails AEAD open silently. There is no
recovery, because keepalive failure calls `disconnect`, which for a relayed
peer finds no entry and returns without touching `SESSIONS`. A session epoch or
generation id would fix both this and the [R33](#r33) TOCTOU.


**Fix applied (2026-08-03).** Three bounds:

- `notify_session_unknown` is **debounced** to one notification per peer per 5 s.
  The legitimate case needs exactly one (the peer re-bootstraps immediately), so
  this costs nothing there while removing the per-packet amplification.
- The re-bootstrap is **single-flight** per peer, tracked in a `BOOTSTRAPPING` set,
  so a burst of `SessionUnknown` messages produces one attempt rather than one task
  and one `PENDING_BOOTSTRAPS` entry each, every one held for up to 30 s.
- `PENDING_BOOTSTRAPS` is **capped** at 256, refusing rather than growing.

[R16](#r16)'s fix removes the injection step entirely for any attacker that is not
itself the proxy.

**Still open:** the *related* note at the end of this finding - that concurrent or
reordered bootstraps can leave the two sides holding different sessions with no
recovery path. That needs the session epoch/generation id it describes, which is a
wire change, and is deferred with [R33](#r33).
---

<a name="r29"></a>
### R29 — HMAC canonicalization is ambiguous: the nonce can be folded into the body · Low–Medium · source

> **Status: fixed** (2026-08-03), via the negotiated versioned form (option 3
> below). See **Fix applied** at the end of this finding.

**Location:** `templemeads/src/bridge_server.rs:57-95` (`sign_api_call`),
`:461-464`, `:521-555`.

The signed string is `\n`-joined with **no length prefixes and no field
count**, in one of four un-tagged shapes selected by `body.is_empty()` and
`nonce.is_some()`. Consequently, for a POST:

```
…\n<function>\n<body>\n<nonce>     ==     …\n<function>\n<body'>
                                          where body' = body ‖ "\n" ‖ nonce
```

The two are the *same bytes*, so the server cannot tell which request produced
the signature — and because the nonce's *presence* is not itself
authenticated, an on-path attacker can replay a captured request with the nonce
folded into the body and **no `X-Nonce` header**, which skips the entire replay
store (no lookup, no insert). F8's reasoning that "the nonce is part of the
signed call string, so all current clients get full nonce-based replay
protection" holds only if the nonce occupies a *distinguishable* slot. It does
not.

**Impact today is bounded, honestly stated:** every handler parses with
`serde_json::from_slice`, which rejects trailing non-whitespace, so the
replayed request authenticates but fails to parse and returns 500. So this is
currently an authenticated-request-forgery primitive **without** a state
change, plus a complete defeat of the nonce mechanism. It becomes exploitable
the moment any endpoint stops using strict JSON, or a client emits a
whitespace-only nonce (an all-space `X-Nonce` arrives as `""` after HTTP OWS
trimming and is still treated as present). GET is accidentally safe, because
with an empty body the two shapes differ.

Also unsigned, and worth noting even though nothing reads them today: the
**query string**, the HTTP method (bound only via the hard-coded `<function>`
literal), and the `Date` header's actual *bytes* (it is parsed and
re-serialised, so alternative RFC-2822 spellings of the same instant all
verify).

**Fix.** Make the canonical string self-describing: length-prefix every field,
or emit a fixed field count with an explicit empty marker for an absent nonce,
plus a leading version tag. Sign `SHA-512(body)` in a fixed-width slot rather
than the raw body. Independently, make `X-Nonce` mandatory (F8's own suggested
switch) and reject an empty or whitespace nonce. Sign the query string before
any endpoint gains a `Query` extractor.


**Compatibility analysis (2026-08-03).** Fixing this **does** break a wire contract -
but the *bridge API* one, not the paddington wire protocol. `sign_api_call` is the
bridge's HMAC over an HTTP call; paddington's transport is untouched. The affected
clients are every computer of that canonical string:
`templemeads::bridge_server::sign_api_call`, the Python library
(`python/src/lib.rs`), the TypeScript bindings, and any portal software signing
requests itself. Changing the canonical form invalidates all of them at once, and
unlike the 0.90.0 nonce rollout the maintainer does **not** control all bridge
clients.

Three options, in increasing order of preference:

1. **Flag day.** Change the canonical form; update the Python and TypeScript
   libraries; coordinate with the portal software. Cleanest result, but requires
   simultaneous updates on both sides of a boundary that crosses an organisation.
2. **Require the nonce on every request.** The actual defect is that the nonce's
   *presence* is unauthenticated, so making the four shapes into two removes half
   the ambiguity for a one-line change. Still breaks any client that omits the
   nonce today (which [F8](security-review.md#f8) explicitly permits), and leaves
   the GET/POST ambiguity.
3. **Negotiated, versioned canonical form — recommended.** Add a request header
   (e.g. `X-OpenPortal-Signature-Version: 2`). When present, the server verifies a
   length-prefixed, fixed-arity string; when absent, it verifies the current form
   exactly as now. Old clients keep working untouched, new clients opt in, and v1
   can be refused later once no client needs it. This is the same shape as the
   `supports_nonce`/`epoch` negotiation that made [R10](#r10)'s change deployable
   against clients of mixed versions, and it is the pattern this project has
   already validated.

Not yet implemented; option 3 is what to build when it is scheduled.

**Fix applied (2026-08-03).** Option 3, negotiated versioning:

- A new `X-OpenPortal-Signature-Version` header. **Absent means version 1**, so
  every existing client - including portal software this project does not control -
  keeps working with no change at all.
- Version 2 signs a seven-field string in which every field is prefixed with its
  byte length and always present (an absent nonce is `0:`), led by a
  length-prefixed `openportal-sig-v2` tag. No field's content can be read as a
  field boundary, the arity is fixed regardless of empty bodies or nonces, and a V2
  string cannot collide with a V1 one.
- An **unrecognised** version value is a 400, never a silent fallback to V1 -
  otherwise mangling one header would downgrade a V2 client to the weaker form.
- `sign_api_call` now produces V2, so any Rust caller is upgraded by
  recompiling; `sign_api_call_with_version` signs an explicit version, which is what
  the server uses to reproduce whatever the client declared. The Python client sends
  the header.
- V1 verification is logged at debug level naming the header to add, so the
  remaining V1 clients are discoverable rather than invisible.

Tested by asserting the exact R29 collision: under V1, a POST signed with body `B`
and nonce `N` produces the *same* signature as one signed with body `B ‖ "\n" ‖ N`
and no nonce (the test pins this, so a change to the legacy form would be caught),
while under V2 the two differ - as do V1 and V2 for identical inputs, and the
empty-body/nonce-in-the-body-slot pair.

**Validated live (2026-08-03)** against a running `op-bridge`, using the project's
own signer rather than a reimplementation, on both a GET (`/health`) and a POST
(`/diagnostics`) path. All six negotiation cases behave as specified:

| Signed as | Header | Result |
|---|---|---|
| V1 | absent | **200** — an un-updated client keeps working |
| V2 | `2` | **200** |
| V1 | `1` | **200** |
| V2 | absent | **401** — verified as V1, correctly rejected |
| V1 | `2` | **401** |
| V2 | `v2` | **400** — never a silent fallback to V1 |

**Removal.** V1 should be refused once every client is known to send `2`; the
maintainer's intent is to drop the negotiation at that point. Until then the
weakness remains reachable for any client that has not moved, which is why this is
recorded as fixed-with-a-migration rather than simply fixed.
---

<a name="r30"></a>
### R30 — `op-cloudaccount` answers usage/limit queries for unassigned projects · Low–Medium · reported

> **Status: fixed** (2026-08-03).

**Location:** `cloudaccount/src/main.rs:188-207`;
`cloudaccount/src/accounting.rs:161-172`, `:203-232`.

`GetProjects` correctly filters by portal, but `GetUsageReport` and `GetLimit`
never consult the assignment state — they scan every `*.json` in the directory
and keep whatever declares the requested project, and `parse_report` accepts
any valid `ProjectIdentifier` from the file. So a peer authorised for portal A
can read the cost history of `someproject.portalB` from this account, and an
operator dropping a report naming another tenant's project has that spend
attributed to that tenant's billing report (with only a warning if the currency
mismatches).

**Fix.** Require `state::get_project_mapping(&project)` to succeed first, as
`GetProjectMapping` already does; in `parse_report`, skip any report whose
project is not in the assignment state.


**Fix applied (2026-08-03).** `get_usage_report` and `get_limit` now call
`assert_project_is_assigned` first, which requires
`state::get_project_mapping` to succeed. Placed inside `accounting` rather than at
the instruction handlers so a future caller cannot bypass it; `GetUsageReports`
passes projects that already come from the assignment state and is unaffected.

Note that the finding's second half - an operator's report naming another tenant's
project having that spend attributed to them - was already prevented by
`dedupe_and_sort`, which filters on an exact project match. The access-control half
was the real gap, and the gate closes both.
---

<a name="r31"></a>
### R31 — Unbounded board and job growth · Low–Medium · reported

> **Status: fixed** (2026-08-03).

**Location:** `templemeads/src/job.rs:200-205` (`expires` is a wire field with
no validation), `:785-845`; `templemeads/src/state.rs:143-163` (`_force_get`);
`templemeads/src/board.rs:74`, `:288`, `:528-539`;
`paddington/src/exchange.rs:355` (`unbounded_channel`).

Four compounding gaps: a wire Job's `expires` is whatever the peer wrote, so a
year-3000 value is never reaped and `Board::jobs` has no size cap or per-peer
quota; the duplicate scan does `for (id, job) in &self.jobs.clone()` on every
new pending job, making N inserts O(N²) full-map deep clones under the write
lock; the `Put` path has no `agent::wait_for`, so `state::get`'s `or_insert`
creates a permanent `State`+`Board` for **any** attacker-chosen name, whose
failed send then queues onto an unbounded `Vec`; and `Command::Sync` accepts an
unbounded `Vec<Job<L>>` re-injected into the unbounded inbound channel. With
[R6](#r6) permanently disabling the cleaner, this is straightforward memory
exhaustion from one link.

**Fix.** Clamp `expires` to a configured maximum on receipt; cap jobs-per-board
and boards-per-process; only create a board for a registered peer; bound
`queued_commands` and `SyncState.jobs`; replace the clone-based duplicate scan
with an index keyed on `(destination.last(), instruction)`.


**Fix applied (2026-08-03).** All five gaps:

- **`expires` is clamped** to at most one hour after creation, in `Board::add` -
  the single point every wire Job passes before being stored. Reaping is what
  bounds a board's size, so a peer-chosen far-future expiry meant a Job was never
  reaped. `checked_add_signed` is used rather than `+`, since `created` is also a
  wire field and could sit near `DateTime::MAX` where the addition would panic
  (cf. [R25](#r25)).
- **Jobs per board** capped at 10,000. Only enforced for Jobs not already held, so
  an update to an existing Job always gets through and a full board can drain.
- **Boards per process** capped at 1,000, and the cap is checked in `_force_get`
  *before* the `or_insert` - so an attacker-chosen sequence of destination names no
  longer leaves a permanent `State`+`Board` behind for each.
- **`queued_commands`** capped at 1,000 per board, so a peer that never reconnects
  stops growing one.
- **`Command::Sync`** refuses a payload of more than 10,000 Jobs.
- **The O(N) deep clone is gone.** The duplicate scan iterated
  `&self.jobs.clone()` - every Job on the board, deep-cloned, on every new pending
  Job, under the write lock. It now finds the candidate through an iterator and
  clones one Job. The loop only ever acted on the first match, so this is
  equivalent; it is still iterated (over an `Option`) rather than an `if let` so the
  existing error paths keep their `continue` semantics exactly.
---

<a name="r32"></a>
### R32 — Bridge responses are neither authenticated nor encrypted · Info (documentation) · reported

> **Status: documented** (2026-08-03), which is the whole of this finding - it was
> always an Info-rated documentation gap rather than a defect. See **Fix applied**.

**Location:** every handler returns a bare `Json<…>`
(`templemeads/src/bridge_server.rs:631-1233`); `python/src/lib.rs:133-134`,
`:209-210` accept any 2xx and deserialise it with no MAC, no nonce echo and no
server authentication.

F12 asserts the bridge is equivalent in protection to the paddington wire and
that "what remains observable … is *metadata*". For the bridge hop that is
inaccurate on two counts: requests are **signed but not encrypted**, so the
payloads — `/run` command strings, `/fetch_jobs` contents, `/get_users` results
including email addresses, usage and storage reports, full diagnostics log
dumps — are *content*, not metadata, and readable on the wire; and the HMAC is
**request-direction only**, so an on-path attacker can forge responses.
Response forgery needs no key material: answer an authenticated
`GET /fetch_jobs` with a fabricated array containing a `remove_project` job and
the portal is *built* to act on it and report back. The deployment note
("op-bridge runs in the same cluster as the portal") is a mitigation, not the
claimed property — and the invite `url` may legitimately be any remote
`http://` host.

**Fix.** Correct F12 and `bridge-api.md` to state plainly that the bridge hop is
integrity-protected in the request direction only, is fully readable on the
wire, and therefore **requires** an external TLS terminator whenever it is not
a loopback hop. Optionally sign responses (HMAC over status, body and the
request nonce) and verify client-side. Consider rejecting a non-`https`,
non-loopback `url` unless explicitly overridden.


**Fix applied (2026-08-03).** [bridge-api.md](bridge-api.md) gains a **§0 —
Deployment requirement: the bridge is not internet-facing**, placed before the
configuration and authentication sections so it cannot be missed, stating that the
bridge must run on a trusted network (private Kubernetes/container network or
loopback) or behind a TLS-terminating reverse proxy, and never be exposed beyond
that boundary.

Crucially it records *why* this is a design choice rather than a limitation:
responsibilities are deliberately split so that **`op-portal` holds the single
internet-facing surface** - one WebSocket endpoint speaking the authenticated,
encrypted paddington protocol - while **`op-bridge` holds the HTTP control surface
on the private side**. Keeping them in separate processes is what stops the portal
from needing both an internet-facing endpoint and an HTTP control endpoint at once.

§0 then states the three consequences plainly, which is what this finding asked for:
request and response bodies are cleartext and the HMAC covers the request direction
only, so a response can be read and tampered with on that hop; there is no
pre-header read timeout (see [R24](#r24)); and the API key is a single shared secret
authenticating the *portal software* rather than individual users. Round 1's
[F12](security-review.md#f12) is cross-referenced in both directions, since its
"an on-path attacker cannot read message content" applies to the paddington wire
protocol and not to this API.

No code change: on a trusted network none of the three is a vulnerability, and the
alternative - authenticating and encrypting the bridge's responses - would duplicate
inside the bridge what the deployment already provides (istio mTLS in the reference
deployment).
---

<a name="r33"></a>
### R33 — Lower-severity hardening cluster · Low · mixed

> **Status: fixed** (2026-08-04) - all 35 items addressed, seven of them by a
> deliberate decision *not* to make the change the item suggested. Each row's outcome
> is in the **Fix applied** table at the end of this finding; the reasoning for the
> whole set is in
> [security-review-2-fixes.md](security-review-2-fixes.md).

| Item | Location | Note |
|---|---|---|
| Relay bootstrap accepts an all-zero session key | `relay.rs:565`, `:797` | F15 fixed the two *direct* handshake paths; the three relay key-transport points have no `is_null()` check. `Key::derive` HKDFs happily from all-zero IKM, so it silently works. Requires the pair's permanent keys. |
| No length validation on `Key` | `crypto.rs:130-135`, `invite.rs:109-115` | Any hex length deserialises. Not exploitable (`chacha20::SecretKey::from_slice` requires exactly 32 bytes and errors cleanly), but a truncated key is accepted at import and only fails opaquely at connect time, and `Key::derive` sizes its HKDF output from the *input* length. |
| Bare `Key`'s derived `Debug` prints the raw key | `crypto.rs:130-135`, doc comment at `:143` | The comment claims Debug renders `[REDACTED]`; that belongs to `SecretBox`, not `Key`. No production call site formats an exposed secret today — but two tests rely on it, proving the vector is live. Hand-write `impl Debug for Key`. |
| Missing salt headers degrade to a constant empty salt | `connection.rs:1254-1266`, `crypto.rs:121-128` | `unwrap_or_default()` plus a hex parse that accepts any length, including `""`. Verified against orion that HMAC accepts an empty key, so this connects successfully with a constant salt, no error and no log. Per-message keys still differ (fresh random `info`), so no key reuse — but the per-connection salt defence silently vanishes. Require `len == SALT_SIZE`. |
| Session keys and config secrets left in un-zeroized heap | `crypto.rs:329-331`, `:376-379`; `config.rs:858-878` | `SecretBox` zeroizes the canonical copy, but the JSON round-trip hex-encodes every session key into a plain `String` and decodes into a plain `Vec<u8>`, neither zeroized; `get_password` returns a bare `String`. Round 1 §3.7's claim is true of the canonical copies only. Local-attacker only (core dumps, swap). |
| `write_secret_file` chmods *after* writing | `config.rs:81-92` | Secret bytes hit disk at umask before the chmod (a local user winning the race keeps its fd); `fs::write` preserves an existing file's mode; it follows symlinks, and several invite paths default to the **CWD**. Use `OpenOptions::mode(0o600)` + `O_NOFOLLOW`, and write-to-temp + rename. |
| Prototype state files world-readable | `cloudaccount/src/state.rs:140-163`, `cloudportal/src/state.rs:124-148` | No `set_permissions` in either crate. Files hold project identifiers, member emails, allocations, and cloudportal's **approval status**, which is trusted completely on read. Create dirs `0700`, files `0600`. |
| cloudaccount state cache keyed on file *contents* | `cloudaccount/src/state.rs:103-131` | Keyed on `state.mapping.project()` read from the file, ignoring the filename, so `a.json` can supply state for project `b.portal` and two files can shadow each other. Validate that filename matches record. |
| `Volume`'s derived `Deserialize` bypasses `Volume::parse` | `greatwestern/src/storage.rs:622-624` | `parse` rejects empty and space-containing names; the `#[serde(transparent)]` derive does not, and `Volume` is a wire-deserialised `HashMap` key. Consumers look it up in the configured volume map, so unknown names error out — hence hardening. Use `#[serde(try_from = "String")]`. |
| Unchecked `u64` arithmetic in `Usage`/`StorageSize` | `usagereport.rs:180`, `:288`, `:295`, `:324`; `storage.rs:108-163` | Release builds have `overflow-checks` off, so a peer-supplied report can wrap a project's billed total silently. `SubAssign` wraps on underflow while the sibling `Sub` clamps. `Div<u64>` has no zero guard. `Allocation::parse` accepts `"inf"`/`"NaN"` (the `< 0.0` guard is false for NaN). |
| `ProjectStorageReport::add_mapping` lacks its sibling's guard | `storagereport.rs:195-200` vs `usagereport.rs:1359-1372` | The usage-report version rejects a mapping whose project differs from the report's; the storage version just inserts, while returning `Result` and thereby advertising a check that does not exist. |
| Relay state keyed by peer *name* only | `relay.rs:162`, `:195`, `:197`, `:215` | `add_client` explicitly supports one name in multiple zones with independent keys; `relay::configure` collapses them, so one provisioned key pair becomes unreachable and the `clients` loop silently wins. Paddington's own registries are keyed `name@zone`. Key all four relay maps the same way. |
| Relay ongoing-traffic replay check has a narrow TOCTOU | `relay.rs:922`, `:950-972` | The lock discipline is otherwise right (the stored window is checked, not the clone), but the `None` arm returns `true`, and a re-bootstrap landing between the read-clone and the write-lock installs a *fresh* window that accepts a replayed old-session nonce by first-nonce init. Stamp sessions with a generation id; reject on the `None` arm. |
| `supports_nonce` is advertised but never enforced | `anti_replay.rs:59-64` | Once a peer says `true`, a nonce-less payload is still accepted unconditionally. Self-defeating for an honest peer, and the state is already tracked, so the check is nearly free. |
| `PENDING_BOOTSTRAPS` keyed by `magic` alone | `relay.rs:197` | Not `(peer, magic)`. Not exploitable (magic is 32 CSPRNG bytes inside the ciphertext) but a free defence-in-depth binding is left on the table. |
| Unbounded caches keyed on peer-supplied identifiers | `slurm/src/cache.rs:26-31`, `freeipa/src/cache.rs:19-26` | Never pruned or capped; slow memory growth under a flood of distinct identifiers. |
| Rate-limiter map unbounded; cleanup probabilistic and O(n) under lock | `bridge_server.rs:423`, `:574`, `:606-612` | `rand::random::<u8>() < 3` is 1.17%, not the 1% the comment claims, and the `retain` runs on the pre-auth path. |
| Nonce store: O(n) `retain` per request under one global mutex; cap 503s everyone | `bridge_server.rs:521-555` | At the allowed request rate the store reaches ~30k entries per address; every subsequent request then scans the whole map serialised on one mutex. At the cap, *all nonced* requests 503 while nonce-**less** ones are unaffected — pushing clients toward the unprotected mode (see [R29](#r29)). Time-bucket it or clean in a background task; prefer evicting oldest over 503. |
| `op-* secret --value` passes the secret as argv | `agent_core.rs:614-620` | Visible in `ps` to any local user. Add stdin/prompt/`--value-file`. Likewise any credential embedded in Slurm's `token-command` argv (F15 fixed the *logging*, not the argv exposure). |
| Slurm raw token-command stdout in an error message | `slurm/src/slurm.rs:213`, `:216` | Reachable only when the output contains no `=`, but it is the one place credential-bearing output can escape to the requesting peer. |
| FreeIPA TLS kill-switch covers the bind-password path | `freeipa/src/freeipa.rs:488-493`, applied at `:519` | Default is verify-on, but one env var downgrades the channel carrying the directory's most privileged credential, process-wide, silently. Prefer a per-server flag plus a loud startup warning, or a custom-CA option. `should_allow_invalid_certs()` logs nothing when enabled anywhere it is used. |
| `is_project_group()` admits `portal == "openportal"` | `freeipa/src/freeipa.rs:1269-1271` vs `:1278-1280` | `remove_project openportal.openportal` passes the guard and resolves to the *managed* group. Blast radius is currently zero because an unrelated filter two layers down skips every user and the group is never deleted — but the guard is being held up by an accident. `get_users` already rejects internal portals outright; do the same here. |
| `clean_and_check_path` is still a pre-canonicalisation deny-list | `filesystem/src/filesystem.rs:156-219` | As F15 noted. With `check_exists: false` (used by `create_dir`, `recycle_dir`, `create_link`) no canonicalisation happens at all, so a symlinked volume root relocates everything. The `.recycle` restore path also `exists()`-checks and renames without `symlink_metadata`, which matters if a volume root is ever group-writable. An allow-list ("must be under a configured root after canonicalising the parent") remains the right end state. |
| `healthcheck.rs` still echoes `Debug` detail | `paddington/src/healthcheck.rs:106-113` | F15 fixed the `bridge_server` copy only. Currently unreachable (`health()` cannot fail); a latent regression for the second handler on that router. |
| `Link.url` has no scheme allow-list | `grammar.rs:1951-1977` | `Url::parse` accepts `javascript:`, `data:`, `file:`. These are documented as for display in a portal UI. Whether that is OpenPortal's problem depends on whether any consumer renders them as anchors. |
| Untagged serde degrades config diagnostics | `config.rs:441-455` | A malformed tagged or `List` value now reports "data did not match any variant of untagged enum Raw" rather than the specific CIDR error. Still rejected. A manual `Visitor` would keep precise errors. |
| `RelayEnvelope` lacks `deny_unknown_fields` | `relay.rs:45-51`, `:1007` | The dispatch handler tries `RelayEnvelope` before the inner handler. No current payload collides, but a discriminating marker field plus `deny_unknown_fields` would make classification robust against future payload shapes. |
| `extract_client_ip` falls back to `127.0.0.1` | `bridge_server.rs:402` | Not spoofable (the header is always stamped), but it silently merges anything reaching it into the loopback bucket. Prefer failing the request. |
| Non-UTF-8 body returns 500 pre-auth | `bridge_server.rs:78-79` | Every other pre-auth rejection is 401; the correct status is 400. An unauthenticated behavioural difference. |
| Legacy v0 config secrets are never warned about | `config.rs:932-936` | A prefix-less secret stays on the weak fixed-salt 8 KiB derivation indefinitely, with no warning on decrypt. No downgrade is possible (a v0 ciphertext is pure hex and can never carry the v1 prefix — verified), so this is operational. Log a one-time warning on the v0 branch. |
| Changing the encryption scheme does not re-encrypt `extras` | `agent_core.rs:396-412` | Switching `Simple`→`Environment` silently invalidates stored secrets until each `secret` command is re-run. |
| `op-proxy` CLI gaps | `proxy/src/main.rs:169`, `:215`, `:266-286` | `allow` does not validate that either name is a known client, compares untrimmed and case-sensitively, and has no `deny`/`remove` counterpart; `init` hardcodes the service name `"proxy"`, so an agent cannot add two proxies in one zone; `config::save` happens *before* `invite.save`, so a failed invite write leaves a half-provisioned pair with an already-leaked key. |
| Invites never zeroized or removed after import | `invite.rs:120-130`, `:178-191` | Read into a plain `String`. Round 1 §5.1 makes destruction the operator's job; the CLI could scrub or delete on success. |
| `Hour`'s `From<NaiveDateTime>` bypasses `from_chrono`'s invariant | `grammar.rs:676-680` vs `:544-556` | "Minutes and seconds must be zero" is not enforced on the `.into()` path. No exploiting caller found. |
| `UsageReport`/`StorageReport` derived `Deserialize` skips `set_report`'s check | `usagereport.rs`, `storagereport.rs` | A wire-supplied report may carry map keys inconsistent with its own `portal` field. Only matters if a receiver trusts the keys rather than re-inserting via `set_report`. |

<a name="r34"></a>
### R34 — The portal-ownership check never runs on the wire path · High · **proven**

> **Status: fixed** (2026-07-30) - see **Fix applied** at the end of this finding.

**Location:** `templemeads/src/job.rs:189` (`Command::parse(&s, false)` inside
`impl Deserialize for Command<L>`), versus `:129-142` (the check itself). The
only `check_portal = true` call sites are `portal/src/main.rs:250` and
`templemeads/src/bridge.rs:99`.

**What it is.** `Command::parse`'s `check_portal` arm enforces "an instruction
naming portal X may only be issued via a destination whose first agent is X" -
the control that stops one portal's client operating in another portal's
namespace, and the control an operator would reasonably assume is what binds an
instruction to its issuing portal. It is passed `true` in exactly two places,
both at the *entry* to the system: where the bridge parses a client command, and
where the portal builds the southbound job. Every Job that arrives over
paddington is deserialised with `check_portal = false`.

And nothing downstream re-checks. Grepping `destination().first()` and
`owning_portal` across `freeipa`, `slurm`, `filesystem`, `cluster`, `clusters`,
`provider` and `localaccount` returns **no hits at all** - the privileged agents
never compare the portal named by an instruction against the destination it
arrived on.

So the portal-ownership property is an entry-point validation, not an invariant
enforced where the action happens. A Job injected directly at any agent never
passes through it.

**Attacker path.** An attacker who controls one agent inside the estate - or who
has had one peer provisioned into it - sends `Command::Put` to a neighbour with
destination `attacker.clusters.cluster` and instruction
`add_user bob.proj.realportal`. `Destination::position` requires only that the
sender appear *somewhere* in the claimed path ([R4](#r4)), which `attacker` at
position 0 satisfies; `clusters` forwards; `cluster` sees itself last and its
runner acts. `destination.first()` is `attacker`, and nothing compares it to
anything.

The consequence is that this needs **one** agent, not two, and the attacker does
not need to be named after the real portal: the check that would have forced
that simply does not run. The route/portal mismatch is real and visible in the
data, but nothing inspects it and nothing logs it.

**Why it was missed.** [R17](#r17) audited `owning_portal`'s *coverage* - which
variants resolve to a portal - and found ten missing arms. It did not ask
whether the function's one caller ever runs on the wire path. Completeness of a
control and reachability of a control are different questions, and this round
initially only asked the first.

**Fix.** Re-run the check where the action happens rather than only at entry.
`Domain::owning_portal` is a trait method, so `templemeads` can call it without
knowing any domain vocabulary, and `agent::my_agent_type()` reads the agent's
*own* configured type from the local registrar - so the receiving agent can
decide entirely from locally-trusted state. See the fix note below for what was
implemented, and [§4.1](#41-accepted-trade-off--portal-authority-is-positional-not-cryptographic)
for why this is bounded rather than a complete answer.

**Fix applied.** The agent that *acts* on a portal-rooted instruction - the Job's
terminal agent, where `Destination::position` returns `Position::Destination` -
re-checks on receipt, in `handler.rs`, that the instruction is issued via the
portal that owns the identifiers it names. The decision is made entirely from locally-trusted state:
`Domain::owning_portal` (a trait method, so `templemeads` needs no domain
vocabulary) and the agent's own declared type from its own startup - nothing from
the wire. An instruction naming no portal is not checked.

It is enforced framework-side rather than in each agent's runner because one
implementation cannot be forgotten by the next agent added - the failure mode that
produced both [R13](#r13) and [R17](#r17)) - and because the discriminator is
already available at that layer.

**Corrected after live testing, twice - and the second correction revises this
finding's diagnosis, not just its implementation.**

The first implementation enforced at every Provider, Platform and Instance agent
rather than only the terminal one. An intermediate hop adds nothing, since the
terminal agent checks the same Job a moment later, and it converts any gap in its
own route table into a silent outage several hops from where it can be diagnosed -
which is what happened in a live test estate. Refusals were also only logged,
leaving the submitting agent waiting for a result that never arrived; they are now
reported back as an errored Job.

The second correction is more substantial. **This finding's premise was wrong.** It
described the check as "an entry-point validation, not an invariant enforced where
the action happens" and treated that as the defect. The description was accurate;
the inference was not. It is an entry-point validation *because the invariant does
not hold internally*: `op-freeipa` builds
`freeipa.shared get_local_home_dir john.aiproject.brics` as part of adding a user,
naming portal `brics` on a destination rooted at itself, and passes
`check_portal = false` when doing so (`freeipa/src/main.rs:218`) precisely because
the rule does not apply to it. Trusted internal creators are deliberately exempt,
and a receiver cannot distinguish such a Job from an injected one.

So the control that was actually missing was never "re-check the same rule at the
receiver" - it was "distinguish portal-originated from delegate-originated
traffic". The zone is the mechanism this architecture already had for that, and
the route table already encodes it, since routes are zone-scoped and only
propagate away from a portal within one zone. The check therefore now applies
**only in a zone where a portal route is known**: an instance's upstream zone,
never the internal zone holding the agents it delegates to.

A consequence worth stating plainly: declaring `type = "portal"` on a peer is what
activates this whole family of controls. An estate that has not done so has no
portal-ownership enforcement on the wire, exactly as before this finding. See
[portal-route-discovery-design.md](../plans/portal-route-discovery-design.md)
§4.7.

**One architecture legitimately differs.** `op-cloudaccount` is an Instance
driven directly by `op-cloudportal`, so its Jobs arrive on a
`cloudportal.cloudaccount` destination while naming the upstream portal that owns
the project (`myproject.waldur`). The ownership property genuinely does not hold
there, so a blanket type-keyed rule would have rejected every Job it receives.
Rather than weaken the rule or infer the exception, it is declared: a new
`instance::run_delegated` opts out, `op-cloudaccount` uses it, and its doc
comment records why and warns against using it elsewhere. The check itself is
split into a pure `check_portal_ownership(job, verify)` so the policy is a
parameter and the behaviour is directly testable without global state.


**Fix applied (2026-08-04).** Every row above is resolved. The table below records
which were changed and which were consciously declined; the rationale lives in
[security-review-2-fixes.md](security-review-2-fixes.md), grouped by subsystem so each
control reads as a whole rather than as 35 disconnected edits.

| Item | Outcome |
|---|---|
| Relay bootstrap accepts an all-zero session key | **Fixed** - refused at both relay receive points, as F15 already did for the two direct paths |
| No length validation on `Key` | **Fixed** - exactly `KEY_SIZE`, via a repr struct so the wire format is unchanged |
| Bare `Key`'s `Debug` prints the raw key | **Fixed** - hand-written `Debug`. Three tests were comparing keys *via* that Debug output, so their assertions were silently vacuous; `Key::equals` (constant-time) added |
| Missing salt headers degrade to a constant empty salt | **Fixed** - exactly `SALT_SIZE`, and a missing header is now distinct from a malformed one. The legacy XOR format is unaffected (un-masking runs on an already-parsed salt) |
| Session keys and config secrets in un-zeroized heap | **Fixed** - `get_password` and both invite loaders use `Zeroizing`, via `secrecy`'s re-export |
| `write_secret_file` chmods *after* writing | **Fixed** - `create_new` temp file plus `rename`, which gives atomicity and symlink safety without needing `O_NOFOLLOW` (and so without a new dependency) |
| Prototype state files world-readable | **Fixed** - `0600`/`0700`, set before the rename so never briefly readable |
| cloudaccount state cache keyed on file *contents* | **Fixed** - a file whose name disagrees with the project inside it is ignored |
| `Volume`'s derived `Deserialize` bypasses `parse` | **Fixed** - `try_from`, with a hand-written `Serialize` so the wire form stays a bare string |
| Unchecked `u64` arithmetic | **Fixed** - all saturating, `checked_div`, and `overflow-checks = true` in release. `Allocation` also rejected `"NaN"`/`"inf"`, a latent hole this surfaced |
| `ProjectStorageReport::add_mapping` lacks its sibling's guard | **Fixed** |
| Relay state keyed by peer *name* only | **Fixed differently** - a colliding configuration is refused on both sides rather than supported; see [§9](security-review-2-fixes.md) |
| Relay replay check has a narrow TOCTOU | **Fixed** - session generation ids, and the `None` arm now rejects rather than accepts |
| `supports_nonce` advertised but never enforced | **Fixed** - now enforced; safe in a mixed fleet because the combination cannot arise between honest peers |
| `PENDING_BOOTSTRAPS` keyed by `magic` alone | **Fixed** - `(peer, magic)` |
| Unbounded caches keyed on peer-supplied identifiers | **Fixed** - capped, with *evict, never flush*, because re-fetching taxes `slurmctld`. Two properties make the caps unreachable by an attacker (no negative caching; a bounded runner pool) and are now documented in the code because they are load-bearing |
| Rate-limiter map unbounded; cleanup probabilistic | **Fixed** - capped, pruned deterministically inside the lock it already holds |
| Nonce store O(n) per request; cap 503s everyone | **Fixed** - lazy purge, and evicts rather than 503s (which failed only *nonced* requests, pushing clients to the unprotected mode) |
| `secret --value` passes the secret as argv | **Fixed** - `--value-file` and stdin added; `--value` kept but warns |
| Slurm raw token stdout in an error message | **Fixed** - that error travels back up the Job chain, so it was the one place credential-bearing output could escape to a peer |
| FreeIPA TLS kill-switch | **Fixed differently** - announced once per process, not per call. A per-server flag was declined (the servers are a redundant identical set); a custom-CA option is recorded as the genuine improvement |
| `is_project_group()` admits `portal == "openportal"` | **Fixed** - rejects all three internal portals, which subsumed and removed `is_system_group()` |
| `clean_and_check_path` is a pre-canonicalisation deny-list | **Fixed** - runtime containment check against the configured roots, plus fd-based `fchown`/`fchmod` opened `O_NOFOLLOW`. A startup allow-list was declined (automounts) |
| `healthcheck.rs` still echoes `Debug` detail | **Fixed** |
| `Link.url` has no scheme allow-list | **Fixed** - http/https only. Its hand-written `Deserialize` was validating *separately*, so the wire path would have bypassed the allow-list; both now route through `set_url` |
| Untagged serde degrades config diagnostics | **Fixed** - a shape-dispatching `Visitor`. The bug was worse than described: a bad entry *inside a `List`* had its specific error discarded entirely |
| `RelayEnvelope` lacks `deny_unknown_fields` | **Fixed** - plus a positive `kind` tag. A wire change, acceptable only because the relay has no production deployments |
| `extract_client_ip` falls back to `127.0.0.1` | **Fixed** - fails the request rather than merging into the loopback bucket |
| Non-UTF-8 body returns 500 pre-auth | **Fixed** - 400 |
| Legacy v0 config secrets never warned about | **Fixed** - warns once per process, naming the fix |
| Changing the encryption scheme does not re-encrypt `extras` | **Fixed** - lists the secrets that will stop decrypting |
| `op-proxy` CLI gaps | **Fixed** - `allow` trims and requires known clients; the invite is written before the config; `init --name` added |
| Invites never zeroized after import | **Fixed** - `Zeroizing`. Destroying the file remains the operator's job (round 1 §5.1) |
| `Hour`'s `From<NaiveDateTime>` bypasses the invariant | **Fixed differently** - truncates to the hour, so the invariant is unconditional, rather than becoming `TryFrom` and breaking the public API |
| `UsageReport`/`StorageReport` derived `Deserialize` skips `set_report` | **Fixed** - both deserialise through `set_report` |

**Also fixed while here, not in the list above:** a cache/Slurm discrepancy called
`cache::clear()` - a wholesale flush of the most expensive cache. All twelve sites now
evict only the named account or user.
---

<a name="s41"></a>
### 4.1 Accepted trade-off — portal authority is positional, not cryptographic

[R4](#r4), [R34](#r34) and [R3](#r3) are all facets of one question: *how does an
agent know that a command genuinely carries its portal's authority?* This
section records the answer the project has settled on, and why it deliberately
stops short of proving that authority cryptographically. It is written in the
same spirit as round 1's F12 and F14: a considered trade-off, documented so it
is not later mistaken for an oversight.

#### The deployment that bounds the risk

Three properties of the intended deployment do most of the work, and were
confirmed by the maintainer:

1. **Routing is confined to a zone.** A Job keeps the zone it arrived in as it
   is forwarded, and the transport binds zone at every hop (round 1 §3.4;
   [R15](#r15) closed the one place it did not). An operator's estate is one or
   more zones that the operator controls end to end.
2. **Only the portal is exposed.** In the reference deployment a single
   `op-portal` is reachable from the internet - via a Cloudflare tunnel or an
   `op-proxy` - and every other agent lives on the operator's private network.
   `op-bridge` sits beside the portal software inside the same cluster, with
   mTLS on that hop, and has no external route.
3. **Portal-to-portal is its own zone.** Portals talk to each other in a
   separate zone with its own rules, so one portal cannot route a command down
   through another portal into its estate.

Together these mean the attacker must either compromise `op-portal` itself -
which no in-protocol control can defend against, since it *is* the authority -
or already have code execution inside the operator's private estate.

#### What the layered fixes achieve

With sender adjacency ([R4](#r4)), the portal-ownership re-check
([R34](#r34)) and declared peer roles ([R3](#r3)) in place, an attacker inside
the estate must:

- hold the pre-shared key for the specific topology position they claim
  (adjacency), and
- issue an instruction whose owning portal matches the first agent of the
  destination it arrives on (ownership), and
- present a role the receiving agent was provisioned to expect (R3).

That moves the requirement from "compromise one agent" to "compromise one agent
**and** have a peer provisioned under a chosen name, in a chosen topology
position, with a chosen role".

#### The residual, stated plainly

None of those three is cryptographic, and all of them ultimately rest on an
agent's **name**, which is a label in a config file. An attacker who can induce
an operator to import an invite naming their agent after the real portal - and
who positions it correctly in the path - satisfies every check above. Worked
through:

1. Attacker controls `op-clusters` and adds a peer named `realportal`. Nothing
   prevents this: `add_client` only rejects a duplicate `(name, zone)`, and
   `op-clusters` has no peer by that name.
2. The fake portal sends a job with destination `realportal.clusters.cluster`
   and an instruction naming `proj.realportal`.
3. Adjacency passes at both hops; ownership passes, because `first()` *is* the
   owning portal; the declared role passes, because the fake portal is declared
   a portal.

**This residual was accepted, and now has a planned route to closure.** The
precondition - code execution inside the estate plus an operator-performed
provisioning step under an attacker-chosen name - is narrow, and the resulting
traffic is anomalous in ways an operator can see: a portal appearing at an
unexpected position, a route shape that differs from the real one.

That last observation turned out to be actionable, and is now **implemented**.
[portal-route-discovery-design.md](../plans/portal-route-discovery-design.md)
describes a scheme in which each agent *derives* the expected route from each
portal - pushed downstream on connection, anchored on the `type = "portal"`
declarations [R3](#r3) added - and refuses instructions that arrive by any other
route, alarming when two different routes claim to lead to the same portal
name. Because the topology is single-pathed and acyclic, two routes to
one portal cannot legitimately occur - so the collision is an unambiguous
signal, and it fires at the compromised agent itself.

Crucially, it detects exactly the case described above: an agent whose **config
or state** was modified while its code remained intact. It does *not* help
against a code-compromised agent, which simply reports one route and lies -
that boundary is still where signing, and only signing, would hold. But
config-level compromise is what this section accepted as residual, so the
residual is addressable without any of signing's cost.

#### Why command signing was considered and deferred

Signing is the only mechanism that closes the residual, because a fake portal
cannot produce the real portal's signature whatever it is named. It was designed
out in full and deliberately not built yet. The reasoning, recorded so it does
not have to be rediscovered:

- **A symmetric shared secret forces a bad choice.** One secret per portal,
  copied to every verifier, means any compromised verifier can forge as the
  portal to every other verifier - reintroducing exactly the god-key property
  round 1 §3.1 says the design does not have. Per-(portal, agent) secrets fix
  that, but require the *portal* to enumerate its verifying agents, which
  destroys a property the design values: today the portal does not need to be
  told which instance agents exist, because that falls out of the agent network.
- **So it would have to be asymmetric** - the portal signs, agents verify with a
  public key. That preserves the dynamism, because the burden lands entirely on
  the agent side and the portal still needs to know nothing about its verifiers.
- **The public key belongs in the agent's config, not behind a URL.** It needs
  authenticity, not secrecy, so out-of-band provisioning - the same mechanism
  every other key in the system uses - is sufficient. Fetching it over HTTPS
  would make the **public web PKI part of OpenPortal's trust base**, in a system
  that deliberately has no PKI (round 1 F14, and residual risk 1); that is a far
  larger change than a config line, and it would be the only place in the system
  whose security did not rest on out-of-band provisioning.
- **The verifier must be the agent that acts.** Verifying at the provider would
  be cheap - portal and provider are already direct peers, so a symmetric
  authority key between them needs no new enumeration - but the attacker chooses
  the destination path and simply does not route through the provider. *Any
  verification an attacker can route around is not a control.* That forces the
  key to the instance agents, where the portal-rooted instruction is acted on or
  delegated.
- **Cost.** A first asymmetric primitive (`orion` is symmetric-only, so a new
  dependency), a canonical signed encoding that must survive the `Erased` hop
  byte-for-byte, a replay store bounded by a clamped `expires` (which makes
  [R31](#r31) security-relevant rather than merely hygienic), a rotation story,
  and one config value per instance agent.

**Two things signing would *not* have covered**, which is part of why it is not
urgent:

- **A spoofed bridge yields genuinely signed commands.** If an attacker
  registers as a `Bridge` and sends `Submit`, the portal signs the resulting
  southbound command with its real key, and every verifier correctly accepts it.
  Signing binds "the portal issued this", not "the portal was right to issue
  it". That decision lives at the bridge boundary - which is [R3](#r3), and
  which signing therefore makes *more* important rather than redundant.
- **Nothing below the first acting agent.** `add_local_user` from a cluster to
  `op-freeipa` is a different instruction, so no portal signature can cover it,
  and the backends trust their instance exactly as they do today. Signing moves
  the high-value target from `op-clusters` up to `op-cluster`; it does not
  remove it.

#### What would change this decision

- A backend serving **more than one portal** in the same zone, which makes the
  positional controls much weaker relative to their cost.
- A less controlled provisioning process - anything that makes "get an invite
  imported under a chosen name" easier than it currently is.
- An estate that is not end-to-end operator-controlled, or agents other than the
  portal becoming internet-reachable, either of which removes property (2)
  above.

**Where this leaves things.** The positional controls are implemented; portal
route discovery is implemented and enforcing, which closes the
config-compromise half of the residual; and signing remains designed but
unbuilt, which is what would be needed for the code-compromise half. That last
piece is the one this section still accepts.

---

### 4.2 Accepted trade-off — the operator control plane spans the whole deployment

[R2](#r2), [R19](#r19) and [R20](#r20) were originally written as authorization
gaps: `Restart`, `HealthCheck` and `DiagnosticsRequest` are honoured for any
authenticated peer, forwarded along an attacker-supplied path, and the reports
carry other tenants' identifiers and up to 500 verbatim log lines. On re-check
(2026-08-03) that framing was wrong twice over, and the corrected picture is
recorded here so it is not re-raised.

#### No unmodified agent binary can originate any of the three

There are five sites in the workspace that construct these commands, and every
one is either an HMAC-authenticated bridge endpoint or a *forward* of an inbound
message:

| Command | Construction site | Origin |
|---|---|---|
| `Restart` | `bridge_server.rs:707` | `POST /restart` |
| `Restart` | `restart.rs:336` | forward only |
| `HealthCheck` | `health.rs:669` | `cascade_health_checks` ← the handler, `GET /health`, the resource monitor |
| `DiagnosticsRequest` | `diagnostics.rs:896` | forward only |
| `HealthResponse` / `DiagnosticsResponse` | `handler.rs:931`, `:975` | reply to the sender only |

`greatwestern`'s `Instruction` enum has no health, diagnostics or restart
variant, so the ordinary Job traffic every agent sends cannot be turned into one
of these commands at any hop - there is no confused-deputy route. And the agents
an attacker is most likely to reach are the least capable: account, filesystem
and scheduler agents register `cascade_health = false`, so they never reach the
cascade, refuse to forward, and run no bridge. Such a binary emits none of the
three.

So the precondition is not "a peer misbehaves" - a peer is an unmodified binary
that cannot do this. It is **host-level read of an agent's config file**, after
which the attacker writes their own client and holds that agent's keys. That bar
is lower than exploiting Rust (it is a file read, which is precisely what
[F9](security-review.md)/[R9](#r9) protect) but far above what the findings
implied. All three are re-rated accordingly.

#### The exposure is a required capability

Whole-deployment visibility *from the portal downwards, deliberately ignoring
zone*, is not an oversight - it is the reason the feature exists. Agents are
routinely deployed where the OpenPortal operators have no other access: inside
customer private networks, behind NAT, on clusters they do not administer.
Health, diagnostics and restart are the only control plane reaching them, and
that includes reading remote logs (`recent_logs`) and remote instruction text,
because those are what make an unreachable agent diagnosable. Restart is
similarly low-cost in this deployment: every agent runs under systemd or
Kubernetes and is restarted automatically, and OpenPortal jobs are idempotent and
re-submitted by the portal software, so a restart is a blip rather than data
loss.

The boundary that *does* matter is **portal to portal**, because a different
estate is operated by a different team, and an external team must not see inside
this one. That filter already exists and holds
(`handler.rs:913`, `:957`, `health.rs:613`, `restart.rs:182`), and after
[R3](#r3) it rests on a config-declared type rather than a self-declared one.

#### What was therefore declined

- **Gating the three commands on a config-declared control principal.** It would
  break the operator path for exactly the agents that need it most, to defend
  against an attacker who already holds a host and a custom client.
- **Restricting them to the downstream direction** by declared agent type. Same
  reasoning - defence in depth against a post-host-compromise attacker, at the
  cost of complicating a relied-upon control plane.
- **Omitting `recent_logs` and instruction text from reports crossing an agent
  boundary.** This is the feature.

#### What was fixed anyway

Three things that were wrong on their own terms, independent of any attacker:
the resource monitor's accidental fleet-wide cascade, diagnostics freshness being
judged on the *peer's* clock, and both response caches being unbounded. Plus the
two ordering bugs in `Restart` (empty destination from a remote sender; the
leaf-node check running after the target decision). Each is described under its
finding.

---

### 4.3 Scope note — the cloud prototype agents

`op-cloudaccount` and `op-cloudportal` are **temporary prototype agents**, deployed
on a single locked-down host with **no inbound network access**, co-developed
alongside cloud operators who are still building their side of the integration.
CLAUDE.md already says as much about their internals; this records what it means for
this review.

Findings against them are therefore weighted differently from the rest. The two
that were fixed - [R26](#r26) (a mistyped cost report exhausting memory) and
[R30](#r30) (answering for unassigned projects) - were fixed because neither needs
an attacker: R26 fires on a **typo** by a cooperating operator, and R30 is a
tenant-isolation error between portals that will matter as soon as more than one
portal uses an account. Both are cheap and neither constrains the design.

What is *not* being pursued is hardening these two against a network attacker, since
there is no network path to them, and reshaping them is expected anyway once the
cloud side of the integration matures (see
`docs/plans/archive/op-cloudaccount-design.md` and
`docs/plans/archive/op-cloudportal-design.md`). [R7](#r7)'s two sub-observations
(`earliest_approve`/`membership_control` having no callers, and `AwardDetails`'s
derived `Deserialize` bypassing `allowed_domains`) sit here: worth closing when
these agents are next reshaped, not worth a targeted change now.

---

## 5. Process and tooling observations

> **Status: all six addressed** (2026-08-03) - see **Fix applied** at the end of
> this section.

Not vulnerabilities, but they bear on how the findings above survived.

1. **`make test` does not run the tests in binary-only crates.** The Makefile
   (and CLAUDE.md) use `cargo test --offline --lib`, which yields **149** tests.
   Plain `cargo test` yields **209** across 22 binaries. The 60-test difference
   is entirely in crates with no lib target — including
   `filesystem` (36), `cloudaccount` (14) and `cloudportal` (5), i.e. the F1
   path-traversal regression tests. CI runs bare `cargo test`, so the gap is
   local-only, but the documented developer command silently skips the tests
   most closely guarding a previous finding. Drop `--lib`.
2. **The privileged agents have essentially no unit tests.** `slurm`, `freeipa`,
   `localaccount`, `cluster`, `clusters`, `portal`, `provider`, `bridge` and
   `proxy` contain **zero** test functions between them — and [R5](#r5),
   [R13](#r13) and [R27](#r27) all live there. Even a handful of guard-behaviour
   tests ("refuses to modify an unmanaged account") would have caught R5.
3. **No dependency-advisory scanning in CI.** `.github/workflows/check.yml` runs
   `fmt`, `clippy`, `build` and `test` but no `cargo audit`/`cargo deny`. The
   409 dependencies are currently clean (verified by inspection of the
   lockfile), so this is preventive. Note also that CI runs bare
   `cargo clippy`, which does not fail on warnings — the crate-level `deny`
   lints still bite, but new clippy warnings do not.
4. **`clippy::indexing_slicing` is not enabled.** The workspace correctly denies
   `unwrap_used` and `expect_used`, which is why the remaining panic class is
   *entirely* slice indexing ([R1](#r1), [R25](#r25), [R27](#r27)). Enabling it
   for `greatwestern` and `templemeads` would close the class structurally
   rather than one site at a time.
5. **`overflow-checks` is off in release** (the default) while
   `panic = "abort"` is on. That pairing means arithmetic bugs corrupt data
   silently rather than crashing ([R33](#r33)'s `Usage`/`StorageSize` items,
   and [R6](#r6)'s non-terminating loop). Turning it on would be a real
   improvement — but only *after* the reachable panics are fixed, since abort
   makes any newly-checked overflow a remote crash.
6. **Two round-1 fixes were applied per-call-site rather than structurally**
   ([R9](#r9): a `pub(crate)` helper the other crate cannot reach;
   [R13](#r13): guards added to remove paths only). Where a fix establishes an
   invariant, making the unsafe primitive unavailable — or adding a
   grep-assertion in CI — is what stops the next site from being missed.

### Fix applied (2026-08-03)

**1 — `make test`.** `--lib` is gone; the target is now
`cargo test --offline --all-targets`, so the binary crates' tests run. The
documented developer command and CI now cover the same set.

**4 — `clippy::indexing_slicing`.** Lints moved from per-crate attributes to a
single `[workspace.lints]` table in the root `Cargo.toml`, inherited by all 19
member crates via `[lints] workspace = true`. `indexing_slicing` is set to
`deny` alongside the existing `unwrap_used`/`expect_used`, plus `dbg_macro`,
`unsafe_code = "forbid"` and `unused_crate_dependencies`. A new `clippy.toml`
exempts test code (`allow-indexing-slicing-in-tests` and friends), so tests stay
readable while production code cannot index. Enabling it turned up exactly one
non-test violation: `impl Index<usize> for Destinations`, whose whole purpose was
to *offer* a panicking accessor. It had **zero callers** — `Destinations::get`
was already used everywhere — so it was deleted rather than made checked. The
panic class is now closed structurally, not site by site.

**5 — `overflow-checks`.** Now `true` in the release profile. Making that safe
required the arithmetic reachable from the wire to be explicitly checked first,
since with `panic = "abort"` a newly-checked overflow would be a remote process
kill:

- `Usage` (`greatwestern/src/usagereport.rs`): `Add`, `AddAssign`, `Sub`,
  `SubAssign`, `Sum`, `parse`'s unit multiplication, and both
  `total_wait_seconds` accumulators are saturating. `Sub` and `SubAssign` also
  *disagreed* — `-` clamped at zero via an `i64` round-trip while `-=` wrapped —
  so the two now behave identically.
- `StorageSize` (`greatwestern/src/storage.rs`): `Add`, `AddAssign`, `Mul<u64>`,
  `MulAssign<u64>` and `Sum` saturate; `Div<u64>` and `DivAssign<u64>` use
  `checked_div(...).unwrap_or(0)`, since **division by zero panics** regardless
  of `overflow-checks` and a zero divisor is reachable from a report file.
- `Allocation::from_size_and_units` and `Allocation::parse`
  (`greatwestern/src/grammar.rs`) now reject non-finite sizes.
  `f64::from_str` accepts `"NaN"`, `"inf"` and `"infinity"`, and the existing
  `size < 0.0` test is *false* for NaN — so both parsed cleanly and then
  saturated to `u64::MAX` downstream. This was a latent hole in [R33](#r33) that
  only surfaced while making the saturation explicit.
- `op-slurm`'s API-version probe incremented the server-supplied patch component
  with `+=`; it now uses `checked_add` and stops probing on saturation.

**3 — CI.** `.github/workflows/check.yml` gains a `Dependency advisories` job
running `cargo audit --deny warnings` against the RustSec database, its `clippy`
step is now `--all-targets --all-features -- -D warnings` (so the workspace lints
are enforced rather than advisory, and test code is linted too), and its `test`
step is `--all-targets`. `make audit` runs the same scan locally.

**6 — structural guards.** Three of them:

- `scripts/check-secret-writes.sh` asserts that no `std::fs::write` or
  `tokio::fs::write` appears outside a small, annotated allow-list, so a new
  key-writing call site cannot reach for the umask-respecting primitive the way
  [F9](security-review.md) and [R9](#r9) both did. Run by `make lint` and by CI.
- The `OPENPORTAL_ALLOW_INVALID_SSL_CERTS` rule was duplicated in `op-bridge`
  and `op-freeipa` — two independent copies of "should I disable TLS
  verification?", either of which could drift into being more permissive. Both
  now call `templemeads::validate::allow_invalid_ssl_certs`, which fails closed
  on anything but the literal `true`, and which has a test.
- `Destination::parse` now rejects an agent name containing whitespace. A
  destination is the first whitespace-separated token of a command string, so
  `"a b.aip1.brics get_projects"` addressed `a`, silently meaning something
  other than what was written. Such a name never worked, so this can only reject
  input that was already broken.

**2 — seed tests in the privileged agents.** 273 tests passed after this step (up
from 263; 285 by the end of the round),
with the new ones chosen to cover the findings that lived in these crates rather
than to raise a coverage number:

| Crate | Tests added | What they pin |
| --- | --- | --- |
| `slurm` | 3 | Only accounts in the managed organization are `is_managed()`, checked against JSON as the server returns it — the exact predicate every mutation path gates on ([R5](#r5)). API-version parsing tolerates two-component, malformed and `u32::MAX` version strings ([R27](#r27)). `clean_account_name` rejects empty input. |
| `freeipa` | 3 | The three internal portal names map to *bare* group names (the `docker.system` → `docker` collision behind [R13](#r13)); legacy IDs keep their `group.` prefix; group membership is read from `member_user`, and degrades to "no members" rather than panicking on a hostile response. |
| `cluster` | 2 | Every delegation in the crate builds its Job by formatting identifiers into a whitespace-separated command string, so the test pins that an identifier which would inject an extra argument or extend the destination path cannot parse in the first place. |
| `portal` | 1 | The offering filter (extracted from `get_offerings` so it is testable without a live agent registry) advertises only relationships in which *this* portal is the local hop. |
| `templemeads` | 2 | The TLS opt-in fails closed; `Destination::parse` rejects whitespace in agent names. |
| `greatwestern` | 4 | Every `Usage`/`StorageSize` operator saturates rather than wrapping, division by zero yields zero, huge durations saturate on parse, and non-finite allocation sizes are rejected. |
| `paddington` | 1 | `write_secret_file` produces mode 0600 (and a 0700 parent) *and lowers the mode of a pre-existing 0644 file* — the case a `set_permissions`-only or `mode()`-only implementation gets wrong ([R9](#r9)). |

`clusters` and `provider` are 61 and 67 lines of pure `main()` boilerplate with
no local logic, and `proxy` is a thin CLI over `paddington::relay`, whose
default-deny `RelayPolicy` is already tested there — so no filler tests were
added for those three. Their logic lives in `templemeads` and `paddington`, which
is where the tests are.

---

## 6. Prioritised remediation

**Tier 1a — done (2026-07-30).** These were prioritised first because they are
the findings an attacker can reach *without* holding a peer key, plus R9, which
any local user on the bridge host can reach. Each is described under **Fix
applied** in its finding.

1. ~~[R1](#r1) — the unguarded `parts` indexes.~~ **Fixed**, and broadened: every
   panicking index/slice operation in our own code is now a checked form, which
   also turned up two latent panics of the same class.
2. ~~[R8](#r8) — the address-family check in `IpOrRange::matches`.~~ **Fixed**,
   with cross-family and IPv4-mapped tests.
3. ~~[R9](#r9) — the bridge invite's file permissions.~~ **Fixed**, and the shared
   writer now sets the mode at creation and creates its parent directory 0700.
4. ~~[R11](#r11) — the `X-Forwarded-For` bypass.~~ **Fixed** (rightmost-untrusted),
   and the test that locked in the old behaviour has been replaced.
5. ~~[R21](#r21) — the handshake deadline.~~ **Fixed** (30 s, pre-authentication
   only).
6. ~~[R22](#r22) — the WebSocket size limit.~~ **Fixed** (2 MiB, both directions).
7. ~~[R23](#r23) — the five inverted `signed_duration_since` comparisons.~~
   **Fixed.**
8. ~~[R10](#r10) — the restart lockout.~~ **Fixed** via a per-process epoch and
   one replay window per sender incarnation. Included here despite needing no
   attacker: it is a live availability bug that any deploy or crash triggers.

**Tier 1b — done (2026-07-30).** The mechanical fixes whose reach requires an
already-compromised peer key or a hostile external service. Each is described
under **Fix applied** in its finding.

1. ~~[R5](#r5) — Slurm's missing managed-object guards.~~ **Fixed** on
   `set_limit` and both `cancel_pending_*_jobs`.
2. ~~[R6](#r6) — the board version loop.~~ **Fixed**, plus an
   implausible-version rejection at 2^60.
3. ~~[R13](#r13) — `op-localaccount`'s add path and `update_homedir`.~~
   **Fixed**, including validation of the peer-supplied home directory.
4. ~~[R14](#r14) — the mapping-target deny-list.~~ **Fixed** (allow-list), plus
   percent-encoding of names in Slurm REST paths.
5. ~~[R15](#r15) — the unauthenticated relayed zone.~~ **Fixed** by using the
   configured `peer.zone`. (The "is zone a trust label?" question stays open in
   Tier 2, but it does not change the fix.)
6. ~~[R17](#r17) — `owning_portal`'s ten missing arms.~~ **Fixed**, with an
   exhaustiveness test so a new variant cannot silently miss the check.
7. ~~[R18](#r18) — `PortalIdentifier`'s missing allow-list.~~ **Fixed** by
   lifting the validator into `templemeads::validate`, now shared with
   `greatwestern`.
8. ~~[R25](#r25) — unbounded date parsing and arithmetic.~~ **Fixed**: year
   range, span cap, checked arithmetic, and terminating `months`/`years` loops.
9. [R27](#r27) — bounds-checking `version_numbers`/`clusters[0]` was done as
   part of R1's sweep, so the panic is gone. What remains is a judgement call,
   not a fix: whether an unexpected Slurm version string should end the probe
   loop (current behaviour) or be a hard login error.
10. ~~[R7](#r7) — disputed, not scheduled.~~ **Closed as not-a-bug
    (2026-08-03).** The human gate is a single review of *award creation* (plus an
    increase above a threshold, detected by the web portal); all subsequent member
    changes are automatic by design. The finding measured the code against a
    requirement that does not exist. Its two sub-observations are recorded in
    [§4.3](#43-scope-note--the-cloud-prototype-agents) as code-quality items for
    when these prototype agents are next reshaped.

**Tier 2 — needs a design decision before implementation.** These are not
independent bugs so much as one missing concept: *the framework has no notion
of which peer is entitled to ask for what*. They should be decided together.

- ~~[R3](#r3) — where does a peer's role come from, if not from the peer?~~
  **Resolved and implemented**: an optional `type = "..."` on each peer's config
  entry, unset meaning unchecked.
- ~~[R4](#r4) — is the agent hierarchy a trust boundary in both directions?~~
  **Resolved and implemented**: yes, `position()` now requires sender adjacency.
  ([R12](#r12), which was cited here as the remaining case, was **falsified** on
  re-check - a peer never could write to a third peer's board.)
- The remaining half of the original question - whether each agent should also
  have an explicit "which senders may ask me this" table like `portal_runner`'s -
  is **not** done, and is what [§4.1](#41-accepted-trade-off--portal-authority-is-positional-not-cryptographic)
  records as accepted residual rather than a planned control.
- ~~[R2](#r2) / [R19](#r19) / [R20](#r20) — which principal may restart an agent,
  read its diagnostics, or answer a health query?~~ **Resolved (2026-08-03), and
  the premise was wrong.** They do not "accept any peer" in any way a peer can
  exercise: no unmodified agent binary can originate a `Restart`,
  `HealthCheck` or `DiagnosticsRequest`, and no `Instruction` maps to one, so the
  precondition is host-level file read plus an attacker-authored client. The
  answer to "which principal" is therefore *the operator, via the bridge* — and
  whole-deployment visibility from the portal downwards, ignoring zone, is a
  required capability rather than a gap, because agents live in private networks
  operators cannot otherwise reach. The Portal↔Portal boundary is the one that
  matters and it already holds. All three are re-rated, the two ordering bugs in
  R2 and both real bugs in R20 are fixed, and the residual is recorded in
  [§4.1](#41-accepted-trade-off--portal-authority-is-positional-not-cryptographic).
- ~~[R10](#r10) — how should handshake freshness survive a restart?~~
  **Resolved and implemented** - a random per-process epoch with one replay
  window per incarnation. See Tier 1a.
- [R15](#r15) — is zone a routing label or a trust label? The answer decides
  whether R15 is Medium or High.

**Tier 3 — documentation corrections. Done (2026-08-03).**

- [security-review.md](security-review.md) now carries a banner saying it is
  superseded in places, plus an inline **Round 2 correction** note at each
  falsified claim: §3.1, §3.3, §3.6, §3.7 and findings F3, F5, F8, F9, F11, F12
  ([R32](#r32)), F13 and F15. Each names the round-2 finding that supersedes it
  and whether it is fixed.
- `replay-protection-design.md` §10.6 was already struck through and superseded by
  its §11 (per-incarnation windows) as part of [R10](#r10)'s fix.
- [bridge-api.md](bridge-api.md) drift, all four verified against the code before
  correcting:
  - **§2.6** described taking the *first* `X-Forwarded-For` value, which is the
    exact bypass [R11](#r11) fixed. It now documents the real algorithm:
    trusted-peer gate, right-to-left walk, stop on an unparseable entry, and
    `X-Real-IP` only as a fallback.
  - **§2.3** branched its canonical-string description on "GET" vs "POST", but the
    code branches on `body.is_empty()`. A POST with an empty body signs with the
    four-field form. Corrected, and [R29](#r29)'s ambiguity is now recorded there
    as a known weakness rather than left implicit.
  - **§3** still showed error bodies echoing internal detail
    (`{"message": "Something went wrong: <error detail>"}`), which F15 removed. Now
    documents the fixed generic strings, with a status-to-message table, plus the
    503 that was missing.
  - **§5 step 4** showed `POST /fetch_job {"job": "<uuid>"}`; the endpoint takes a
    bare JSON UUID string, as §4 already said correctly.

**Tier 4 — done (2026-08-04).** The hardening cluster ([R33](#r33), 35 items) and the
process items in [§5](#5-process-and-tooling-observations) are complete. Seven of
R33's items were resolved by a deliberate decision *not* to make the suggested change;
see [security-review-2-fixes.md §9](security-review-2-fixes.md).

---

## 7. Method and verification standard

Each of the seven areas was audited independently against the same brief: read
the actual source, trace every claim from an untrusted entry point to its sink,
and cite `file:line`. Findings were required to state a concrete attacker path
grounded in round 1's threat model (§1); anything without one is labelled
hardening rather than a vulnerability.

Findings were then re-checked before inclusion here, and the table in
[§1](#1-executive-summary) records the standard each one met:

- **proven** — confirmed by executing code. [R1](#r1) and [R23](#r23) were
  reproduced against the real crates; [R8](#r8) was reproduced through
  paddington's own public API against the locked `iptools` version. The
  anti-replay window in [§3](#3-what-round-1-got-right) was differential-tested
  against a reference model.
- **source** — the cited code was read directly and the mechanism confirmed,
  without executing an exploit.
- **reported** — cited with `file:line` by the area audit and consistent with
  the surrounding code, but not independently re-confirmed line-by-line. These
  should be treated as likely-but-unverified, and re-checked when they are
  fixed.

**That last caveat earned its keep (2026-08-03).** Re-checking the two *reported*
findings [R19](#r19) and [R20](#r20) before fixing them found that both overstated
their case, and that R20's recommended fix - "only cache a response whose name
equals the authenticated sender" - is architecturally impossible, since responses
are relayed back hop by hop and legitimately describe an agent several hops away.
Two of R20's three specific claims were wrong: health already stamps a local
timestamp on receipt, and the "deep clone on every check" happens once per
collection rather than per poll. The remaining two were real and are fixed. Any
*reported* finding should be re-verified the same way before code is changed on
its account. A related correction applies to the *source*-rated [R2](#r2), whose
mechanism was right but whose attacker path assumed a capability - originating a
`Restart` from an ordinary peer - that no unmodified binary has; see
[§4.2](#42-accepted-trade-off--the-operator-control-plane-spans-the-whole-deployment).

Two findings were reached independently by two separate audits — [R10](#r10)
(by the cryptography and the connection audits) and [R9](#r9) (by the
config-at-rest and the bridge audits) — as was [R15](#r15). That convergence is
part of why they are rated as highly as they are.

**Live validation of the fixes (2026-07-30).** The eight fixes above were
subsequently exercised against real running agents, both directly connected and
via a real `op-proxy`, including repeated disconnection and reconnection - which
is the path [R10](#r10) broke and the one no offline test can reach. Behaviour
was robust and reconnection stayed transparent to the user. That closes the
verification action carried over from round 1 §6 for the direct and proxied
paths.

**Also validated live (2026-08-03).** Two later changes that alter a wire contract
were exercised against real running processes rather than only unit-tested, since
both make a backwards-compatibility claim:

- The [R3](#r3) follow-up (agent type carried in the invite): confirmed that with no
  `type` declared nothing is checked, that setting it activates enforcement, and that
  an invite with the `type` field stripped - i.e. one written by an older version -
  still imports and leaves the peer unchecked.
- [R29](#r29)'s signature versioning: all six negotiation cases confirmed against a
  running `op-bridge`, on both a GET and a POST path, using the project's own signer.
  A V1-signed request with no version header still returns 200, which is the whole
  compatibility claim.

**Still not verified live:** interoperation with a *not-yet-upgraded* peer
binary - the `epoch: None` / `nonce: None` fallback in [R10](#r10) and the
legacy XOR salt-format fallback from round 1's F15. Both are unit-tested at the
serialisation level (a field-less message deserialises to `None` and takes the
old path) but neither has been run against an actual older build, so the
compatibility claim rests on those tests rather than on observation. Round 1 §6
asked for exactly this and it remains open.

**Otherwise not covered by this round:** the Python bindings beyond their
authentication and key-handling paths; the TypeScript bindings; the `docs/`
example agents; and fuzzing of any parser. A fuzz harness over
`Instruction::parse`, `Command::parse` and `Job`'s `Deserialize` would be the
highest-value addition, since that is where [R1](#r1) lived and where
[R25](#r25) still does.

---

## 8. Relationship to other documents

- [security-review-2-fixes.md](security-review-2-fixes.md) — the companion record of
  **what was changed and why**, grouped by subsystem, including the seven
  recommendations deliberately not followed and the five places this review was itself
  wrong. Read that for rationale; read this for what was found.

- [security-review.md](security-review.md) — round 1. Its threat model (§1) and
  strengths (§3) are reused here; the claims this round falsifies are listed in
  [§2](#2-round-1-claims-this-round-falsifies).
- [security-model.md](security-model.md) — the intended model. §4.1 (IP
  allow-list) is affected by [R8](#r8); §7 (blast radius) by [R2](#r2)–[R4](#r4);
  §3.5's relay claims by [R15](#r15), [R16](#r16) and [R28](#r28).
- [wire-protocol.md](wire-protocol.md) — the frame and handshake formats behind
  [R10](#r10), [R15](#r15) and [R22](#r22).
- [portal-route-discovery-design.md](../plans/portal-route-discovery-design.md)
  — the implemented scheme for deriving and enforcing each portal's expected
  route, which closes the config-compromise half of the residual in
  [§4.1](#41-accepted-trade-off--portal-authority-is-positional-not-cryptographic).
- [replay-protection-design.md](../plans/replay-protection-design.md) — §10.6
  needs correcting per [R10](#r10).
- [bridge-api.md](bridge-api.md) — the HMAC/nonce/rate-limit model assessed in
  [R11](#r11), [R24](#r24), [R29](#r29) and [R32](#r32); several sections have
  drifted from the code (Tier 3 above).
- [agent-configuration.md](agent-configuration.md) — `trusted_proxy` guidance
  affected by [R8](#r8) and [R11](#r11); a peer role field would be added here
  for [R3](#r3).
- [blind-relay-proxy-design.md](../plans/archive/blind-relay-proxy-design.md) —
  the relay trust model reassessed in [R15](#r15), [R16](#r16), [R28](#r28).
- [op-cloudportal-design.md](../plans/archive/op-cloudportal-design.md) — §7's
  approval workflow is the subject of [R7](#r7).
