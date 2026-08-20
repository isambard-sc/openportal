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


def check(name: str, condition: bool, detail: str = "") -> None:
    if not condition:
        raise AssertionError(f"{name} FAILED {detail}")
    print(f"  {name:52} OK {detail}")


def run() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        store.STATE_DIR = Path(tmp)
        _run_all()


def _run_all() -> None:
    print("\n-- create_award, and the pending answer -------------------------")

    # A new award is recorded and answers "not yet", because a human has not
    # looked at it. This is the normal first response, not a fault.
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
    award.raw["state"] = store.Award.APPROVED
    award.raw["local_project_id"] = LOCAL_PROJECT
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

    print("\n-- an award can be moved to a different project -----------------")

    # An award is *attached* to a project, not identical to one, so the operator
    # can change which project holds it. The mapping follows, and the project it
    # leaves behind becomes available again.
    moved = store.load(OFFERING, AWARD)
    moved.raw["local_project_id"] = "elsewhere.site"
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
    moved.raw["local_project_id"] = LOCAL_PROJECT
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
    yesterday = (datetime.date.today() - datetime.timedelta(days=1)).isoformat()
    award.raw["usage"] = {yesterday: {"alice@bristol.ac.uk": 12.5}}
    store.save(award)

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
        "...marked complete, since the day is past",
        all(d.is_complete for d in report.daily_reports()),
    )

    job = site_portal.answer(make_job(f"get_usage_reports award this_month"))
    check("get_usage_reports rolls up to the portal", not job.is_error)
    check("...as a UsageReport", type(job.result) is openportal.UsageReport)

    print("\n-- a second award, on the other resource ------------------------")

    # The ordinary two-resource case: a different award, on a different
    # resource, mapped to a different project of ours. Nothing about it touches
    # the first, and both are live at once.
    site_portal.answer(
        make_job(f"create_project {AWARD2} {details()}", offering=OTHER_OFFERING)
    )
    second = store.load(OTHER_OFFERING, AWARD2)
    second.raw["state"] = store.Award.APPROVED
    second.raw["local_project_id"] = LOCAL_PROJECT2
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
        "removing from one resource leaves the other",
        store.load(OFFERING, AWARD) is not None
        and store.load(OTHER_OFFERING, AWARD) is None,
    )

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

    job = site_portal.answer(make_job(f"remove_project {AWARD}"))
    check("remove_award answers with :None", str(job.result) == f"{AWARD}:None")
    check("...and the award is gone", store.load(OFFERING, AWARD) is None)

    job = site_portal.answer(make_job(f"remove_project {AWARD}"))
    check("removing it twice is not an error", not job.is_error)

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
