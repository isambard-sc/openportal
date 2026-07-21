<!--
SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# OpenPortal Bridge HTTP API Specification

The **bridge** agent (`op-bridge`) runs a local HTTP/JSON API server that allows
non-Rust portal implementations (e.g. a Python or Django web application) to
interact with the OpenPortal agent network without speaking the native WebSocket
wire protocol.

The bridge sits between the external portal software and the OpenPortal network:

```
┌──────────────────────┐         ┌───────────┐        ┌─────────────────┐
│  Portal software     │  HTTP   │  Bridge   │  WSS   │  OpenPortal     │
│  (e.g. Python app)   │◄──────►│  API      │◄──────►│  agent network  │
└──────────────────────┘         └───────────┘        └─────────────────┘
```

The bridge handles two directions of communication:

- **Portal → OpenPortal** (`/run`, `/status`): the portal submits instruction
  strings; the bridge wraps them in Jobs and routes them through the agent
  hierarchy.
- **OpenPortal → Portal** (`/fetch_jobs`, `/fetch_job`, `/send_result`): when
  OpenPortal sends jobs to the bridge (e.g. `create_project`, `remove_project`),
  the bridge queues them on a local board and notifies the portal via a
  configurable signal URL; the portal then retrieves and processes each job and
  posts the result back.

---

## 1. Configuration

### 1.1 Default Addresses

| Setting | Default |
|---------|---------|
| Bridge API listen address | `127.0.0.1:3000` |
| Bridge API public URL | `http://localhost:3000` |
| Signal URL | `http://localhost/signal` |
| Notification URL | `http://localhost/notification` |

### 1.2 Bridge Invite File

The bridge generates a random 32-byte HMAC key when it is first initialised.
This key is written to a TOML **bridge invite file**:

```toml
url = "http://localhost:3000"
key = "<64-hex-char key>"
```

The portal software must load this invite file to obtain the key before making
any API calls. The invite file must be transferred securely (it is equivalent to
an API credential).

### 1.3 Environment Variables

| Variable | Effect |
|----------|--------|
| `OPENPORTAL_ALLOW_INVALID_SSL_CERTS` | Set to `true` to disable TLS certificate verification when the bridge calls the signal URL (development only) |

---

## 2. Authentication

Every request must include three authentication headers. Requests missing any
required header are rejected with HTTP 401.

### 2.1 Required Headers

| Header | Description |
|--------|-------------|
| `Authorization` | `OpenPortal <hmac-signature>` |
| `Date` | RFC 2822 timestamp, e.g. `Mon, 01 Jan 2024 12:00:00 GMT` |
| `Content-Type` | Must be `application/json` for POST requests |

### 2.2 Optional Headers

| Header | Description |
|--------|-------------|
| `X-Nonce` | Unique string per request; strongly recommended to prevent replay attacks |

### 2.3 Signature Calculation

The `Authorization` header value is `OpenPortal <signature>`, where
`<signature>` is an HMAC-SHA512 tag (hex-encoded) computed using the bridge
invite key over a canonical call string.

The canonical call string is built differently for GET and POST requests:

**GET (empty body):**

Without nonce:
```
<protocol>\napplication/json\n<date>\n<function>
```

With nonce:
```
<protocol>\napplication/json\n<date>\n<function>\n<nonce>
```

**POST (with body):**

Without nonce:
```
<protocol>\napplication/json\n<date>\n<function>\n<body>
```

With nonce:
```
<protocol>\napplication/json\n<date>\n<function>\n<body>\n<nonce>
```

Where:

| Field | Value |
|-------|-------|
| `<protocol>` | `"get"` or `"post"` (lowercase) |
| `<date>` | Date formatted as `"%a, %d %b %Y %H:%M:%S GMT"` — must match the `Date` header exactly |
| `<function>` | Endpoint name, e.g. `"health"`, `"run"`, `"status"` |
| `<body>` | Raw UTF-8 request body string |
| `<nonce>` | The `X-Nonce` header value |

The HMAC is computed using `orion::auth::authenticate` (HMAC-SHA512) and
hex-encoded. The bridge verifies it using a **constant-time comparison** to
prevent timing attacks.

