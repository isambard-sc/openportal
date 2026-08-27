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
is dictated by the contract (site-portal-api.md §3.1, §5).

**What an operator calls** - everything under `/offerings` and `/awards`. This
is *not* part of any OpenPortal contract; it exists because someone has to say
which resources this site offers, approve the awards made on them and push usage
figures in, and a portal with no web interface needs some way to do it. Yours
will look nothing like this.

Note the order those two are in. A fresh portal offers nothing, so nothing can
be asked of it: `POST /offerings` with a resource name comes first, and only then
can an awarding portal make an award on it (§1.1).
"""

from __future__ import annotations

import asyncio
import datetime
import json
import logging
import os
from contextlib import asynccontextmanager

import openportal
from fastapi import BackgroundTasks, FastAPI, HTTPException, Query
from pydantic import BaseModel

import site_portal
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

#: Our own site's agent name, read from the bridge once at startup.
#:
#: Held rather than fetched per request: it cannot change while we are running,
#: and asking the bridge for it on every approval would make approving depend on
#: the bridge being reachable at that moment.
_my_portal: str | None = None


def my_portal() -> str:
    """Our portal's name, or a 503 if startup has not completed."""
    if _my_portal is None:
        raise HTTPException(status_code=503, detail="not connected to the bridge yet")

    return _my_portal


# --------------------------------------------------------------------------
# Startup: advertise what we offer
# --------------------------------------------------------------------------


def awarding_portals() -> list[str]:
    """
    The portals allowed to make awards here.

    A real portal reads this from its own configuration, and would let an
    operator manage it in the same way the resources below are managed. It is an
    environment variable here so that the example has one fewer moving part.
    """
    raw = os.environ.get("PORTAL_AWARDING_PORTALS", "allocator")

    return [them.strip() for them in raw.split(",") if them.strip()]


def destinations_for(offering: str) -> list[openportal.Destination]:
    """
    The wire names of one resource: `<offering>.<us>.<them>`, one per awarding
    portal.

    The middle element must be our own agent name - an offering in somebody
    else's namespace is not something this portal can advertise, and the portal
    agent rejects it. One registration per (resource, awarding portal) pair,
    because each is a separate virtual agent that that portal may address.
    """
    return [
        openportal.Destination(f"{offering}.{my_portal()}.{them}")
        for them in awarding_portals()
    ]


def publish_offerings() -> list[str]:
    """
    Tell OpenPortal the complete set of resources we advertise.

    **Until an offering is registered, requests for it have nowhere to land**
    (§1.1) - they are held and only delivered once it exists. So this runs at
    startup, before anything is served, and again after every change below.

    `sync_offerings` is a *replace*, not a merge: anything absent is withdrawn,
    and an empty set withdraws everything. That is what makes this one function
    enough for adding and removing alike - there is no separate "unregister".
    """
    offerings = [
        destination
        for offering in site_portal.offering_names()
        for destination in destinations_for(offering)
    ]

    active = [str(o) for o in openportal.sync_offerings(offerings)]
    logger.info("registered offerings: %s", active)

    return active


@asynccontextmanager
async def lifespan(app: FastAPI):
    """
    Connect to the bridge and register whatever we currently offer.

    A fresh portal offers nothing, and starts up perfectly happily: an operator
    adds a resource through `POST /offerings` and it is registered from then on.
    """
    global _my_portal

    openportal.load_config(os.environ["OPENPORTAL_CONFIG"])

    me = openportal.get_portal()
    _my_portal = str(me)

    publish_offerings()

    sweeper = asyncio.create_task(_sweep_forever())
    try:
        yield
    finally:
        sweeper.cancel()


