<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# The OpenPortal Java client

A Java client for the OpenPortal bridge API — the same surface the
[`openportal` Python module](../python) wraps, for a portal that would rather
not embed a Rust extension.

```java
BridgeClient bridge = BridgeClient.load(Path.of("bridge.toml"));

String me = bridge.getPortal();                                   // "site"
bridge.syncOfferings(List.of(Destination.parse("cluster1." + me + ".allocator")));

for (Job job : bridge.fetchJobs()) {
    bridge.sendResult(job.errored(new ManagedProjectPendingError("awaiting approval")));
}
```

`java/examples/site_portal` is the worked example built on this — the Java
counterpart of [`python/examples/site_portal`](../python/examples/site_portal).
Start there if you are implementing a site portal; start here if you only need
the connection.

## What it is for

A site portal talks to a running `op-bridge` over localhost HTTP, and **every
request has to be signed**. That signing is the part with no room for
interpretation: get one byte of it wrong and the bridge answers `401
Unauthorized` with nothing to say why. `BridgeAuth` is that, written out and
tested against vectors, and it is the file to read first.

Everything else here is the shapes on the wire — a `Job`, an `AwardDetails`, a
`UsageReport` — and the endpoints that carry them. Every type the Python module
exposes has a counterpart here, and they agree by test rather than by
inspection: `TypesTest` pins each one against JSON and strings produced by the
published Python module, which is the Rust implementation through pyo3.

## Building

```bash
mvn test        # 65 unit tests, no bridge needed
mvn install     # to your local repository as org.openportal:openportal:0.92.0
```

Java 17 or later. Two dependencies, both ordinary:

| Dependency | Why |
|---|---|
| `org.bouncycastle:bcprov-jdk18on` | BLAKE2b, which the bridge signs with and the JDK does not have |
| `com.fasterxml.jackson.core:jackson-databind` | JSON, used as a tree model rather than by data binding |

## The four things needed to get the signing right

1. **The primitive is keyed BLAKE2b-256, not HMAC.** The bridge uses
   `orion::auth::authenticate`, which is BLAKE2b in its native keyed mode with a
   32-byte digest. `javax.crypto.Mac` cannot do it and neither can anything else
   in the JDK — hence BouncyCastle's `Blake2bDigest(key, 32, null, null)`.

2. **The bytes signed are the canonical string's JSON encoding**, not the
   canonical string. The bridge's signing helper serialises to JSON before
   signing, so what reaches BLAKE2b is the canonical string wrapped in double
   quotes with its newlines written as the two characters `\n`.

3. **Non-ASCII is not escaped** in that JSON encoding. A JSON library that
   escapes it (several do by default) produces a signature the bridge rejects —
   and only for the requests that happen to carry an accented character, which is
   the worst way to find out.

4. **The length prefixes are byte lengths.** `String.length()` counts UTF-16 code
   units, so a body containing `café` would be prefixed one short.

The signature is a pure function of the request, so all of that is testable
without a bridge: `BridgeAuthTest` carries vectors that were produced
independently and then accepted by a running bridge.

## The types

Here are all of the OpenPortal types that have been wrapped in Java.

| Type | The thing to know |
|---|---|
| `AwardDetails` | The argument to every award instruction. Its wire type name is `ProjectDetails`, not `AwardDetails`. An absent `membership_control` means **open**, and an absent `allowed_domains` means **everything is allowed** while an empty list means nothing is. |
| `Allocation` | How much, and **in whose unit**. That unit is the one every usage report about the award must come back in. An award with no allocation is not an award. Only six unit names are canonicalised; everything else is lower-cased and kept, so agree the exact string out of band. |
| `Usage` | Whole seconds of *something* — the unit lives on the award, not here. Every operation saturates and subtraction clamps at zero. An object on the wire, `{"seconds": 7200}`, not a bare number. |
| `UsageReport` → `ProjectUsageReport` → `DailyProjectUsageReport` | The three levels of a usage report. The daily figures are keyed by **local** username and the mapping to portal identifiers lives one level up; without it the allocator cannot attribute a single figure. |
| `StorageReport` → `ProjectStorageReport` | A **snapshot**, not a total — so merging two takes the newer rather than summing. |
| `StorageSize`, `Quota` | Binary units throughout, despite the decimal-looking names. A quota with no measurement declines to answer `percentageUsed()` rather than saying zero. |
| `ProjectIdentifier`, `UserIdentifier`, `ProjectMapping`, `UserMapping` | Bare strings on the wire, and validated against an allow-list — a space, comma or `?` in a name would break an instruction string, a `sacctmgr` argument or a REST URL. A `UserMapping`'s `local_user` is a Unix name from an account agent and an **email address** from a portal; ask before using it as one. |
| `DateRange` | Inclusive as dates, half-open as instants, and capped at five years — the span is what bounds how much work one instruction can ask for. |
| `Notification` | Fire-and-forget. Spelt `{"UserAdded": …}` in JSON and `user_added …` as a string; dispatch on `eventType()`, which is always the snake_case name. |
| `Health`, `Diagnostics` | Operational rather than part of the contract, but `health()` is the first thing to try when something else is failing — it answers even when the network behind the bridge does not. |

