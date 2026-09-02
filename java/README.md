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

Everything else here is the shapes on the wire — a `Job`, a `Destination`, the
error classes — and the endpoints that carry them.

## Building

```bash
mvn test        # 21 unit tests, no bridge needed
mvn install     # to your local repository as org.openportal:openportal:0.92.0
```

Java 17 or later. Two dependencies, both ordinary:

| Dependency | Why |
|---|---|
| `org.bouncycastle:bcprov-jdk18on` | BLAKE2b, which the bridge signs with and the JDK does not have |
| `com.fasterxml.jackson.core:jackson-databind` | JSON, used as a tree model rather than by data binding |

## The four things the signing gets right

Each of these has cost somebody an afternoon, and each is a `401` with no
explanation. They are in `BridgeAuth`, commented where they are done.

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
| `ManagedProjectPendingError` and friends | the same names, same hierarchy |

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

## Checking it against a real bridge

The unit tests cover the pure functions. Three `Live*` checks in
`src/test/java` need a running bridge, which is the only thing that can settle
whether a signature is right — the easiest one to get is the Python example's:

```bash
cd ../python/examples/site_portal && python example.py start
```

then, from `java/`:

```bash
CP=target/classes:$(ls ~/.m2/repository/org/bouncycastle/bcprov-jdk18on/*/*.jar):…

# every call is accepted, and an error round-trips as a typed error
java -cp "$CP" org.openportal.LiveBridgeCheck ../python/examples/site_portal/data/python/site_bridge.toml

# take whatever is on the board and answer it
java -cp "$CP" org.openportal.LiveJobCheck ../python/examples/site_portal/data/python/site_bridge.toml

# be signalled, fetch, answer - the whole cycle
java -cp "$CP" org.openportal.LiveLoopCheck ../python/examples/site_portal/data/python/site_bridge.toml 18780 30
```

`LiveLoopCheck` stands up the `signal_url` the bridge was initialised with, so
stop the Python app first (or point the bridge at a different port) — two things
answering the same board will race.

## Not in this client

The bridge API has endpoints for restarting agents and reading their diagnostics
(`bridge-api.md` §4). They are operational rather than part of the site portal
contract, and are not wrapped here; `BridgeClient.health()` is, because it is one
call and it answers "is the bridge there".

The instruction grammar is not implemented either. `Instruction` splits a command
into its verb and arguments; it does not parse or validate identifiers the way
the Rust side does, so validate what you use. See its javadoc.
