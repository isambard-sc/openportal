<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# OpenPortal Security Review

This is an independent, code-level security assessment of OpenPortal. Where
[security-model.md](security-model.md) *describes* the intended security model,
this document *evaluates* it: how strong the design and its implementation
actually are, what an attacker can and cannot do, and where the genuine gaps
are.

## Executive summary

**OpenPortal has a strong, deliberately minimal security architecture, and this
review leaves no unresolved findings.** The foundations are conservative and
orthodox: memory-safe Rust with `unsafe` code forbidden and `unwrap`/`expect`
denied at the lint level; standard, vetted cryptographic primitives (via
`orion`: XChaCha20-Poly1305 AEAD, HKDF-SHA512, BLAKE2b MAC, Argon2) used with no
nonce reuse; a textbook IPsec/WireGuard-style anti-replay window; and a
least-privilege, *no-god-key* trust model in which every peer link holds its own
independent keys so that compromise of one link cannot cascade to others.

This review examined the full attack surface — the cryptographic core,
connection and handshake authentication, the blind-relay proxy, config-at-rest,
the bridge HTTP boundary, and the privileged agents' command/path/identifier
handling — and identified 15 findings. **All 15 are now resolved.** The
substantive ones (an arbitrary-file-write via a crafted identifier, a reachable
pre-authentication panic, spoofable IP-trust decisions, weak at-rest key
derivation, and pre-authentication resource exhaustion) were fixed in code and
covered by tests. The remainder are either lower-severity hardening — also
fixed — or explicit, reasoned engineering decisions rather than defects.

Three properties are deliberate trade-offs, documented as such so they are not
mistaken for oversights: (1) **no in-protocol TLS** — the wire is independently
confidential and authenticated (double-envelope AEAD; HMAC on the bridge), and
operators add TLS externally where they want the residual metadata protected;
(2) **no forward secrecy** — keys are never negotiated in-band by design, and the
long-term keys only ever encrypt fresh high-entropy session keys, so security
rests on out-of-band key secrecy plus first-class key rotation; and (3) **all
trust derives from out-of-band pre-shared keys**, with no PKI. Each is
appropriate for the intended operator-controlled agent network and is defensible
against the threat model in §1.

**Bottom line.** The cryptography is standard and correctly applied, the trust
boundaries are tight and enforced before any expensive work, the privileged
agents never invoke a shell and validate their inputs, and every issue this audit
surfaced has been closed. The one outstanding item is operational, not a defect:
the handshake-path changes were verified offline (unit tests, clean build,
clippy) and should be confirmed with a live end-to-end run before fleet rollout.

---

It is written for security professionals who need to understand OpenPortal's
security posture quickly, decide whether it is fit for a given deployment, and
see exactly where residual risk lives. Findings are grounded in the source, not
the specifications, and cite `file:line` so they can be verified directly.

- **Review date:** 2026-07-24
- **Version reviewed:** 0.32.2 (branch `feature_greatwestern`)
- **Scope:** `paddington` (transport/crypto), `templemeads` (agent framework,
  bridge HTTP server), `greatwestern` (command vocabulary), and the agent
  executables (`portal`, `provider`, `bridge`, `freeipa`, `slurm`,
  `filesystem`, `localaccount`, `cloudaccount`, `cloudportal`, `proxy`).
- **Method:** manual code audit of the cryptographic core, connection
  establishment and authentication, the blind-relay proxy trust model, config
  provisioning and at-rest encryption, the bridge HTTP boundary, and the
  privileged agents' command/path/identifier handling. Primitive behaviour was
  verified against the `orion` 0.17.x source.

> **Status note.** This document reflects the codebase *after* remediation.
> Every finding this review identified has been addressed, and **none remain
> open**. Each is one of: **[Fixed]** — corrected in code (most findings);
> **[Resolved]** — a cluster addressed by a mix of code fixes and
> deliberate, documented decisions (F15); or **[By design]** — a reasoned
> engineering trade-off, not a defect (F8, F12, F14). The status shown for each
> finding describes the current code, with a note on what the original issue
> was. All fixes were verified by the crate's unit-test suite (148 tests
> passing), a clean release build, and `clippy`; the connection/handshake
> changes have been validated offline but not yet by a live end-to-end run —
> the single remaining action, tracked as an operational note in §6. §6 also
> lists the standing operational guidance (key rotation, TLS, encryption
> scheme, salt-format rollout order).

---

## 1. Threat model

The security of an OpenPortal network rests on **pre-shared symmetric key pairs
provisioned out-of-band** (see [security-model.md](security-model.md) §3).
There is no PKI, no transport TLS at the protocol layer, and no central
authority. The adversaries this review considers are:

| Adversary | Capability assumed | Primary concern |
|-----------|--------------------|-----------------|
| **On-path network attacker** | Can observe, drop, reorder, and inject arbitrary bytes on the wire between two agents (or between an agent and a proxy). Does **not** hold any pre-shared key. | Confidentiality/integrity of traffic; replay; downgrade; MITM. |
| **Compromised peer key** | Holds one agent's pre-shared key pair for one relationship (e.g. an attacker who stole one link's keys, or a malicious neighbour). | Blast radius: what can be done to the peer on the other end of that one link. |
| **Malicious / compromised proxy** | Runs an `op-proxy` that two agents relay through. Authenticated to each on its own hop. | Whether it can read, forge, or disrupt the relayed pair's traffic. |
| **Bridge client** | Can reach the `op-bridge` HTTP port and holds (or is guessing) the HMAC key. | Auth bypass, replay, injection, DoS at the portal→OpenPortal boundary. |
| **Local user on an agent host** | An unprivileged local account on a machine running an agent. | Access to key material on disk; privilege escalation via the agent. |

**Out of scope / assumed:** the security of the out-of-band invite transfer
(operator responsibility); the correctness of `orion` and other vetted crypto
dependencies; physical/host security; and the trustworthiness of the portal
software that drives the bridge.