Three of them hold their own JSON rather than a field per field:
`DailyProjectUsageReport`, `ProjectStorageReport` and `Job`. That is deliberate.
The daily report alone carries about thirty wire fields — requeue accounting,
expansion factors, reservation occupancy — which an agent populates and reads
back, and a report rebuilt from the fields this client happens to model would
silently drop the rest. `plus` and `times` work from a table of field *kinds*
for the same reason, so a field added to the wire later is still summed and
scaled correctly.

## What maps to what

| Python | Java |
|---|---|
| `openportal.load_config(path)` | `BridgeClient.load(Path)` |
| `openportal.get_portal()` | `BridgeClient.getPortal()` |
| `openportal.sync_offerings(list)` | `BridgeClient.syncOfferings(List<Destination>)` |
| `openportal.get_offerings()` | `BridgeClient.getOfferings()` |
| `openportal.fetch_job(id)` / `fetch_jobs()` | `BridgeClient.fetchJob(UUID)` / `fetchJobs()` |
| `openportal.send_result(job)` | `BridgeClient.sendResult(Job)` |
| `openportal.run(command)` / `status(job)` | `BridgeClient.run(String)` / `status(Job)` |
| `openportal.notify(command)` | `BridgeClient.notify(String)` |
| `openportal.fetch_notification(id)` | `BridgeClient.fetchNotification(UUID)` |
| `job.completed(value)` / `job.errored(e)` | `Job.completed(OpenPortalType)` / `Job.errored(OpenPortalError)` |
| `openportal.error_from_message(text)` | `OpenPortalError.decode(String)` |
| `openportal.health()` / `diagnostics(dest)` / `restart(type, dest)` | `BridgeClient.health()` / `diagnostics(String)` / `restart(String, String)` |
| `ManagedProjectPendingError` and friends | the same names, same hierarchy |
| `AwardDetails(json)` / `ProjectDetails` | `AwardDetails.fromJson(String)` |
| `Allocation.parse` / `Usage.from_hours` | `Allocation.parse` / `Usage.fromHours` |
| `UsageReport.from_json` and the report tree | the same names, `fromJson` / `toJson` |
| `Uuid` | `java.util.UUID` — no wrapper, the JDK has one |
| the operators (`+`, `*`, `/`) on usage and reports | `plus`, `times`, `dividedBy` |
| a property (`job.result`, `award.allocation`) | a method, and `Optional<T>` where the Python is `None`-able |

Two differences beyond naming. `DailyProjectUsageReport` here can also *write*
job counts and queue waits (`addJobs`, `addWaitSeconds`), which the Python module
reads but cannot set — the fields are on the wire either way, and a report that
carries usage but no job counts cannot answer "how many jobs". And a Java site
gets the same `Live*` checks in place of Python's interactive interpreter.

## Two things worth knowing before you write a handler

**A job is answered from a copy of what arrived.** A job carries fields this
client has no opinion about — `board`, `domain`, and whatever a later version
adds — and the bridge matches an answer to the board by `id` and `version`.
`Job.completed` and `Job.errored` deep-copy the job they were given and change
only what they must, so nothing is dropped by a client that does not understand
it. Rebuilding a job from typed fields is how a result stops matching.

**A result carries its type name as well as its JSON.** The awarding portal
deserialises the one against the other, so a `ProjectMapping` returned as a bare
string with no `result_type` is a different answer. That is what
`OpenPortalType` is for, and `site-portal-api.md` §4 lists the type each
instruction must return.
