# SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
# SPDX-License-Identifier: MIT
"""
The contract: what this portal answers, and how.

This is the file to read. Everything OpenPortal asks of a project portal is
here, one function per instruction, in the order
`docs/specifications/project-portal-api.md` §4 lists them.

The shape to take away is `handle()` at the bottom: **every job gets an
answer.** A handler either returns a value or raises; either way a result is
posted. A job left unanswered is indistinguishable from an outage until it
expires two minutes later, and it is the one failure mode worth designing out
structurally rather than remembering to avoid.
"""

from __future__ import annotations

import datetime
import json
import logging
import time

import openportal

import store

logger = logging.getLogger(__name__)


# --------------------------------------------------------------------------
# What this portal offers
# --------------------------------------------------------------------------

#: The resources we offer, and to whom.
#:
#: An offering is a **virtual agent** on this portal: a name the awarding portal
#: addresses directly, standing for one resource we run. On the wire it is
#: written `<offering>.<us>.<them>` - the resource, offered by us, to them - and
#: `app.py` registers the full paths with the bridge at startup.
#:
#: Two of them here, because one would hide the most important thing about them:
#: **the offering is part of an award's identity, not a permission check.** An
#: award created through `cluster1` is an award *on `cluster1`*. The same
#: awarding portal can hold a different award of the same name on `cluster2`,
#: and asking one resource about a project that lives on the other gets nothing
#: back - see `_offering_of` and `build_usage_report` below.
OFFERINGS = {"cluster1", "cluster2"}

#: The `AwardDetails.template` values each offering accepts.
#:
#: Per-offering, because a template means something different on each resource -
#: in Waldur it selects the organisation, the default offerings and the billing
#: that a project is created with, all of which are properties of the resource.
#: An award naming a template this resource does not offer is rejected rather
#: than quietly given a default (§4.1).
OFFERED_TEMPLATES = {
    "cluster1": {"standard", "large"},
    "cluster2": {"standard"},
}


# --------------------------------------------------------------------------
# Small helpers
# --------------------------------------------------------------------------


def _local_project(award: store.Award) -> openportal.ProjectIdentifier:
    """
    Our own `ProjectIdentifier` for an award, which only an approved award has.
    """
    if award.local_project_id is None:
        raise openportal.OpenPortalError(
            f"{award.project_id} has no local project - it is not approved"
        )

    return openportal.ProjectIdentifier(award.local_project_id)


def _mapping(award: store.Award) -> openportal.ProjectMapping:
    """
    The `ProjectMapping` most award instructions return:
    `<their project id>:<our project id>`.

    **This is the whole point of the exchange.** The awarding portal knows the
    award as `myaward1.allocator`; we know the project we created for it as
    `myproject1.site`. Neither side can guess the other's name, so the mapping is
    where they are joined - and once it has been returned, both sides know that
    their award and our project are the same object. Award ID and project ID
    become two names for one thing at this interface.

    It matters beyond bookkeeping. Our accounting produces usage figures for
    `myproject1.site` and has never heard of `myaward1.allocator`; the mapping is what
    lets `get_usage_report` answer a question asked in their namespace with
    figures recorded in ours.

    Only ever built for an *approved* award. One still awaiting approval has no
    local project, so there is no honest identifier to put here - which is
    precisely why the answer in that case is an error, not a mapping (§4.1).
    """
    return openportal.ProjectMapping(f"{award.project_id}:{_local_project(award)}")


def _offering_of(job: openportal.Job) -> str:
    """
    Which resource this request is about.

    Every request arrives addressed to one of our virtual agents, and that name
    is the last element of the path. `forwarded_for` carries the original
    `allocator.site.cluster1` when the request came from another portal; the job's
    own destination is `site.<bridge>.cluster1` and ends the same way, so it
    is the fallback for a locally-originated request.

    This is not decoration. It scopes everything below: an award belongs to the
    offering it was created on, and a question asked of a different offering is
    a question about a different thing.
    """
    path = job.forwarded_for if job.forwarded_for is not None else job.destination

    return path.agents[-1]


