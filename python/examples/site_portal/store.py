# SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
# SPDX-License-Identifier: MIT
"""
The portal's memory: awards, their approval state, and their usage figures.

**This is the file you replace.** Everything else in this example is about the
OpenPortal contract and is broadly the same whatever your portal is; this file
is about *your* portal, and a real one keeps this in a database with migrations,
transactions and backups rather than in a directory of JSON files.

It is deliberately dull, and deliberately separate from `site_portal.py`, to
make one point: the contract and your state are different concerns. Keeping them
apart is what lets you throw this file away without touching the handlers.

Four design choices worth copying even so:

* **The set of offerings is state, not a constant.** Which resources a site
  offers changes - a cluster is procured, retired, or opened to a second
  awarding portal - and none of those are code changes. So the offerings live
  here with everything else, `app.py` exposes them, and OpenPortal is told the
  new set whenever it changes.

* **Awards are keyed on `(offering, project identifier)`, not on either
  alone.** The offering names *which resource* the award is for, so the same
  awarding portal can hold two separate awards under the same name on two
  different resources - see site-portal-api.md §1.3. And the identifier must
  be the full `myaward1.allocator`, because the same project name can exist under two
  different awarding portals and mean different projects (§1.2). The reference
  implementation keys its own records the same way.

* **Usage belongs to the project, not to the award.** This one is easy to get
  wrong, and it follows from how billing works (§4.1.2): a project's usage on a
  given day is billed to whichever award it was *last attached to* that day. So
  a day's usage cannot simply be filed under "the current award" when it is
  recorded - which award owns that day is not settled until the day ends, and
  attaching a new award part-way through the day changes the answer for the
  whole of it. Usage is therefore stored per project, and awards claim days of
  it when a report is built.

* **State is read fresh from disk on every access.** An operator approving an
  award through the REST API and the job handler answering a request are
  different requests, possibly different workers; a cache would go stale
  between them.
"""

from __future__ import annotations

import datetime
import json
import os
import tempfile
from pathlib import Path
from typing import Any

# Where the JSON files live. Awards under `awards/<offering>/`, projects under
# `projects/` - two separate namespaces, because they are two different things
# keyed on two different identifiers.
STATE_DIR = Path(os.environ.get("PORTAL_STATE_DIR", "./portal-state"))


def _safe(component: str, what: str) -> str:
    """
    Refuse anything that could escape the state directory.

    Both an identifier and an offering name arrive from the network and are used
    here as path components. Agent names and identifier halves are restricted to
    `[A-Za-z0-9_-]` by the grammar, so anything carrying a path separator or
    `..` is not one at all, and is refused rather than allowed through.
    """
    if not component or "/" in component or "\\" in component or ".." in component:
        raise ValueError(f"unsafe {what}: {component!r}")

    return component


def _path_for(offering: str, project_id: str) -> Path:
    """
    The file backing one award: one directory per offering, one file per award.

    The directory *is* the key. An award for `myaward1.allocator` on `cluster1` and
    one for `myaward1.allocator` on `cluster2` are two different awards for two
    different resources, and they must not collide.
    """
    return (
        STATE_DIR
        / "awards"
        / _safe(offering, "offering")
        / f"{_safe(project_id, 'project identifier')}.json"
    )


def _offerings_path() -> Path:
    """
    The file holding every offering we advertise.

    One file for the whole set rather than one per offering, because the set is
    what gets published: `sync_offerings` replaces OpenPortal's idea of what we
    offer with the complete list, so reading and writing it as a whole is the
    shape the contract asks for.
    """
    return STATE_DIR / "offerings.json"


def _project_path(local_project_id: str) -> Path:
    """The file backing one of *our* projects, which is where usage lives."""
    return (
        STATE_DIR
        / "projects"
        / f"{_safe(local_project_id, 'local project identifier')}.json"
    )