**Example signature (pseudocode):**

```python
import hmac, hashlib, time

date_str = time.strftime("%a, %d %b %Y %H:%M:%S GMT", time.gmtime())
body = '{"command":"waldur.provider get_offerings"}'
nonce = "unique-nonce-abc123"

canonical = f"post\napplication/json\n{date_str}\nrun\n{body}\n{nonce}"
signature = hmac.new(key_bytes, canonical.encode(), hashlib.sha512).hexdigest()
auth_header = f"OpenPortal {signature}"
```

### 2.4 Time Window

The `Date` header must be within **5 seconds** of the server's current time
(either direction). Requests outside this window are rejected with HTTP 401.

### 2.5 Nonce Replay Prevention

If `X-Nonce` is provided, the bridge tracks seen nonces for a 30-second window.
A request reusing a nonce within that window is rejected with HTTP 401
(`"Nonce has already been used (replay attack)"`).

### 2.6 Rate Limiting

Requests are rate-limited per client IP address at **10,000 requests per
10-second window**. Exceeding the limit returns HTTP 429.

Client IP is extracted from `X-Forwarded-For` (first value) or `X-Real-IP`
headers if present, falling back to the TCP peer address.

---

## 3. Common Response Format

**Error responses** return a JSON object with a `message` field:

```json
{"message": "Something went wrong: <error detail>"}
```

HTTP status codes used:

| Code | Meaning |
|------|---------|
| 200 | Success |
| 401 | Authentication failed (bad signature, expired date, replay) |
| 404 | Resource not found |
| 429 | Rate limit exceeded |
| 500 | Internal server error |

---

## 4. Endpoint Reference

### `GET /`

Health probe. Returns `null`. No authentication required.

**Response:** `null`

---

### `GET /health`

Returns the health status of the bridge and all agents in the connected
OpenPortal network.

**Authentication:** required (GET signature over `"health"`)

**Response:**

```json
{
  "status": "ok",
  "health": { <HealthInfo> }
}
```

On error:

```json
{"status": "error"}
```

---

### `GET /get_portal`

Returns the `PortalIdentifier` (in `name.zone` format) of the portal agent
that the bridge is connected to.

**Authentication:** required (GET signature over `"get_portal"`)

**Response:** a JSON string (the portal identifier)

```json
"waldur"
```

Returns HTTP 500 if no portal agent has connected yet.

---

### `GET /get_offerings`

Returns the current set of resource offerings available through the portal.

**Authentication:** required (GET signature over `"get_offerings"`)

**Response:** a `Destinations` string (comma-separated, wrapped in `[...]`):

```json
"[resource-a.waldur.provider, resource-b.waldur.provider]"
```

See [instruction-protocol.md](instruction-protocol.md) §Destinations for the
format.

---

### `GET /fetch_jobs`

Returns all unfinished jobs that OpenPortal has sent to the bridge for the
portal to process (e.g. `create_project`, `remove_project`, `update_project`,
`get_project`, `get_projects`, `get_project_mapping`, `get_usage_report`,
`get_usage_reports`).

**Authentication:** required (GET signature over `"fetch_jobs"`)

**Response:** JSON array of `Job` objects:

```json
[
  {
    "id":          "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
    "created":     1700000000,
    "changed":     1700000005,
    "expires":     1700000120,
    "version":     2,
    "command":     "resource-a.waldur.provider create_project myproject.waldur ...",
    "state":       "pending",
    "result":      null,
    "result_type": null
  }
]
```

See [json-types.md](json-types.md) §Job for the full `Job` field reference. Only
jobs that are not yet in a terminal state (`complete` or `error`) are returned.

---

### `POST /run`

Submits an OpenPortal instruction string for execution. Returns a `Job` object
immediately; the job may still be `pending` or `running` when it is returned.
Use `/status` to poll for completion.

**Authentication:** required (POST signature over `"run"` and request body)

**Request body:**

```json
{"command": "<destination> <instruction>"}
```

