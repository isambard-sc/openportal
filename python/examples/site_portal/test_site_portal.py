# SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
# SPDX-License-Identifier: MIT
"""
Exercise every handler without a bridge, an agent or a network.

Run it with `python test_portal.py`, or under pytest.

This is here for two reasons. It proves the example actually works rather than
merely looking as though it should - and it shows that **the contract is testable
in isolation**. A `Job` deserialises from JSON, and `site_portal.answer()` returns the
answered job, so every instruction can be driven from a fixture. Anyone writing
a real portal should do this: it is much faster than round-tripping through two
agent networks, and it catches the cases that are awkward to provoke live, like
an award stuck pending or an instruction you do not implement.
"""

from __future__ import annotations

import datetime
import json
import logging
import tempfile
from pathlib import Path

import openportal

import site_portal
import store

# The cast. `allocator` is the awarding portal, `site` is us, and `cluster1` and
# `cluster2` are two resources we offer - virtual agents on this portal that
# `allocator` addresses directly.
#
# Two awards, on two resources, each mapped to a project of ours:
#
#     allocator.site.cluster1   myaward1.allocator  ->  myproject1.site
#     allocator.site.cluster2   myaward2.allocator  ->  myproject2.site
#
# Most checks below use the first pair. `cluster2` is here because with one
# resource the most important property of an offering is invisible: an award
# lives on one resource, and the other knows nothing about it.

#: What the awarding portal calls the award we mostly work with.
AWARD = "myaward1.allocator"

#: The project we attach it to. The two halves of the mapping.
LOCAL_PROJECT = "myproject1.site"

#: A second award, on the other resource.
AWARD2 = "myaward2.allocator"
LOCAL_PROJECT2 = "myproject2.site"

OFFERING = "cluster1"
OTHER_OFFERING = "cluster2"


def make_job(
    instruction: str,
    *,
    offering: str = OFFERING,
    forwarded_for: str | None = None,
) -> openportal.Job:
    """
    A pending bridge-board job, as `fetch_job` would return it.

    The destination ends with the offering - `site.bridge.cluster1` - which
    is how the portal knows which resource the request is about. `forwarded_for`
    carries the awarding portal's original `allocator.site.cluster1` when there is
    one, and takes precedence.

    Note `state` is capitalised on the wire (`"Pending"`, not `"pending"`) -
    Python lower-cases it only for display.
    """
    return openportal.Job.from_json(
        json.dumps(
            {
                "id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
                "created": 1700000000,
                "changed": 1700000000,
                "expires": 4000000000,
                "version": 1,
                "command": f"site_portal.bridge.{offering} {instruction}",
                "state": "Pending",
                "result": None,
                "result_type": None,
                "forwarded_for": forwarded_for,
            }
        )
    )


def details(template: str = "standard", **extra) -> str:
    """An `AwardDetails` JSON blob, as an awarding portal would send it."""
    d = openportal.AwardDetails()
    d.name = extra.pop("name", "My Project")
    d.project_template = openportal.ProjectTemplate(template)
    d.add_member("alice@bristol.ac.uk", "member")
    for key, value in extra.items():
        setattr(d, key, value)
    return d.to_json()


def portal_answer_mapping(award: str, offering: str = None):
    """The `ProjectMapping` this site currently answers for one award."""
    job = make_job(
        f"get_project_mapping {award}",
        offering=offering or OFFERING,
    )
    return site_portal.answer(job).result


def approve(award: store.Award, local: str, on: datetime.date = None) -> store.Award:
    """Attach an award to one of our projects, as the operator API does."""
    return store.attach(award, local, on or datetime.date.today())


def set_usage(local: str, usage: dict) -> None:
    """Record usage against one of *our* projects, as the operator API does."""
    project = store.load_project(local)
    project.raw["usage"] = usage
    store.save_project(project)


def set_final(local: str, months: list) -> None:
    """Declare months final on one of our projects."""
    project = store.load_project(local)
    project.raw["final_months"] = months
    store.save_project(project)


def check(name: str, condition: bool, detail: str = "") -> None:
    if not condition:
        raise AssertionError(f"{name} FAILED {detail}")
    print(f"  {name:52} OK {detail}")


def run() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        store.STATE_DIR = Path(tmp)
        _run_all()


