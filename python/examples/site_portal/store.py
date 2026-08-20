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

Two design choices worth copying even so:

* **Awards are keyed on `(offering, project identifier)`, not on either
  alone.** The offering names *which resource* the award is for, so the same
  awarding portal can hold two separate awards under the same name on two
  different resources - see site-portal-api.md §1.3. And the identifier must
  be the full `myaward1.allocator`, because the same project name can exist under two
  different awarding portals and mean different projects (§1.2). The reference
  implementation keys its own records the same way.

* **State is read fresh from disk on every access.** An operator approving an
  award through the REST API and the job handler answering a request are
  different requests, possibly different workers; a cache would go stale
  between them.
"""

from __future__ import annotations

import json
import os
import tempfile
from pathlib import Path
from typing import Any

# Where the JSON files live. One file per award, named after its identifier.
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
        / _safe(offering, "offering")
        / f"{_safe(project_id, 'project identifier')}.json"
    )


def _write_atomically(path: Path, data: dict[str, Any]) -> None:
    """
    Write via a temporary file and rename, so a crash mid-write cannot leave a
    half-written award behind. `os.replace` is atomic on POSIX and Windows.
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


class Award:
    """
    One award as this portal holds it.

    `details` is the `AwardDetails` JSON exactly as the awarding portal sent it,
    merged across updates. `state` is ours alone and never goes on the wire -
    the awarding portal learns about it only through which error we answer with.
    """

    #: Waiting for a human. `create_award` answers ManagedProjectPendingError.
    PENDING = "pending"
    #: Approved and provisioned. `create_award` answers with the mapping.
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
    def local_project_id(self) -> str | None:
        """
        **Our own `ProjectIdentifier` for this award**, e.g. `myproject1.site`.

        This is our half of the mapping, and it is the single most important
        thing this record holds. It is `None` until the award is approved,
        because until then no local project exists to name.

        Two things hang off it:

        * It goes back to the awarding portal in the `ProjectMapping`, so both
          sides end up agreeing that *their* award and *our* project are the
          same thing. After that exchange, award ID and project ID are two names
          for one object.
        * It is the identifier our own accounting knows this project by, so it
          is what usage is posted against - see `usage` below.

        Note it is a full identifier in *our* portal's namespace
        (`<project>.<our-portal>`), not a bare group name.
        """
        return self.raw.get("local_project_id")

    @property
    def reason(self) -> str:
        """Why it is pending or rejected - the text we put in the error."""
        return self.raw.get("reason", "")

    @property
    def forwarded_for(self) -> str | None:
        """The offering path the request arrived through, for authorisation."""
        return self.raw.get("forwarded_for")

    @property
    def usage(self) -> dict[str, Any]:
        """
        Usage keyed by ISO date, then by member email:

            {"2026-08-01": {"alice@example.ac.uk": 12.5}}

        Hours, as a float. Pushed in by the operator's own parsers, which
        identify the project by `local_project_id` - they have never heard of
        the awarding portal's identifier. Translating between the two is what
        the mapping is for; see `site_portal.build_usage_report`.
        """
        return self.raw.get("usage", {})

    @property
    def final_months(self) -> list[str]:
        """
        The months whose accounting the site has declared final, as `"YYYY-MM"`.

        This is the operations team's lever over how long the allocator keeps
        asking. A month listed here is reported with `is_complete` set, and the
        allocator stops re-requesting it; a month absent from here is reported
        incomplete, and keeps being asked for while it is still inside the
        allocator's window. See `site_portal.build_usage_report`.
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
            # No local project exists yet - approving is what creates one and
            # gives it an identifier.
            "local_project_id": None,
            "forwarded_for": forwarded_for,
            "usage": {},
            "final_months": [],
        },
    )
    save(award)
    return award


def all_awards() -> list[Award]:
    """Every award we hold, across every offering. A real store would paginate."""
    if not STATE_DIR.exists():
        return []

    awards = []
    for path in sorted(STATE_DIR.glob("*/*.json")):
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


def load_by_local_id(local_project_id: str) -> Award | None:
    """
    Find an award by **our** identifier for it, rather than the awarding
    portal's.

    This is the reverse lookup, and it is the reason the mapping matters
    operationally: your accounting produces figures for `myproject1.site` and has
    no idea that some other portal calls it `myaward1.allocator` on `cluster1`.

    No offering is needed here - a local project identifier is unique across the
    whole portal, because it names a real project of ours, and that project is
    on exactly one resource. A real store makes this an indexed column rather
    than a scan.
    """
    for award in all_awards():
        if award.local_project_id == local_project_id:
            return award

    return None


def delete(offering: str, project_id: str) -> bool:
    """Forget one award on one offering. Returns whether we held it."""
    path = _path_for(offering, project_id)

    if not path.exists():
        return False

    path.unlink()
    return True
