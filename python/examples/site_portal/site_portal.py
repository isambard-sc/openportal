# SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
# SPDX-License-Identifier: MIT
"""
The contract: what this portal answers, and how.

This is the file to read. Everything OpenPortal asks of a site portal is
here, one function per instruction, in the order
`docs/specifications/site-portal-api.md` §4 lists them.

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
import re
import time

import openportal

import store

logger = logging.getLogger(__name__)


# --------------------------------------------------------------------------
# What this portal offers
# --------------------------------------------------------------------------

# An offering is a **virtual agent** on this portal: a name the awarding portal
# addresses directly, standing for one resource we run. On the wire it is
# written `<offering>.<us>.<them>` - the resource, offered by us, to them - and
# `app.py` registers the full paths with the bridge.
#
# **The offering is part of an award's identity, not a permission check.** An
# award created through `cluster1` is an award *on `cluster1`*. The same awarding
# portal can hold a different award of the same name on `cluster2`, and asking
# one resource about a project that lives on the other gets nothing back - see
# `_offering_of` and `build_usage_report` below. Which is why running this with
# two resources teaches more than running it with one.
#
# The set lives in `store.py` rather than in a constant here, because it is
# state: a site procures a cluster, retires one, or opens one to a second
# awarding portal, and none of those are code changes. `app.py` exposes
# add/remove/list endpoints over these functions, and re-registers the set with
# OpenPortal whenever it changes. A fresh portal therefore offers **nothing**
# until an operator adds a resource - which is not a misconfiguration, just a
# site that cannot be asked for anything yet (§1.1).

#: What an offering may be called. It becomes one element of a `Destination`, so
#: it is what the grammar allows for an agent name and nothing more - checked
#: here, at the point an operator types it, rather than failing later inside a
#: destination nobody is looking at.
OFFERING_NAME = re.compile(r"^[A-Za-z0-9_][A-Za-z0-9_-]{0,63}$")

def offerings() -> list[store.Offering]:
    """Every resource we advertise, in name order."""
    return list(store.load_offerings().values())


def offering_names() -> list[str]:
    """Just the names, which is what most callers want."""
    return sorted(store.load_offerings())


def templates_for(offering: str) -> set[str]:
    """The templates one resource accepts - empty if we do not offer it."""
    found = store.load_offerings().get(offering)

    return set(found.templates) if found else set()


# --------------------------------------------------------------------------
# Units: what a figure in a usage report means
# --------------------------------------------------------------------------

# An award is for a *quantity*, and the two portals do not have to count in the
# same thing. The awarding portal allocates in its unit; this site accounts in
# its own; the two agree a factor between them, once, out of band.
#
#     N allocator units awarded  ->  M site units to spend here
#     X site units used          ->  Y allocator units reported back
#
# That is the whole of it, and it is deliberately the whole of it. How a site
# turns its own unit into real hardware - cores, GPUs, memory, a scheduler's
# billing weight - is the site's own business logic and no part of this
# contract. Nor are the units necessarily hours: they are numbers with an agreed
# name, and one day they may be money or cloud credits without any of the logic
# above changing.
#
# Converting on the way out is the same kind of act as remapping the
# identifiers, and it happens in the same place for the same reason: a report is
# built from what we recorded and then translated into what the other portal
# understands.

#: The unit this site accounts in - what the figures pushed to
#: `PUT /projects/{id}/usage` are in, and what its own records hold.
#:
#: Node hours here, and worth being honest about how notional that is: a real
#: site with heterogeneous clusters may measure a scheduler billing unit
#: underneath and present a *hypothetical* node-hour equivalent to its users. The
#: contract does not care. It needs one unit, named, that every figure this
#: portal reports is expressed in.
SITE_UNIT = "NHR"


def _canonical(unit: str) -> str:
    """
    A unit name in the spelling `Allocation` uses.

    `Allocation` canonicalises the ones it knows - "gpu hours", "GPUhr" and
    `GPUHR` are one unit - and passes anything else through lower-cased. Both
    sides of every comparison below go through this, so an agreed unit of
    `CREDITS` matches an award allocated in `credits`.
    """
    return openportal.Allocation.canonicalize(unit or "")


def conversions_for(offering: str) -> dict[str, float]:
    """
    The agreed factors for one resource: allocator unit → how many of them one
    `SITE_UNIT` is worth.

    `{"GPUHR": 4.0}` reads "one of our node hours is four of their GPU hours",
    so an award of 5000 GPUHR is 1250 node hours to spend here, and 12.5 node
    hours used is 50 GPU hours to report back.

    Per-resource, because the agreement is: a node hour on a GPU cluster and a
    node hour on a CPU cluster are not worth the same credit. Our own unit is
    always in the table at 1.0 - if an awarding portal allocates in the unit we
    already count in, there is nothing to agree.
    """
    found = store.load_offerings().get(offering)
    agreed = dict(found.conversions) if found else {}

    return {_canonical(SITE_UNIT): 1.0} | {
        _canonical(unit): float(factor) for unit, factor in agreed.items()
    }


def converter_for(offering: str, allocation: openportal.Allocation | None):
    """
    Return a function turning a figure in *our* unit into the award's unit.

    **This is what decides what the numbers in a usage report mean.** A `Usage`
    is a bare number; nothing in it says whether 50 is 50 node hours or 50 GPU
    hours. The unit is the one the awarding portal allocated in, so a site that
    reports its own figures unconverted is not reporting slightly differently -
    it is reporting a different quantity under the same name, and nothing on the
    wire will catch it.

    Returns `None` when there is no agreed factor, which the caller must treat as
    "we cannot hold this award" rather than as zero or as one-for-one. Guessing
    1.0 would silently report a quarter of the usage; guessing 0 would report
    none. There is no safe default for a number whose meaning was never agreed.
    """
    if allocation is None or allocation.is_empty:
        # No allocation, so no unit was declared, so there is nothing to convert
        # to and our own figures are the only sensible reading of them.
        return lambda hours: openportal.Usage.from_hours(hours)

    factor = conversions_for(offering).get(_canonical(str(allocation.units or "")))

    if not factor:
        return None

    return lambda hours: openportal.Usage.from_hours(hours * factor)


def to_site_units(offering: str, allocation: openportal.Allocation | None) -> float | None:
    """
    The other direction: what an award is worth here, in `SITE_UNIT`.

    The same agreed factor, divided rather than multiplied. This is the number a
    site actually enforces against - a quota, a budget, a limit in its own
    scheduler - and it is worth showing because it makes the round trip visible:
    5000 of their units in, 1250 of ours to spend, and every report back
    multiplied by four again.
    """
    if allocation is None or allocation.is_empty or allocation.size is None:
        return None

    factor = conversions_for(offering).get(_canonical(str(allocation.units or "")))

    if not factor:
        return None

    return float(allocation.size) / factor


#: The fields describing one node. `cpus` and `cores_per_cpu` together give the
#: core count, so a site asked for core hours needs both.
NODE_FIELDS = ("cpus", "cores_per_cpu", "gpus", "memory_gb", "billing")


def add_offering(
    name: str,
    templates: list[str],
    conversions: dict[str, float] | None = None,
) -> store.Offering:
    """
    Start advertising a resource, or change its templates or agreed conversions.

    **`templates` is required, and there is deliberately no default.** What a
    resource can be asked for is a decision about that resource - which
    organisation, billing and default offerings a project on it is created with -
    and nobody but the site knows it. A default would be a guess published under
    the site's name, and the awarding portal has no way to tell a guess from a
    policy: it would simply see the template accepted and make awards against
    it.

    `conversions` records what the two portals agreed each of this site's units
    is worth in an awarding portal's: `{"GPUHR": 4}` means one node hour here is
    four of their GPU hours. It is optional, and omitting it is a position rather
    than an oversight - a resource with no agreed factor can only hold awards
    allocated in this site's own unit, and any other is refused when it arrives.
    An omitted `conversions` on a later call keeps what was already agreed, so
    templates can be changed without restating it.

    Raises `ValueError` on a name that could not survive being part of a
    destination, on an empty or blank template list - an offering that
    accepts no template rejects every award made through it, which is a
    misconfiguration rather than a policy - and on a factor that is not a
    positive number.
    """
    if not OFFERING_NAME.match(name or ""):
        raise ValueError(
            f"'{name}' is not a usable offering name: 1-64 characters of "
            "A-Z, a-z, 0-9, '_' or '-', not starting with '-'"
        )

    templates = [t.strip() for t in (templates or []) if t and t.strip()]

    if not templates:
        raise ValueError(
            f"name at least one template that awards on '{name}' may use, "
            'e.g. ["standard", "large"] - there is no default'
        )

    if conversions is not None:
        checked = {}

        for unit, factor in conversions.items():
            try:
                factor = float(factor)
            except (TypeError, ValueError):
                raise ValueError(
                    f"the conversion for '{unit}' must be a number, not {factor!r}"
                )

            # Zero would report every award in that unit as having used nothing;
            # a negative or infinite factor is not a quantity at all. Neither is
            # a thing to store and discover later in a report.
            if not factor > 0 or factor == float("inf"):
                raise ValueError(
                    f"the conversion for '{unit}' must be a positive number, "
                    f"not {factor}"
                )

            checked[str(unit).strip()] = factor

        conversions = checked

    return store.add_offering(name, templates, datetime.date.today(), conversions)


def remove_offering(name: str) -> store.Offering | None:
    """
    Stop advertising a resource; returns what was removed, or `None`.

    The awards on it are kept - see `store.remove_offering` for why that is not
    laziness.
    """
    return store.remove_offering(name)


# --------------------------------------------------------------------------
# Small helpers
# --------------------------------------------------------------------------


def _local_project(award: store.Award) -> openportal.ProjectIdentifier:
    """
    Our own `ProjectIdentifier` for an award, which only an award that is
    *currently attached* to a project of ours has.
    """
    if award.local_project_id is None:
        raise openportal.OpenPortalError(
            f"{award.project_id} is not attached to a project here"
        )

    return openportal.ProjectIdentifier(award.local_project_id)


def _mapping(award: store.Award) -> openportal.ProjectMapping:
    """
    The `ProjectMapping` most award instructions return:
    `<their project id>:<our project id>`.

    **This is the whole point of the exchange.** The awarding portal knows the
    award as `myaward1.allocator`; we know the project we attached it to as
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

    # Approved once, but not attached to anything now - `remove_award` severed
    # it. "Pending", not "rejected": there is nothing wrong with the award and
    # an operator may attach it again, so the allocator should keep asking
    # rather than writing it off (§3.3, §4.1.2).
    if not award.is_attached:
        raise openportal.ManagedProjectPendingError(
            award.reason or "this award is not attached to a project"
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
    `create_award <project_id> <AwardDetails JSON>` (arrives as `create_project`)

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

    if str(details.project_template) not in templates_for(offering):
        raise openportal.ManagedProjectRejectedError(
            f"template '{details.project_template}' is not offered on {offering}"
        )

    # **Can we report usage for this award at all?**
    #
    # The allocation names the unit - "5000 GPUHR" - and that unit is what every
    # usage report for this award will be read in. If this resource cannot
    # express it, saying so now is the only honest answer: the alternative is to
    # accept the award and then answer every `get_usage_report` with a
    # well-formed zero (see `converter_for`).
    #
    # Terminal rather than pending, because no amount of asking again changes
    # what hardware we have. §3.3 lists an allocation we will never grant as a
    # legitimate rejection; this is the same shape of answer.
    if converter_for(offering, details.allocation) is None:
        agreed = ", ".join(sorted(conversions_for(offering))) or SITE_UNIT
        raise openportal.ManagedProjectRejectedError(
            f"no agreed conversion between '{details.allocation}' and this "
            f"site's {SITE_UNIT} on {offering} - it can hold awards in: {agreed}"
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

        # An award we previously detached, being asserted again. The allocator
        # still holds it, so it is asking us to attach it to a project - which
        # is a fresh decision for an operator, not something to resurrect on the
        # old project's behalf. Back to the pending queue it goes, and the
        # attachment history is left exactly as it is: the days it owned before
        # are still its days (§4.1.2).
        if award.state == store.Award.APPROVED and not award.is_attached:
            award.raw["state"] = store.Award.PENDING
            award.raw["reason"] = "awaiting re-attachment to a project"
            logger.info(
                "award %s on %s was re-asserted after removal - pending again",
                project_id,
                offering,
            )

        store.save(award)
        logger.debug("re-asserted award %s on %s", project_id, offering)

    return _answer_for_state(award)


def update_award(job: openportal.Job) -> openportal.ProjectMapping:
    """
    `update_award <project_id> <AwardDetails JSON>` (arrives as `update_project`)

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
    `remove_award <project_id>` (arrives as `remove_project`)

    **Disconnects an award from a project. It does not delete the project.**
    The answer is `<project_id>:None` - there is no longer a project attached to
    name (§4.1.2).

    What removal actually ends is the award's claim on *future* days. Billing is
    per-day, and a day belongs to whichever award the project was last attached
    to during it, so:

    * the day of removal still belongs to this award, unless another award is
      attached later the same day, in which case the whole day belongs to that
      one instead;
    * from the following day the project bills to nothing, until another award
      is attached.

    So this **keeps the record and the usage figures** and only stamps the
    detachment date. The days the award already owns still have to be
    reportable - the allocator has not necessarily collected the final ones yet,
    and it cannot ask a question we have destroyed the answer to. Deleting would
    also make the month report as empty, and an empty report is vacuously
    *complete*: we would be telling the allocator that nothing was ever used and
    that the figure is final.

    Removing an award we do not hold is *not* an error: the caller wants it
    gone, and it is gone. Being idempotent here means a retried removal does not
    produce a spurious failure. A second removal of an award already detached
    likewise does nothing - the first detachment date is the true one, and
    moving it later would hand the award days it did not own.
    """
    project_id = job.instruction.arguments[0]
    offering = _offering_of(job)

    # Only from this resource. An award of the same name on another offering is
    # a different award, and was not what the caller asked to remove.
    award = store.load(offering, project_id)

    if award is not None and award.is_attached:
        store.detach(offering, project_id, datetime.date.today())
        logger.info("detached award %s on %s", project_id, offering)

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
    shapes. An award with no project attached maps to `:None`, the same spelling
    `remove_award` uses.

    The test is `is_attached`, not the approval state. A detached award is still
    *approved* - it was approved once and that did happen - but it has no project
    attached now, so there is no identifier to put in a mapping. Keying this on
    the state instead would build a mapping for an award with nothing to map, and
    fail the whole listing rather than one entry (§4.1.2).
    """
    mappings = []

    for award in store.awards_on(_offering_of(job), job.instruction.arguments[0]):
        if award.is_attached:
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


def owner_of_day(
    awards: list[store.Award], local_project_id: str, date: datetime.date
) -> store.Award | None:
    """
    Which award a project's usage on `date` is billed to, or `None` for nobody.

    The rule (§4.1.2) is *the award the project was last attached to on that
    day*, and every part of that sentence is doing work:

    * **"last attached"** - if two awards were attached during the day, the
      later attachment takes the whole day, not just the part after the
      handover. Usage is accounted per day, so a day is indivisible; splitting
      it would need per-hour attribution that neither side keeps.
    * **"on that day"** - an award detached *during* the day was attached during
      it, so it stays a candidate. It stops being one from the next day on. This
      is why removal takes effect at most the day after.
    * **`None`** - a day on which the project was attached to nothing is billed
      to nothing. The usage is real and stays in our own accounting; there is
      simply no award for it to appear under, so it appears in no report.

    A consequence worth being explicit about: **a day's attribution is not
    settled until the day is over.** Attaching an award this afternoon changes
    who owns this morning. That is the deeper reason completeness is a decision
    rather than a calendar comparison, and the reason a day whose award changed
    has to be re-reported to both awards - the one that lost it needs to see it
    go.
    """
    owner = None
    owner_since = None

    for award in awards:
        for attachment in award.attachments:
            if attachment.project != local_project_id:
                # The same award may have been attached to several projects of
                # ours over its life. Only this project's episodes bill here.
                continue

            if not attachment.covers(date):
                continue

            if owner_since is None or attachment.since > owner_since:
                owner, owner_since = award, attachment.since

    return owner


def _could_own_any_of(
    award: store.Award, local_project_id: str, month: openportal.DateRange
) -> bool:
    """
    Whether `award` was attached to `local_project_id` at any point during
    `month`.

    Used to decide whether a month with no figures deserves an "incomplete, ask
    again" placeholder. A month wholly outside every attachment this award had
    on this project is a different case: the award owned nothing then and never
    will, so an empty and therefore complete answer is the truth rather than an
    accident.
    """
    for attachment in award.attachments:
        if attachment.project != local_project_id:
            continue

        if attachment.since > month.end_date:
            continue

        if attachment.to is None or attachment.to >= month.start_date:
            return True

    return False


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

    **On `is_complete`.** The allocator asks per month, and a report that comes
    back complete is one it will not ask for again. That makes completeness a
    claim about the future - "these figures will not change" - which only the
    site's operations team can make, so here it is driven by
    `store.Project.final_months` rather than inferred from the calendar. Until a
    month is declared final it is reported incomplete and keeps being asked
    for. See the README for what the allocator does with that.
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

    # Never attached to anything of ours, so there are no figures to find.
    # Note the test is the award's *history*, not whether it is attached now:
    # a removed award still owns the days it was attached for, and refusing to
    # report them - or reporting them as an empty, and therefore vacuously
    # complete, month - is how the final days of an award get silently lost
    # (§4.1.2).
    if award is None or not award.projects_ever_attached:
        return report

    # The identifier the answer is expressed in. Almost always the award's one
    # and only project; the most recent one if an operator has moved the award
    # between projects. Everything is remapped into the *awarding* portal's
    # namespace at the end, which is where usernames and emails end up, so this
    # choice affects only the intermediate form.
    local_project = openportal.ProjectIdentifier(award.projects_ever_attached[-1])

    # **The unit every figure below is expressed in.** Our accounting produced
    # them in `SITE_UNIT`; the award was allocated in whatever the awarding
    # portal chose, and that is what its reports mean. Read from the award we
    # hold rather than from the request, because the request does not carry it.
    #
    # Falling back to an identity conversion cannot happen for an award we
    # accepted - `create_award` refused any allocation we could not convert -
    # but a record predating that check would otherwise crash a report, and a
    # report is not the place to discover it.
    allocation = openportal.AwardDetails(json.dumps(award.details)).allocation
    convert = converter_for(offering, allocation)

    if convert is None:
        logger.error(
            "award %s on %s is allocated in '%s', which this site cannot "
            "account in - reporting our own %s unconverted",
            project_id,
            offering,
            allocation,
            SITE_UNIT,
        )
        convert = lambda hours: openportal.Usage.from_hours(hours)  # noqa: E731

    report = openportal.ProjectUsageReport(local_project)
    months_with_days: set[str] = set()
    final: set[str] = set()

    # The figures are the *project's*, and this award only claims days of them.
    # Every award ever attached to the same project is needed to work out which
    # days, because "the award last attached that day" is a question about the
    # whole attachment history and not about this award alone (§4.1.2).
    for local_id in award.projects_ever_attached:
        project = store.load_project(local_id)
        siblings = store.awards_for_local_project(local_id)
        final |= set(project.final_months)

        for iso_date, per_user in sorted(project.usage.items()):
            date = datetime.date.fromisoformat(iso_date)

            # **Whose day is this?** A day of this project's usage is billed to
            # the award it was last attached to during that day - which may be
            # a different award than the one being asked about, or none at all
            # if the project was unattached then. Either way it is not ours to
            # report, and reporting it anyway would bill it twice.
            owner = owner_of_day(siblings, local_id, date)

            if owner is None or _key(owner) != _key(award):
                continue

            daily = openportal.DailyProjectUsageReport()

            for email, hours in per_user.items():
                # `add_mapping` records which portal user each local name
                # belongs to. At the portal layer the local name is the
                # member's email.
                user = openportal.UserIdentifier(f"{_username(email)}.{local_project}")
                report.add_mapping(
                    openportal.UserMapping(f"{user}:{email}:{local_project}")
                )
                # ...and the figure is converted out of our unit into the one
                # the award was allocated in. `create_award` refused any award
                # we could not do this for, so `convert` is never None here.
                daily.add_usage(email, convert(float(hours)))

            # Completeness is a *decision*, not a date comparison. A day is
            # reported complete only when the site has declared its month final
            # - see `store.Project.final_months`. Guessing from the calendar
            # ("the day has passed, so it must be settled") claims the figures
            # will not change, which nobody but the operations team can know.
            month = _month_key(date)
            months_with_days.add(month)

            if month in final:
                daily.set_complete()

            report.set_report(date, daily)

    # A month this award could still own days in, but that we have no data for,
    # needs an explicit, incomplete placeholder.
    #
    # `ProjectUsageReport.is_complete` is "every day I contain is complete",
    # which is vacuously **true** for a report containing no days at all. So a
    # month we have simply not ingested yet would otherwise answer "nothing was
    # used, and that is final" - and the allocator would believe it and stop
    # asking. A zero-usage day that is *not* marked complete says the honest
    # thing instead: nothing so far, ask again.
    for month_range in date_range.months:
        month = _month_key(month_range.start_date)

        if month in final or month in months_with_days:
            continue

        # A month wholly outside this award's attachment window is a different
        # case, and an empty report for it is *correct*: this award owned
        # nothing then and never will, so "nothing, and that is final" is the
        # truth rather than an accident. Only months the award could still be
        # billed days in get a placeholder.
        if not any(
            _could_own_any_of(award, local_id, month_range)
            for local_id in award.projects_ever_attached
        ):
            continue

        # `date_range.months` yields whole calendar months, so a month's first
        # day can fall before the range that was actually asked about - and the
        # `filter` at the end of this function would drop the placeholder
        # again, putting the vacuous-complete answer straight back. Anchor it
        # inside the requested range instead.
        anchor = max(month_range.start_date, date_range.start_date)

        report.set_report(anchor, openportal.DailyProjectUsageReport())

    # Now translate the whole report into the awarding portal's namespace. This
    # is the mapping being used: they asked about `myaward1.allocator`, so that is
    # what the answer must be about. `remap_project` rewrites the project and
    # rebuilds every `UserIdentifier` with it, turning `alice.myproject1.site` into
    # `alice.myaward1.allocator` - the member's email is unchanged, because that is
    # the same person either way.
    report.remap_project(openportal.ProjectIdentifier(project_id))

    return report.filter(date_range)


def _month_key(date: datetime.date) -> str:
    """`"YYYY-MM"` - how a month is named in `store.Project.final_months`."""
    return f"{date.year:04d}-{date.month:02d}"


def _key(award: store.Award) -> tuple[str, str]:
    """
    An award's identity: the offering *and* the identifier (§1.3).

    Records are read fresh from disk on every access, so two objects describing
    the same award are different objects - identity has to be compared on the
    key rather than with `is`.
    """
    return (award.offering, award.project_id)


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

#: Every instruction this portal answers, keyed on the command name as it
#: arrives.
#:
#: Anything absent is answered with `OpenPortalUnsupportedCommandError`, which
#: is a legitimate answer: a portal implements as much of the contract as it has
#: answers for (§4.0). `get_users` is deliberately absent - members travel in
#: `AwardDetails.members` instead.
#:
#: **Both spellings of the award instructions are here, deliberately.** An
#: awarding portal sends `create_award`; the agents currently deliver it under
#: its original name, `create_project`, and that is what you see in a job's
#: `command` field today. The wire vocabulary is moving to the `*_award`
#: spellings (and the attach/detach pair may end up named for what they actually
#: do) before 1.0, so a table keyed on only one of the two will start answering
#: `OpenPortalUnsupportedCommandError` on the day it changes. Keying on both
#: costs three entries and spans the change - and since each pair is one
#: instruction under two names, they share a handler rather than duplicating it.
HANDLERS = {
    # Attaching an award to a project, and detaching it again.
    "create_award": create_award,
    "create_project": create_award,
    "update_award": update_award,
    "update_project": update_award,
    "remove_award": remove_award,
    "remove_project": remove_award,
    # Reading awards back.
    "get_award": get_award,
    "get_project": get_award,
    "get_awards": get_awards,
    "get_projects": get_projects,
    "get_project_mapping": get_project_mapping,
    # Accounting.
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

    if offering not in offering_names():
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