app = FastAPI(
    title="OpenPortal example site portal",
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
    tasks.add_task(site_portal.handle, job)

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
                await asyncio.to_thread(site_portal.handle, job)

        except asyncio.CancelledError:
            raise
        except Exception:  # noqa: BLE001 - the sweep must never die
            logger.exception("sweep failed; will retry")

        await asyncio.sleep(SWEEP_SECONDS)


# --------------------------------------------------------------------------
# What an operator calls - not part of any OpenPortal contract
# --------------------------------------------------------------------------


class OfferingRequest(BaseModel):
    """
    A resource to start advertising, and the templates it accepts.

    `name` is the resource's own name - `cluster1`, not `cluster1.site.allocator`.
    The other two elements are added from our own portal name and the list of
    awarding portals, because neither is an operator's to choose here: an
    offering in somebody else's namespace is not something this portal can
    advertise.

    `templates` are the `AwardDetails.template` values awards on this resource
    may name, and it is **required**: what a resource can be asked for is the
    site's decision about that resource, and defaulting it would publish a guess
    under the site's name that an awarding portal could not tell from a policy.
    Name every template the resource really offers.
    """

    name: str
    templates: list[str]


def _offering_json(
    offering: store.Offering, registered: set[str], awards: list[store.Award]
) -> dict:
    """
    One offering as the endpoints below report it.

    `awards` is passed in rather than read here so that listing many offerings
    reads the award store once instead of once per row.
    """
    destinations = [str(d) for d in destinations_for(offering.name)]

    return {
        "name": offering.name,
        "templates": offering.templates,
        "since": offering.since.isoformat() if offering.since else None,
        # How many awards this resource holds. Shown because it is what makes
        # withdrawing one consequential: those awards stay on record and stop
        # being reachable, rather than being deleted (§4.1.2).
        "awards": len([a for a in awards if a.offering == offering.name]),
        # What the awarding portals address, and whether OpenPortal currently
        # has it registered. The second is the agents' view rather than ours,
        # and the two differing means a sync did not happen or did not take.
        "destinations": destinations,
        "registered": all(d in registered for d in destinations),
    }


@app.get("/offerings")
async def list_offerings():
    """
    `GET /offerings` - every resource we advertise.

    Two sources, deliberately: `offerings` is our own state, and `registered`
    on each row is what the OpenPortal agents actually hold right now. They
    should agree; if they do not, the set was changed while the bridge was
    unreachable and `POST /offerings/sync` puts it right.
    """
    try:
        registered = {str(o) for o in openportal.get_offerings()}
    except OSError:
        # The bridge is not answering. Our own records are still worth serving -
        # they are the source of truth, and this is exactly the state where an
        # operator most wants to see them.
        registered = set()

    awards = store.all_awards()

    return {
        "portal": my_portal(),
        "awarding_portals": awarding_portals(),
        "offerings": [
            _offering_json(offering, registered, awards)
            for offering in site_portal.offerings()
        ],
    }


@app.post("/offerings")
async def add_offering(request: OfferingRequest):
    """
    `POST /offerings` - start advertising a resource.

    ```bash
    curl -X POST localhost:8080/offerings \\
         -H 'content-type: application/json' \\
         -d '{"name": "cluster1", "templates": ["standard", "large"]}'
    ```

    An upsert, and idempotent: posting a resource we already offer updates its
    templates and re-registers it, rather than failing. Everything here is
    retried, including by operators. It is also how the templates on a resource
    are changed - post it again with the new list.

    Registration happens immediately, so an award request for this resource that
    an awarding portal has been retrying can land on the very next attempt.
    """
    try:
        offering = site_portal.add_offering(request.name, request.templates)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))

    registered = set(publish_offerings())

    return _offering_json(offering, registered, store.all_awards())


@app.delete("/offerings/{name}")
async def remove_offering(name: str):
    """
    `DELETE /offerings/{name}` - stop advertising a resource.

    The registration is withdrawn, so the awarding portals can no longer address
    it and new requests for it have nowhere to land.

    **The awards on it are kept**, and the response says how many. That is not
    laziness: an award still owns the days it was attached for, and those days
    may not have been collected yet - deleting the record would make a later
    usage report empty, and an empty report is vacuously complete, which is how
    the final days of an award get silently lost (§4.1.2). The operator API goes
    on working for them, and adding the resource back makes them reachable
    again.
    """
    removed = site_portal.remove_offering(name)

    if removed is None:
        raise HTTPException(status_code=404, detail=f"'{name}' is not offered")

    orphaned = len([a for a in store.all_awards() if a.offering == name])
    publish_offerings()

    return {
        "removed": removed.name,
        "templates": removed.templates,
        # Kept, not deleted - still readable and reportable through /awards.
        "awards_kept": orphaned,
        "offerings": site_portal.offering_names(),
    }