def _write_atomically(path: Path, data: dict[str, Any]) -> None:
    """
    Write via a temporary file and rename, so a crash mid-write cannot leave a
    half-written record behind. `os.replace` is atomic on POSIX and Windows.
    """
    path.parent.mkdir(parents=True, exist_ok=True)

    fd, tmp = tempfile.mkstemp(dir=path.parent, suffix=".tmp")
    try:
        with os.fdopen(fd, "w") as handle:
            json.dump(data, handle, indent=2, sort_keys=True)
        os.replace(tmp, path)
    except BaseException:
        Path(tmp).unlink(missing_ok=True)
        raise


def _as_date(value: str | None) -> datetime.date | None:
    """An ISO date read back from storage, or `None`."""
    return datetime.date.fromisoformat(value) if value else None


class Attachment:
    """
    One period during which an award was attached to one of our projects.

    Both ends are inclusive days, and `to` is `None` while the attachment is
    open. Inclusive at the far end is the point: an award detached *during* a
    day was attached during that day, and still owns it (§4.1.2).
    """

    def __init__(self, raw: dict[str, Any]):
        self.raw = raw

    @property
    def project(self) -> str:
        """Our `ProjectIdentifier` for the project that was attached."""
        return self.raw["project"]

    @property
    def since(self) -> datetime.date:
        """The first day this attachment covers."""
        return datetime.date.fromisoformat(self.raw["since"])

    @property
    def to(self) -> datetime.date | None:
        """The last day it covers, or `None` while it is still open."""
        return _as_date(self.raw.get("to"))

    def covers(self, date: datetime.date) -> bool:
        """Whether this attachment was in force on `date`."""
        return self.since <= date and (self.to is None or self.to >= date)


class Offering:
    """
    One resource we advertise, and the templates it accepts.

    The name is the resource's own - `cluster1`, not `cluster1.site.allocator`.
    The full three-part form is assembled per awarding portal when the set is
    registered (`app.py`), because the same resource is normally offered to
    several of them and that is a property of the relationship rather than of
    the resource.
    """

    def __init__(self, name: str, raw: dict[str, Any]):
        self.name = name
        self.raw = raw

    @property
    def templates(self) -> list[str]:
        """
        The `AwardDetails.template` values this resource accepts, sorted.

        Per-resource because a template selects things that belong to the
        resource - in Waldur the organisation, the default offerings and the
        billing a project is created with. An award naming a template this
        resource does not offer is rejected rather than quietly given a
        default (§4.1).
        """
        return sorted(self.raw.get("templates", []))

    @property
    def since(self) -> datetime.date | None:
        """The day we started advertising it, for the operator's benefit."""
        return _as_date(self.raw.get("since"))


def load_offerings() -> dict[str, Offering]:
    """
    Every offering we advertise, by name. Empty until an operator adds one.

    Empty is a perfectly ordinary state and not a misconfiguration: a site that
    advertises nothing simply cannot be asked for anything yet (§1.1).
    """
    path = _offerings_path()

    if not path.exists():
        return {}

    with path.open() as handle:
        stored = json.load(handle)

    return {name: Offering(name, raw) for name, raw in sorted(stored.items())}


def save_offerings(offerings: dict[str, Offering]) -> None:
    _write_atomically(
        _offerings_path(), {name: o.raw for name, o in offerings.items()}
    )


def add_offering(name: str, templates: list[str], on: datetime.date) -> Offering:
    """
    Start advertising a resource, or change the templates one accepts.

    An upsert rather than an insert, and deliberately so: the operator API is
    retried and re-run like everything else here, and "add the cluster I already
    have" should not be an error. `since` is kept from the first time, because
    that is when we started offering it.
    """
    offerings = load_offerings()
    existing = offerings.get(name)

    raw = {
        "templates": sorted(set(templates)),
        "since": (existing.since or on).isoformat() if existing else on.isoformat(),
    }

    offerings[name] = Offering(_safe(name, "offering"), raw)
    save_offerings(offerings)

    return offerings[name]