The `command` string follows the OpenPortal instruction protocol format:
`<destination> <instruction-keyword> [arguments...]`. See
[instruction-protocol.md](instruction-protocol.md) for the full grammar.

**Example:**

```json
{"command": "waldur.provider get_offerings"}
```

**Response:** a `Job` object (see [json-types.md](json-types.md) §Job)

```json
{
  "id":          "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "created":     1700000000,
  "changed":     1700000000,
  "expires":     1700000300,
  "version":     1,
  "command":     "waldur.provider get_offerings",
  "state":       "pending",
  "result":      null,
  "result_type": null
}
```

**Routing note:** The bridge validates that the destination's first component
matches either the bridge's own name or the portal's name. Commands addressed
directly to the bridge must use a two-component destination
(`bridge.portalname`). All other commands are wrapped in a `submit` instruction
and routed through the portal.

---

### `POST /status`

Polls the current state of a previously submitted job.

**Authentication:** required (POST signature over `"status"` and request body)

**Request body:** a JSON UUID string

```json
{"job": "a1b2c3d4-e5f6-7890-abcd-ef1234567890"}
```

**Response:** the current `Job` object

When `job.state` is `"complete"`, `job.result` holds the JSON-encoded result
payload and `job.result_type` identifies its type. When `job.state` is
`"error"`, `job.result` holds a plain-text error message. See
[json-types.md](json-types.md) for result type schemas.

---

### `POST /fetch_job`

Retrieves a specific unfinished job from the bridge board by UUID. Returns HTTP
404 if the job is not found or has already been completed/removed.

**Authentication:** required (POST signature over `"fetch_job"` and request body)

**Request body:** a JSON UUID string

```json
"a1b2c3d4-e5f6-7890-abcd-ef1234567890"
```

**Response:** the `Job` object

---

### `POST /fetch_notification`

Retrieves a pending notification by UUID. Called by the web portal after
receiving a `GET <notification_url>?notification_id=<uuid>` signal from the
bridge. Returns HTTP 404 if the UUID is not found (already removed or never
stored).

**Authentication:** required (POST signature over `"fetch_notification"` and
request body)

**Request body:** a JSON UUID string

```json
"a1b2c3d4-e5f6-7890-abcd-ef1234567890"
```

**Response:** the `Notification` object

```json
{
  "id":          "<uuid-string>",
  "destination": "<dot-separated-agent-path>",
  "event":       "<event-string>"
}
```

The web portal should return HTTP 200 to the original `GET <notification_url>`
request after successfully fetching and processing the notification. The bridge
interprets the 200 as delivery confirmation and removes the notification from
the pending store.

---

### `POST /send_result`

Posts the result of a bridge-board job back to the bridge. Used by the portal
after it has processed a job retrieved via `/fetch_jobs` or `/fetch_job`.

**Authentication:** required (POST signature over `"send_result"` and request body)

**Request body:** a completed (or errored) `Job` object

The `Job` must have the same `id` as the original job from the bridge board.
Set `state` to `"complete"` and populate `result` and `result_type`, or set
`state` to `"error"` and put the error message in `result`.

```json
{
  "id":          "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "created":     1700000000,
  "changed":     1700000010,
  "expires":     1700000120,
  "version":     3,
  "command":     "resource-a.waldur.provider create_project myproject.waldur ...",
  "state":       "complete",
  "result":      null,
  "result_type": "None"
}
```

**Response:**

```json
{"status": "ok"}
```

---

### `POST /sync_offerings`

Replaces the full set of resource offerings with the supplied list. Any
virtual agents registered for offerings no longer in the list are removed;
virtual agents for new offerings are created.

**Authentication:** required (POST signature over `"sync_offerings"` and request body)

**Request body:** a `Destinations` string

```json
"[resource-a.waldur.provider, resource-b.waldur.provider]"
```

**Response:** the accepted (validated) `Destinations` set

```json
"[resource-a.waldur.provider, resource-b.waldur.provider]"
```

**Offering format:** each destination must have exactly three components:
`<resource-name>.<local-portal-name>.<remote-portal-name>`. The middle
component must match the connected portal's name.

---