---

## 2. Assessment summary and findings

This expands on the executive summary above with the detailed reasoning and the
full findings table.

**OpenPortal's core design is sound and, in several respects, notably strong.**
The "no god key" trust model is real and enforced in code: every peer
relationship has an independent symmetric key pair, an agent can only talk to
its direct neighbours, and there is no master credential whose loss compromises
the fleet. The transport cryptography is built from well-chosen primitives
(XChaCha20-Poly1305 AEAD, HKDF-SHA512 per-message subkeys, a CSPRNG throughout)
with **no nonce-reuse risk**, and the recently added IPsec/WireGuard-style
anti-replay window is implemented correctly, including the awkward
overflow/shift/boundary cases. Connection authentication checks key possession
*before* peer identity, session keys are mutually contributed and cannot be
forced weak or downgraded by a MITM, and the blind-relay proxy genuinely never
holds the relayed pair's keys or sees their plaintext. On the privileged-agent
side, no shell is ever invoked, external services are called through structured
RPC/JSON rather than string interpolation, and Rust's memory-safety lints
(`unsafe` forbidden, `unwrap`/`expect` denied) hold up well.

**The genuine gaps originally concentrated in three areas:** (1) config-at-rest
encryption, where the password KDF ran at its weakest possible setting with a
fixed salt; (2) the `op-bridge` HTTP boundary, where rate limiting was
bypassable; and (3) trust placed in client-supplied HTTP headers
(`X-Forwarded-For`) for IP-based decisions. **These, along with a set of
injection/path/panic issues and the pre-auth resource-exhaustion vectors, were
fixed as part of this review** (F1–F7, F9, F10, F11). What remains are two items
that are correct by design — the bridge's optional nonce (F8, backward
compatibility — the official client always sends one), TLS being an external
concern (F12 — the wire protocol is confidential and authenticated over plain
HTTP on its own, and operators layer on HTTPS/`wss` with standard infrastructure
if they want the metadata protected too), and the deliberate absence of in-band
key negotiation (F14 — no Diffie-Hellman, hence no forward secrecy, a considered
trade-off with permanent-key secrecy and out-of-band rotation as the control) —
plus the lower-severity hardening cluster ([F15](#f15)).

### Findings at a glance

| ID | Severity | Status | Finding |
|----|----------|--------|---------|
| [F1](#f1) | High | **Fixed** | `op-cloudaccount`/`op-cloudportal` arbitrary absolute-path file write via crafted `ProjectIdentifier` |
| [F2](#f2) | High | **Fixed** | Config-at-rest KDF ran at Argon2 minimum cost with a hardcoded salt; `Simple` scheme uses a public "password" |
| [F3](#f3) | High | **Fixed** | Bridge rate limiter keyed on spoofable `X-Forwarded-For`; never used the real peer IP |
| [F4](#f4) | Medium | **Fixed** | Reachable panic in `deenvelope_message` from a crafted (non-char-boundary) frame |
| [F5](#f5) | Medium | **Fixed** | No charset allow-list on identifiers → argument injection into privileged tools |
| [F6](#f6) | Medium | **Fixed** | `proxy_header` / `X-Forwarded-For` trusted blindly → IP-allowlist spoofing |
| [F7](#f7) | Medium | **Fixed** | Proxy did not bind `envelope.from` to the authenticated sender |
| [F8](#f8) | Low | **By design** | Bridge nonce is optional for backward compatibility; the official client always sends one |
| [F9](#f9) | Low–Med | **Fixed** | Config/invite files (plaintext keys) written world-readable |
| [F10](#f10) | Medium | **Fixed** | Secret password / env-var value interpolated into error messages |
| [F11](#f11) | Medium | **Fixed** | No pre-auth connection/rate limiting; pre-auth state mutation (DoS) |
| [F12](#f12) | Info | **By design** | Transport TLS is left to an external layer; the wire protocol is confidential/authenticated over plain HTTP regardless |
| [F13](#f13) | Medium | **Fixed** | `op-localaccount` lacked the "managed" guard its FreeIPA/Slurm siblings have |
| [F14](#f14) | Info | **By design** | No forward secrecy: keys are deliberately never negotiated in-band (fresh session keys, key-transported under the PSK) |
| [F15](#f15) | Low | **Resolved** | Cluster of lower-severity hardening items — fixed in code, or left as-is/documented with rationale |

---

## 3. Security strengths

These are the properties a deployer can rely on. Each was verified in code.

### 3.1 No "god key"; bounded trust topology
Every peer relationship uses an independent `inner_key`/`outer_key` pair
(`paddington/src/config.rs`), and an agent only holds keys for its direct
neighbours ([security-model.md](security-model.md) §7). Compromise of one link's
keys does not expose any other link's, and there is no master credential.
This is the system's central security property and it holds.

### 3.2 Sound transport cryptography, no nonce reuse
Message content is sealed with **XChaCha20-Poly1305** (`orion aead::seal`,
`crypto.rs:259`), whose 192-bit nonce is drawn from the CSPRNG per call — so
there is no AEAD nonce-reuse risk. On top of that, each message derives a fresh
per-message subkey via **HKDF-SHA512** with a fresh random 32-byte `info`
(`connection.rs:328`, `crypto.rs:155`), so two messages never share a derived
key. `random_bytes`, `Salt::generate`, and `Key::generate` all draw from the
OS CSPRNG (`crypto.rs:61,81,144`). MAC/signature verification is constant-time
(`orion auth::authenticate_verify`, `crypto.rs:397`).

### 3.3 Correct anti-replay window
The IPsec/WireGuard-style sliding window (`paddington/src/anti_replay.rs`) is
implemented correctly: first-nonce init, forward-advance bitmap shift,
too-old rejection, and duplicate detection are all right, including the
shift-by-≥64 and `u64::MAX`-jump edge cases that are easy to get wrong. The
nonce lives *inside* the AEAD-authenticated ciphertext, so it cannot be forged
or stripped by a proxy/attacker. Handshake/bootstrap nonces correctly persist
across reconnects (keyed under the permanent PSK), while ongoing-traffic windows
correctly reset per session. See [replay-protection-design.md](../plans/replay-protection-design.md).

### 3.4 Authentication order: key possession before identity
On connection, the server selects a peer only if one of the IP-matched
`ClientConfig`s can AEAD-decrypt the opening `Handshake` (`connection.rs:1291`),
and *only then* checks the peer's claimed name and zone (`connection.rs:1445`,
`:1456`) against that config. Session keys are mutually contributed via CSPRNG
(`connection.rs:685`, `:1383`) and travel only under the permanent keys, so a
MITM without the PSK cannot force a weak session key or downgrade capability
flags (`supports_nonce`, `version`, and nonces all ride inside the AEAD).

### 3.5 The blind relay proxy is genuinely blind
`op-proxy` depends only on `paddington` and never deserialises past the
`RelayEnvelope` wrapper — it forwards the inner `ciphertext` untouched
(`relay.rs`, `proxy_handler`). Both session-key halves are transported inside
AEAD-sealed bootstrap messages under the relayed pair's *permanent* keys, which
the proxy never holds. `RelayPolicy` is genuinely default-deny (an empty policy
`permits()` nothing), and `SessionUnknown`/bootstrap messages are
nonce-protected against replay. A compromised proxy can deny service or observe
metadata, but cannot read or forge relayed content.

### 3.6 No shell; structured external calls; containment
No agent ever invokes a shell — every external process is spawned argv-style
(`Command::new(...).arg(...)`), eliminating classic shell-metacharacter
injection. FreeIPA uses structured JSON-RPC args/kwargs and Slurm uses JSON
request bodies, so identifiers cannot break out into other parameters. FreeIPA
and Slurm additionally refuse to act on objects not in the OpenPortal-managed
org/group, limiting what a rogue peer can do to pre-existing accounts.

### 3.7 Memory-safety and secret hygiene
`unsafe_code = "forbid"`, `unwrap_used`/`expect_used = "deny"` are enforced
crate-wide; no payload-reachable panic was found in the agents. Key material is
held in `secrecy::SecretBox`, zeroised on drop, and redacted from `Debug`
output; `Display` impls for configs/invites deliberately omit key fields.

---

## 4. Findings

Each finding gives severity, status, location, what the issue is, its impact,
and remediation. Fixed findings describe the current (hardened) code and note
what the vulnerability was.

<a name="f1"></a>
### F1 — Arbitrary absolute-path file write via crafted identifier · High · **Fixed**

**Location:** `cloudaccount/src/state.rs`, `cloudportal/src/state.rs`
(the `state_path`/`record_path` helpers).

**What it was:** both prototype agents persisted per-project state to
`state_dir.join(format!("{}.json", project))`. `Path::join` *discards the base*
when its argument is absolute, so a `ProjectIdentifier` whose project component
began with `/` (e.g. an identifier parsing to project `"/etc/cron.d/x"`) caused
the agent to write attacker-controlled JSON to an arbitrary absolute path, with
the agent's privileges. Reachable by any upstream peer able to send a job
carrying a crafted identifier.

**Fix:** two layers. (1) The identifier grammar now rejects `/` outright (see
[F5](#f5)), so such an identifier can no longer be constructed. (2) The write
paths no longer trust that invariant: `state_path`/`record_path` now return an
error unless the derived filename is a single, plain path component
(`Path::components()` yields exactly one `Component::Normal`).

<a name="f2"></a>
### F2 — Weak config-at-rest key derivation · High · **Fixed**

**Location:** `paddington/src/crypto.rs` (`from_password`,
`from_password_with_salt`); `paddington/src/config.rs`
(`ServiceConfig::encrypt`/`decrypt`, `EncryptionScheme`).

**Scope note:** this "encryption" protects individual **secret values stored in
the config's `extras`** (e.g. a FreeIPA bind password or Slurm token set via the
`secret` CLI command) — decrypted via `AgentConfig::secret`. The pre-shared
peer keys themselves live in the TOML as hex and are protected by file
permissions (see [F9](#f9)), not by this scheme.

**What it was:** the secret-encryption key was derived with
`orion::kdf::derive_key(password, salt, iterations=3, memory=8, length=32)`.
`orion`'s `memory` argument is in **KiB**, and both parameters were `orion`'s
*absolute minimum* (`MIN_ITERATIONS = 3`, `MIN_MEMORY = 8`) — 8 KiB entirely
defeats Argon2's memory-hardness. The salt was a **hardcoded 16-byte constant**,
so the derivation was fully deterministic: identical passwords produced
identical keys across every installation, and one precomputed dictionary worked
against all of them.

**Fix — versioned secret format.** New secrets are written in a versioned (v1)
format: `op-secret-v1:<hex salt>:<hex ciphertext>`, where the salt is a fresh
16-byte random value stored alongside the ciphertext and the key is derived with
`Key::from_password_with_salt` at a realistic Argon2 cost (**19 MiB / 3 passes**,
the OWASP floor — orion provides Argon2i). Because the derivation runs rarely
(startup and the `secret` command), the cost is never on a hot path. Decryption
detects the version prefix: v1 values use the salted strong derivation, while any
prefix-less (legacy v0) value is still decrypted with the old fixed-salt
derivation — so **existing config files keep working**, and re-running the
`secret` command re-encrypts a value in the strong format. Covered by
`test_secret_encrypt_roundtrip_is_versioned` and
`test_secret_decrypt_reads_legacy_v0`.

**`Simple` scheme — documented as obfuscation only.** The `Simple` scheme uses
the agent's own (non-secret) name as the password, so anyone who can read the
config can re-derive the key regardless of KDF strength. Its doc comment now
states plainly that it is **obfuscation, not encryption**, and is not for
production; `Environment` is the production scheme. The stronger v1 KDF applies
to both — it just cannot compensate for a public password under `Simple`.

<a name="f3"></a>
### F3 — Bridge rate limiter keyed on spoofable client IP · High · **Fixed**

**Location:** `templemeads/src/bridge_server.rs` (`resolve_client_ip_middleware`,
`forwarded_ip`, `extract_client_ip`, `Config::trusted_proxy`).

**What it was:** the bridge derived the client IP used for rate limiting from the
`X-Forwarded-For` then `X-Real-IP` request headers — both fully
attacker-controlled — and never read the actual TCP peer address (`ConnectInfo`
was not wired up). An attacker could rotate `X-Forwarded-For` per request to get
a fresh rate-limit bucket every time, defeating the limiter.

**Fix.** The server is now served with
`into_make_service_with_connect_info::<SocketAddr>()`, and a new
`resolve_client_ip_middleware` runs on every request. It takes the **real TCP
peer address** from `ConnectInfo` and treats it as authoritative *unless* that
peer matches the configured `trusted_proxy` allow-list, in which case (and only
then) it honours the forwarded client IP. It stamps the resolved address into an
internal header (stripping any client-supplied copy first), and `extract_client_ip`
now reads *only* that header — never `X-Forwarded-For`/`X-Real-IP` directly.

**Deployment fit (Cloudflare tunnel / in-cluster ingress).** `trusted_proxy`
accepts the same comma-separated IP/CIDR syntax as an agent's `ip`
(`paddington::config::IpOrRange`), so a Cloudflare tunnel daemon or ingress on an
internal/loopback address is expressed as e.g. `--trusted-proxy 127.0.0.0/8`.
With no `trusted_proxy` set, forwarded headers are ignored entirely and the real
peer IP is always used (fail-closed). Covered by
`test_extract_client_ip_ignores_forwarded_headers` and
`test_forwarded_ip_parses_first_xff_then_xri`.

**Note:** the raw request-rate limit (10,000 / 10 s) is intentionally generous —
the bridge fronts a single trusted portal — and is now applied against a real,
non-forgeable key. See [F11](#f11) for the related pre-auth-work ordering.

<a name="f4"></a>
### F4 — Reachable panic in `deenvelope_message` · Medium · **Fixed**

**Location:** `paddington/src/connection.rs` (`deenvelope_message`).

**What it was:** the de-envelope path sliced the incoming text frame at fixed
**byte** offsets (`&message[0..64]`, `&message[64..128]`, `&message[128..]`)
guarded only by a length check. Slicing a `&str` at an offset that is not a
UTF-8 char boundary **panics**; with `panic = "abort"` (workspace profile) that
aborts the process. An attacker could send a ≥130-byte text frame with a
multi-byte character straddling byte 64 or 128 to crash the receiver. This path
is reachable pre-authentication on direct connections (the peer-selection filter
calls it) *and* via a relayed peer/proxy (`relay.rs` decrypt paths), so it
undermined the "a compromised proxy can at worst deny service to the pair it
relays" boundary by letting any allowed peer crash a victim.

**Fix:** the slices now use `str::get(range)`, which returns `None` on a
non-char-boundary (turned into a clean de-envelope error) instead of panicking.
Legitimate frames are pure ASCII hex, so real traffic is unaffected.

<a name="f5"></a>
### F5 — No charset allow-list on identifiers → argument injection · Medium · **Fixed**

**Location:** `greatwestern/src/grammar.rs` (`ProjectIdentifier::parse`,
`UserIdentifier::parse`, `ProjectMapping::new`, `UserMapping::new`).

**What it was:** identifier parsing only split on `.` and rejected empty
components. Any other byte — leading `-`, `/`, `;`, `$`, spaces, control chars —
was accepted. These identifiers become Unix account/group names, filesystem
path components, Slurm account names, and RPC/URL parameters downstream. Because
they are passed to spawned tools as bare argv operands, a name beginning with
`-` could be interpreted as a **flag** by `useradd`/`sacctmgr`/etc. (argument
injection), and a `/` enabled the path escape in [F1](#f1). This was the
root-cause enabler for several downstream issues.

**Fix:** identifier components are now validated against a strict allow-list —
`[A-Za-z0-9_-]`, no leading `-`, length-capped (`validate_identifier_component`).
Mapping targets (local user/group names), which may legitimately contain `.`,
get a targeted deny-list instead: no `/`, no leading `-`, no control characters.
New unit tests (`test_identifier_validation_rejects_dangerous_characters`,
`test_mapping_validation_rejects_dangerous_local_names`) lock this in. This also
closes the argument-injection vector at the source and neutralises the
URL-path-injection concern for Slurm REST calls (such characters can no longer
appear in an identifier).

> Note: because a shell is never used ([§3.6](#36-no-shell-structured-external-calls-containment)),
> this was argument (flag) injection, not arbitrary command execution — bounded,
> but real. Defence-in-depth remains available (an explicit `--`
> end-of-options separator before user-derived operands, e.g. at
> `slurm/src/sacctmgr.rs` `set_limit`); see [F15](#f15).

<a name="f6"></a>
### F6 — `proxy_header` / `X-Forwarded-For` trusted blindly · Medium · **Fixed**

**Location:** `paddington/src/connection.rs` (handshake IP resolution);
`paddington/src/config.rs` (`ServiceConfig::trusted_proxy`/`set_trusted_proxy`).

**What it was:** when `proxy_header` was configured, the header value from the
client's own request unconditionally overwrote the connecting IP used for the
allow-list check ([security-model.md](security-model.md) §4.1) — with **no check
that the TCP peer was actually the trusted proxy**. Any attacker reaching the
port directly could set the header to an allow-listed address and defeat Layer 1
(classic `X-Forwarded-For` spoofing). Layers 2–4 still held, so it was not full
compromise, but it negated the IP allow-list and its audit value.

**Fix.** A new `trusted_proxy` allow-list (`Option<IpOrRange>`) on the service
config gates the override: a `proxy_header` value is honoured only when the real
TCP peer (`stream.peer_addr()`) matches `trusted_proxy`; otherwise the header is
ignored, the real peer address is used, and a warning is logged. It **fails
closed** — if `proxy_header` is set but no `trusted_proxy` is configured,
forwarded addresses are never trusted.

**Deployment fit.** Set it at init with `--trusted-proxy` (comma-separated
IP/CIDR, e.g. `--trusted-proxy 127.0.0.0/8` for a Cloudflare tunnel or
in-cluster proxy on loopback), or add `trusted_proxy = "…"` to an existing
config's `[service]` section. The bridge's `--trusted-proxy` applies to both this
paddington layer and the bridge HTTP layer ([F3](#f3)).

<a name="f7"></a>
### F7 — Proxy did not bind `envelope.from` to the authenticated sender · Medium · **Fixed**

**Location:** `paddington/src/relay.rs` (`proxy_handler`).

**What it was:** the proxy evaluated `RelayPolicy` on the `from`/`to` labels
*inside* the relayed envelope, but never checked that `envelope.from` matched
the peer identity the connection had actually authenticated as. Any legitimate
client of the proxy could therefore assert a spoofed `from` to satisfy a policy
pair it was not part of. (It could not produce ciphertext the victim would
decrypt, so confidentiality held — but combined with [F4](#f4) it let any
proxy-connected agent route a crash to any victim it had an allowed pairing
with.)

**Fix:** `proxy_handler` now drops any envelope whose `from` does not equal
`message.sender()` — the identity set by the framework from the authenticated
handshake (`connection.rs:1090`), not a wire field. Policy enforcement no longer
rests on attacker-supplied labels.

<a name="f8"></a>
### F8 — Bridge nonce is optional (backward compatibility) · Low · **By design**

**Location:** `templemeads/src/bridge_server.rs` (`verify_headers`);
`python/src/lib.rs` (client).

The server treats the `X-Nonce` header as optional: when present it is checked
against the replay store and included in the signed material; when absent, replay
protection falls back to the ±5 s `Date` window only. This is a **deliberate
backward-compatibility affordance** so that older clients that predate the nonce
mechanism continue to work.

**Verified:** the official Python client (the supported way portals talk to the
bridge) **always** opts in — every request generates a fresh UUID nonce and
sends it as `X-Nonce`, and the nonce is part of the signed call string
(`python/src/lib.rs:116-127` for GET, `:183-202` for POST). So all current
clients get full nonce-based replay protection; the optional path exists only for
legacy compatibility, exactly like the negotiated nonce rollout on the paddington
side ([replay-protection-design.md](../plans/replay-protection-design.md) §5).

**If stricter behaviour is later wanted:** once no legacy clients remain, a
config switch could make a nonce mandatory on state-changing endpoints. Not done
now, to avoid breaking older clients — the same trade-off F8's paddington
counterpart already makes.

<a name="f9"></a>
### F9 — Config/invite files written world-readable · Low–Medium · **Fixed**

**Location:** `paddington/src/config.rs` (`save`), `paddington/src/invite.rs`
(`save` ×2).

**What it was:** the service config and invite files — which contain plaintext
`inner_key`/`outer_key` material (and, under the `Simple` scheme, are
effectively unencrypted, see [F2](#f2)) — were written with `std::fs::write`, so
they landed at the process umask (commonly `0644`, group/world-readable). Any
local user could read long-term keys.

**Fix:** both now go through a shared `write_secret_file` helper that restricts
the file to owner-only `0600` on Unix after writing.

<a name="f10"></a>
### F10 — Secrets interpolated into error messages · Medium · **Fixed**

**Location:** `paddington/src/crypto.rs` (`from_password`),
`paddington/src/config.rs` (`get_key`).

**What it was:** `Key::from_password` interpolated the raw password into an error
context string, and `get_key` interpolated the *value* of the secret environment
variable (not its name) into a "could not parse key" error. On the error path,
these secrets would land in logs/stderr.

**Fix:** the password is no longer included in `from_password`'s error context,
and `get_key` now interpolates only the environment variable's *name*, never its
value.

<a name="f11"></a>
### F11 — No pre-auth connection/rate limiting; pre-auth state mutation · Medium · **Fixed**

**Location:** `paddington/src/server.rs` (accept loop),
`paddington/src/config.rs` (`may_attempt_connection`),
`paddington/src/connection.rs` (permit release);
`templemeads/src/bridge_server.rs` (`verify_headers`).

**What it was.** Two related availability issues: (a) paddington `tokio::spawn`ed
every accepted TCP connection with no concurrency cap and performed the full
WebSocket upgrade + handshake crypto *before* the IP allow-list applied, so an
attacker could open unlimited connections to exhaust tasks/sockets/memory
pre-auth; and (b) the bridge wrote (and ran an O(n) cleanup over) the nonce store
*before* signature verification, so an unauthenticated caller could grow it.

**Fix — paddington, two layers:**

1. **Fail-fast source check.** The accept loop now calls
   `ServiceConfig::may_attempt_connection(peer_ip)` the instant a TCP connection
   is accepted and **drops it before any WebSocket-upgrade or cryptographic
   work** unless the source matches a configured client IP *or* the
   `trusted_proxy` range (the union — so a proxied deployment, where the TCP peer
   is the proxy, still passes; the real client IP is validated later from the
   forwarded header). A flood from unexpected addresses now costs only an
   `accept()` + an IP-range check. Covered by `test_may_attempt_connection`.
2. **Bounded unauthenticated pool.** A process-wide semaphore
   (`MAX_UNAUTHENTICATED_CONNECTIONS = 2048`) is acquired with `try_acquire_owned`
   (never blocking the accept loop) before spawning each connection; if the pool
   is exhausted the new connection is dropped with a warning. The permit is
   **released the moment the peer authenticates** (after the key/name/zone/version
   checks, in `Connection::handle_connection`), so long-lived authenticated peers
   never occupy the pool — only in-progress handshakes do. 2048 is far above any
   real deployment (a few dozen agents) yet bounds a flood; it is a named
   constant if it ever needs tuning.

**Fix — bridge.** `verify_headers` now performs HMAC signature verification (and
the `Date` window check) **before** any nonce-store access, so the replay store
is only ever read or grown by an already-authenticated caller. The nonce is part
of the signed material, so this loses no replay protection. The store also gains
a hard size cap (`MAX_NONCE_ENTRIES = 100_000`, a defence-in-depth backstop that
should never be reached given the auth-gating and TTL). The rate-limit check
stays first (cheap CPU-flood protection) and, since [F3](#f3), is keyed on the
real peer IP, so its map is bounded by distinct real sources rather than by
spoofable header values.

**Residual (accepted):** an attacker who can both originate from an allow-listed
source (or via the trusted proxy) *and* hold 2048 simultaneous half-open
handshakes could still deny *new* connections until slots free — but existing
authenticated peers are unaffected, and this requires a genuine allow-listed
origin. A per-source connection rate limit could further harden this if ever
needed; it is lower priority now that both the fail-fast check and the pool cap
are in place.

<a name="f12"></a>
### F12 — Transport TLS is an external concern (by design) · Info · **By design**

**Location:** `templemeads/src/bridge_server.rs` (bare `TcpListener`);
`paddington/src/server.rs` (raw `ws`).

OpenPortal does not terminate TLS itself: the bridge serves HTTP and paddington
speaks `ws`. **This is an explicit design decision, not an omission** — the
protocols are built to be confidential and authenticated on their own, and
whether to add an outer TLS layer is left to the operator's existing
infrastructure.

**The wire protocol is secure over plain HTTP/`ws`.** Paddington message content
is sealed with the double-envelope XChaCha20-Poly1305 AEAD under per-peer
pre-shared keys ([§3.2](#32-sound-transport-cryptography-no-nonce-reuse)), and
the bridge HTTP API is HMAC-authenticated over method, path, body, date, and
nonce ([bridge-api.md](bridge-api.md)). An on-path attacker without the keys
therefore cannot read message content, forge or tamper with a message, or open a
connection, with or without TLS. What remains observable without an outer TLS
layer is *metadata* — handshake salts, IP addresses, message sizes, and timing —
but there is no practical mechanism to turn any of that into a forged or hijacked
connection (the salts are useless without the pre-shared keys; a captured message
cannot be replayed, see [§3.3](#33-correct-anti-replay-window)). So this is
metadata exposure only, not a content-confidentiality or authentication weakness.

**Adding TLS is deliberately trivial and external.** An operator who wants the
metadata protected too — or who simply wants HTTPS/`wss` end-to-end — layers it
on with standard infrastructure: an nginx/Caddy reverse proxy, an in-cluster
ingress, or a **Cloudflare tunnel** in front of the bridge and/or the paddington
listener. When a terminating proxy is used, set `trusted_proxy`
([F3](#f3)/[F6](#f6)) to its address so forwarded client IPs are believed only
from it. The reference deployment does exactly this: `op-bridge` runs in the same
cluster as the portal it serves (so that hop stays on the cluster network), with
a Cloudflare tunnel providing HTTPS for anything reached from outside.

**Not a bug to fix, an option to document.** Native TLS (rustls) for the bridge
and a `wss` option for paddington could be added later as a convenience, but they
would only duplicate what an external terminator already provides and are not
required for a secure deployment. See §5 and
[agent-configuration.md](agent-configuration.md).

<a name="f13"></a>
### F13 — `op-localaccount` lacks a managed-object guard · Medium · **Fixed**

**Location:** `localaccount/src/localaccount.rs` (`remove_user`,
`remove_project`, `is_protected_user`, `is_protected_project`).

**What it was:** unlike the FreeIPA and Slurm agents — which refuse to act on
objects outside the OpenPortal-managed org/group
([§3.6](#36-no-shell-structured-external-calls-containment)) —
`op-localaccount` ran `userdel`/`groupdel` unconditionally. A compromised
upstream peer could delete any local account/group whose name matched the
derived naming. Notably, because a *system-portal* project identifier maps to a
**bare** group name (`identifier_to_projectid` drops the portal prefix for
`openportal`/`system`/`instance`), an identifier like `docker.system` mapped to
the group `docker` — so `remove_project` could `groupdel` a real system group.

**Context:** `op-localaccount` is a **testing agent** (it manages accounts in a
containerised test Slurm cluster; `op-freeipa` is the production path). This fix
hardens it anyway, so a mistaken production deployment fails safe.

**Fix:**
- `remove_user` now applies the existing `is_protected_user` guard (the same one
  `block_user`/`unblock_user` use): a user is only deleted if they are a member
  of the managed group. Pre-existing system accounts are refused (warn + no-op).
- `remove_project` now applies a new `is_protected_project` guard: a group is
  only deleted if it has a **normal (non-system) GID** (`≥ MANAGED_GID_MIN`,
  1000) and is not the managed group, the blocked group, or a configured system
  group. The GID check robustly protects `wheel`/`sudo`/`docker`/etc. by class
  rather than by a name denylist, closing the bare-name collision above. An
  unparseable GID fails safe (refuse).
- The agent now logs a prominent **testing-only** warning on every startup, and
  the module/`main` docs and [agent-configuration.md](agent-configuration.md)
  §3.6.1 state the same.

**Residual:** removal remains idempotent for genuinely managed objects, and the
guards fail safe (refuse) on ambiguity. The GID threshold assumes the usual
Linux `GID_MIN` of 1000; a container with an unusual `GID_MIN` would want it
adjusted (it is a named constant).

<a name="f14"></a>
### F14 — No forward secrecy: keys are never negotiated in-band (by design) · Info · **By design**

**Location:** `paddington/src/connection.rs` (direct `Handshake`),
`paddington/src/relay.rs` (relayed bootstrap).

Each connection/bootstrap uses a **freshly-generated random session key pair**,
but those session keys are **key-transported (encrypted) under the long-term
pre-shared keys**, not negotiated via an ephemeral Diffie-Hellman exchange.
OpenPortal therefore has **no forward secrecy**, and this document states that
plainly rather than implying otherwise.

**This is a deliberate design decision.** OpenPortal intentionally provides *no
in-band mechanism by which agents can share or change key material themselves* —
not even a Diffie-Hellman exchange. All key material is provisioned out-of-band
([security-model.md](security-model.md) §3), and the only thing ever sent on the
wire is a session key already sealed under the permanent pre-shared keys. Adding
DH to gain forward secrecy would reintroduce exactly the in-band key-agreement
route the design excludes, so it is a considered trade-off, not an oversight.

**Why the residual risk is narrow in practice:**

- To decrypt any past traffic, an attacker must **both** have logged the traffic
  in full **and** recover the *permanent* pre-shared keys — there is no way to
  derive a session key from captured bytes alone (each is fresh random, never
  transmitted in the clear).
- The permanent keys are hard to attack from the wire: they are **only ever used
  to encrypt the initial, randomly-generated, high-entropy session keys**. There
  is no low-entropy or known plaintext sealed under a permanent key to serve as a
  crib — an attacker sees only high-entropy plaintext encrypted under a
  high-entropy key, which gives no leverage for reverse-guessing the permanent
  key. So "recover the permanent keys" means obtaining them out-of-band (a
  compromised config/invite file), not cracking them from traffic.
- Because the model rests on the **secrecy of the permanent keys**, OpenPortal
  provides a first-class **`rotate`** path (see
  [security-model.md](security-model.md) §3.3) to make periodic, out-of-band key
  rotation straightforward — which also bounds the window any single key covers.

**Not a remediation item.** Forward secrecy via ephemeral DH is deliberately not
offered; the appropriate operational control is permanent-key secrecy plus
rotation.

<a name="f15"></a>
### F15 — Lower-severity hardening cluster · Low · **Resolved**

Individually minor; listed for completeness with location and status. All were
addressed in this pass — most fixed in code; two left as-is with rationale (the
timing distinction and the healthcheck worker count are not worth changing); and
one (key/MAC) resolved by documenting the invariant rather than making a breaking
change with no benefit.

| Item | Location | Note |
|------|----------|------|
| Bridge hand-rolled constant-time compare | `bridge_server.rs` (`verify_headers`) | Replaced with `paddington::constant_time_eq` (orion `secure_cmp`). **Fixed.** |
| Bridge error bodies echo internal `Debug` detail | `bridge_server.rs` (`AppError::into_response`) | Now logs detail server-side and returns only a generic, status-appropriate message. **Fixed.** |
| Slurm logs the token-fetch command at `info` | `slurm/src/slurm.rs` | No longer logs the command (it may embed a credential). **Fixed.** |
| FreeIPA login body not URL-encoded | `freeipa/src/freeipa.rs` | Now uses `reqwest`'s `.form()`. **Fixed.** |
| `--` end-of-options separator before user-derived operands | `localaccount` | Added `--` before operands in the shadow-utils mutation commands. `sacctmgr`'s bare-name site is already safe (name is `[A-Za-z0-9_-]`, no leading `-`, via [F5](#f5)) and it does not use getopt-style `--`, so it is left as-is. **Fixed.** |
| Received session key not checked with `is_null()` | `connection.rs` | Both handshake paths now reject an all-zero session key from the peer. **Fixed.** |
| `clean_and_check_path` is a pre-canonicalisation deny-list | `filesystem/src/filesystem.rs` | Now rejects relative paths and any `..` component up front, in addition to the sensitive-location deny-list, and is backstopped by [F5](#f5). A full canonicalise-and-verify-within-configured-root allow-list remains possible future hardening. **Hardened.** |
| Auth-layer timing/behaviour oracle | `connection.rs` | Round-trip count reveals which layer (IP/key/name-zone) rejected. There is no practical way to use this (it leaks no key material and cannot advance an attack), so it is left as-is. **Accepted, no action.** |
| Salt XOR-masked with long-term key in cleartext headers | `connection.rs` | Salts are now sent in the clear (HKDF salts are public), removing the fragile coupling. A client advertises the plain format with an `openportal-salt-format: plain` header; a server without the header falls back to the legacy XOR un-masking, so an upgraded server keeps talking to old clients. The client commits to one encoding blindly (it initiates), so this needs a server-before-client rollout, not per-pair negotiation. **Fixed (negotiated, server-first).** |
| Unauthenticated healthcheck leaks worker count | `healthcheck.rs` | Minor internal-load disclosure on `config.ip()`. Left as-is by choice - it is a useful monitoring signal and only an approximate load hint. **Accepted, no action.** |
| No key/MAC domain separation | `crypto.rs` | No code path uses the same key for both AEAD and MAC (wire = AEAD only, bridge = MAC only, config = AEAD only), so there is no live cross-protocol reuse. Real domain separation would change AEAD/MAC outputs — breaking the wire, bridge↔client, and stored config secrets — for **zero** current benefit, so it is deliberately not done. Instead the invariant ("a `Key` is never used for both") is now documented on `Key::sign`, with instructions to derive purpose-specific sub-keys if a future caller ever needs both. **Resolved (documented invariant).** |

---

## 5. Residual risks & accepted trade-offs

A deployer should treat the following as inherent to the current design and plan
around them, independent of the open findings above:

1. **Security rests entirely on out-of-band key management.** There is no PKI or
   revocation infrastructure. A leaked invite file *is* a compromised link until
   the operator rotates it ([security-model.md](security-model.md) §3.3). Invite
   files must be transferred over a secure channel and destroyed after import.
2. **TLS is an external layer, by design.** See [F12](#f12): the wire protocol
   is confidential and authenticated over plain HTTP/`ws` on its own, so TLS is
   deliberately left to the operator's infrastructure. If you want the residual
   metadata (salts, IPs, sizes, timing) protected too, front the bridge and/or
   paddington listener with a TLS terminator (nginx, ingress, Cloudflare tunnel)
   and set `trusted_proxy` to it — it is trivial to add and not required for a
   secure deployment.
3. **Replay protection is negotiated, not universal.** A peer pair where either
   end has not been upgraded gets no ongoing-traffic replay protection for that
   pair — by design, for gradual rollout
   ([replay-protection-design.md](../plans/replay-protection-design.md) §5).
4. **No forward secrecy, by design.** See [F14](#f14): keys are never negotiated
   in-band (no Diffie-Hellman), so an attacker who logs all traffic *and*
   obtains the permanent keys out-of-band could decrypt past sessions. The
   permanent keys only ever encrypt fresh high-entropy session keys (no crib to
   attack them from the wire); the control is permanent-key secrecy plus the
   `rotate` path, not forward secrecy.
5. **`op-cloudaccount` and `op-cloudportal` are explicitly prototypes.** They
   hold state as plain JSON files and merge roles that would normally be
   separate agents (see their design docs under `docs/plans/archive/`). Expect
   them to be reshaped; do not treat them as hardened production agents.
6. **A compromised peer key is a real, if bounded, adversary.** Within one link,
   a rogue peer can drive the neighbouring agent to create/modify/delete
   OpenPortal-namespaced objects with any (now validated, [F5](#f5)) names it
   likes. The trust topology ([§3.1](#31-no-god-key-bounded-trust-topology))
   is what bounds this — it cannot reach beyond that one relationship.

---

## 6. Prioritised remediation

All findings from this review are now either fixed or accepted/documented as
deliberate design decisions — there are no outstanding code-remediation items.
The one remaining action is verification, not a fix:

- **Live end-to-end validation (recommended before fleet rollout).** The
  connection/handshake changes in this pass (the salt-format negotiation, the
  null-session-key check, and the F11 pre-auth fail-fast and semaphore) were
  verified offline — 148 unit tests, a clean build, and `clippy` — but not yet
  by a live run against real peers. Confirm with running `op-portal`/`op-provider`
  processes (directly connected and via a real `op-proxy`), including at least
  one not-yet-upgraded client, to exercise the legacy salt fallback in anger.

One operational rollout note applies to the salt-format change:

- **[F15] Salt-format rollout is server-first.** The handshake now sends HKDF
  salts in the clear, negotiated via the `openportal-salt-format` header. A
  client commits to its encoding before any negotiation is possible, so upgrade
  **listening/server sides before initiating/client sides**: an upgraded server
  accepts both old (XOR) and new (plain) clients, but an old server cannot read a
  new client's plain salts.

Standing operational notes (not code fixes):

- **[F14] Protect and rotate the permanent keys** — there is no forward secrecy
  by design (no in-band key negotiation), so permanent-key secrecy is the
  control. Keep config/invite files secret and use the `rotate` path to rotate
  keys out-of-band periodically.
- **[F12] Add TLS externally if you want it** — the wire protocol is secure over
  plain HTTP/`ws`; layering HTTPS/`wss` (nginx, ingress, Cloudflare tunnel) is an
  optional operator choice, not a prerequisite. Set `trusted_proxy` to the
  terminator when one is used.
- **[F2] Prefer `Environment` over `Simple`** encryption in production; re-run
  the `secret` command to upgrade any legacy v0 secrets to the strong v1 format.

Fixed as part of this review: [F1](#f1), [F2](#f2), [F3](#f3), [F4](#f4),
[F5](#f5), [F6](#f6), [F7](#f7), [F9](#f9), [F10](#f10), [F11](#f11),
[F13](#f13), and [F15](#f15) (fixed in code, or accepted/documented with
rationale). Correct by design (documented): [F8](#f8), [F12](#f12),
[F14](#f14).

---

## 7. Relationship to other documents

- [security-model.md](security-model.md) — the intended model this review
  evaluates (key structure, invite provisioning, four-layer auth, zones, blind
  relay trust model, replay protection, memory safety).
- [wire-protocol.md](wire-protocol.md) — the double-envelope frame, handshake,
  and relay/nonce wire formats referenced throughout §3–§4.
- [replay-protection-design.md](../plans/replay-protection-design.md) — the
  anti-replay design assessed in [§3.3](#33-correct-anti-replay-window) and the
  negotiated-rollout trade-off in [§5](#5-residual-risks--accepted-trade-offs).
- [agent-configuration.md](agent-configuration.md) — IP allow-list, CIDR list,
  `proxy_header`, and encryption-scheme configuration referenced in [F2](#f2),
  [F3](#f3), [F6](#f6).
- [bridge-api.md](bridge-api.md) — the HMAC/nonce/rate-limit model whose
  implementation is assessed in [F3](#f3), [F8](#f8), [F11](#f11).

## 8. Source file reference

| Area | Primary source |
|------|----------------|
| Symmetric crypto, KDF, AEAD, MAC | `paddington/src/crypto.rs` |
| Anti-replay window | `paddington/src/anti_replay.rs` |
| Connection auth, handshake, envelope | `paddington/src/connection.rs` |
| Blind relay, bootstrap, `RelayPolicy` | `paddington/src/relay.rs` |
| Config, encryption schemes, allow-list | `paddington/src/config.rs` |
| Invite provisioning | `paddington/src/invite.rs` |
| Bridge HTTP boundary | `templemeads/src/bridge_server.rs` |
| Identifier grammar & validation | `greatwestern/src/grammar.rs` |
| Privileged operations | `freeipa/`, `slurm/`, `filesystem/`, `localaccount/`, `cloudaccount/`, `cloudportal/` |