@app.post("/offerings/sync")
async def resync_offerings():
    """
    `POST /offerings/sync` - re-register the current set with OpenPortal.

    Nothing needs this in normal operation; startup and every change above
    already register. It is here for the case that produces a puzzling silence:
    the set was changed while the bridge was down, so our records and the
    agents' disagree and requests for a resource we believe we offer have
    nowhere to land.
    """
    return {"registered": publish_offerings()}


class Approval(BaseModel):
    """
    An approval, and **the project the award is being attached to**.

    `project` is the project's name on *this* site - `myproject1`, not
    `myproject1.site`. The `.site` half is added for you from the portal's own
    name, because it is the one part of the identifier an operator can get wrong
    and cannot usefully vary: an award attached to a project in somebody else's
    namespace is not a claim this portal can make.

    It is required and cannot be defaulted, because it is our half of the
    mapping: it is what goes back to the allocator, and what our own accounting
    records usage against. Approving without deciding it would leave both sides
    unable to name the same thing.

    The name must be 1-64 characters of `A-Za-z0-9_-` and must not start with
    `-`, which is what `ProjectIdentifier` allows for a single component. Within
    that it must uniquely identify the project on this site.

    A real portal generates this when it creates a project - a slug, a sequence
    number, whatever it already uses - rather than asking an operator to type it.
    It is a parameter here so the example can show it being chosen.
    """

    project: str
    reason: str = ""


class Rejection(BaseModel):
    """A refusal, with the reason the awarding portal will be told."""

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


class Finalisation(BaseModel):
    """
    An operations decision that one month's accounting will not change again.

    `month` is `"YYYY-MM"`. `final` is separate from it so the decision can be
    taken back - if a late correction lands, clear it and the allocator starts
    asking about that month again.
    """

    month: str
    final: bool = True


@app.get("/awards")
async def list_awards():
    """Every award we hold, with its approval state."""
    return [
        {
            "offering": a.offering,
            "project_id": a.project_id,
            "state": a.state,
            "reason": a.reason,
            "local_project_id": a.local_project_id,
            "name": a.details.get("name"),
            "template": a.details.get("template"),
            "members": list((a.details.get("members") or {}).keys()),
            # Whether it is attached now, and the full attachment history. A
            # detached award is kept rather than deleted: it still owns the days
            # it was attached for, and those still have to be reportable
            # (§4.1.2), so the operator needs to see them.
            "attached": a.is_attached,
            "attachments": [att.raw for att in a.attachments],
            # Which months the site has declared final. A property of the
            # project rather than of the award - see `/usage/finalise` below -
            # so it is looked up against the project this award was *last*
            # attached to, which a detached award still has.
            "final_months": (
                store.load_project(a.projects_ever_attached[-1]).final_months
                if a.projects_ever_attached
                else []
            ),
        }
        for a in store.all_awards()
    ]


@app.get("/awards/{offering}/{project_id}")
async def get_one_award(offering: str, project_id: str):
    """
    One award. Keyed on the resource as well as the identifier, because that
    pair is what identifies an award - the same name on another resource is a
    different award.
    """
    award = store.load(offering, project_id)

    if award is None:
        raise HTTPException(status_code=404, detail="no such award on that offering")

    return award.raw