### `POST /add_offerings`

Adds offerings to the existing set without removing current ones.

**Authentication:** required (POST signature over `"add_offerings"` and request body)

**Request/Response:** same format as `/sync_offerings`

---

### `POST /remove_offerings`

Removes the supplied offerings from the current set.

**Authentication:** required (POST signature over `"remove_offerings"` and request body)

**Request/Response:** same format as `/sync_offerings`

---

### `POST /restart`

Sends a restart command to an agent in the OpenPortal network.

**Authentication:** required (POST signature over `"restart"` and request body)

**Request body:**

```json
{
  "restart_type": "<restart-type-string>",
  "destination":  "<destination-string>"
}
```

**Response:**

```json
{"status": "ok", "message": "Restart command sent successfully"}
```

On error:

```json
{"status": "error"}
```

---

### `POST /diagnostics`

Collects a diagnostic report from the specified agent.

**Authentication:** required (POST signature over `"diagnostics"` and request body)

**Request body:**

```json
{"destination": "<destination-string>"}
```

**Response:**

```json
{
  "status": "ok",
  "report": { <diagnostics-report-object> }
}
```

---

### `POST /notify`

Sends a fire-and-forget notification into the OpenPortal agent network via the
portal. Returns immediately once the notification has been handed off — no
result or acknowledgement is ever received back.

**Authentication:** required (POST signature over `"notify"` and request body)

**Request body:**

```json
{"command": "<destination> <event> [<argument>]"}
```

The `command` string follows the notification format described in
[notification-protocol.md](notification-protocol.md) §2. The destination must
contain the connected portal's name.

**Examples:**

```json
{"command": "portal.clusters.instance user_added chris.project.portal"}
{"command": "isambard-ai.brics.ukri user_added chris.project.brics"}
```

The second example uses a virtual agent (`isambard-ai`) as the apparent sender,
routing northbound through the `brics` portal to notify the `ukri` peer portal.

**Response:**

```json
{"status": "ok"}
```

Returns HTTP 500 if the portal agent is not connected or the destination is
invalid.

**Routing note:** The bridge wraps the notification in a `Forward` event
addressed to `<bridge-name>.<portal-name>`. The portal unwraps it and routes
to the next agent in the inner destination path (see
[notification-protocol.md](notification-protocol.md) §5.1–5.2).

---

## 5. Bridge Board: OpenPortal → Portal Flow

Certain instructions are not initiated by the portal but by OpenPortal itself.
For example, when an upstream system calls `create_project`, the command flows
down the agent hierarchy until it reaches the bridge. The bridge places the
resulting `Job` on its internal **bridge board** and signals the portal.

The flow is:

```
1. OpenPortal sends create_project (or similar) to the bridge agent.
2. Bridge adds the Job to the bridge board.
3. Bridge calls GET <signal_url>?job_id=<uuid> to notify the portal.
4. Portal receives the signal; calls POST /fetch_job {"job": "<uuid>"} or GET /fetch_jobs.
5. Portal processes the job (e.g. creates the project in its own system).
6. Portal calls POST /send_result with the completed Job.
7. Bridge unblocks and returns the result to OpenPortal.
```

### 5.1 Signal URL

The signal URL is called with a `job_id` query parameter each time a new job
arrives on the bridge board:

```
GET <signal_url>?job_id=<uuid>
```

The bridge retries up to 5 times with a 2-second delay between attempts. If all
retries fail, the job is removed from the board and an error is returned to the
OpenPortal caller.

The signal endpoint should respond with HTTP 2xx. The bridge does not parse the
response body.

---

## 6. OpenPortal → Portal Notification Delivery (Pull Model)

When a notification arrives at the bridge from the OpenPortal network, it is
placed on an internal **delivery queue**. A single background task drains the
queue and delivers each notification to the web portal using a pull model:
rather than pushing the notification body to an unauthenticated endpoint, the
bridge stores the notification and signals the web portal to fetch it.

### 6.1 Delivery Queue

All notifications — whether from the agent network or from award events
(`award_added`, `award_removed`, `award_changed`) — are pushed onto an internal
bounded queue before delivery. This serialises all web-portal signals through
a single consumer and protects the web portal from notification storms.