def remove_offering(name: str) -> Offering | None:
    """
    Stop advertising a resource. Returns what was removed, or `None`.

    **The awards on it are kept.** Withdrawing an offering says what we advertise
    *now*; it does not rewrite what happened. Those awards still own the days
    they were attached for, and deleting them would make a later usage report
    empty - and an empty report is vacuously complete, which is how the last
    days of an award get silently lost (§4.1.2). They simply become unreachable
    until the offering is added back.
    """
    offerings = load_offerings()
    removed = offerings.pop(name, None)

    if removed is not None:
        save_offerings(offerings)

    return removed


class Award:
    """
    One award as this portal holds it.

    `details` is the `AwardDetails` JSON exactly as the awarding portal sent it,
    merged across updates. `state` is ours alone and never goes on the wire -
    the awarding portal learns about it only through which error we answer with.
    """

    #: Waiting for a human. `create_award` answers ManagedProjectPendingError.
    PENDING = "pending"
    #: Approved and attached to a project. `create_award` answers the mapping.
    APPROVED = "approved"
    #: Refused for good. `create_award` answers ManagedProjectRejectedError.
    REJECTED = "rejected"

    def __init__(self, offering: str, project_id: str, raw: dict[str, Any]):
        #: The resource this award is for - the virtual agent it arrived
        #: through. Half of this award's identity, not an attribute of it.
        self.offering = offering
        #: What the awarding portal calls this award.
        self.project_id = project_id
        self.raw = raw

    @property
    def state(self) -> str:
        return self.raw.get("state", self.PENDING)

    @property
    def details(self) -> dict[str, Any]:
        return self.raw.get("details", {})

    @property
    def attachments(self) -> list[Attachment]:
        """
        Every period this award has been attached to a project of ours, oldest
        first.

        **A history rather than a single field**, which is the shape the billing
        rule actually needs (§4.1.2). An award can be detached and re-attached,
        and can be moved from one project to another, and each of those episodes
        owns its own days. Collapsing it to one "attached from" date would
        silently disown every day before the latest attachment.
        """
        return [Attachment(a) for a in self.raw.get("attachments", [])]

    @property
    def current_attachment(self) -> Attachment | None:
        """The open attachment, if this award is attached to anything now."""
        for attachment in self.attachments:
            if attachment.to is None:
                return attachment

        return None

    @property
    def local_project_id(self) -> str | None:
        """
        **Our own `ProjectIdentifier` for this award**, e.g. `myproject1.site`,
        while it is attached - and `None` when it is not.

        This is our half of the mapping, and it is the single most important
        thing this record holds. It is `None` until the award is approved,
        because until then no project of ours is attached to name, and `None`
        again after `remove_award`, because the link has been severed.

        Two things hang off it:

        * It goes back to the awarding portal in the `ProjectMapping`, so both
          sides end up agreeing that *their* award and *our* project are the
          same thing. After that exchange, award ID and project ID are two names
          for one object.
        * It is the identifier our own accounting knows this project by, so it
          is what usage is recorded against - see `load_project`.

        Note it is a full identifier in *our* portal's namespace
        (`<project>.<our-portal>`), not a bare group name.

        A detached award reports `None` here but keeps its `attachments`, so the
        days it owned stay reportable (§4.1.2). Use `projects_ever_attached` for
        the historical question.
        """
        current = self.current_attachment

        return current.project if current is not None else None

    @property
    def projects_ever_attached(self) -> list[str]:
        """Every project of ours this award has been attached to, in order."""
        seen = []

        for attachment in self.attachments:
            if attachment.project not in seen:
                seen.append(attachment.project)

        return seen

    @property
    def is_attached(self) -> bool:
        """Whether this award is currently connected to a project of ours."""
        return self.current_attachment is not None

    @property
    def reason(self) -> str:
        """Why it is pending or rejected - the text we put in the error."""
        return self.raw.get("reason", "")

    @property
    def forwarded_for(self) -> str | None:
        """The offering path the request arrived through, for authorisation."""
        return self.raw.get("forwarded_for")