def _require_award(job: openportal.Job, project_id: str) -> store.Award:
    """
    Fetch an award we hold **on the offering this request came through**, or
    fail clearly.

    `OpenPortalError`, not `ManagedProjectRejectedError`: we are not refusing
    this award, we simply do not have it here. The distinction matters because a
    rejection is terminal to the caller (§3.3).
    """
    offering = _offering_of(job)
    award = store.load(offering, project_id)

    if award is None:
        raise openportal.OpenPortalError(
            f"no award {project_id} on {offering}"
        )

    return award


def _answer_for_state(award: store.Award) -> openportal.ProjectMapping:
    """
    Turn our local approval state into the contract's answer.

    The most important function in this example, because getting it wrong is
    costly in both directions (§3.3):

    * `ManagedProjectPendingError` means *not yet, ask again*. The awarding
      portal logs it quietly and retries next cycle. An award parked here for a
      week raises this every cycle for a week, and nothing is wrong.

    * `ManagedProjectRejectedError` means *no*. The awarding portal records the
      award as errored and stops asking. Raise it only when asking again cannot
      help - an unknown template, an expired end date, an allocation above what
      you will ever grant.

    A rejection where you meant "pending" strands an award that only needed
    approving. A "pending" where you meant "rejected" leaves the caller
    retrying forever against a decision that will never change.
    """
    if award.state == store.Award.PENDING:
        raise openportal.ManagedProjectPendingError(
            award.reason or "awaiting approval by a site administrator"
        )

    if award.state == store.Award.REJECTED:
        raise openportal.ManagedProjectRejectedError(
            award.reason or "this award was refused"
        )

    return _mapping(award)


def _username(email: str) -> str:
    """
    A portal-level username from an email address.

    `UserIdentifier` is `username.project.portal` and each part is restricted to
    `[A-Za-z0-9_-]`, so the local part of the address is sanitised rather than
    used raw.
    """
    local = email.split("@", 1)[0]
    return "".join(c if c.isalnum() or c in "_-" else "-" for c in local) or "user"


# --------------------------------------------------------------------------
# §4.1 Awards
# --------------------------------------------------------------------------


def create_award(job: openportal.Job) -> openportal.ProjectMapping:
    """
    `create_project <project_id> <AwardDetails JSON>`

    **This arrives repeatedly for awards you already hold.** The awarding
    portal re-sends it every synchronisation cycle to re-assert the award's
    state - waldur-mastermind's own comment reads "add it again just to be
    sure" (§3.5). It is not asking for a second project. So: look it up, merge
    what changed, and answer as you answered last time.
    """
    project_id = job.instruction.arguments[0]
    details = openportal.AwardDetails(job.instruction.arguments[1])

    # Which resource this award is for. It came in addressed to one of our
    # virtual agents, and that is the resource being asked for.
    offering = _offering_of(job)

    if details.project_template is None:
        raise openportal.ManagedProjectRejectedError("no template named in the award")

    if str(details.project_template) not in OFFERED_TEMPLATES.get(offering, set()):
        raise openportal.ManagedProjectRejectedError(
            f"template '{details.project_template}' is not offered on {offering}"
        )

    award = store.load(offering, project_id)

    if award is None:
        # New award *on this resource*. An award of the same name on another
        # offering is a different award and is left alone.
        forwarded_for = str(job.forwarded_for) if job.forwarded_for else None
        award = store.create(
            offering, project_id, json.loads(details.to_json()), forwarded_for
        )
        logger.info(
            "recorded new award %s on %s, awaiting approval", project_id, offering
        )
    else:
        # Known already: merge the incoming details over what we hold, so a
        # changed member list or end date takes effect. `merge` replaces
        # `members` and `allowed_domains` wholesale - they are definitive sets
        # owned by the awarding portal - while `notes` accumulate.
        held = openportal.AwardDetails(json.dumps(award.details))
        award.raw["details"] = json.loads(held.merge(details).to_json())
        store.save(award)
        logger.debug("re-asserted award %s on %s", project_id, offering)

    return _answer_for_state(award)


def update_award(job: openportal.Job) -> openportal.ProjectMapping:
    """
    `update_project <project_id> <AwardDetails JSON>`

    An update for an award we have never seen is normal, not an error - a
    missed message or a rebuilt database gets us here. Treat it as a create,
    which routes it through the approval path rather than silently provisioning
    something nobody approved (§3.5).
    """
    if store.load(_offering_of(job), job.instruction.arguments[0]) is None:
        logger.info(
            "update for unknown award %s on %s - treating it as a create",
            job.instruction.arguments[0],
            _offering_of(job),
        )

    return create_award(job)