| Property | Value |
|----------|-------|
| Queue capacity | 500 notifications |
| On overflow | Clear **all** queued entries, log at `WARN`, increment failed counter |
| Delivery rate | ≤ 100 notifications/s (10 ms sleep after each delivery) |
| Consumer tasks | 1 (serialised delivery) |

When the queue overflows, dropping the entire stale backlog and accepting the
newest notification is preferred over dropping the newest entry: during a
prolonged outage the older notifications represent superseded state.

### 6.2 Pull Flow

```
 1. An OpenPortal agent emits a notification (e.g. cluster fires user_added).
 2. The notification travels up the agent hierarchy to the portal.
 3. The portal's notify runner forwards the notification to the bridge.
 4. The bridge accepts it via the sidecar check and pushes it onto the queue.
 5. Background delivery task pops the notification from the queue.
 6. Bridge stores the notification in a pending map keyed by its UUID.
 7. Bridge sends GET <notification_url>?notification_id=<uuid> to the web portal.
 8. Web portal calls POST /fetch_notification on the bridge with the UUID.
 9. Bridge returns the Notification JSON; web portal processes the event.
10. Web portal returns HTTP 200 to the original GET.
11. Bridge removes the notification from the pending store.
```

### 6.3 Notification URL Signal

The bridge sends:

```
GET <notification_url>?notification_id=<uuid>
```

The bridge makes up to **3 attempts** with a **2-second delay** between
attempts. If all attempts fail the notification is logged at `ERROR` level and
dropped. No error is returned to the OpenPortal sender (notifications are
fire-and-forget).

The `notification_url` endpoint only receives a UUID in a query parameter — no
body. It should respond HTTP 2xx after the web portal has fetched and processed
the notification via `POST /fetch_notification` (§4). The response body is
ignored.

### 6.4 Fetching the Notification

After receiving the signal, the web portal calls `POST /fetch_notification`
(§4) with the UUID to retrieve the full `Notification` object. This endpoint
is authenticated with the bridge HMAC key (§2), so the notification content is
never exposed to unauthenticated callers.

### 6.5 Notification URL Configuration

The `notification_url` is set at initialisation time:

```bash
op-bridge init --notification-url http://localhost/notification
```

To update it, reinitialise the bridge with `--force`, or edit the config file
directly. The default is `http://localhost/notification`.

---

## 7. Instructions Handled by the Bridge Board

The following instructions, when sent from the OpenPortal network to the bridge,
are placed on the bridge board for the portal to handle:

| Instruction | Description |
|-------------|-------------|
| `create_project` / `create_award` | Create a new project |
| `remove_project` | Remove an existing project |
| `update_project` / `update_award` | Update project details |
| `get_project` | Get details of a project |
| `get_projects` | Get all projects |
| `get_award` | Get award details for a project |
| `get_awards` | Get award details for all projects |
| `get_project_mapping` | Get the local group mapping for a project |
| `get_usage_report` | Get compute usage for a project over a date range |
| `get_usage_reports` | Get compute usage for all projects over a date range |

See [instruction-protocol.md](instruction-protocol.md) for the full instruction
grammar and argument formats.

---

## 8. Source File Reference

| Concept | Source file |
|---------|-------------|
| HTTP API server (all endpoints including `/notify`) | `templemeads/src/bridge_server.rs` |
| `sign_api_call` function | `templemeads/src/bridge_server.rs` |
| Bridge board (OpenPortal → portal jobs), `notification_url` storage | `templemeads/src/bridgeboard.rs` |
| `run`, `status`, and `notify` logic | `templemeads/src/bridge.rs` |
| `deliver_notification`, `spawn_notification_delivery_task`, `bridge_notify_runner` | `bridge/src/main.rs` |
| Delivery queue, pending-fetch map (`enqueue`, `pop_queued`, `add`, `get`, `remove`) | `templemeads/src/notificationstate.rs` |
| Bridge agent main (instruction dispatch) | `bridge/src/main.rs` |
