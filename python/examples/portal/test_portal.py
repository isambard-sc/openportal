# SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
# SPDX-License-Identifier: MIT
"""
Exercise every handler without a bridge, an agent or a network.

Run it with `python test_portal.py`, or under pytest.

This is here for two reasons. It proves the example actually works rather than
merely looking as though it should - and it shows that **the contract is testable
in isolation**. A `Job` deserialises from JSON, and `portal.answer()` returns the
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

import portal
import store

#: What the awarding portal calls the award.
PROJECT = "myproject.ukri"

#: What *we* call the project we create for it. The two halves of the mapping.
LOCAL_PROJECT = "proj001.aip1"

#: The resource the award is for, and one it is not. Both are virtual agents on
#: this portal, and the difference between them is the point of several checks
#: below: an award lives on one resource, and the other knows nothing about it.
OFFERING = "isambard-ai"
OTHER_OFFERING = "isambard3"


def make_job(
    instruction: str,
    *,
    offering: str = OFFERING,
    forwarded_for: str | None = None,
) -> openportal.Job:
    """
    A pending bridge-board job, as `fetch_job` would return it.

    The destination ends with the offering - `aip1.bridge.isambard-ai` - which
    is how the portal knows which resource the request is about. `forwarded_for`
    carries the awarding portal's original `ukri.aip1.isambard-ai` when there is
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
                "command": f"aip1.bridge.{offering} {instruction}",
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
    job = portal.answer(make_job(f"create_project {PROJECT} {details()}"))
    check("new award is answered with an error", job.is_error)
    check(
        "...and the error is ManagedProjectPendingError",
        type(job.error) is openportal.ManagedProjectPendingError,
        f"-> {job.error}",
    )
    check("...carrying the award_pending kind", job.error_kind == "award_pending")
    check("the award was recorded on that offering", store.load(OFFERING, PROJECT) is not None)

    print("\n-- idempotency: the same create arrives again -------------------")

    # The awarding portal re-sends this every cycle. It must not create a
    # second award, and must answer the same way.
    again = portal.answer(make_job(f"create_project {PROJECT} {details()}"))
    check(
        "a repeated create answers the same",
        type(again.error) is openportal.ManagedProjectPendingError,
    )
    check("still exactly one award", len(store.all_awards()) == 1)

    print("\n-- a rejected template -----------------------------------------")

    bad = portal.answer(make_job(f"create_project other.ukri {details('gpu-mega')}"))
    check(
        "an unoffered template is rejected terminally",
        type(bad.error) is openportal.ManagedProjectRejectedError,
        f"-> {bad.error}",
    )
    check("...carrying the award_rejected kind", bad.error_kind == "award_rejected")

    print("\n-- approval names the project on our side ----------------------")

    # Approving is what creates a project here and gives it an identifier in
    # *our* namespace. That identifier is our half of the mapping.
    award = store.load(OFFERING, PROJECT)
    award.raw["state"] = store.Award.APPROVED
    award.raw["local_project_id"] = LOCAL_PROJECT
    store.save(award)

    job = portal.answer(make_job(f"create_project {PROJECT} {details()}"))
    check("an approved award answers with a mapping", not job.is_error)
    check(
        "...pairing their award id with our project id",
        str(job.result) == f"{PROJECT}:{LOCAL_PROJECT}",
        f"-> {job.result}",
    )
    check("...typed as a ProjectMapping", type(job.result) is openportal.ProjectMapping)
    check(
        "...their half is the award they asked about",
        str(job.result.project) == PROJECT,
    )
    check(
        "...our half is a project in our own namespace",
        str(job.result.local_group) == LOCAL_PROJECT,
        f"-> {job.result.local_group}",
    )

    # The reverse lookup: our accounting knows only our identifier.
    check(
        "an award can be found by our identifier for it",
        store.load_by_local_id(LOCAL_PROJECT).project_id == PROJECT,
    )

    print("\n-- get_award ---------------------------------------------------")

    job = portal.answer(make_job(f"get_award {PROJECT}"))
    check("get_award returns AwardDetails", type(job.result) is openportal.AwardDetails)
    check(
        "...with members populated (there is no get_users)",
        "alice@bristol.ac.uk" in (job.result.members or {}),
        f"-> {list((job.result.members or {}).keys())}",
    )

    print("\n-- update_award for an award we do not hold ---------------------")

    job = portal.answer(make_job(f"update_project fresh.ukri {details()}"))
    check(
        "an update for an unknown award becomes a create",
        type(job.error) is openportal.ManagedProjectPendingError,
    )
    check("...and the award now exists", store.load(OFFERING, "fresh.ukri") is not None)
    store.delete(OFFERING, "fresh.ukri")

    print("\n-- usage: recorded in our namespace, answered in theirs --------")

    # Push figures in the way an operator's parser would - against our own
    # project identifier, which is the only one that accounting knows.
    award = store.load(OFFERING, PROJECT)
    yesterday = (datetime.date.today() - datetime.timedelta(days=1)).isoformat()
    award.raw["usage"] = {yesterday: {"alice@bristol.ac.uk": 12.5}}
    store.save(award)

    job = portal.answer(make_job(f"get_usage_report {PROJECT} this_month"))
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
        str(report.project) == PROJECT,
        f"-> {report.project}",
    )
    check(
        "...with user identifiers rebuilt into their namespace",
        all(str(u).endswith(f".{PROJECT}") for u in report.users),
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

    job = portal.answer(make_job(f"get_usage_reports ukri this_month"))
    check("get_usage_reports rolls up to the portal", not job.is_error)
    check("...as a UsageReport", type(job.result) is openportal.UsageReport)

    print("\n-- the same question asked of the wrong resource ----------------")

    # The award lives on isambard-ai. An awarding portal can perfectly well ask
    # isambard3 about it - same identifier, nothing stops the question - and the
    # honest answer is that nothing was used there, because the project is not
    # there. Empty, not an error: a caller sweeping every offering it knows
    # about should not be failed by the ones that hold nothing.
    job = portal.answer(
        make_job(f"get_usage_report {PROJECT} this_month", offering=OTHER_OFFERING)
    )
    check("usage from the wrong offering succeeds", not job.is_error, f"{job.error_message}")
    check(
        "...and is empty, because the project is not on that resource",
        job.result.total_usage.seconds == 0,
        f"-> {job.result.total_usage}",
    )
    check(
        "...while the right offering still has the usage",
        portal.answer(
            make_job(f"get_usage_report {PROJECT} this_month")
        ).result.total_usage.seconds > 0,
    )

    # An award is only visible through the resource it was created on.
    job = portal.answer(make_job(f"get_award {PROJECT}", offering=OTHER_OFFERING))
    check("get_award from the wrong offering finds nothing", job.is_error, f"-> {job.error}")
    check(
        "...listings are scoped too",
        portal.answer(make_job("get_projects ukri", offering=OTHER_OFFERING)).result == [],
    )
    check(
        "...while the right offering lists it",
        len(portal.answer(make_job("get_projects ukri")).result) > 0,
    )

    print("\n-- the same name on two resources is two awards -----------------")

    # `myproject.ukri` on isambard3 is a different award from `myproject.ukri`
    # on isambard-ai, and creating it must not disturb the first.
    job = portal.answer(
        make_job(f"create_project {PROJECT} {details()}", offering=OTHER_OFFERING)
    )
    check(
        "creating the same name on another resource is a new, pending award",
        type(job.error) is openportal.ManagedProjectPendingError,
    )
    check(
        "...the original is untouched and still approved",
        str(portal.answer(make_job(f"get_project_mapping {PROJECT}")).result)
        == f"{PROJECT}:{LOCAL_PROJECT}",
    )
    check(
        "...and they are stored as two separate awards",
        len([a for a in store.all_awards() if a.project_id == PROJECT]) == 2,
    )

    # The two awards must not end up sharing one local project either. This is
    # the same mistake as everywhere else in this section - comparing only the
    # identifier and forgetting the offering - and it is worth a test because it
    # is so easy to write.
    other = store.load(OTHER_OFFERING, PROJECT)
    check(
        "the two awards are distinct despite the shared name",
        (other.offering, other.project_id) != (OFFERING, PROJECT)
        and other.project_id == PROJECT,
    )
    check(
        "...and our identifier for the first is not free to reuse",
        store.load_by_local_id(LOCAL_PROJECT).offering == OFFERING,
    )

    # Removing from one resource leaves the other alone.
    portal.answer(make_job(f"remove_project {PROJECT}", offering=OTHER_OFFERING))
    check(
        "removing from one resource leaves the other",
        store.load(OFFERING, PROJECT) is not None
        and store.load(OTHER_OFFERING, PROJECT) is None,
    )

    print("\n-- a template this resource does not offer ----------------------")

    # Templates are per-resource: `large` is offered on isambard-ai and not on
    # isambard3, because a template selects things that belong to the resource.
    job = portal.answer(
        make_job(f"create_project big.ukri {details('large')}", offering=OTHER_OFFERING)
    )
    check(
        "a template offered elsewhere is rejected here",
        type(job.error) is openportal.ManagedProjectRejectedError,
        f"-> {job.error}",
    )
    job = portal.answer(make_job(f"create_project big.ukri {details('large')}"))
    check(
        "...and accepted on the resource that offers it",
        type(job.error) is openportal.ManagedProjectPendingError,
    )
    store.delete(OFFERING, "big.ukri")

    print("\n-- an award with no local project yet --------------------------")

    # An unapproved award has no project on our side, so there is nothing for
    # usage to have been recorded against. Empty, for the same reason as the
    # wrong-offering case above: nothing was used, and that is not a failure.
    pending = "notyet.ukri"
    portal.answer(make_job(f"create_project {pending} {details()}"))
    job = portal.answer(make_job(f"get_usage_report {pending} this_month"))
    check("usage for an unapproved award is empty, not an error", not job.is_error)
    check("...and reports zero", job.result.total_usage.seconds == 0)
    check(
        "...while the mapping for it is still refused, being pending",
        portal.answer(make_job(f"get_project_mapping {pending}")).is_error,
    )
    check(
        "...but get_projects still lists it, as :None",
        any(
            str(m) == f"{pending}:None"
            for m in portal.answer(make_job("get_projects ukri")).result
        ),
    )
    store.delete(OFFERING, pending)

    print("\n-- storage: empty beats absent ---------------------------------")

    job = portal.answer(make_job(f"get_storage_report {PROJECT} this_month"))
    check("get_storage_report succeeds rather than failing", not job.is_error)
    check("...with an empty report", job.result.is_empty())

    print("\n-- an instruction we do not implement ---------------------------")

    job = portal.answer(make_job(f"get_users {PROJECT}"))
    check(
        "get_users is declined, not ignored",
        type(job.error) is openportal.OpenPortalUnsupportedCommandError,
        f"-> {job.error}",
    )
    check("...carrying the unsupported kind", job.error_kind == "unsupported")

    print("\n-- authorisation against forwarded_for -------------------------")

    job = portal.answer(
        make_job(f"get_award {PROJECT}", forwarded_for=f"ukri.aip1.{OFFERING}")
    )
    check("a request through an advertised offering is allowed", not job.is_error)

    job = portal.answer(
        make_job(f"get_award {PROJECT}", forwarded_for="ukri.aip1.not-ours")
    )
    check(
        "a request through an unknown offering is refused",
        type(job.error) is openportal.ManagedProjectRejectedError,
        f"-> {job.error}",
    )

    print("\n-- an award we have never heard of ------------------------------")

    job = portal.answer(make_job("get_award nosuch.ukri"))
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

    job = portal.answer(make_job(f"remove_project {PROJECT}"))
    check("remove_award answers with :None", str(job.result) == f"{PROJECT}:None")
    check("...and the award is gone", store.load(OFFERING, PROJECT) is None)

    job = portal.answer(make_job(f"remove_project {PROJECT}"))
    check("removing it twice is not an error", not job.is_error)

    print("\n-- every job gets an answer ------------------------------------")

    # The structural guarantee: even a handler that blows up unexpectedly
    # produces an errored job rather than silence.
    broken = dict(portal.HANDLERS)
    portal.HANDLERS["get_award"] = lambda job: 1 / 0
    logging.disable(logging.CRITICAL)  # the traceback below is deliberate
    try:
        job = portal.answer(make_job("get_award anything.ukri"))
        check("an unexpected exception still answers", job.is_error)
        check("...as an OpenPortalError", isinstance(job.error, openportal.OpenPortalError))
    finally:
        logging.disable(logging.NOTSET)
        portal.HANDLERS.clear()
        portal.HANDLERS.update(broken)

    print("\nall checks passed\n")


if __name__ == "__main__":
    run()