def _run_all() -> None:
    print("\n-- what this site offers ---------------------------------------")

    # Offerings are state, not a constant, so a fresh portal offers nothing and
    # the suite starts by adding the two resources it works with. This is the
    # same call `POST /offerings` makes.
    check("a fresh portal offers nothing", site_portal.offering_names() == [])

    site_portal.add_offering(OFFERING, ["standard", "large"])
    site_portal.add_offering(OTHER_OFFERING, ["standard"])
    check(
        "both resources are now offered",
        site_portal.offering_names() == [OFFERING, OTHER_OFFERING],
        f"-> {site_portal.offering_names()}",
    )
    check(
        "each carries its own templates",
        site_portal.templates_for(OFFERING) == {"standard", "large"}
        and site_portal.templates_for(OTHER_OFFERING) == {"standard"},
    )

    # Adding one twice is an update, not an error - the operator API is retried
    # like everything else here.
    site_portal.add_offering(OTHER_OFFERING, ["standard", "small"])
    check(
        "re-adding a resource updates its templates",
        site_portal.templates_for(OTHER_OFFERING) == {"standard", "small"},
    )
    site_portal.add_offering(OTHER_OFFERING, ["standard"])
    check("still two resources", len(site_portal.offerings()) == 2)

    for bad in ["", "-lead", "my.cluster", "my cluster", "caf\u00e9"]:
        try:
            site_portal.add_offering(bad, ["standard"])
            raise AssertionError(f"{bad!r} should not be a usable offering name")
        except ValueError:
            pass
    check("a name that could not be part of a destination is refused", True)

    # There is no default template. A resource that accepts none would reject
    # every award made through it, and guessing one on the site's behalf would
    # publish a policy nobody decided.
    for no_templates in [[], [""], ["  "], None]:
        try:
            site_portal.add_offering("cluster9", no_templates)
            raise AssertionError(f"{no_templates!r} should not be accepted")
        except ValueError:
            pass
    check("a resource with no templates is refused", "cluster9" not in site_portal.offering_names())

    print("\n-- create_award, and the pending answer -------------------------")

    # A new award is recorded and answers "not yet", because a human has not
    # looked at it. This is the normal first response, not a fault.
    # Dates the whole suite works in. The first of the current month is used
    # both as the day the award is attached and as the day usage lands on, so
    # every figure is inside `this_month` whenever the suite is run - and inside
    # the award's attachment window, which is what decides whether the award is
    # billed for it at all (§4.1.2).
    today = datetime.date.today()
    month_start = today.replace(day=1)
    a_day_this_month = month_start.isoformat()
    this_month_key = f"{today.year:04d}-{today.month:02d}"

    job = site_portal.answer(make_job(f"create_project {AWARD} {details()}"))
    check("new award is answered with an error", job.is_error)
    check(
        "...and the error is ManagedProjectPendingError",
        type(job.error) is openportal.ManagedProjectPendingError,
        f"-> {job.error}",
    )
    check("...carrying the award_pending kind", job.error_kind == "award_pending")
    check(
        "the award was recorded on that offering",
        store.load(OFFERING, AWARD) is not None,
    )

    print("\n-- idempotency: the same create arrives again -------------------")

    # The awarding portal re-sends this every cycle. It must not create a
    # second award, and must answer the same way.
    again = site_portal.answer(make_job(f"create_project {AWARD} {details()}"))
    check(
        "a repeated create answers the same",
        type(again.error) is openportal.ManagedProjectPendingError,
    )
    check("still exactly one award", len(store.all_awards()) == 1)

    print("\n-- a rejected template -----------------------------------------")

    bad = site_portal.answer(make_job(f"create_project myaward5.allocator {details('gpu-mega')}"))
    check(
        "an unoffered template is rejected terminally",
        type(bad.error) is openportal.ManagedProjectRejectedError,
        f"-> {bad.error}",
    )
    check("...carrying the award_rejected kind", bad.error_kind == "award_rejected")

    print("\n-- approval names the project on our side ----------------------")

    # Approving is what attaches the award to a project of ours - newly created
    # or pre-existing. That project's identifier is our half of the mapping.
    award = store.load(OFFERING, AWARD)
    approve(award, LOCAL_PROJECT, month_start)
    store.save(award)

    job = site_portal.answer(make_job(f"create_project {AWARD} {details()}"))
    check("an approved award answers with a mapping", not job.is_error)
    check(
        "...pairing their award id with our project id",
        str(job.result) == f"{AWARD}:{LOCAL_PROJECT}",
        f"-> {job.result}",
    )
    check("...typed as a ProjectMapping", type(job.result) is openportal.ProjectMapping)
    check(
        "...their half is the award they asked about",
        str(job.result.project) == AWARD,
    )
    check(
        "...our half is a project in our own namespace",
        str(job.result.local_group) == LOCAL_PROJECT,
        f"-> {job.result.local_group}",
    )

    # The reverse lookup: our accounting knows only our identifier.
    check(
        "an award can be found by our identifier for it",
        store.load_by_local_id(LOCAL_PROJECT).project_id == AWARD,
    )

    print("\n-- what a project name may contain -----------------------------")

    # The operator supplies only the project's own name; `.site` is added for
    # them. These are the rules that name has to satisfy - one component of a
    # ProjectIdentifier - and they are wider than they look: uppercase,
    # underscore and hyphen are all allowed.
    for name in ("myproject1", "MyProject_1", "my-project", "P1", "a" * 64):
        try:
            openportal.ProjectIdentifier(f"{name}.site")
        except OSError as e:  # pragma: no cover - a failure here is the point
            raise AssertionError(f"{name!r} should be a usable project name: {e}")
    check("valid project names are accepted", True, "A-Za-z0-9_- , 1-64 chars")

    for name, why in (
        ("-lead", "leading dash"),
        ("my project", "space"),
        ("my,project", "comma"),
        ("", "empty"),
        ("a" * 65, "too long"),
        ("caf\u00e9", "non-ascii"),
    ):
        try:
            openportal.ProjectIdentifier(f"{name}.site")
            raise AssertionError(f"{name!r} ({why}) should have been rejected")
        except OSError:
            pass
    check("invalid ones are rejected", True, "dash-first, space, comma, empty, >64, non-ascii")

    print("\n-- an award can be moved to a different project -----------------")

    # An award is *attached* to a project, not identical to one, so the operator
    # can change which project holds it. The mapping follows, and the project it
    # leaves behind becomes available again.
    moved = store.load(OFFERING, AWARD)
    approve(moved, "elsewhere.site")
    store.save(moved)

    check(
        "moving the award moves the mapping",
        str(portal_answer_mapping(AWARD)) == f"{AWARD}:elsewhere.site",
        f"-> {portal_answer_mapping(AWARD)}",
    )
    check(
        "...and the award is now found by the new identifier",
        store.load_by_local_id("elsewhere.site").project_id == AWARD,
    )
    check(
        "...while the old one holds nothing",
        store.load_by_local_id(LOCAL_PROJECT) is None,
    )

    # Put it back for the rest of the checks.
    approve(moved, LOCAL_PROJECT, month_start)
    store.save(moved)

    print("\n-- get_award ---------------------------------------------------")

    job = site_portal.answer(make_job(f"get_award {AWARD}"))
    check("get_award returns AwardDetails", type(job.result) is openportal.AwardDetails)
    check(
        "...with members populated (there is no get_users)",
        "alice@bristol.ac.uk" in (job.result.members or {}),
        f"-> {list((job.result.members or {}).keys())}",
    )

    print("\n-- update_award for an award we do not hold ---------------------")

    job = site_portal.answer(make_job(f"update_project myaward4.allocator {details()}"))
    check(
        "an update for an unknown award becomes a create",
        type(job.error) is openportal.ManagedProjectPendingError,
    )
    check(
        "...and the award now exists",
        store.load(OFFERING, "myaward4.allocator") is not None,
    )
    store.delete(OFFERING, "myaward4.allocator")

    print("\n-- usage: recorded in our namespace, answered in theirs --------")

    # Push figures in the way an operator's parser would - against our own
    # project identifier, which is the only one that accounting knows.
    award = store.load(OFFERING, AWARD)
    set_usage(LOCAL_PROJECT, {a_day_this_month: {"alice@bristol.ac.uk": 12.5}})

    job = site_portal.answer(make_job(f"get_usage_report {AWARD} this_month"))
    check("get_usage_report succeeds", not job.is_error, f"{job.error_message}")
    report = job.result
    check("...returns a ProjectUsageReport", type(report) is openportal.ProjectUsageReport)
    check(
        "...with the pushed figure in it",
        abs(report.total_usage.seconds / 3600 - 12.5) < 0.01,
        f"-> {report.total_usage}",
    )

    # The translation: the caller asked about their award, so the report must be
    # about their award - even though the figures were recorded against ours.
    check(
        "...reported against THEIR project identifier",
        str(report.project) == AWARD,
        f"-> {report.project}",
    )
    check(
        "...with user identifiers rebuilt into their namespace",
        all(str(u).endswith(f".{AWARD}") for u in report.users),
        f"-> {[str(u) for u in report.users]}",
    )
    check(
        "...and the email unchanged, being the same person either way",
        "alice@bristol.ac.uk" in report.user_mapping.values(),
        f"-> {list(report.user_mapping.values())}",
    )
    check(
        "...and NOT marked complete, because nobody has said the month is final",
        not report.is_complete,
        f"-> is_complete={report.is_complete}",
    )

    job = site_portal.answer(make_job(f"get_usage_reports award this_month"))
    check("get_usage_reports rolls up to the portal", not job.is_error)
    check("...as a UsageReport", type(job.result) is openportal.UsageReport)

    print("\n-- is_complete is an operations decision, not a calendar one ----")

    # `is_complete` is the allocator's signal that a month is settled and need
    # not be requested again. It is therefore a claim about the future, and the
    # only people who can make it are the ones who run the accounting - so the
    # example drives it from `final_months`, which an operator sets.
    check(
        "an un-finalised month reports incomplete",
        not site_portal.answer(
            make_job(f"get_usage_report {AWARD} this_month")
        ).result.is_complete,
    )

    award = store.load(OFFERING, AWARD)
    set_final(LOCAL_PROJECT, [this_month_key])

    report = site_portal.answer(
        make_job(f"get_usage_report {AWARD} this_month")
    ).result
    check("...and a finalised one reports complete", report.is_complete)
    check(
        "...with the figures unchanged by the decision",
        abs(report.total_usage.seconds / 3600 - 12.5) < 0.01,
        f"-> {report.total_usage}",
    )

    # Taking the decision back must work, or a late correction could never be
    # collected: the allocator only re-reads a month it has not been told is
    # final.
    set_final(LOCAL_PROJECT, [])
    check(
        "...and un-finalising makes it incomplete again",
        not site_portal.answer(
            make_job(f"get_usage_report {AWARD} this_month")
        ).result.is_complete,
    )

    # The trap this guards. `ProjectUsageReport.is_complete` is "every day I
    # contain is complete", which is vacuously **true** for a report holding no
    # days at all. So a month whose figures have simply not been ingested yet
    # would answer "nothing was used, and that is final" - and the allocator
    # would believe it and never ask again. An explicit incomplete placeholder
    # says the honest thing: nothing so far, ask again.
    empty = openportal.ProjectUsageReport(openportal.ProjectIdentifier(AWARD))
    check(
        "an empty report is vacuously complete - the trap being guarded",
        empty.is_complete,
    )

    set_usage(LOCAL_PROJECT, {})
    report = site_portal.answer(
        make_job(f"get_usage_report {AWARD} this_month")
    ).result
    check("a month with no data at all reports zero", report.total_usage.seconds == 0)
    check(
        "...and is NOT complete, so the allocator asks again",
        not report.is_complete,
        f"-> is_complete={report.is_complete}",
    )

    # ...but a month with no data that the site *has* declared final is
    # complete. "Nothing was used and that is settled" is a legitimate answer,
    # and it is the operator's to give.
    set_final(LOCAL_PROJECT, [this_month_key])
    check(
        "...unless the site declared it final, which is a legitimate answer",
        site_portal.answer(
            make_job(f"get_usage_report {AWARD} this_month")
        ).result.is_complete,
    )

    # A range that does not start on the 1st. `DateRange.months` yields whole
    # calendar months, so the placeholder has to be anchored inside the range
    # that was actually asked about - anchoring it on the month's first day
    # would put it outside, the final `filter` would drop it, and the vacuous
    # "empty means complete" answer would be back.
    set_final(LOCAL_PROJECT, [])
    set_usage(LOCAL_PROJECT, {})
    mid_month = f"{this_month_key}-10:{this_month_key}-20"
    report = site_portal.answer(
        make_job(f"get_usage_report {AWARD} {mid_month}")
    ).result
    check(
        "a partial range with no data is still incomplete",
        not report.is_complete,
        f"-> is_complete={report.is_complete} over {mid_month}",
    )

    # Put the award back the way the rest of the suite expects it.
    set_final(LOCAL_PROJECT, [])
    set_usage(LOCAL_PROJECT, {a_day_this_month: {"alice@bristol.ac.uk": 12.5}})

    print("\n-- a second award, on the other resource ------------------------")

    # The ordinary two-resource case: a different award, on a different
    # resource, mapped to a different project of ours. Nothing about it touches
    # the first, and both are live at once.
    site_portal.answer(
        make_job(f"create_project {AWARD2} {details()}", offering=OTHER_OFFERING)
    )
    second = store.load(OTHER_OFFERING, AWARD2)
    approve(second, LOCAL_PROJECT2, month_start)
    store.save(second)

    job = site_portal.answer(
        make_job(f"get_project_mapping {AWARD2}", offering=OTHER_OFFERING)
    )
    check(
        "the second award maps to its own project",
        str(job.result) == f"{AWARD2}:{LOCAL_PROJECT2}",
        f"-> {job.result}",
    )
    check(
        "...and the first is unaffected",
        str(site_portal.answer(make_job(f"get_project_mapping {AWARD}")).result)
        == f"{AWARD}:{LOCAL_PROJECT}",
    )
    check(
        "each resource lists only its own award",
        [str(m) for m in site_portal.answer(make_job("get_projects allocator")).result]
        == [f"{AWARD}:{LOCAL_PROJECT}"]
        and [
            str(m)
            for m in site_portal.answer(
                make_job("get_projects allocator", offering=OTHER_OFFERING)
            ).result
        ]
        == [f"{AWARD2}:{LOCAL_PROJECT2}"],
    )

    print("\n-- the same question asked of the wrong resource ----------------")

    # The award lives on cluster1. An awarding portal can perfectly well ask
    # cluster2 about it - same identifier, nothing stops the question - and the
    # honest answer is that nothing was used there, because the project is not
    # there. Empty, not an error: a caller sweeping every offering it knows
    # about should not be failed by the ones that hold nothing.
    job = site_portal.answer(
        make_job(f"get_usage_report {AWARD} this_month", offering=OTHER_OFFERING)
    )
    check("usage from the wrong offering succeeds", not job.is_error, f"{job.error_message}")
    check(
        "...and is empty, because the project is not on that resource",
        job.result.total_usage.seconds == 0,
        f"-> {job.result.total_usage}",
    )
    check(
        "...while the right offering still has the usage",
        site_portal.answer(
            make_job(f"get_usage_report {AWARD} this_month")
        ).result.total_usage.seconds > 0,
    )

    # An award is only visible through the resource it was created on.
    job = site_portal.answer(make_job(f"get_award {AWARD}", offering=OTHER_OFFERING))
    check("get_award from the wrong offering finds nothing", job.is_error, f"-> {job.error}")
    check(
        "...listings are scoped too, so it is absent from the other resource",
        not any(
            str(m).startswith(f"{AWARD}:")
            for m in site_portal.answer(
                make_job("get_projects allocator", offering=OTHER_OFFERING)
            ).result
        ),
    )
    check(
        "...while the right offering lists it",
        any(
            str(m).startswith(f"{AWARD}:")
            for m in site_portal.answer(make_job("get_projects allocator")).result
        ),
    )

    print("\n-- the same name on two resources is two awards -----------------")

    # A stronger case than the two awards above: the *same* name on both
    # resources. `myaward1.allocator` on cluster2 is a different award from
    # `myaward1.allocator` on cluster1, and creating it must not disturb the first.
    job = site_portal.answer(
        make_job(f"create_project {AWARD} {details()}", offering=OTHER_OFFERING)
    )
    check(
        "creating the same name on another resource is a new, pending award",
        type(job.error) is openportal.ManagedProjectPendingError,
    )
    check(
        "...the original is untouched and still approved",
        str(site_portal.answer(make_job(f"get_project_mapping {AWARD}")).result)
        == f"{AWARD}:{LOCAL_PROJECT}",
    )
    check(
        "...and they are stored as two separate awards",
        len([a for a in store.all_awards() if a.project_id == AWARD]) == 2,
    )

    # The two awards must not end up sharing one local project either. This is
    # the same mistake as everywhere else in this section - comparing only the
    # identifier and forgetting the offering - and it is worth a test because it
    # is so easy to write.
    other = store.load(OTHER_OFFERING, AWARD)
    check(
        "the two awards are distinct despite the shared name",
        (other.offering, other.project_id) != (OFFERING, AWARD)
        and other.project_id == AWARD,
    )
    check(
        "...and our identifier for the first is not free to reuse",
        store.load_by_local_id(LOCAL_PROJECT).offering == OFFERING,
    )

    # Removing from one resource leaves the other alone.
    site_portal.answer(make_job(f"remove_project {AWARD}", offering=OTHER_OFFERING))
    check(
        "removing from one resource leaves the other attached",
        store.load(OFFERING, AWARD).is_attached
        and not store.load(OTHER_OFFERING, AWARD).is_attached,
    )
    # And the record survives the removal, because the days it owned still have
    # to be reportable. Deleting it is the tempting mistake - see the
    # remove_award section below.
    check(
        "...and the removed award's record is kept, not deleted",
        store.load(OTHER_OFFERING, AWARD) is not None,
    )
    store.delete(OTHER_OFFERING, AWARD)

    print("\n-- a template this resource does not offer ----------------------")

    # Templates are per-resource: `large` is offered on cluster1 and not on
    # cluster2, because a template selects things that belong to the resource.
    job = site_portal.answer(
        make_job(f"create_project myaward6.allocator {details('large')}", offering=OTHER_OFFERING)
    )
    check(
        "a template offered elsewhere is rejected here",
        type(job.error) is openportal.ManagedProjectRejectedError,
        f"-> {job.error}",
    )
    job = site_portal.answer(make_job(f"create_project myaward6.allocator {details('large')}"))
    check(
        "...and accepted on the resource that offers it",
        type(job.error) is openportal.ManagedProjectPendingError,
    )
    store.delete(OFFERING, "myaward6.allocator")

    print("\n-- an award with no local project yet --------------------------")

    # An unapproved award has no project on our side, so there is nothing for
    # usage to have been recorded against. Empty, for the same reason as the
    # wrong-offering case above: nothing was used, and that is not a failure.
    pending = "myaward3.allocator"
    site_portal.answer(make_job(f"create_project {pending} {details()}"))
    job = site_portal.answer(make_job(f"get_usage_report {pending} this_month"))
    check("usage for an unapproved award is empty, not an error", not job.is_error)
    check("...and reports zero", job.result.total_usage.seconds == 0)
    check(
        "...while the mapping for it is still refused, being pending",
        site_portal.answer(make_job(f"get_project_mapping {pending}")).is_error,
    )
    check(
        "...but get_projects still lists it, as :None",
        any(
            str(m) == f"{pending}:None"
            for m in site_portal.answer(make_job("get_projects allocator")).result
        ),
    )
    store.delete(OFFERING, pending)

    print("\n-- storage: empty beats absent ---------------------------------")

    job = site_portal.answer(make_job(f"get_storage_report {AWARD} this_month"))
    check("get_storage_report succeeds rather than failing", not job.is_error)
    check("...with an empty report", job.result.is_empty())

    print("\n-- an instruction we do not implement ---------------------------")

    job = site_portal.answer(make_job(f"get_users {AWARD}"))
    check(
        "get_users is declined, not ignored",
        type(job.error) is openportal.OpenPortalUnsupportedCommandError,
        f"-> {job.error}",
    )
    check("...carrying the unsupported kind", job.error_kind == "unsupported")

    print("\n-- authorisation against forwarded_for -------------------------")

    job = site_portal.answer(
        make_job(f"get_award {AWARD}", forwarded_for=f"allocator.site.{OFFERING}")
    )
    check("a request through an advertised offering is allowed", not job.is_error)

    job = site_portal.answer(
        make_job(f"get_award {AWARD}", forwarded_for="allocator.site.not-ours")
    )
    check(
        "a request through an unknown offering is refused",
        type(job.error) is openportal.ManagedProjectRejectedError,
        f"-> {job.error}",
    )

    print("\n-- an award we have never heard of ------------------------------")

    job = site_portal.answer(make_job("get_award nosuch.allocator"))
    check("an unknown award is an error, not a rejection", job.is_error)
    check(
        "...recovered as the class it was raised as",
        type(job.error) is openportal.OpenPortalError,
        f"-> {type(job.error).__name__}",
    )
    check(
        "...and specifically not terminal",
        not isinstance(job.error, openportal.ManagedProjectPermissionError),
    )

    print("\n-- remove_award ------------------------------------------------")

    # `remove_award` disconnects an award from a project. It does not delete the
    # project, and - the part that is easy to get wrong - it must not delete the
    # usage the award already accrued.
    set_final(LOCAL_PROJECT, [])
    set_usage(LOCAL_PROJECT, {a_day_this_month: {"alice@bristol.ac.uk": 12.5}})

    job = site_portal.answer(make_job(f"remove_project {AWARD}"))
    check("remove_award answers with :None", str(job.result) == f"{AWARD}:None")
    check(
        "...and the award is no longer attached",
        not store.load(OFFERING, AWARD).is_attached,
    )
    check(
        "...but the record survives, because it still owns days",
        store.load(OFFERING, AWARD) is not None,
    )
    check(
        "...and the project is untouched - removal disconnects, it does not delete",
        store.load_project(LOCAL_PROJECT).usage != {},
    )

    # The failure this guards against. If removal had deleted the record, the
    # allocator's next request would get an empty report - and an empty report
    # is vacuously complete, so it would record "nothing was ever used, final"
    # and stop asking. The last days of every award would vanish.
    report = site_portal.answer(
        make_job(f"get_usage_report {AWARD} this_month")
    ).result
    check(
        "a removed award still reports the usage it accrued",
        abs(report.total_usage.seconds / 3600 - 12.5) < 0.01,
        f"-> {report.total_usage}",
    )
    check(
        "...and still says so incompletely, so the figures can still be corrected",
        not report.is_complete,
    )

    # A detached award is still *approved* - that did happen - but has nothing
    # attached, so listings must not try to build a mapping for it. Keying this
    # on the state rather than the attachment fails the whole call, not one row.
    job = site_portal.answer(make_job("get_projects allocator"))
    check("get_projects survives a detached award", not job.is_error, f"{job.error}")
    check(
        "...and reports it as :None",
        any(str(m) == f"{AWARD}:None" for m in job.result),
        f"-> {[str(m) for m in job.result]}",
    )

    # The project is free again, which is what lets an operator attach a
    # different award to it.
    check(
        "the project it left is available again",
        store.load_by_local_id(LOCAL_PROJECT) is None,
    )

    job = site_portal.answer(make_job(f"remove_project {AWARD}"))
    check("removing it twice is not an error", not job.is_error)

    # Re-asserting a removed award puts it back in the pending queue rather than
    # silently re-attaching it. Attaching is an operator's decision.
    job = site_portal.answer(make_job(f"create_project {AWARD} {details()}"))
    check(
        "re-asserting a removed award is pending again, not rejected",
        type(job.error) is openportal.ManagedProjectPendingError,
        f"-> {job.error}",
    )

    print("\n-- which award a day is billed to ------------------------------")

    # The rule (§4.1.2): a project's usage on a given day is billed to the award
    # it was last attached to *on that day*. Set up a handover part-way through
    # the month and check each side of it.
    store.delete(OFFERING, AWARD)
    for stale in store.all_awards():
        store.delete(stale.offering, stale.project_id)

    early = month_start
    handover = month_start + datetime.timedelta(days=10)

    site_portal.answer(make_job(f"create_project {AWARD} {details()}"))
    first = store.load(OFFERING, AWARD)
    approve(first, LOCAL_PROJECT, early)

    set_final(LOCAL_PROJECT, [])
    set_usage(
        LOCAL_PROJECT,
        {
            early.isoformat(): {"alice@bristol.ac.uk": 1.0},
            handover.isoformat(): {"alice@bristol.ac.uk": 2.0},
            (handover + datetime.timedelta(days=1)).isoformat(): {
                "alice@bristol.ac.uk": 4.0
            },
        },
    )

    # Before the handover, everything is the first award's.
    everything = f"{early.isoformat()}:{(handover + datetime.timedelta(days=1)).isoformat()}"
    check(
        "one award is billed every day it is attached for",
        abs(
            site_portal.answer(
                make_job(f"get_usage_report {AWARD} {everything}")
            ).result.total_usage.seconds
            / 3600
            - 7.0
        )
        < 0.01,
    )

    # Now the handover: the first award is removed and a second attached, both
    # on the same day.
    store.detach(OFFERING, AWARD, handover)
    site_portal.answer(make_job(f"create_project {AWARD2} {details()}"))
    second_award = store.load(OFFERING, AWARD2)
    approve(second_award, LOCAL_PROJECT, handover)

    outgoing = site_portal.answer(
        make_job(f"get_usage_report {AWARD} {everything}")
    ).result
    incoming = site_portal.answer(
        make_job(f"get_usage_report {AWARD2} {everything}")
    ).result

    check(
        "the whole handover day goes to the award attached last that day",
        abs(incoming.total_usage.seconds / 3600 - 6.0) < 0.01,
        f"-> {incoming.total_usage} (expected 2.0 + 4.0)",
    )
    check(
        "...and the outgoing award keeps only the days before it",
        abs(outgoing.total_usage.seconds / 3600 - 1.0) < 0.01,
        f"-> {outgoing.total_usage} (expected 1.0)",
    )
    check(
        "...so the project's usage is billed exactly once",
        abs(
            (outgoing.total_usage.seconds + incoming.total_usage.seconds) / 3600 - 7.0
        )
        < 0.01,
    )

    # And with nothing attached, from the first whole day onwards nothing is
    # billed to anybody.
    store.detach(OFFERING, AWARD2, handover)
    unbilled = site_portal.answer(
        make_job(f"get_usage_report {AWARD2} {everything}")
    ).result
    check(
        "a detached award keeps the day it was detached on",
        abs(unbilled.total_usage.seconds / 3600 - 2.0) < 0.01,
        f"-> {unbilled.total_usage} (expected the handover day only)",
    )
    check(
        "...and the day after it belongs to nobody",
        site_portal.owner_of_day(
            store.awards_for_local_project(LOCAL_PROJECT),
            LOCAL_PROJECT,
            handover + datetime.timedelta(days=1),
        )
        is None,
    )

    for stale in store.all_awards():
        store.delete(stale.offering, stale.project_id)

    print("\n-- withdrawing a resource --------------------------------------")

    # A resource can be retired. What that ends is its *reachability*: requests
    # for it are refused, because the virtual agent behind it is withdrawn too.
    site_portal.add_offering("cluster3", ["standard"])
    job = site_portal.answer(make_job(f"create_project temp.allocator {details()}", offering="cluster3"))
    check(
        "an award can be made on a newly added resource",
        type(job.error) is openportal.ManagedProjectPendingError,
        f"-> {job.error}",
    )

    check("removing it reports what went", site_portal.remove_offering("cluster3") is not None)
    check("removing it again reports nothing", site_portal.remove_offering("cluster3") is None)

    job = site_portal.answer(make_job("get_award temp.allocator", offering="cluster3"))
    check(
        "a request through a withdrawn resource is refused",
        type(job.error) is openportal.ManagedProjectRejectedError,
        f"-> {job.error}",
    )

    # ...and what it does *not* end is the record. Those days still have to be
    # reportable if the resource comes back (§4.1.2).
    check(
        "the awards on it are kept, not deleted",
        store.load("cluster3", "temp.allocator") is not None,
    )
    store.delete("cluster3", "temp.allocator")

    print("\n-- every job gets an answer ------------------------------------")

    # The structural guarantee: even a handler that blows up unexpectedly
    # produces an errored job rather than silence.
    broken = dict(site_portal.HANDLERS)
    site_portal.HANDLERS["get_award"] = lambda job: 1 / 0
    logging.disable(logging.CRITICAL)  # the traceback below is deliberate
    try:
        job = site_portal.answer(make_job("get_award anything.award"))
        check("an unexpected exception still answers", job.is_error)
        check("...as an OpenPortalError", isinstance(job.error, openportal.OpenPortalError))
    finally:
        logging.disable(logging.NOTSET)
        site_portal.HANDLERS.clear()
        site_portal.HANDLERS.update(broken)

    print("\nall checks passed\n")


if __name__ == "__main__":
    run()
