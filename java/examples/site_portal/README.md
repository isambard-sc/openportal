<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# An example site portal, in Java

The Java counterpart of
[`python/examples/site_portal`](../../../python/examples/site_portal). Same
contract, same walkthrough, same answers — a site portal written against the
[`org.openportal` Java client](../..) rather than the Python module.

```bash
# from the repository root
cd python/examples/site_portal
python example.py start --app java
```

That builds the Java portal, stands up two OpenPortal portals with a bridge
each, and prints a ten-step walkthrough. **Follow the Python example's
[README](../../../python/examples/site_portal/README.md) and the printed
steps** — every `curl` and every allocator-side snippet works identically
against this portal, which is the property worth having. This file covers only
what is different.

## Why the harness is shared

One `example.py`, and it starts either portal. The four agents are the same
Rust binaries wired the same way whichever language answers them, and keeping
two thousand-line orchestrators in step would be a standing cost for nothing:
the thing that differs between the two examples is the **portal**, so that is
what `--app` selects.

```bash
python example.py start --app python   # app.py (the default)
python example.py start --app java     # this
python example.py start --app none     # neither - you start one yourself
```

`--app none` is the one to use from an IDE or a debugger. Start the portal on
port **18780**: that is the port the site bridge's `signal_url` names, and a
portal listening anywhere else is never told a job arrived.

So Python is needed to run the *tutorial*, not to run a site portal. What a site
actually deploys is the jar and a bridge:

```bash
mvn install                       # in java/ first, then here
java -jar target/site-portal-0.92.0.jar <bridge invite file> <port> [state dir]
```

## What to read, and in what order

| File | What is in it |
|---|---|
| **`SitePortal.java`** | **The contract.** One method per instruction, in the order `site-portal-api.md` §4 lists them. Read this one. |
| `OperatorApi.java` | The HTTP surface: the two `/signal/*` endpoints OpenPortal calls, and the rest — approve, reject, offerings, usage — which are this site's own and no part of any contract. |
| `Store.java` | Awards, offerings and projects as JSON files. A real portal has a database; what is worth copying is the shape. |
| `Award.java`, `Attachment.java`, `Offering.java`, `LocalProject.java` | The records the store holds. `Attachment` is small and carries the whole billing rule. |
| `App.java` | The wiring: connect, register what we offer, serve, sweep. |
| `SitePortalTest.java` | 36 checks, no bridge needed — every handler driven through `SitePortal.answer`. |

The shape to take away is `answer()` at the bottom of `SitePortal`: **every job
gets an answer.** A handler either returns a value or throws; either way a
result is posted. A job left unanswered is indistinguishable from an outage
until it expires two minutes later, and it is the one failure mode worth
designing out structurally rather than remembering to avoid.

## Differences from the Python example

Four, and each one is a deliberate choice rather than a gap.

**No web framework.** The operator API is served by
`com.sun.net.httpserver`, which ships with the JDK, so the example has no
framework dependency and its routing is twenty lines you can read rather than a
layer of annotations. The cost is that there is no `/docs` — FastAPI's generated
API browser has no counterpart here, so the endpoints are the ones the
walkthrough prints and the ones in `OperatorApi`. A real portal would serve them
from whatever it already uses.

**The store is an object, not a module.** Python's `store.py` reads
`PORTAL_STATE_DIR` at import; `new Store(path)` takes the directory. That is
what lets `SitePortalTest` point it at a temporary directory instead of the
portal's own, so the tests need no environment at all.

**A list result names its element type explicitly.** `ListHandler` carries the
type alongside the handler, so an empty `get_projects` still answers
`Vec<ProjectMapping>`. Python doesn't need to type empty lists, so
`get_awards` answers `[]` rather than `Vec<ProjectDetails>`.

## Not a production portal

The operator API has **no authentication**, exactly as the Python example's
does not, and for the same reason: it is bound to localhost and the point is the
contract rather than the admin interface. The state directory is plain JSON with
no locking, `seen` grows without bound, and the sweep is a fixed 30 seconds.
Each is fine for a walkthrough and none of them is fine for a site.
