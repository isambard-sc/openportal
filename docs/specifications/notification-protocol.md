<!--
SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# OpenPortal Notification Protocol

This document specifies the notification system in OpenPortal — a lightweight
fire-and-forget signalling mechanism that complements the robust, acknowledged
Job system.

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
The event name and argument together form a `NotificationEvent`.

**Source file:** `templemeads/src/notification.rs`

---

## 3. `NotificationEvent` Grammar

A `NotificationEvent` describes something that has already occurred. All event
names use past-tense, snake_case keywords.

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

## 4. Wire Representation

A `Notification` is carried in the `Notify` variant of the Templemeads
`Command` enum (see [wire-protocol.md](wire-protocol.md) §1.2). Its JSON
structure is:

```json
{
  "type": "Notify",
  "notification": {
    "id":          "<uuid-string>",
    "destination": "<dot-separated-agent-path>",
    "event":       "<event-string>"
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `id` | UUID string | Generated at creation; used only for logging and tracing. Not stored anywhere. |
| `destination` | string | Dot-separated agent path, e.g. `portal.clusters.shared` |
| `event` | string | `NotificationEvent` serialised as its display string, e.g. `user_added chris.project.portal` |

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
NotificationEvent::Forward(Box<Notification>)
```

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
- `templemeads/src/notification.rs` — types and `default_notify_runner`
- `templemeads/src/handler.rs` — `set_notify_runner`

### 6.1 Rust API

```rust
use templemeads::async_runnable;
use templemeads::notification::{AsyncNotifyRunnable, NotificationEnvelope, NotificationEvent};
use templemeads::set_notify_runner;
use templemeads::Error;
use templemeads::job::Job;

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
use templemeads::command::Command;
use templemeads::notification::{Notification, NotificationEvent};
use templemeads::destination::Destination;
use templemeads::agent::Peer;

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
notify runner POSTs it to the web portal via the notification URL callback
(§7.3).

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

### 7.3 Notification URL Callback

The bridge is configured with a `notification_url`. When a notification arrives
from the OpenPortal network and passes the sidecar check (§5.3), the bridge's
notify runner POSTs the `Notification` object as JSON to this URL:

```
POST <notification_url>
Content-Type: application/json

{
  "id":          "<uuid-string>",
  "destination": "<dot-separated-agent-path>",
  "event":       "<event-string>"
}
```

The bridge makes up to **3 attempts** with a **2-second delay** between
attempts. If all attempts fail the notification is logged at `ERROR` level and
dropped — no error is propagated back to the OpenPortal sender. The endpoint
should respond with HTTP 2xx. The response body is not read.

The notification URL is unauthenticated. It is called from the bridge process
(typically localhost), so no credential is needed. Configure
`OPENPORTAL_ALLOW_INVALID_SSL_CERTS=true` to disable TLS verification in
development.

---

## 8. Guarantees and Limitations

- **No delivery guarantee.** If the destination agent is unreachable, the
  notification is silently dropped. There is no retry queue and no error is
  propagated back to the sender.
- **No ordering guarantee.** Two notifications sent in sequence may arrive
  out of order if there are multiple hops.
- **No deduplication.** The `id` field is for logging only. If a sender
  retransmits after a suspected drop, the destination may receive duplicates.
- **No result.** The notify runner's return value is used only for local error
  logging; it is never transmitted anywhere.

For operations where delivery confirmation matters, use a Job instead.

---

## 9. Source File Reference

| Concept | Source file |
|---------|-------------|
| `NotificationEvent`, `Notification`, `NotificationEnvelope` | `templemeads/src/notification.rs` |
| `AsyncNotifyRunnable`, `default_notify_runner` | `templemeads/src/notification.rs` |
| `Command::Notify`, `Command::notify()` | `templemeads/src/command.rs` |
| `set_notify_runner`, routing in `process_command`, sidecar check | `templemeads/src/handler.rs` |
| `bridge::notify()`, `Forward` wrapping | `templemeads/src/bridge.rs` |
| `notification_url` config, `signal_web_portal_notification` | `bridge/src/main.rs` |
| `BridgeBoard::set_notification_url` | `templemeads/src/bridgeboard.rs` |
| `POST /notify` HTTP endpoint | `templemeads/src/bridge_server.rs` |
| Portal notify runner (Forward dispatch, south-to-north) | `portal/src/main.rs` |