@app.post("/awards/{offering}/{project_id}/approve")
async def approve(offering: str, project_id: str, approval: Approval):
    """
    Approve an award, and **give it its identifier here**.

    This is the moment the mapping is made. Until now the awarding portal knows
    the award as `myaward1.allocator` on `cluster1` and we have nothing to pair
    it with; approving attaches it to a project of ours - newly created, or one
    that already exists - and that project's identifier is what closes the loop.

    A project holds at most one award at a time, so an identifier already
    attached to a different award is refused. Re-approving with a different
    identifier *moves* the award, which is allowed: the operator may change which
    project an award is attached to whenever their records say so.

    The project is created **on the resource the award came in through**, and
    stays tied to it. That is why the offering is in the path here: approving
    `myaward1.allocator` on `cluster1` says nothing about an award of the same
    name on `cluster2`, which would be a different project.

    Nothing is pushed back to the awarding portal, and nothing needs to be. It
    is already re-sending `create_award` every cycle, so the next one gets a
    `ProjectMapping` instead of `ManagedProjectPendingError` - and that mapping
    is how it learns our identifier. Approval needs no notification path of its
    own, which is the most useful consequence of the retry contract.
    """
    award = store.load(offering, project_id)

    if award is None:
        raise HTTPException(status_code=404, detail="no such award on that offering")

    me = my_portal()

    # The operator supplies only the project's own name; we qualify it with our
    # portal. Catching a dotted value explicitly rather than letting the parse
    # fail gives the operator the actual answer - "just the project part" - which
    # is the mistake worth anticipating when the full identifier is what appears
    # everywhere else in the API.
    if "." in approval.project:
        raise HTTPException(
            status_code=400,
            detail=(
                f"send only the project name, not the full identifier: "
                f"'{approval.project.split('.')[0]}' rather than "
                f"'{approval.project}' - the '.{me}' is added for you"
            ),
        )

    try:
        local = openportal.ProjectIdentifier(f"{approval.project}.{me}")
    except OSError as e:
        raise HTTPException(
            status_code=400,
            detail=(
                f"'{approval.project}' is not a usable project name: {e}. "
                "Use 1-64 characters of A-Za-z0-9_- not starting with '-'."
            ),
        )

    # One local project per award. The comparison is on the *whole* key -
    # offering and identifier - because `myaward1.allocator` on cluster1 and
    # `myaward1.allocator` on cluster2 are two different awards and must not end
    # up sharing one project.
    clash = store.load_by_local_id(str(local))

    if clash is not None and (clash.offering, clash.project_id) != (offering, project_id):
        raise HTTPException(
            status_code=409,
            detail=(
                f"'{local}' is already the local project for "
                f"{clash.project_id} on {clash.offering}"
            ),
        )

    # Attaching records the date as well as the identifier, because billing is
    # per-day: this award is billed the project's usage from today onwards, and
    # takes the whole of today from whichever award held it before (§4.1.2).
    store.attach(award, str(local), datetime.date.today())
    award.raw["reason"] = approval.reason
    store.save(award)

    logger.info("approved %s on %s as %s", project_id, offering, local)
    return {
        "offering": offering,
        "project_id": project_id,
        "local_project_id": str(local),
        "state": award.state,
        "attachments": [att.raw for att in award.attachments],
    }


@app.post("/awards/{offering}/{project_id}/reject")
async def reject(offering: str, project_id: str, decision: Rejection):
    """
    Refuse an award, terminally.

    The reason given here is what the awarding portal receives inside
    `ManagedProjectRejectedError`, so write it for whoever reads it there.
    """
    award = store.load(offering, project_id)

    if award is None:
        raise HTTPException(status_code=404, detail="no such award on that offering")

    award.raw["state"] = store.Award.REJECTED
    award.raw["reason"] = decision.reason or "refused by a site administrator"
    store.save(award)

    logger.info("rejected %s on %s: %s", project_id, offering, award.reason)
    return {"offering": offering, "project_id": project_id, "state": award.state}


