<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# Writing your own Domain

This document is for anyone who wants to reuse `paddington` (the secure
peer-to-peer transport) and `templemeads` (the Agent/Job framework) for
infrastructure that has nothing to do with HPC or Waldur - a different kind
of resource entirely, with its own commands, identifiers, and events.

Everything specific to OpenPortal's original HPC/Waldur vocabulary -
`add_user`, `get_usage_report`, `ProjectIdentifier`, and so on - lives in one
crate, `greatwestern`, not inside `templemeads` itself. `greatwestern` is the
**reference implementation** of a `Domain`, not a privileged part of the
protocol. Write your own crate implementing the same trait, and your own
agents get everything `paddington`/`templemeads` provide - encrypted
peer-to-peer transport, distributed Job boards, automatic recovery from
disconnects, the fire-and-forget Notification system, standardised CLI/config
handling - for whatever vocabulary you define.

If you only need to *use* OpenPortal's existing HPC vocabulary, you don't need
anything in this document - start with
[instruction-protocol.md](instruction-protocol.md) instead. This document is
for building a *different* one.

---

## 1. The `Domain` trait

Everything templemeads moves around - `Job`, `Envelope`, `Board`,
`Notification`, `Command` - is generic over one type parameter, conventionally
called `L`, bounded by `templemeads::domain::Domain`:

```rust
pub trait Domain: Clone + std::fmt::Debug + 'static {
    /// The command vocabulary a `Job` carries: what an agent is being asked
    /// to do, and with what arguments.
    type Instruction: Clone + PartialEq + std::fmt::Debug + std::fmt::Display
        + Serialize + for<'de> Deserialize<'de> + Send + Sync + 'static;

    /// The fire-and-forget event vocabulary a `Notification` carries.
    type NotificationEvent: Clone + PartialEq + std::fmt::Debug + std::fmt::Display
        + Serialize + for<'de> Deserialize<'de> + Send + Sync + 'static;

    /// Parse an `Instruction` from the text that follows the destination in
    /// a command string (e.g. `"add_user alice.myproject.myportal"`).
    fn parse_instruction(s: &str) -> Result<Self::Instruction, Error>;

    /// Parse a `NotificationEvent` from the text that follows the event
    /// name in a notification string.
    fn parse_notification_event(s: &str) -> Result<Self::NotificationEvent, Error>;

    /// The portal that "owns" this instruction, if it has one. Default: no
    /// such policy - opt in only if you need it (see §4).
    fn owning_portal(_instruction: &Self::Instruction) -> Option<PortalIdentifier> {
        None
    }

    /// Wrap an inner `Notification` for southbound forwarding (see §5).
    fn wrap_forward(inner: Notification<Self>) -> Self::NotificationEvent
    where
        Self: Sized;
}
```

A `Domain` implementation is a single, usually zero-sized, marker type (like
`greatwestern::Hpc`) that never holds a value - it only ever appears as a type
parameter (`Job<Hpc>`, `Envelope<Hpc>`, ...). Everything your agents actually
*do* lives in the two associated types and the handful of methods above.

**Two agents built against different `Domain`s cannot talk to each other.**
`Job<YourDomain>` and `Job<greatwestern::Hpc>` are unrelated Rust types with no
conversion between them, and their JSON serialisations aren't expected to
match either. This is intentional, not a limitation to work around - the
`Domain` choice is a single, compile-time decision per agent binary, exactly
like choosing which protocol version to speak.

---

## 2. Define your vocabulary

### 2.1 `Instruction`

Design an enum (or any type satisfying the bounds above) describing every
command your agents can send each other. `greatwestern::grammar::Instruction`
is the fullest real example to read alongside this guide, but a minimal one
looks like:

