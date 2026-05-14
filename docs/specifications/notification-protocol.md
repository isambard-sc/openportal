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
| `Error` | Log a warning and drop the notification. |

Notifications are **allowed through a soft restart** (unlike Jobs, which are
rejected with an error during restart). Because no acknowledgement is ever
sent, a rejected notification would simply be lost silently; allowing it
through is safer.

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

## 7. Guarantees and Limitations

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

## 8. Source File Reference

| Concept | Source file |
|---------|-------------|
| `NotificationEvent`, `Notification`, `NotificationEnvelope` | `templemeads/src/notification.rs` |
| `AsyncNotifyRunnable`, `default_notify_runner` | `templemeads/src/notification.rs` |
| `Command::Notify`, `Command::notify()` | `templemeads/src/command.rs` |
| `set_notify_runner`, routing in `process_command` | `templemeads/src/handler.rs` |