@app.put("/projects/{local_project_id}/usage")
async def push_usage(local_project_id: str, push: UsagePush):
    """
    Push usage figures in, so `get_usage_report` can answer them from cache.

    **Note this endpoint is keyed on our own project identifier**, not the
    awarding portal's, and that is deliberate. Everything under `/awards` speaks
    the awarding portal's language because that is the language OpenPortal asks
    questions in. This one speaks ours, because your accounting produces figures
    for `myproject1.site` and has never heard of `myaward1.allocator`. The mapping made
    at approval is what joins them, and `site_portal.build_usage_report` is where the
    translation happens.

    This is the half of the integration that is genuinely yours: your accounting
    is the source of truth, your parsers produce the numbers, and this endpoint
    is how they reach the portal. `get_usage_report` then serves them inside the
    thirty seconds it has (§3.4).
    """
    # Deliberately *not* `load_by_local_id` alone. A project whose award has
    # just been removed still needs its last days pushed in - the removed award
    # owns them and the allocator has not necessarily collected them yet
    # (§4.1.2). So the check is "is this a project we know", not "is an award
    # attached right now".
    if not store.awards_for_local_project(local_project_id):
        raise HTTPException(
            status_code=404,
            detail=(
                f"no award has ever been attached to '{local_project_id}' - an "
                "award only gets a local project identifier when it is approved"
            ),
        )

    # Only to report back where today's figures will land; may be None if the
    # project is currently unattached, which is not an error.
    award = store.load_by_local_id(local_project_id)

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
        #
        # A report pushed here is expected to be in *our* namespace, since it
        # came from our accounting. Only the email is kept, so a report built
        # against either namespace flattens the same way.
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

    project = store.load_project(local_project_id)
    project.raw["usage"] = hours
    store.save_project(project)

    # Which award each day is billed to is deliberately *not* decided here. It
    # depends on the attachment history and can still change - attaching an
    # award this afternoon takes the whole of today - so it is worked out when a
    # report is built, not when figures are recorded (§4.1.2).
    return {
        "local_project_id": local_project_id,
        "days": len(hours),
        "billing_to": award.project_id if award is not None else None,
    }


@app.post("/projects/{local_project_id}/usage/finalise")
async def finalise_usage(local_project_id: str, decision: Finalisation):
    """
    Declare one month's accounting final - or take that declaration back.

    This is the endpoint that stops the allocator asking. It sets `is_complete`
    on the days of that month in every `get_usage_report` answer, and
    `is_complete` is the allocator's signal that a month is settled and need not
    be requested again (see the README, "How often you are asked").

    It is a deliberately *manual* decision, and it is here rather than inside
    `site_portal.py` for that reason. Completeness is a claim about the future -
    "these figures will not change" - and nothing in the code can know it: a
    scheduler outage, a late job record or a billing correction can all move a
    number after the month has ended. Only the operations team knows when their
    own pipeline has settled, so only they get to say so.

    Nothing forces you to call it. An award whose months are never finalised
    still reports correct figures; those months are simply re-requested every
    sync cycle, which costs one request each and nothing else. Getting it
    *wrong* is the expensive direction: finalise a month early and the allocator
    records the figures it has and stops asking, so a correction that lands
    afterwards is never collected. When in doubt, leave it open.
    """
    # As with pushing usage: a month of a detached award can still be declared
    # final, and often needs to be - it is the last thing the allocator is
    # waiting for before it stops asking (§4.1.2).
    if not store.awards_for_local_project(local_project_id):
        raise HTTPException(
            status_code=404,
            detail=(
                f"no award has ever been attached to '{local_project_id}' - an "
                "award only gets a local project identifier when it is approved"
            ),
        )

    month = decision.month.strip()

    # Validated rather than trusted: this string is compared against a key built
    # from a real date in `site_portal.build_usage_report`, so a month written
    # any other way would silently never match and the finalisation would look
    # like it had been applied when it had not.
    try:
        parsed = datetime.datetime.strptime(month, "%Y-%m")
    except ValueError:
        raise HTTPException(
            status_code=400, detail=f"month must be 'YYYY-MM', not {month!r}"
        )

    month = f"{parsed.year:04d}-{parsed.month:02d}"

    project = store.load_project(local_project_id)
    months = set(project.final_months)

    if decision.final:
        months.add(month)
    else:
        months.discard(month)

    project.raw["final_months"] = sorted(months)
    store.save_project(project)

    return {
        "local_project_id": local_project_id,
        "final_months": project.final_months,
    }


@app.get("/health")
async def health():
    """Whether the bridge and the agents behind it are reachable."""
    try:
        return {"openportal": str(openportal.health().status)}
    except OSError as e:
        raise HTTPException(status_code=503, detail=str(e))