class Project:
    """
    One of *our* projects, and the usage recorded against it.

    Not an OpenPortal concept - the awarding portal never sees this record, and
    never learns our project identifier except as the second half of a
    `ProjectMapping`. It exists because usage is a property of the project and
    only a property of an award *derivatively*, by way of which award was
    attached on which day.
    """

    def __init__(self, local_project_id: str, raw: dict[str, Any]):
        self.local_project_id = local_project_id
        self.raw = raw

    @property
    def usage(self) -> dict[str, Any]:
        """
        Usage keyed by ISO date, then by member email:

            {"2026-08-01": {"alice@example.ac.uk": 12.5}}

        Hours, as a float. Pushed in by the operator's own parsers, which
        identify the project by our identifier - they have never heard of the
        awarding portal's. Translating between the two is what the mapping is
        for; see `site_portal.build_usage_report`.
        """
        return self.raw.get("usage", {})

    @property
    def final_months(self) -> list[str]:
        """
        The months whose accounting the site has declared final, as `"YYYY-MM"`.

        This is the operations team's lever over how long the allocator keeps
        asking. A month listed here is reported with `is_complete` set, and the
        allocator stops re-requesting it; a month absent from here is reported
        incomplete, and keeps being asked for. See
        `site_portal.build_usage_report`.

        A property of the project rather than of the award, for the same reason
        usage is: "August is settled" is a statement about the site's
        accounting, and it stays true across a change of award.
        """
        return self.raw.get("final_months", [])


def load(offering: str, project_id: str) -> Award | None:
    """
    Read one award, or `None` if we hold no such award **on that offering**.

    Both halves of the key are required. Asking for an award on a resource it
    was never created on is a legitimate question with the answer "no".
    """
    path = _path_for(offering, project_id)

    if not path.exists():
        return None

    with path.open() as handle:
        return Award(offering, project_id, json.load(handle))


def save(award: Award) -> None:
    _write_atomically(_path_for(award.offering, award.project_id), award.raw)


def create(
    offering: str,
    project_id: str,
    details: dict[str, Any],
    forwarded_for: str | None,
) -> Award:
    """Record a brand-new award on one offering, awaiting approval."""
    award = Award(
        offering,
        project_id,
        {
            "details": details,
            "state": Award.PENDING,
            "reason": "awaiting approval by a site administrator",
            # Nothing of ours is attached yet - approving is what attaches it.
            "attachments": [],
            "forwarded_for": forwarded_for,
        },
    )
    save(award)
    return award


def attach(award: Award, local_project_id: str, on: datetime.date) -> Award:
    """
    Attach an approved award to one of our projects, from `on` onwards.

    Appends to the history rather than overwriting it, so an award re-attached
    after a gap keeps the days it owned before, and an award moved from one
    project to another keeps the days it owned on the first (§4.1.2).

    Any attachment still open is closed as of `on` - not the day before. Both
    ends of an attachment are inclusive, so an award moved today owns today on
    the project it left *and* on the project it joined. Those are two different
    projects' usage, so nothing is double-counted.

    Re-attaching to the project it is already on is a no-op. Appending a second
    interval starting today would be harmless for ownership but would misreport
    the history, and re-approving is something an operator does routinely.
    """
    award.raw["state"] = Award.APPROVED
    award.raw["reason"] = ""

    current = award.current_attachment

    if current is not None:
        if current.project == local_project_id:
            save(award)
            return award

        current.raw["to"] = on.isoformat()

    award.raw.setdefault("attachments", []).append(
        {"project": local_project_id, "since": on.isoformat(), "to": None}
    )
    save(award)
    return award