def remove_award(job: openportal.Job) -> openportal.ProjectMapping:
    """
    `remove_project <project_id>`

    Severs the link. The answer is `<project_id>:None` - the award is gone, so
    there is no local group left to name (§4.1).

    Removing an award we do not hold is *not* an error: the caller wants it
    gone, and it is gone. Being idempotent here means a retried removal does
    not produce a spurious failure.
    """
    project_id = job.instruction.arguments[0]
    offering = _offering_of(job)

    # Only from this resource. An award of the same name on another offering is
    # a different award, and was not what the caller asked to remove.
    if store.delete(offering, project_id):
        logger.info("removed award %s from %s", project_id, offering)

    return openportal.ProjectMapping(f"{project_id}:None")


def get_award(job: openportal.Job) -> openportal.AwardDetails:
    """
    `get_award <project_id>` → `AwardDetails`

    How the awarding portal finds out what actually happened to an award it
    created.

    **Populate `members` here.** This portal does not implement `get_users`,
    and neither does waldur-mastermind - members travel with the award instead,
    and this is the field callers read (§4.2). They are already in the stored
    details because that is how they arrived.
    """
    award = _require_award(job, job.instruction.arguments[0])
    details = openportal.AwardDetails(json.dumps(award.details))

    # A real portal overlays live project state here - the current member list,
    # the allocation as actually spent, the current end date - since those may
    # have moved on from what was last agreed.

    # `notes` is the one field that accumulates across a merge, which makes it
    # the right place for commentary that is not part of the award itself.
    if award.state != store.Award.APPROVED:
        details.add_note(openportal.Note("portal", f"{award.state}: {award.reason}"))

    return details


def get_awards(job: openportal.Job) -> list[openportal.AwardDetails]:
    """`get_awards <portal_id>` → every award that portal has made here."""
    return [
        openportal.AwardDetails(json.dumps(a.details))
        for a in store.awards_on(_offering_of(job), job.instruction.arguments[0])
    ]


def get_projects(job: openportal.Job) -> list[openportal.ProjectMapping]:
    """
    `get_projects <portal_id>` → a list of **mappings**, not details.

    Easy to confuse with `get_awards` above; the return types are different
    shapes. An award with no local project yet maps to `:None`, the same
    spelling `remove_award` uses.
    """
    mappings = []

    for award in store.awards_on(_offering_of(job), job.instruction.arguments[0]):
        if award.state == store.Award.APPROVED:
            mappings.append(_mapping(award))
        else:
            mappings.append(openportal.ProjectMapping(f"{award.project_id}:None"))

    return mappings


def get_project_mapping(job: openportal.Job) -> openportal.ProjectMapping:
    """`get_project_mapping <project_id>` → that one award's mapping."""
    return _answer_for_state(_require_award(job, job.instruction.arguments[0]))


# --------------------------------------------------------------------------
# §4.3 Reports
# --------------------------------------------------------------------------