```rust
use serde::{Deserialize, Serialize};
use std::fmt;
use templemeads::Error;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Instruction {
    Ping,
    SetTemperature(String, f64), // (device_id, degrees_celsius)
    GetTemperature(String),
}

impl Instruction {
    pub fn parse(s: &str) -> Result<Self, Error> {
        let (command, rest) = match s.split_once(' ') {
            Some((c, r)) => (c, r.trim()),
            None => (s.trim(), ""),
        };

        match command {
            "ping" => Ok(Self::Ping),
            "set_temperature" => {
                let (device, temp) = rest.split_once(' ').ok_or_else(|| {
                    Error::Parse(format!("Invalid set_temperature arguments: '{}'", rest))
                })?;
                let temp: f64 = temp
                    .parse()
                    .map_err(|_| Error::Parse(format!("Invalid temperature: '{}'", temp)))?;
                Ok(Self::SetTemperature(device.to_string(), temp))
            }
            "get_temperature" => Ok(Self::GetTemperature(rest.to_string())),
            unknown => Err(Error::Parse(format!("Unknown instruction: '{}'", unknown))),
        }
    }
}

impl fmt::Display for Instruction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ping => write!(f, "ping"),
            Self::SetTemperature(device, temp) => write!(f, "set_temperature {} {}", device, temp),
            Self::GetTemperature(device) => write!(f, "get_temperature {}", device),
        }
    }
}
```

The parse/`Display` pair must round-trip (`Instruction::parse(&i.to_string())
== Ok(i)`), exactly as `greatwestern::grammar::Instruction` does - this is
what keeps instruction strings safe to log, store, and re-parse without
ambiguity, and it's what makes command injection through instruction
arguments structurally impossible: arguments are typed fields inside an enum
variant, never concatenated back into a shell command or query string.

**Result types.** If an instruction returns a value (`GetTemperature` above),
the type you pass to `job.completed(value)` must implement
`templemeads::named::NamedType` - a one-method trait that gives it a stable
string name recorded in `Job::result_type` (see
[json-types.md](json-types.md)):

```rust
impl templemeads::named::NamedType for Temperature {
    fn type_name() -> String {
        "Temperature".to_string()
    }
}
```

`String`, `bool`, `Vec<T: NamedType>`, and `HashMap<K: NamedType, V: NamedType>`
already implement it in templemeads, covering most simple cases without any
extra code on your part.

### 2.2 `NotificationEvent`

Same shape, for the fire-and-forget side (see
[notification-protocol.md](notification-protocol.md) for the full concept).
One variant is not optional: `Forward`, used internally by templemeads' bridge
infrastructure to route a notification through a portal without the bridge's
name appearing in the destination path:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NotificationEvent {
    TemperatureChanged(String, f64),
    DeviceOffline(String),
    /// Infrastructure-only - not accepted by `parse()`. See `Domain::wrap_forward`.
    Forward(Box<templemeads::notification::Notification<MyDomain>>),
}
```

`templemeads` cannot construct this wrapping itself - it has no concrete
`NotificationEvent` to build - which is why `Domain::wrap_forward` exists as a
required method rather than a default: every `Domain` must supply its own
`Forward` variant and the one line of code that constructs it (§5).

### 2.3 The `Domain` marker type

Tie the two together:

```rust
use templemeads::domain::Domain;
use templemeads::notification::Notification;
use templemeads::portal_identifier::PortalIdentifier;
use templemeads::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MyDomain;

impl Domain for MyDomain {
    type Instruction = Instruction;
    type NotificationEvent = NotificationEvent;

    fn parse_instruction(s: &str) -> Result<Self::Instruction, Error> {
        Instruction::parse(s)
    }

    fn parse_notification_event(s: &str) -> Result<Self::NotificationEvent, Error> {
        NotificationEvent::parse(s)
    }

    fn wrap_forward(inner: Notification<Self>) -> Self::NotificationEvent {
        NotificationEvent::Forward(Box::new(inner))
    }

