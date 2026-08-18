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

PROJECT = "myproject.ukri"
OFFERING = "example-resource"


def make_job(instruction: str, *, forwarded_for: str | None = None) -> openportal.Job:
    """
    A pending bridge-board job, as `fetch_job` would return it.

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
                "command": f"aip1.bridge.{OFFERING} {instruction}",
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
    check("the award was recorded", store.load(PROJECT) is not None)

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

    print("\n-- approval, and the mapping -----------------------------------")

    award = store.load(PROJECT)
    award.raw["state"] = store.Award.APPROVED
    award.raw["local_group"] = "grp001"
    store.save(award)

    job = portal.answer(make_job(f"create_project {PROJECT} {details()}"))
    check("an approved award answers with a mapping", not job.is_error)
    check(
        "...of the specified shape",
        str(job.result) == f"{PROJECT}:grp001",
        f"-> {job.result}",
    )
    check("...typed as a ProjectMapping", type(job.result) is openportal.ProjectMapping)

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
    check("...and the award now exists", store.load("fresh.ukri") is not None)
    store.delete("fresh.ukri")

    print("\n-- usage reports -----------------------------------------------")

    # Push figures in the way an operator's parser would.
    award = store.load(PROJECT)
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
    check(
        "...and a user mapping so the caller knows who used it",
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
    check("...and the award is gone", store.load(PROJECT) is None)

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