def build_usage_report(
    offering: str, project_id: str, date_range: openportal.DateRange
) -> openportal.ProjectUsageReport:
    """
    Assemble a `ProjectUsageReport` from the figures we hold.

    Two things are going on, and the second is the interesting one.

    Nothing here hand-assembles JSON: construct the report, add a daily report
    per date, and the type handles the wire format.

    More importantly, **the figures are recorded in our namespace and asked for
    in theirs**. Our accounting produces usage for `myproject1.site`; the awarding
    portal asked about `myaward1.allocator`. So the report is built against our own
    project identifier and then `remap_project`ped into theirs at the end. That
    translation is only possible because approving the award fixed the mapping
    between the two.

    The report is also scoped to one resource. A project lives on the offering
    its award was created through, so asking a different offering about it
    returns an empty report rather than an error - see below.

    `app.py` also accepts a complete `ProjectUsageReport` JSON pushed in by an
    operator's own parser. Both routes end up in the same store, and this
    function serves either.
    """
    report = openportal.ProjectUsageReport(openportal.ProjectIdentifier(project_id))

    # **The project may simply not be on this resource.** An awarding portal
    # holding an award on `cluster1` can perfectly well ask `cluster2` about
    # it - the identifier is the same, and nothing stops the question. The
    # honest answer is an empty report: nothing was used here, because the
    # project is not here. An error would say "something is broken", which is
    # not true, and would fail a caller that is simply sweeping every offering
    # it knows about.
    award = store.load(offering, project_id)

    if award is None or award.local_project_id is None:
        return report

    local_project = _local_project(award)

    # Build the report in *our* namespace first, because that is the namespace
    # the figures were recorded in - our accounting knows `myproject1.site` and has
    # never heard of `myaward1.allocator`.
    report = openportal.ProjectUsageReport(local_project)
    today = datetime.date.today()

    for iso_date, per_user in sorted(award.usage.items()):
        date = datetime.date.fromisoformat(iso_date)
        daily = openportal.DailyProjectUsageReport()

        for email, hours in per_user.items():
            # `add_mapping` records which portal user each local name belongs
            # to. At the portal layer the local name is the member's email.
            user = openportal.UserIdentifier(f"{_username(email)}.{local_project}")
            report.add_mapping(
                openportal.UserMapping(f"{user}:{email}:{local_project}")
            )
            daily.add_usage(email, openportal.Usage.from_hours(float(hours)))

        # A day in the past will not change, so say so - it lets the caller
        # cache the figure and stop asking for it.
        if date < today:
            daily.set_complete()

        report.set_report(date, daily)

    # Now translate the whole report into the awarding portal's namespace. This
    # is the mapping being used: they asked about `myaward1.allocator`, so that is
    # what the answer must be about. `remap_project` rewrites the project and
    # rebuilds every `UserIdentifier` with it, turning `alice.myproject1.site` into
    # `alice.myaward1.allocator` - the member's email is unchanged, because that is
    # the same person either way.
    report.remap_project(openportal.ProjectIdentifier(project_id))

    return report.filter(date_range)


def _date_range(job: openportal.Job, index: int = 1) -> openportal.DateRange:
    """
    The `DateRange` argument, which the grammar fills in as `this_week` when the
    caller omits it - so in practice it is always there.
    """
    if len(job.instruction.arguments) > index:
        return openportal.DateRange.parse(job.instruction.arguments[index])

    return openportal.DateRange.this_week()


def get_usage_report(job: openportal.Job) -> openportal.ProjectUsageReport:
    """
    `get_usage_report <project_id> <DateRange>` → `ProjectUsageReport`

    **Answer from cache.** You have about 30 seconds before the caller gives up
    (§3.4) - not the two minutes the job expiry suggests. So this reads figures
    pushed in earlier rather than going away to compute them. If your accounting
    takes minutes, serve what you have and let the next request collect the
    fresher numbers; there will be a next request, because callers retry.
    """
    return build_usage_report(
        _offering_of(job), job.instruction.arguments[0], _date_range(job)
    )


def get_usage_reports(job: openportal.Job) -> openportal.UsageReport:
    """
    `get_usage_reports <portal_id>` → the portal-level roll-up.

    A loop over the per-project path. `to_usage_report()` lifts each
    single-project report into the portal-level shape so they can be combined.
    """
    portal = job.instruction.arguments[0]
    offering = _offering_of(job)
    date_range = _date_range(job)

    # Only the awards on this resource, so a portal-level roll-up asked of
    # `cluster2` covers `cluster2` and nothing else.
    reports = [
        build_usage_report(offering, award.project_id, date_range).to_usage_report()
        for award in store.awards_on(_offering_of(job), portal)
    ]

    # `combine` needs at least one report, so an empty portal answers with an
    # empty report rather than failing.
    if not reports:
        return openportal.UsageReport(openportal.PortalIdentifier(portal))

    return openportal.UsageReport.combine(reports)