    // owning_portal: use the default (no such policy) unless you need §4.
}
```

That's the entire contract. Compare this to `greatwestern/src/lib.rs`'s
`impl Domain for Hpc` - it is exactly this shape, just with a real HPC
vocabulary behind `Instruction`/`NotificationEvent` instead of the toy one
above.

---

## 3. Wiring up agent binaries

Every place existing OpenPortal code says `Job`, `Envelope`, or `Notification`
without a type parameter, it actually means `Job<L>` for whichever `Domain`
that binary is compiled against. The established pattern - used by every
built-in agent and the `docs/job`/`docs/cmdline` examples - is a couple of
type aliases near the top of `main.rs`:

```rust
use my_domain_crate::MyDomain;

type Job = templemeads::job::Job<MyDomain>;
type Envelope = templemeads::job::Envelope<MyDomain>;
```

The rest of the file can then say `Job`/`Envelope` exactly as the
domain-agnostic examples in [docs/job](../job/README.md) and
[docs/cmdline](../cmdline/README.md) do.

### 3.1 Job handlers

`async_runnable!` (from `templemeads::async_runnable`) accepts an optional
generic parameter, so a handler can be written either against your concrete
`Job`/`Envelope` aliases (the common case) or, if you're writing something
domain-agnostic yourself, generically over any `L: Domain`:

```rust
async_runnable! {
    pub async fn device_runner(envelope: Envelope) -> Result<Job, Error> {
        let mut job = envelope.job();

        match job.instruction() {
            Instruction::Ping => {
                job = job.completed("pong".to_string())?;
            }
            Instruction::SetTemperature(device, temp) => {
                // ... business logic ...
                job = job.completed_none()?;
            }
            Instruction::GetTemperature(device) => {
                // ... business logic ...
                job = job.completed(Temperature { celsius: 21.5 })?;
            }
        }

        Ok(job)
    }
}
```

### 3.2 Registering the handler

`templemeads` no longer has a built-in default job handler for any agent role,
since it has no concrete `Instruction` to write one against. Every
`agent::{instance, portal, provider, ...}::run()` function therefore takes an
explicit runner:

```rust
pub async fn run<L: Domain>(config: Config, runner: AsyncRunnable<L>) -> Result<(), Error>
```

so your `main.rs` calls `run(config, device_runner).await?` exactly as the
[cmdline example](../cmdline/README.md) does - type inference picks `L =
MyDomain` up from `device_runner`'s signature.

---

## 4. Optional: `owning_portal`

Some domains need a trust rule: "a job about resource X can only be *issued*
by the portal that owns X" (this is what `greatwestern` uses to ensure, say,
a job about `alice.myproject.brics` can only enter the network via the
`brics` portal). If your domain needs the same policy, implement
`owning_portal`:

```rust
fn owning_portal(instruction: &Self::Instruction) -> Option<PortalIdentifier> {
    match instruction {
        Instruction::SetTemperature(device, _) => Some(device_owner(device)),
        _ => None,
    }
}
```

`templemeads::job::Command::parse` calls this after parsing an instruction
and, if it returns `Some(portal)`, checks that the job's destination path
actually starts at that portal - rejecting the parse otherwise. The default
implementation returns `None` for every instruction, i.e. no such check runs
at all; most domains can simply omit this method.

---

## 5. Notifications and the bridge

If your domain has a `bridge`-equivalent agent (bridging a non-Rust portal
application into your network, as `op-bridge` does for `greatwestern`),
templemeads' generic bridge infrastructure needs one thing from your `Domain`
to route a notification *through* the portal without exposing the bridge's
own name in the path: `wrap_forward` (§2.3). Beyond providing that one method
and matching your own `Forward` variant in your portal's notify runner (to
unwrap and re-route it - see
[notification-protocol.md](notification-protocol.md) §5.2), nothing else
about notification delivery, routing, or the bridge's pull-based delivery
queue needs any domain-specific code at all.

---

## 6. TypeScript bindings (optional)

If you want auto-generated TypeScript types for your own result/instruction
types, most of them derive normally:

```rust
#[derive(Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Temperature {
    pub celsius: f64,
}
```

`Job<L>` itself is the one exception, and it will bite you if you try the
obvious thing. ts-rs's `#[derive(TS)]` requires every generic parameter to
also implement `TS`, which would force `MyDomain` to depend on ts-rs just to
be usable as a type parameter - not something templemeads can require of
every `Domain`. A direct `impl TS for Job<MyDomain>` isn't legal Rust either:
both `TS` and `Job` are foreign to your crate, and the orphan-rule exception
for foreign traits only applies when a local type appears as a parameter of
the *trait*, not of the type being implemented for.

