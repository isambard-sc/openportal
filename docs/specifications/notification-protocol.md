<!--
SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# OpenPortal Notification Protocol

This document specifies the notification system in OpenPortal — a lightweight
fire-and-forget signalling mechanism that complements the robust, acknowledged
Job system.

Like `Job`, `Notification` is generic over a `Domain` (see
[writing-a-domain.md](writing-a-domain.md)): the envelope, routing, and
delivery mechanics described in §1, §2, §4, §5, and §7 below belong to
`templemeads` and apply to any `Domain`. The concrete event vocabulary in §3
(`NotificationEvent`'s variants) belongs to `greatwestern`, the reference
`Domain` every built-in OpenPortal agent uses - a different `Domain` would
define its own, unrelated `NotificationEvent` type.

For the full Job and instruction protocol see
[instruction-protocol.md](instruction-protocol.md) and
[wire-protocol.md](wire-protocol.md).

---

## 1. Concept

A **Notification** is a one-way event signal sent along the same destination
path used by Jobs. It differs from a Job in every respect that makes Jobs
robust:

| Property | Job | Notification |
|----------|-----|--------------|
| Stored on board | yes | **no** |
| Acknowledged | yes (Update sent back) | **no** |
| Result returned | yes | **no** |
| State machine | Created → Pending → Running → Complete/Error | **none** |
| Delivery guarantee | at-least-once (retry on reconnect) | **best-effort** |
| Analogy | TCP | **UDP** |

Notifications are appropriate for communicating that something **has already
happened** — they inform downstream agents of a state change without requiring
or waiting for any response.

---

## 2. Notification String Format

A notification is identified by a destination path and an event string:

```
<destination> <event> [<argument>]
```

Example:

```
portal.clusters.instance user_added chris.project.portal
```

The destination follows the same dot-separated agent-path format used by Jobs
(see [instruction-protocol.md](instruction-protocol.md) §Destinations).
The event name and argument together form a `NotificationEvent` - the
domain-supplied event vocabulary (`Domain::NotificationEvent`).

**Source files:** `templemeads/src/notification.rs` (`Notification`,
`NotificationEnvelope` — generic, domain-agnostic transport);
`greatwestern/src/notification.rs` (`NotificationEvent` — the concrete
`greatwestern` event vocabulary described below)

---

## 3. `NotificationEvent` Grammar

`NotificationEvent` is `greatwestern`'s concrete implementation of
`Domain::NotificationEvent` - the vocabulary every built-in OpenPortal agent
uses, since they are all compiled against the `Hpc` domain. It describes
something that has already occurred. All event names use past-tense,
snake_case keywords.

The argument types (`UserIdentifier`, `ProjectIdentifier`) are identical to
those used in the instruction protocol — see
[instruction-protocol.md](instruction-protocol.md) §Identifier Types.

### 3.1 User Events

#### `user_added`

A user was successfully added to a system.

```
user_added <UserIdentifier>
```

Example: `user_added chris.project.portal`

---

#### `user_removed`

A user was removed from a system.

```
user_removed <UserIdentifier>
```

---

#### `user_changed`

A user's details were changed (e.g. home directory updated after provisioning).

```
user_changed <UserIdentifier>
```

---

#### `user_blocked`

A user was blocked from logging in without removing their account.

```
user_blocked <UserIdentifier>
```

---

#### `user_unblocked`

A previously blocked user was re-enabled for login.

```
user_unblocked <UserIdentifier>
```

---

### 3.2 Project Events

#### `project_added`

A project was added to a system.

```
project_added <ProjectIdentifier>
```

Example: `project_added myproject.portal`

---

#### `project_removed`

A project was removed from a system.

```
project_removed <ProjectIdentifier>
```

---

#### `project_changed`

A project's details were changed.

```
project_changed <ProjectIdentifier>
```

---

#### `project_blocked`

All users in a project were blocked.

```
project_blocked <ProjectIdentifier>
```

---

#### `project_unblocked`

All users in a project were unblocked.

```
project_unblocked <ProjectIdentifier>
```

---

### 3.3 Award Events

Award events are fired by the bridge when the web portal creates, updates, or
removes an award, and by the web portal itself to signal acceptance or
rejection. Unlike project events, award events originate at the bridge/portal
boundary rather than from infrastructure agents.

#### `award_added`

An award (project) was created or registered in the web portal.

```
award_added <ProjectIdentifier>
```

---

#### `award_removed`

An award was removed from the web portal.

```
award_removed <ProjectIdentifier>
```

---

#### `award_changed`

An award's details were updated in the web portal.

```
award_changed <ProjectIdentifier>
```

---

#### `award_accepted`

An award was accepted by the receiving portal. Sent by the connected web portal
— not generated by OpenPortal infrastructure agents.

```
award_accepted <ProjectIdentifier>
```

---

#### `award_rejected`

An award was rejected by the receiving portal. Sent by the connected web portal
— not generated by OpenPortal infrastructure agents.

```
award_rejected <ProjectIdentifier>
```

---

## 4. Wire Representation

A `Notification` is carried in the `Notify` variant of the Templemeads
`Command` enum (see [wire-protocol.md](wire-protocol.md) §1.2 - including
that note's correction that this is serde's externally-tagged
representation, `{"Notify": {...}}`, not a literal `"type"` key). Unlike
`Instruction` (which serialises as a single opaque string via `Command`'s
custom `Display`/`parse` - see [instruction-protocol.md](instruction-protocol.md)),
`NotificationEvent` has no such custom serialisation - it serialises via
serde's ordinary derive, so `event` is a **structured JSON object**, one key
per enum variant:

```json
{
  "Notify": {
    "id":              "<uuid-string>",
    "destination":     "<dot-separated-agent-path>",
    "event":           { "<EventVariant>": <variant-data-or-omitted> },
    "domain":          "<domain-name>" | null,
    "domain_version":  "<domain-version>" | null
  }
}
```

**Example** (`user_added chris.project.brics`, addressed to `portal.clusters.shared`):

```json
{
  "Notify": {
    "id": "b2e...{uuid}",
    "destination": "portal.clusters.shared",
    "event": { "UserAdded": "chris.project.brics" },
    "domain": "greatwestern",
    "domain_version": "0.33.0"
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `id` | UUID string | Generated at creation; used only for logging and tracing. Not stored anywhere. |
| `destination` | string | Dot-separated agent path, e.g. `portal.clusters.shared` |
| `event` | object | `NotificationEvent` serialised structurally (ordinary serde derive), one key per variant, e.g. `{"UserAdded": "chris.project.portal"}`. `event.to_string()` (via `Display`) gives the space-separated text form (`user_added chris.project.portal`) used in notification *strings* (§2) - the two are different representations of the same value, not interchangeable on the wire. |
| `domain` | string or null | The `Domain::name()` that authored this event, set once at construction and unchanged thereafter - including by any domain-oblivious routing hop it passes through. `null` only for a Notification from a peer running templemeads from before this field existed. See [writing-a-domain.md](writing-a-domain.md#1-the-domain-trait). |
| `domain_version` | string or null | The domain's version, alongside `domain`. |

---

## 5. Routing

Routing follows the same destination-path logic as Jobs, using the same
`position()` / `next()` checks on each hop.

```
Sender                    Intermediate agents                    Destination
  │                             │                                     │
  │──── Notify ────────────────►│──── Notify ──────────────────────►│
  │                             │     (forwarded, no board write)    │
  │                             │                                     │ notify_runner()
  │                             │                                     │ called here
  │  (no Update sent back)      │                                     │
```

At each hop, the receiving agent checks its position in the destination path:

| Position | Action |
|----------|--------|
| `Downstream` | Forward `Notify` to the next agent in the path. No board write. No update sent back. |
| `Destination` | Call the registered `notify_runner`. No board write. No update sent back. |
| `Error` | Log a warning and drop the notification (unless the bridge sidecar case applies — see §5.3). |

Notifications are **allowed through a soft restart** (unlike Jobs, which are
rejected with an error during restart). Because no acknowledgement is ever
sent, a rejected notification would simply be lost silently; allowing it
through is safer.

### 5.1 Forward Routing

The bridge agent is a **sidecar** — its name never appears in a notification's
destination path, which names only the OpenPortal agents that must handle the
notification. To route a notification through the portal from the bridge (or
vice versa), the bridge wraps the inner notification in a `Forward` event:

```rust
NotificationEvent::Forward(Box<Notification<Hpc>>)
```

`templemeads`' bridge infrastructure cannot construct this itself - it has no
concrete `NotificationEvent` type to build - so every `Domain` must supply
this wrapping via `Domain::wrap_forward` (see
[writing-a-domain.md](writing-a-domain.md)); `greatwestern`'s implementation
is the `Forward` variant shown above.

The `Forward` wrapper is addressed to `<bridge-name>.<portal-name>`. The portal
receives it, extracts the inner notification, and routes it by finding its own
name in the inner destination path, then forwarding to the agent at the next
index (see §5.2).

### 5.2 Bidirectional Portal Forwarding

When the portal handles a `Forward` notification it locates its own position in
the inner destination path and routes to `agents()[portal_index + 1]`:

| Portal index in inner destination | Direction | Example inner path | Routes to |
|-----------------------------------|-----------|-------------------|-----------|
| 0 | South (bridge → downstream agents) | `portal.clusters.instance` | `clusters` |
| 1 | North (virtual agent → peer portal) | `isambard-ai.brics.ukri` | `ukri` |

For security, when the portal is at index 1, index 0 **must** be the name of a
registered virtual agent connected to the portal. Any other portal index (2 or
higher) is rejected with an error.

### 5.3 Bridge Sidecar (Position::Error)

When an infrastructure agent emits a notification addressed to agents that do
not include the bridge name — for example `portal.clusters.instance user_added
chris.project.portal` — the notification travels up the hierarchy and the
portal's notify runner forwards it to the bridge unchanged (§7.1). Because the
bridge is not named in the destination, `position()` returns `Error`.

The bridge handles this sidecar case with a security check:

1. The receiving agent must be of type `Bridge`.
2. It must have a connected portal.
3. The portal's name must be the **last** or **penultimate** agent in the
   notification destination (i.e. the final destination is either the portal
   itself or a virtual agent one hop past the portal).

If all three conditions hold, the notification is accepted and passed to the
bridge's `notify_runner`. Otherwise it is logged as a warning and dropped.

---

## 6. Implementing a Notification Handler

Agents that want to react to incoming notifications register an
`AsyncNotifyRunnable` using `set_notify_runner`. Agents that do not register
a handler receive a no-op default that logs the notification at `DEBUG` level.

**Source files:**
- `templemeads/src/notification.rs` — generic `Notification<L>`,
  `NotificationEnvelope<L>`, `AsyncNotifyRunnable<L>`, `default_notify_runner`
- `greatwestern/src/notification.rs` — the concrete `NotificationEvent` type
  used below (`L = Hpc`)
- `templemeads/src/handler.rs` — `set_notify_runner`

### 6.1 Rust API

As with `Job`, agent code fixes the `Domain` via a type alias - here, to
`greatwestern`'s `Hpc`:

```rust
use greatwestern::{Hpc, NotificationEvent};
use templemeads::async_runnable;
use templemeads::notification::AsyncNotifyRunnable;
use templemeads::set_notify_runner;
use templemeads::Error;

type NotificationEnvelope = templemeads::notification::NotificationEnvelope<Hpc>;

async_runnable! {
    pub async fn my_notify_runner(envelope: NotificationEnvelope) -> Result<(), Error> {
        match envelope.notification().event() {
            NotificationEvent::UserAdded(user) => {
                tracing::info!("User {} was added", user);
                // react to the event...
            }
            NotificationEvent::ProjectChanged(project) => {
                tracing::info!("Project {} was changed", project);
            }
            _ => {}
        }
        Ok(())
    }
}

// Call this after instance::run / portal::run / etc. setup:
set_notify_runner(my_notify_runner).await?;
```

### 6.2 Sending a Notification

To send a notification from within an agent runner, construct a `Notification`
and wrap it in `Command::notify`:

```rust
use greatwestern::{Hpc, NotificationEvent};
use templemeads::agent::Peer;
use templemeads::command::Command;
use templemeads::destination::Destination;

type Notification = templemeads::notification::Notification<Hpc>;

let dest = Destination::parse("portal.clusters.shared")?;
let event = NotificationEvent::UserAdded(user.clone());
let notification = Notification::new(dest, event);

let peer = Peer::new("clusters", zone);
Command::notify(&notification).send_to(&peer).await?;
```

Or parse from a string:

```rust
let notification = Notification::parse(
    "portal.clusters.shared user_added chris.project.portal"
)?;
```

---

## 7. Bridge and Portal Notification Flow

The bridge is the boundary between the OpenPortal agent network and an
external web portal application (e.g. a Python/Django service). Two directions
of notification flow are relevant.

### 7.1 South-to-North: Infrastructure → Web Portal

When an agent emits a notification — for example `freeipa` fires `user_added`
addressed to `portal.clusters.instance` — it travels up the agent hierarchy.
At the portal, the notify runner checks whether the notification reaches the
portal itself (addressed to the portal only) or whether it should be passed to
the bridge for delivery to the web portal. In either case the portal forwards
the notification to the connected bridge **unchanged** (preserving the original
destination path). The bridge accepts it via the sidecar check (§5.3) and its
notify runner places the notification on the **delivery queue** (§7.3). A
single background delivery task drains the queue and signals the web portal via
the notification URL callback (§7.3).

### 7.2 North-to-South: Web Portal → Agent Network (via Forward)

When the web portal wants to emit a notification into the OpenPortal network —
for example, to signal that an event occurred in the web portal itself — it
calls `POST /notify` on the bridge HTTP API with a notification command string:

```
POST /notify
{"command": "isambard-ai.brics.ukri user_added chris.project.brics"}
```

The bridge's `notify` function:

1. Parses the inner notification string.
2. Validates that the destination contains the connected portal's name.
3. Wraps it in a `Forward` event addressed to `<bridge-name>.<portal-name>`.
4. Sends the `Forward` notification to the portal.
5. The portal unwraps it, finds its own position in the inner destination, and
   routes to the next agent (§5.2).

This allows virtual agents registered with the portal to act as notification
sources for peer portals in other zones (e.g. `isambard-ai` as a virtual agent
on `brics` notifying `ukri`).

### 7.3 Notification URL Callback (Pull Model)

The bridge uses a **pull model** with a **rate-limited delivery queue** to
deliver notifications to the web portal securely.

#### Delivery Queue

All incoming notifications — whether from the agent network or from award
events — are placed on an internal bounded `VecDeque` before delivery. A
**single background task** drains the queue, serialising all deliveries so the
web portal never receives concurrent notification signals.

| Property | Value |
|----------|-------|
| Queue capacity | 500 notifications |
| On overflow | Clear **all** queued entries (stale during outage), log at `WARN`, increment failed counter |
| Delivery rate | ≤ 100 notifications/s (10 ms sleep after each delivery) |
| Consumer tasks | 1 (serialised delivery) |

When the queue overflows, dropping the stale backlog and accepting the newest
notification is preferable to losing recent events — older notifications
represent state that has already been superseded during the outage.

#### Pull Flow

Rather than pushing the notification body directly to an unauthenticated
endpoint, the bridge stores each notification internally and signals the web
portal to fetch it:

```
1. Notification placed on delivery queue.
2. Background delivery task pops the notification.
3. Bridge stores the notification in a pending map keyed by its UUID.
4. Bridge sends GET <notification_url>?notification_id=<uuid> to the web portal.
5. Web portal receives the GET and calls POST /fetch_notification on the bridge
   with the UUID as the JSON body (authenticated — see bridge-api.md §4).
6. Bridge returns the full Notification JSON from the pending map.
7. Web portal processes the notification and returns HTTP 200 to the original GET.
8. Bridge removes the notification from the pending map.
```

If the web portal returns a non-2xx status or the request fails, the bridge
retries up to **3 times** with a **2-second delay** between attempts. After all
attempts are exhausted the notification is logged at `ERROR` level, removed
from the pending map, and dropped — no error is propagated to the sender.

**Security rationale:** The web portal's `notification_url` endpoint only
receives a UUID in a query parameter — no body to parse, no injection surface.
The notification content is served exclusively by the authenticated
`POST /fetch_notification` bridge endpoint. UUID entropy (128 bits) makes the
token effectively unguessable.

The notification URL is typically unauthenticated (it is a GET signal, not a
data endpoint). Configure `OPENPORTAL_ALLOW_INVALID_SSL_CERTS=true` to disable
TLS verification in development.

---

## 8. Guarantees and Limitations

- **No delivery guarantee.** If the destination agent is unreachable, the
  notification is silently dropped. There is no retry queue in the agent
  network and no error is propagated back to the sender.
- **No ordering guarantee.** Two notifications sent in sequence may arrive
  out of order if there are multiple hops.
- **No deduplication.** The `id` field is for logging only. If a sender
  retransmits after a suspected drop, the destination may receive duplicates.
- **No result.** The notify runner's return value is used only for local error
  logging; it is never transmitted anywhere.
- **Bridge delivery queue cap.** The bridge holds at most 500 notifications
  in its delivery queue at any time. If the queue fills (e.g. the web portal
  is unreachable for an extended period), all queued notifications are dropped
  and the failed counter in `DiagnosticsTracker` is incremented in bulk. The
  newest notification is then accepted.
- **Bridge delivery rate limit.** The bridge delivers at most ~100
  notifications per second to the web portal. Bursts above this rate are
  absorbed by the queue up to its capacity.

For operations where delivery confirmation matters, use a Job instead.

---

## 9. Source File Reference

| Concept | Source file |
|---------|-------------|
| `Notification<L>`, `NotificationEnvelope<L>` (generic, any `Domain`) | `templemeads/src/notification.rs` |
| `AsyncNotifyRunnable<L>`, `default_notify_runner` | `templemeads/src/notification.rs` |
| `NotificationEvent` (`greatwestern`'s concrete vocabulary) | `greatwestern/src/notification.rs` |
| `Domain::wrap_forward` (the `Forward` wrapping contract every `Domain` implements) | `templemeads/src/domain.rs` |
| `agent::ensure_notification_domain_matches` (per-notification, opt-in domain check) | `templemeads/src/agent.rs` |
| `templemeads::erased::Erased`, `RawNotificationEvent` (domain-oblivious relaying) | `templemeads/src/erased.rs` |
| `Command::Notify`, `Command::notify()` | `templemeads/src/command.rs` |
| `set_notify_runner`, routing in `process_command`, sidecar check | `templemeads/src/handler.rs` |
| `bridge::notify()`, `Forward` wrapping | `templemeads/src/bridge.rs` |
| `notification_url` config, `deliver_notification`, `spawn_notification_delivery_task` | `bridge/src/main.rs` |
| Delivery queue, pending-fetch map (`enqueue`, `pop_queued`, `add`, `get`, `remove`) | `templemeads/src/notificationstate.rs` |
| `BridgeBoard::set_notification_url` | `templemeads/src/bridgeboard.rs` |
| `POST /notify`, `POST /fetch_notification` HTTP endpoints | `templemeads/src/bridge_server.rs` |
| Portal notify runner (Forward dispatch, south-to-north) | `portal/src/main.rs` |
