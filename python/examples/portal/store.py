# SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
# SPDX-License-Identifier: MIT
"""
The portal's memory: awards, their approval state, and their usage figures.

**This is the file you replace.** Everything else in this example is about the
OpenPortal contract and is broadly the same whatever your portal is; this file
is about *your* portal, and a real one keeps this in a database with migrations,
transactions and backups rather than in a directory of JSON files.

It is deliberately dull, and deliberately separate from `portal.py`, to make one
point: the contract and your state are different concerns. Keeping them apart is
what lets you throw this file away without touching the handlers.

Two design choices worth copying even so:

* **Awards are keyed on the full project identifier** (`myproj.ukri`), not on
  the project name. The same name can exist under two different awarding
  portals and they are different projects - see project-portal-api.md §1.2.

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


def _path_for(project_id: str) -> Path:
    """
    The file backing one award.

    `project_id` arrives from the network, so it is not used as a filename
    without checking. An identifier is `<project>.<portal>` and both halves are
    restricted to `[A-Za-z0-9_-]` by the grammar, so anything containing a path
    separator or `..` is not an identifier at all and is refused here rather
    than being allowed to escape the state directory.
    """
    if not project_id or "/" in project_id or "\\" in project_id or ".." in project_id:
        raise ValueError(f"unsafe project identifier: {project_id!r}")

    return STATE_DIR / f"{project_id}.json"


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

    def __init__(self, project_id: str, raw: dict[str, Any]):
        self.project_id = project_id
        self.raw = raw

    @property
    def state(self) -> str:
        return self.raw.get("state", self.PENDING)

    @property
    def details(self) -> dict[str, Any]:
        return self.raw.get("details", {})

    @property
    def local_group(self) -> str | None:
        """What we call this project locally. Assigned on approval."""
        return self.raw.get("local_group")

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

        Hours, as a float. Pushed in by the operator's own parsers - see
        `app.py`'s `PUT /awards/{id}/usage`.
        """
        return self.raw.get("usage", {})


def load(project_id: str) -> Award | None:
    """Read one award, or `None` if we hold no such award."""
    path = _path_for(project_id)

    if not path.exists():
        return None

    with path.open() as handle:
        return Award(project_id, json.load(handle))


def save(award: Award) -> None:
    _write_atomically(_path_for(award.project_id), award.raw)


def create(project_id: str, details: dict[str, Any], forwarded_for: str | None) -> Award:
    """Record a brand-new award, awaiting approval."""
    award = Award(
        project_id,
        {
            "details": details,
            "state": Award.PENDING,
            "reason": "awaiting approval by a site administrator",
            "local_group": None,
            "forwarded_for": forwarded_for,
            "usage": {},
        },
    )
    save(award)
    return award


def all_awards() -> list[Award]:
    """Every award we hold. A real store would paginate."""
    if not STATE_DIR.exists():
        return []

    awards = []
    for path in sorted(STATE_DIR.glob("*.json")):
        with path.open() as handle:
            awards.append(Award(path.stem, json.load(handle)))
    return awards


def awards_for_portal(portal: str) -> list[Award]:
    """Every award made by one awarding portal - `get_awards` needs this."""
    return [a for a in all_awards() if a.project_id.endswith(f".{portal}")]


def delete(project_id: str) -> bool:
    """Forget an award. Returns whether we held one."""
    path = _path_for(project_id)

    if not path.exists():
        return False

    path.unlink()
    return True