The fix - already applied for `greatwestern` in
`greatwestern/src/job_bindings.rs` - is a zero-sized local marker type that
exists purely to host a hand-written `impl TS`, with `name()`/`output_path()`
overridden to still produce a `Job.ts` with the right shape, and a test that
serialises a real `Job::<MyDomain>::parse(...)` and checks its JSON keys
against the hand-written declaration so the two can't silently drift apart.
Copy that file's structure - it's under 100 lines and every line is explained
inline. See [typescript-bindings.md](typescript-bindings.md#job--a-hand-written-binding)
for the full writeup.

---

## 7. What templemeads/paddington give you for free

None of the following need any domain-specific code - they work identically
whichever `Domain` you choose:

| Capability | Where it lives |
|---|---|
| Encrypted peer-to-peer transport, handshake, reconnection | `paddington` (untouched by the `Domain` choice entirely) |
| Distributed Job boards, robust recovery from disconnects, idempotent re-delivery | `templemeads::board`, `templemeads::job` |
| Standardised CLI (`init`/`client`/`server`/`run`/...) and TOML config handling | `templemeads::agent::{instance, portal, ...}`, `templemeads::config` |
| Health checks, diagnostics, restart signalling | `templemeads::health`, `templemeads::diagnostics`, `templemeads::restart` |
| Fire-and-forget `Notification` delivery and routing mechanics | `templemeads::notification`, `templemeads::handler` |
| `PortalIdentifier` and the Portal/Provider/Platform/Instance/Account agent hierarchy | `templemeads::portal_identifier`, `templemeads::agent` |
| Bridging a non-Rust portal application over HTTP | `templemeads::bridge`, `templemeads::bridge_server` |

What you provide is entirely contained in §1-§5 above: two types
(`Instruction`, `NotificationEvent`), two parse functions, and (usually) one
`wrap_forward` one-liner.

---

## 8. Reference implementations to read

- `greatwestern/src/lib.rs` — the full, real `impl Domain for Hpc`, alongside
  `greatwestern/src/grammar.rs` (the `Instruction` enum and
  `owning_portal`) and `greatwestern/src/notification.rs` (`NotificationEvent`).
  This is the complete worked example behind every code sample in this
  document.
- `templemeads/src/test_domain.rs` — a deliberately trivial `Domain`
  (`cfg(test)`-only, internal to templemeads) used purely to exercise the
  generic framework machinery in templemeads' own test suite. Good for seeing
  the absolute minimum that satisfies the trait; not a template for a real
  agent network, since it doesn't parse anything meaningful.
- [docs/plans/grammar-split-design.md](../plans/grammar-split-design.md) —
  the design document recording *why* the split is shaped this way (the
  coupling audit, the rejected type-erasure alternative, the
  `PortalIdentifier`/`NamedType` boundary decisions). Read this if "why does
  `PortalIdentifier` stay in templemeads while everything else moves?" or
  similar design questions come up.

---

## 9. What next?

Once your `Domain` is implemented, everything else in these specifications
applies unchanged: [wire-protocol.md](wire-protocol.md),
[security-model.md](security-model.md),
[agent-configuration.md](agent-configuration.md), and
[bridge-api.md](bridge-api.md) describe `templemeads`/`paddington` mechanics
that don't vary by `Domain`. Only
[instruction-protocol.md](instruction-protocol.md),
[notification-protocol.md](notification-protocol.md), and
[json-types.md](json-types.md) are `greatwestern`-specific - your own
equivalents of those three documents are the natural place to write down
*your* vocabulary for the next person who needs to implement it independently.