def get_storage_report(job: openportal.Job) -> openportal.ProjectStorageReport:
    """
    `get_storage_report <project_id> <DateRange>` → `ProjectStorageReport`

    This portal has no storage to report, and answers with an **empty report**
    rather than an error. Empty says "nothing here"; an error says "something is
    broken", and only the first is true (§4.3). That is the same answer a
    project which is not on this resource gets, for the same reason.

    Usage and storage are requested independently, on separate schedules, so
    failing this would not have cost us the usage figures - but there is no
    reason to fail it.
    """
    return openportal.ProjectStorageReport(
        openportal.ProjectIdentifier(job.instruction.arguments[0])
    )


def get_storage_reports(job: openportal.Job) -> openportal.StorageReport:
    """`get_storage_reports <portal_id>` → an empty portal-level roll-up."""
    return openportal.StorageReport(
        openportal.PortalIdentifier(job.instruction.arguments[0])
    )


# --------------------------------------------------------------------------
# Dispatch
# --------------------------------------------------------------------------

#: Canonical instruction name → handler. The `*_award` spellings arrive as their
#: `*_project` equivalents, so dispatching on the canonical name handles both
#: (§2).
#:
#: Anything absent is answered with `OpenPortalUnsupportedCommandError`, which
#: is a legitimate answer: a portal implements as much of the contract as it has
#: answers for (§4.0). `get_users` is deliberately absent - members travel in
#: `AwardDetails.members` instead.
HANDLERS = {
    "create_project": create_award,
    "update_project": update_award,
    "remove_project": remove_award,
    "get_project": get_award,
    "get_award": get_award,
    "get_awards": get_awards,
    "get_projects": get_projects,
    "get_project_mapping": get_project_mapping,
    "get_usage_report": get_usage_report,
    "get_usage_reports": get_usage_reports,
    "get_storage_report": get_storage_report,
    "get_storage_reports": get_storage_reports,
}


def _authorise(job: openportal.Job) -> None:
    """
    Check the request came in through an offering we actually advertise.

    Note what this is *not* doing. It is not deciding whether the caller may see
    a particular award - the offering is not a permission, it is which resource
    is being talked about, and every handler scopes itself by it via
    `_offering_of`. This only refuses a name we do not offer at all, which
    should never happen: the portal agent only forwards requests for offerings
    we registered. It is here as a backstop, not as the access control.

    `forwarded_for` is set by our own portal agent and not by the caller, which
    is why it is the field worth trusting (§1.2). Its first element is the
    portal that asked; its last is the offering they came in through.
    """
    offering = _offering_of(job)

    if offering not in OFFERINGS:
        raise openportal.ManagedProjectRejectedError(
            f"offering '{offering}' is not advertised by this portal"
        )


def answer(job: openportal.Job) -> openportal.Job:
    """
    Run one job and return the answered job, without sending it.

    Split out from `handle` so it can be tested without a bridge - see
    `test_portal.py`, which drives every handler through here.
    """
    command = job.instruction.command

    try:
        handler = HANDLERS.get(command)

        if handler is None:
            raise openportal.OpenPortalUnsupportedCommandError(
                f"this portal does not implement '{command}'"
            )

        _authorise(job)
        result = handler(job)
        logger.info("%s %s -> %s", command, job.id, type(result).__name__)
        return job.completed(result)

    except openportal.OpenPortalError as e:
        # An expected failure, already the right class. `errored` encodes the
        # class into the message so the awarding portal recovers it.
        logger.info("%s %s -> %s", command, job.id, type(e).__name__)
        return job.errored(e)

    except Exception as e:  # noqa: BLE001 - deliberately broad, see below
        # A bug in this portal. Still answered, so the caller learns that
        # something went wrong instead of waiting for the job to expire.
        logger.exception("unhandled error in %s", command)
        return job.errored(openportal.OpenPortalError(f"internal error: {e}"))


def handle(job: openportal.Job) -> None:
    """Run one job and post its result. **Always posts something.**"""
    _send(answer(job))


def _send(job: openportal.Job) -> None:
    """
    Post a result, retrying a few times.

    A failure here loses the answer entirely, so it is worth more than one
    attempt - waldur-mastermind retries five times at one-second intervals.
    """
    for attempt in range(5):
        try:
            openportal.send_result(job)
            return
        except OSError as e:
            logger.warning("send_result failed (attempt %d/5): %s", attempt + 1, e)
            time.sleep(1)

    logger.error("gave up sending the result for job %s", job.id)