def detach(offering: str, project_id: str, on: datetime.date) -> Award | None:
    """
    Sever an award from its project, as of `on`. Returns the award, or `None` if
    we hold no such award.

    **The record is kept, not deleted** - and so is the project and its usage.
    This is the whole subtlety of `remove_award` (§4.1.2). Removal ends the
    award's claim on *future* days and changes nothing about the days it was
    already attached for, and those days still have to be reportable. Deleting
    the record would make them unreportable - or worse, make the month report as
    empty, and an empty report is vacuously *complete*, which would tell the
    allocator that nothing was ever used and that this is final.

    Detaching something already detached does nothing. The first date is the
    true one, and moving it later would hand the award days it did not own.
    """
    award = load(offering, project_id)

    if award is None:
        return None

    current = award.current_attachment

    if current is not None:
        current.raw["to"] = on.isoformat()
        save(award)

    return award


def all_awards() -> list[Award]:
    """Every award we hold, across every offering. A real store would paginate."""
    root = STATE_DIR / "awards"

    if not root.exists():
        return []

    awards = []
    for path in sorted(root.glob("*/*.json")):
        with path.open() as handle:
            awards.append(Award(path.parent.name, path.stem, json.load(handle)))
    return awards


def awards_on(offering: str, portal: str) -> list[Award]:
    """
    Every award made by one awarding portal **on one offering**.

    Both filters matter. `get_awards allocator` arriving through `cluster1` is
    asking what `allocator` has on *that* resource, and an award on a different
    resource is no more relevant than one from a different portal.
    """
    return [
        a
        for a in all_awards()
        if a.offering == offering and a.project_id.endswith(f".{portal}")
    ]


def awards_for_local_project(local_project_id: str) -> list[Award]:
    """
    Every award that has *ever* been attached to one of our projects, whether it
    still is or not.

    This is what decides billing. A project can have held several awards over
    its life - one after another, or one replacing another part-way through a
    day - and which award a given day is billed to depends on that history, not
    on whichever award happens to be attached now (§4.1.2).
    """
    return [
        a
        for a in all_awards()
        if local_project_id in a.projects_ever_attached
    ]


def load_by_local_id(local_project_id: str) -> Award | None:
    """
    Find the award **currently attached** to one of our projects, by our own
    identifier for it rather than the awarding portal's.

    This is the reverse lookup, and it is the reason the mapping matters
    operationally: your accounting produces figures for `myproject1.site` and has
    no idea that some other portal calls it `myaward1.allocator` on `cluster1`.

    Detached awards are deliberately excluded, because the question this answers
    is "what is this project billing to *now*". `awards_for_local_project` is
    the one that includes them.

    No offering is needed here - a local project identifier is unique across the
    whole portal, because it names a real project of ours, and that project is
    on exactly one resource. A real store makes this an indexed column rather
    than a scan.
    """
    for award in awards_for_local_project(local_project_id):
        # `is_attached` alone is not enough: an award moved to a *different*
        # project is still attached, just not here. The project it left has to
        # come back as free, or it could never be attached to anything else.
        if award.local_project_id == local_project_id:
            return award

    return None


def load_project(local_project_id: str) -> Project:
    """
    Read one of our projects. Always succeeds: a project we have recorded
    nothing about yet is a project with no usage, which is a fact rather than an
    error.
    """
    path = _project_path(local_project_id)

    if not path.exists():
        return Project(local_project_id, {"usage": {}, "final_months": []})

    with path.open() as handle:
        return Project(local_project_id, json.load(handle))


def save_project(project: Project) -> None:
    _write_atomically(_project_path(project.local_project_id), project.raw)


def delete(offering: str, project_id: str) -> bool:
    """
    Forget one award entirely. Returns whether we held it.

    Note this is **not** what `remove_award` does - see `detach`. It is here for
    an operator expunging a record, and for the test suite to clean up after
    itself.
    """
    path = _path_for(offering, project_id)

    if not path.exists():
        return False

    path.unlink()
    return True
