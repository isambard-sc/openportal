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

#: The templates a resource accepts if the operator does not say. One, called
#: `standard`, so that adding a cluster and creating an award on it needs no
#: further decisions.
DEFAULT_TEMPLATES = ("standard",)


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


def add_offering(name: str, templates: list[str] | None = None) -> store.Offering:
    """
    Start advertising a resource, or change the templates it accepts.

    Raises `ValueError` on a name that could not survive being part of a
    destination, or on an empty template list - an offering that accepts no
    template would reject every award made through it, which is a
    misconfiguration rather than a policy.
    """
    if not OFFERING_NAME.match(name or ""):
        raise ValueError(
            f"'{name}' is not a usable offering name: 1-64 characters of "
            "A-Z, a-z, 0-9, '_' or '-', not starting with '-'"
        )

    templates = list(templates) if templates else list(DEFAULT_TEMPLATES)

    if not all(t and t.strip() for t in templates):
        raise ValueError("a template name cannot be empty")

    return store.add_offering(name, templates, datetime.date.today())


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

    if str(details.project_template) not in templates_for(offering):
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
                daily.add_usage(email, openportal.Usage.from_hours(float(hours)))

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
