# SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
# SPDX-License-Identifier: MIT
"""
The web layer: the two endpoints OpenPortal calls, and a small operator API.

Run it with:

    uvicorn app:app --port 8080

Two quite different sets of endpoints live here, and it is worth being clear
which is which:

**What OpenPortal calls** - `/signal/job` and `/signal/notification`. These are
the `signal_url` and `notification_url` you gave `op-bridge init`. Their shape
is dictated by the contract (project-portal-api.md §3.1, §5).

**What an operator calls** - everything under `/awards`. This is *not* part of
any OpenPortal contract; it exists because someone has to approve awards and
push usage figures in, and a portal with no web interface needs some way to do
it. Yours will look nothing like this.
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
from contextlib import asynccontextmanager

import openportal
from fastapi import BackgroundTasks, FastAPI, HTTPException, Query
from pydantic import BaseModel

import portal
import store

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

#: How often to sweep for jobs a missed signal left behind. Jobs expire after
#: two minutes and the caller gives up after about thirty seconds, so this is a
#: safety net, not the primary path - keep it well inside that budget.
SWEEP_SECONDS = int(os.environ.get("PORTAL_SWEEP_SECONDS", "15"))

#: Job ids we have already run. The bridge can signal the same job twice - a
#: retried signal racing the sweep is the usual way - and the work must not
#: happen twice (§3.5).
#:
#: An in-memory set is fine for an example and wrong for anything real: it is
#: lost on restart and not shared between workers. Put this next to your award
#: state, in the same database, keyed on the job id.
_seen: set[str] = set()


# --------------------------------------------------------------------------
# Startup: advertise what we offer
# --------------------------------------------------------------------------


@asynccontextmanager
async def lifespan(app: FastAPI):
    """
    Connect to the bridge and register our offerings.

    **Until an offering is registered, requests for it have nowhere to land**
    (§1.1) - they are held and only delivered once it exists. So this happens at
    startup, before we serve anything, and again whenever the set changes.

    `sync_offerings` is a *replace*, not a merge: anything absent is withdrawn.
    """
    openportal.load_config(os.environ["OPENPORTAL_CONFIG"])

    me = openportal.get_portal()

    # `<offering>.<us>.<them>` - the middle element must be our own agent name.
    # A real portal reads the list of awarding portals from its configuration.
    awarding_portals = os.environ.get("PORTAL_AWARDING_PORTALS", "ukri").split(",")

    offerings = [
        openportal.Destination(f"{offering}.{me}.{them.strip()}")
        for offering in portal.OFFERINGS
        for them in awarding_portals
        if them.strip()
    ]

    active = openportal.sync_offerings(offerings)
    logger.info("registered offerings: %s", [str(o) for o in active])

    sweeper = asyncio.create_task(_sweep_forever())
    try:
        yield
    finally:
        sweeper.cancel()


app = FastAPI(
    title="OpenPortal example project portal",
    description=__doc__,
    lifespan=lifespan,
)


# --------------------------------------------------------------------------
# What OpenPortal calls
# --------------------------------------------------------------------------


@app.get("/signal/job")
async def signal_job(tasks: BackgroundTasks, job_id: str = Query(...)):
    """
    `GET /signal/job?job_id=<uuid>` - the bridge telling us a job has arrived.

    Three things this endpoint gets right, all of which matter:

    **It returns immediately.** The job is queued and 200 goes back at once; the
    work happens afterwards. The bridge retries a failed signal five times at
    two-second intervals and then *removes the job from the board and errors
    it*, so a slow signal endpoint fails requests outright (§3.4).

    **It treats the job id as a secret.** The endpoint has no credential of its
    own; the id is a random UUID known only to the bridge and to us. An id the
    bridge does not have is not a request to act on, so it gets a 403 rather
    than being fetched (§3.1).

    **It de-duplicates.** The same id can arrive more than once, and the work
    must not happen twice (§3.5).
    """
    if job_id in _seen:
        logger.info("job %s already handled - ignoring duplicate signal", job_id)
        return {}

    try:
        job = openportal.fetch_job(job_id)
    except OSError:
        # The bridge does not know this id. Either it has already been dealt
        # with, or somebody is guessing.
        raise HTTPException(status_code=403)

    _seen.add(job_id)
    tasks.add_task(portal.handle, job)

    return {}


@app.get("/signal/notification")
async def signal_notification(notification_id: str = Query(...)):
    """
    `GET /signal/notification?notification_id=<uuid>` - a fire-and-forget event.

    Notifications are pull-model, so the body is never posted to an
    unauthenticated endpoint: we are told an id, we fetch it, we return 200
    (§5). There is nothing to answer.

    Make the handling idempotent - the same notification can be delivered more
    than once if a retry races a successful fetch.
    """
    try:
        notification = openportal.fetch_notification(notification_id)
    except OSError:
        # Already collected, or unknown. Either way there is nothing to do, and
        # 200 stops the bridge retrying.
        return {}

    logger.info(
        "notification %s: %s %s",
        notification_id,
        notification.event_type,
        notification.event_argument,
    )

    # A real portal would act on these - `user_added`, `award_changed`, and so
    # on. See notification-protocol.md for the vocabulary.

    return {}


async def _sweep_forever() -> None:
    """
    Poll for jobs that a missed signal left behind.

    The signal is the primary path and this is the safety net. It exists because
    a signal can be lost - a restart at the wrong moment, a network blip - and
    without it that job would sit on the board until it expired.
    """
    while True:
        try:
            for job in openportal.fetch_jobs():
                if str(job.id) in _seen:
                    continue

                logger.info("sweep picked up job %s that no signal delivered", job.id)
                _seen.add(str(job.id))
                await asyncio.to_thread(portal.handle, job)

        except asyncio.CancelledError:
            raise
        except Exception:  # noqa: BLE001 - the sweep must never die
            logger.exception("sweep failed; will retry")

        await asyncio.sleep(SWEEP_SECONDS)


# --------------------------------------------------------------------------
# What an operator calls - not part of any OpenPortal contract
# --------------------------------------------------------------------------


class Decision(BaseModel):
    """An approval or a refusal, with the reason the caller will be told."""

    reason: str = ""


class UsagePush(BaseModel):
    """
    Usage figures for one award.

    Either shape is accepted, because a real operator has both:

    * `hours`, a simple `{date: {email: hours}}` mapping, for a parser that
      produces numbers; or
    * `report`, a complete `ProjectUsageReport` JSON, for a parser that already
      produces OpenPortal types.
    """

    hours: dict[str, dict[str, float]] | None = None
    report: dict | None = None


@app.get("/awards")
async def list_awards():
    """Every award we hold, with its approval state."""
    return [
        {
            "project_id": a.project_id,
            "state": a.state,
            "reason": a.reason,
            "local_group": a.local_group,
            "name": a.details.get("name"),
            "template": a.details.get("template"),
            "members": list((a.details.get("members") or {}).keys()),
        }
        for a in store.all_awards()
    ]


@app.get("/awards/{project_id}")
async def get_one_award(project_id: str):
    award = store.load(project_id)

    if award is None:
        raise HTTPException(status_code=404, detail="no such award")

    return award.raw


@app.post("/awards/{project_id}/approve")
async def approve(project_id: str, decision: Decision):
    """
    Approve an award.

    Nothing is pushed to the awarding portal here, and nothing needs to be: it
    is already re-sending `create_award` on every cycle, so the next one gets a
    mapping instead of `ManagedProjectPendingError` and the award goes live by
    itself. That is what makes the retry contract so useful - approval does not
    need a notification path of its own.
    """
    award = store.load(project_id)

    if award is None:
        raise HTTPException(status_code=404, detail="no such award")

    award.raw["state"] = store.Award.APPROVED
    award.raw["reason"] = decision.reason
    award.raw["local_group"] = award.local_group or project_id
    store.save(award)

    logger.info("approved %s", project_id)
    return {"project_id": project_id, "state": award.state}


@app.post("/awards/{project_id}/reject")
async def reject(project_id: str, decision: Decision):
    """
    Refuse an award, terminally.

    The reason given here is what the awarding portal receives inside
    `ManagedProjectRejectedError`, so write it for whoever reads it there.
    """
    award = store.load(project_id)

    if award is None:
        raise HTTPException(status_code=404, detail="no such award")

    award.raw["state"] = store.Award.REJECTED
    award.raw["reason"] = decision.reason or "refused by a site administrator"
    store.save(award)

    logger.info("rejected %s: %s", project_id, award.reason)
    return {"project_id": project_id, "state": award.state}


@app.put("/awards/{project_id}/usage")
async def push_usage(project_id: str, push: UsagePush):
    """
    Push usage figures in, so `get_usage_report` can answer them from cache.

    This is the half of the integration that is genuinely yours: your accounting
    is the source of truth, your parsers produce the numbers, and this endpoint
    is how they reach the portal. `get_usage_report` then serves them inside the
    thirty seconds it has (§3.4).
    """
    award = store.load(project_id)

    if award is None:
        raise HTTPException(status_code=404, detail="no such award")

    if push.report is not None:
        # A complete `ProjectUsageReport`. Parsing it validates the shape before
        # anything is stored - a malformed report is rejected here rather than
        # failing later, inside a job we have thirty seconds to answer.
        try:
            report = openportal.ProjectUsageReport.from_json(json.dumps(push.report))
        except OSError as e:
            raise HTTPException(status_code=400, detail=f"not a ProjectUsageReport: {e}")

        # Flatten it into our own storage shape. Walking a report means going
        # date by date: `dates` lists them, `get_report(date)` narrows to one
        # day, and `user_mapping` gives the portal user → local name pairs whose
        # usage that day holds. At the portal layer the local name is the
        # member's email.
        hours: dict[str, dict[str, float]] = {}

        for date in report.dates:
            day = report.get_report(date)
            per_user = {}

            for user, local_name in day.user_mapping.items():
                seconds = day.usage(user).seconds
                if seconds:
                    per_user[local_name] = seconds / 3600.0

            if per_user:
                hours[date.isoformat()] = per_user

    elif push.hours is not None:
        # The simpler shape, for a parser that just produces numbers.
        hours = push.hours

    else:
        raise HTTPException(status_code=400, detail="send either `hours` or `report`")

    award.raw["usage"] = hours
    store.save(award)

    return {"project_id": project_id, "days": len(hours)}


@app.get("/health")
async def health():
    """Whether the bridge and the agents behind it are reachable."""
    try:
        return {"openportal": str(openportal.health().status)}
    except OSError as e:
        raise HTTPException(status_code=503, detail=str(e))
