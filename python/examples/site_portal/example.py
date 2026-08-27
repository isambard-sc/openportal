#!/usr/bin/env python3
# SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
# SPDX-License-Identifier: MIT
"""
A minimal, complete, entirely local OpenPortal setup for this example.

Reading the example teaches you the contract; *running* it teaches you what the
two portals actually say to each other. Doing that by hand means four agents to
initialise, three pairs of `client`/`server` commands to peer them, three invite
files to carry between them, two more commands for the Python configs, and a set
of environment variables - and getting any one of them wrong produces a silence
rather than an error. This script does all of it, into one git-ignored
directory, and can take it all away again:

    python example.py setup     # write the configs (idempotent)
    python example.py start     # start everything, and say how to talk to it
    python example.py status    # what is running
    python example.py stop      # stop everything
    python example.py clean     # stop everything and delete ./data

What it builds is the smallest arrangement in which an *awarding* portal can ask
a *site* portal for an award (site-portal-api.md §1):

    your Python  ──►  allocator_bridge  ──►  allocator          (awards portal)
                                                │
                                                ▼
    FastAPI app  ◄──  site_bridge       ◄──  site               (site portal)

Both bridges are usable from Python, so you can drive either end: the allocator
to *make* requests, the site to see what its own bridge holds.

Everything binds to 127.0.0.1 on ports in the 187xx range - high enough to be
out of the way of the defaults in agent-configuration.md §4, and of anything
else likely to be running.

This is a development toy. The agents talk plain `ws://` on loopback, the
FastAPI app has no authentication (see the README), and the config files are
unencrypted. None of that is safe anywhere but your own machine.
"""

from __future__ import annotations

import argparse
import datetime
import json
import os
import shutil
import signal
import socket
import subprocess
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path

# --------------------------------------------------------------------------
# Where everything lives
# --------------------------------------------------------------------------

#: This directory - the example root. Every path below is derived from it, so
#: the script works from wherever it is invoked.
HERE = Path(__file__).resolve().parent

#: The workspace root, four levels up (openportal/python/examples/site_portal).
#: Used only to find the agent binaries in `target/`.
REPO = HERE.parents[2]

#: Where the prebuilt agents live, for someone who would rather not install a
#: Rust toolchain to look at a Python example. Linux x86-64 and aarch64 only -
#: anywhere else has to build. The tag tracks the `openportal>=0.92.0` pin in
#: requirements.txt, so the agents and the module are the same version.
RELEASES = "https://github.com/isambard-sc/openportal/releases/download/0.92.0"

#: Everything this script creates, and the only thing `clean` deletes. It is in
#: .gitignore: it holds key material, and nothing in it is worth keeping.
BASE = HERE / "data"

CONFIG_DIR = BASE / "config"  # agent config files (op-portal, op-bridge)
PY_CONFIG_DIR = BASE / "python"  # bridge invites for the openportal module
INVITE_DIR = BASE / "invites"  # scratch space for invites, emptied as we go
STATE_DIR = BASE / "state"  # the FastAPI app's awards and usage (store.py)
LOG_DIR = BASE / "logs"  # one log per process
RUN_DIR = BASE / "run"  # one pid file per process


# --------------------------------------------------------------------------
# The four agents, and the app
# --------------------------------------------------------------------------


@dataclass(frozen=True)
class Agent:
    """One OpenPortal agent: which binary runs it, and on which ports."""

    name: str
    binary: str
    port: int

    #: Bridges only: the port their HTTP API listens on, which is what the
    #: `openportal` Python module talks to.
    http_port: int | None = None

    @property
    def config(self) -> Path:
        """Its agent config file - what `--config-file` is given."""
        return CONFIG_DIR / f"{self.name}.toml"

    @property
    def py_config(self) -> Path:
        """Bridges only: the config file `openportal.load_config()` reads."""
        return PY_CONFIG_DIR / f"{self.name}.toml"

    @property
    def ready_port(self) -> int:
        """
        The port whose appearance means "this agent is up".

        A portal binds its websocket port and waits to be dialled. A bridge
        never does - it only dials out to its portal - so the port that proves a
        bridge is running is its HTTP API.
        """
        return self.http_port or self.port

    @property
    def url(self) -> str:
        return f"ws://127.0.0.1:{self.port}"

    @property
    def http_url(self) -> str:
        return f"http://127.0.0.1:{self.http_port}"


#: The awarding portal - it decides awards and asks for them to be provisioned
#: elsewhere. In the specification this is `allocator`.
ALLOCATOR = Agent("allocator", "op-portal", 18740)

#: The site portal - it runs the resources, and is what this example implements.
SITE = Agent("site", "op-portal", 18741)

#: The awarding portal's bridge. Your Python submits `create_award` through it.
ALLOCATOR_BRIDGE = Agent("allocator_bridge", "op-bridge", 18742, http_port=18752)

#: The site portal's bridge. The FastAPI app in app.py serves *its* jobs.
SITE_BRIDGE = Agent("site_bridge", "op-bridge", 18743, http_port=18753)

#: Start order: a portal listens and its bridge connects in, and `allocator`
#: connects out to `site`, so the listeners come up first. Nothing breaks if
#: they do not - paddington reconnects - but the logs are much easier to read.
AGENTS = (SITE, ALLOCATOR, SITE_BRIDGE, ALLOCATOR_BRIDGE)

BRIDGES = (SITE_BRIDGE, ALLOCATOR_BRIDGE)

#: The FastAPI app (app.py), which is the site portal itself.
APP = "site_portal_app"
APP_PORT = 18780

#: What `app.py` will accept awards from - the awarding portal's agent name.
AWARDING_PORTALS = ALLOCATOR.name

#: The zone the two portals peer in. Not "default", and not cosmetic: an
#: offering registers a *virtual agent* named after the resource, in a zone
#: named `<awarding portal>><this portal>` (op-portal's `sync_offerings`), and a
#: job travelling downstream stays in the zone it arrived in. So a
#: `create_award` that arrives from `allocator` in any other zone reaches `site`
#: and then has nowhere to go: the virtual agent it names is in this zone, and
#: the log says "Connection cluster1 not found" while the caller waits out its
#: timeout. Peer the two portals here and the hand-off lands.
PORTAL_ZONE = f"{ALLOCATOR.name}>{SITE.name}"

#: The resource used in the worked example printed by `start`. Nothing here
#: creates it: a site offers nothing until an operator adds a resource through
#: the app's `POST /offerings`, which is step 1 of those instructions.
OFFERING = "cluster1"


class Failed(Exception):
    """Something went wrong that the user has to fix. The message is the report."""


# --------------------------------------------------------------------------
# Finding and running the agent binaries
# --------------------------------------------------------------------------


def binary(name: str) -> Path:
    """
    Locate an agent binary: `$OPENPORTAL_BIN_DIR`, then `$PATH`, then the
    workspace's own `target/release` and `target/debug`.

    Release is preferred over debug only because it is the faster of the two if
    both happen to be built; either is fine for this.
    """
    override = os.environ.get("OPENPORTAL_BIN_DIR")
    if override:
        candidate = Path(override) / name
        if candidate.is_file():
            return candidate
        raise Failed(f"OPENPORTAL_BIN_DIR is set but {candidate} does not exist")

    found = shutil.which(name)
    if found:
        return Path(found)

    for profile in ("release", "debug"):
        candidate = REPO / "target" / profile / name
        if candidate.is_file():
            return candidate

    raise Failed(
        f"Could not find the {name} binary. Two ways to get it:\n"
        f"  Download it (Linux x86-64; on aarch64 fetch {name}-aarch64 and save "
        f"it under this name):\n"
        f"    curl -L -o {name} {RELEASES}/{name} && chmod +x {name}\n"
        f"    export OPENPORTAL_BIN_DIR=$PWD\n"
        f"  Build it, which is the only option off Linux:\n"
        f"    cd {REPO} && cargo build\n"
        f"  Looked in: $OPENPORTAL_BIN_DIR, $PATH, "
        f"{REPO}/target/release, {REPO}/target/debug"
    )


def agent_cli(agent: Agent, *args: str) -> None:
    """
    Run one of an agent's configuration subcommands.

    `cwd` is the invite directory because `client --add` writes its invite file
    into the *current* directory, with a name it chooses - so this is how those
    end up somewhere we can find them and delete them again.
    """
    command = [str(binary(agent.binary)), "--config-file", str(agent.config), *args]

    INVITE_DIR.mkdir(parents=True, exist_ok=True)

    result = subprocess.run(
        command,
        cwd=INVITE_DIR,
        capture_output=True,
        text=True,
    )

    if result.returncode != 0:
        raise Failed(
            "Configuration command failed:\n"
            f"  {' '.join(command)}\n"
            f"{_indent(result.stdout)}{_indent(result.stderr)}"
        )


def _indent(text: str) -> str:
    return "".join(f"  | {line}\n" for line in text.splitlines())


# --------------------------------------------------------------------------
# setup
# --------------------------------------------------------------------------


def is_configured() -> bool:
    """True if a previous `setup` completed - every file it writes is present."""
    return all(agent.config.is_file() for agent in AGENTS) and all(
        bridge.py_config.is_file() for bridge in BRIDGES
    )


def setup(force: bool = False) -> None:
    """
    Write every config file, and connect the agents to each other.

    Safe to call repeatedly: if the configs are all there it does nothing, which
    is what makes `start` able to call it unconditionally. `--force` throws them
    away and starts again - keys and all, so every peer relationship is rebuilt.
    """
    if is_configured() and not force:
        print(f"Already set up in {_short(BASE)} - nothing to do.")
        return

    running = [name for name, _ in _running()]
    if running:
        raise Failed(
            "These are still running, and rewriting their configs underneath "
            f"them would leave them unable to talk to each other: {', '.join(running)}.\n"
            "  Stop them first:  python example.py stop"
        )

    # A half-written setup is worse than none: the keys in one agent's config
    # have to match the keys in its peer's. So start the derived directories
    # afresh, and leave state/ and logs/ alone.
    for directory in (CONFIG_DIR, PY_CONFIG_DIR, INVITE_DIR):
        if directory.exists():
            shutil.rmtree(directory)

    for directory in (CONFIG_DIR, PY_CONFIG_DIR, INVITE_DIR, STATE_DIR, LOG_DIR, RUN_DIR):
        directory.mkdir(parents=True, exist_ok=True)

    print(f"Writing configuration into {_short(BASE)}")

    # 1. The two portals. Nothing distinguishes an "awarding" portal from a
    #    "site" portal in its own config - the roles come from what each one is
    #    asked to do, and from which offerings the site advertises.
    for portal in (ALLOCATOR, SITE):
        print(f"  - {portal.binary} {portal.name} on {portal.url}")
        agent_cli(
            portal,
            "init",
            "--service", portal.name,
            "--url", portal.url,
            "--ip", "127.0.0.1",
            "--port", str(portal.port),
        )

    # 2. The two bridges. `--signal-url` and `--notification-url` are how a
    #    bridge tells the portal software a job has arrived, so the site's point
    #    at the FastAPI app's two endpoints (app.py).
    #
    #    The allocator's bridge is only ever used in the other direction here -
    #    your Python submits through it - so nothing listens on its signal URLs.
    #    They are still set, because a bridge with no signal URL cannot deliver
    #    anything inbound, and pointing them at a port nobody is using makes the
    #    "nothing is listening" complaint in the log honest rather than confusing.
    for bridge, signal_port in ((SITE_BRIDGE, APP_PORT), (ALLOCATOR_BRIDGE, APP_PORT + 1)):
        print(
            f"  - {bridge.binary} {bridge.name} on {bridge.url}, "
            f"HTTP API on {bridge.http_url}"
        )
        agent_cli(
            bridge,
            "init",
            "--service", bridge.name,
            "--url", bridge.url,
            "--ip", "127.0.0.1",
            "--port", str(bridge.port),
            "--bridge-url", bridge.http_url,
            "--bridge-ip", "127.0.0.1",
            "--bridge-port", str(bridge.http_port),
            "--signal-url", f"http://127.0.0.1:{signal_port}/signal/job",
            "--notification-url", f"http://127.0.0.1:{signal_port}/signal/notification",
        )

    # 3. Connect them. Each bridge connects in to its own portal...
    connect(host=ALLOCATOR, guest=ALLOCATOR_BRIDGE, guest_type="bridge")
    connect(host=SITE, guest=SITE_BRIDGE, guest_type="bridge")

    #    ...and `allocator` connects out to `site`, which is the link the whole
    #    example rests on. Either direction of connection would do - one
    #    websocket carries traffic both ways - but *both* ends must declare the
    #    other `--type portal`. That declaration is not decoration: an agent
    #    learns which of its peers is a portal from its own config, and
    #    originates that portal's route from it (portalroutes.rs). Leave it off
    #    and instructions naming the other portal are refused.
    connect(host=SITE, guest=ALLOCATOR, guest_type="portal", zone=PORTAL_ZONE)

    # 4. The config files the `openportal` Python module reads: the bridge's
    #    HTTP URL and its API key. One per bridge, so you can point Python at
    #    either end of the conversation.
    for bridge in BRIDGES:
        agent_cli(bridge, "bridge", "--config", str(bridge.py_config))
        print(f"  - Python config for {bridge.name}: {_short(bridge.py_config)}")

    # Invites carry key material and have all been consumed by now.
    shutil.rmtree(INVITE_DIR, ignore_errors=True)

    print("Set up.")


def connect(host: Agent, guest: Agent, guest_type: str, zone: str = "default") -> None:
    """
    Connect `guest` to `host`: `guest` dials out, `host` listens.

    `host` admits the guest as a client and, in doing so, writes an invite - the
    keys for that one relationship, plus its own name, URL and agent type. The
    guest adds it as a server. This is the whole of peering, and it is the same
    two commands whether the pair is bridge-to-portal or portal-to-portal.

    `--type` declares what `host` will insist `guest` presents itself as; the
    invite carries the mirror of it, so the guest's side needs no flag.
    """
    print(f"  - connecting {guest.name} -> {host.name} (as a '{guest_type}' agent)")

    agent_cli(
        host,
        "client",
        "--add", guest.name,
        "--ip", "127.0.0.1",
        "--type", guest_type,
        "--zone", zone,
    )

    # The invite is named after the *issuer* and the zone, not the guest.
    invites = sorted(INVITE_DIR.glob(f"invite_{host.name}_*.toml"))

    if len(invites) != 1:
        raise Failed(
            f"Expected exactly one invite from {host.name} in {_short(INVITE_DIR)}, "
            f"found {len(invites)}"
        )

    agent_cli(guest, "server", "--add", str(invites[0]))
    invites[0].unlink()


# --------------------------------------------------------------------------
# Processes: starting, checking, stopping
# --------------------------------------------------------------------------


@dataclass
class Process:
    """A process we started, as recorded in its pid file."""

    name: str
    pid: int
    #: Substrings that must all appear in the live process's command line for
    #: it to be *our* process. A pid on its own proves nothing - it can be
    #: reused by anything at all between our writing it down and reading it
    #: back, and killing a stranger's process is not a recoverable mistake.
    match: list[str] = field(default_factory=list)

    #: True if `start` found this one already running rather than starting it.
    #: Not part of the pid file - it is about this invocation, not the process.
    resumed: bool = False

    @property
    def pid_file(self) -> Path:
        return RUN_DIR / f"{self.name}.pid"

    def save(self) -> None:
        self.pid_file.write_text(
            json.dumps({"pid": self.pid, "match": self.match}, indent=2) + "\n"
        )

    @classmethod
    def load(cls, name: str) -> "Process | None":
        path = RUN_DIR / f"{name}.pid"

        try:
            record = json.loads(path.read_text())
            return cls(name=name, pid=int(record["pid"]), match=list(record["match"]))
        except (OSError, ValueError, KeyError, TypeError):
            return None

    def is_alive(self) -> bool:
        """
        True if the recorded pid is still running *and* is still what we started.
        """
        command = command_line(self.pid)

        if command is None:
            return False

        return all(fragment in command for fragment in self.match)


def command_line(pid: int) -> str | None:
    """
    The full command line of `pid`, or None if there is no such process.

    `ps` is asked first because it is what an operator would use to check the
    same thing by hand, and it works on macOS as well as Linux; /proc is the
    fallback for a container with no ps installed.
    """
    try:
        result = subprocess.run(
            ["ps", "-p", str(pid), "-o", "args="],
            capture_output=True,
            text=True,
        )
        if result.returncode == 0 and result.stdout.strip():
            return result.stdout.strip()
        if result.returncode == 0:
            return None
    except (OSError, subprocess.SubprocessError):
        pass

    try:
        return Path(f"/proc/{pid}/cmdline").read_bytes().replace(b"\0", b" ").decode(
            errors="replace"
        ).strip()
    except OSError:
        return None


def _all_names() -> list[str]:
    return [agent.name for agent in AGENTS] + [APP]


def _running() -> list[tuple[str, Process]]:
    """Every process we started that is still alive, in start order."""
    alive = []

    for name in _all_names():
        process = Process.load(name)
        if process is not None and process.is_alive():
            alive.append((name, process))

    return alive


def start(
    name: str,
    command: list[str],
    match: list[str],
    cwd: Path,
    env: dict[str, str] | None = None,
) -> Process:
    """
    Start one process in the background, logging to `data/logs/<name>.log`.

    `start_new_session` puts it in its own process group, which does two useful
    things: it survives this script exiting, and `stop` can signal the whole
    group, so anything it spawned goes with it.
    """
    existing = Process.load(name)

    if existing is not None and existing.is_alive():
        existing.resumed = True
        return existing

    log = LOG_DIR / f"{name}.log"

    full_env = dict(os.environ)
    full_env.setdefault("RUST_LOG", "info")
    full_env.update(env or {})

    with open(log, "ab") as handle:
        handle.write(f"\n=== started {time.strftime('%Y-%m-%d %H:%M:%S')} ===\n".encode())
        handle.flush()

        popen = subprocess.Popen(
            command,
            cwd=cwd,
            env=full_env,
            stdin=subprocess.DEVNULL,
            stdout=handle,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )

    process = Process(name=name, pid=popen.pid, match=match)
    process.save()

    return process


def port_taken(port: int) -> bool:
    """
    True if something already holds `port` on loopback.

    Tested by trying to bind it ourselves rather than by connecting to it: a
    connection to an agent's websocket port that then goes away is logged by
    that agent as a failed handshake, and a scary-looking error in the log of a
    process that started perfectly well is the opposite of helpful. Binding asks
    the kernel the same question and leaves no trace.
    """
    with socket.socket() as probe:
        try:
            probe.bind(("127.0.0.1", port))
            return False
        except OSError:
            return True


def wait_for_port(port: int, process: Process, timeout: float = 20.0) -> None:
    """
    Wait until `port` is held, or `process` has died.

    A dead process is reported with the tail of its own log, because that is
    where the reason is, and hunting for it is exactly the friction this script
    exists to remove.
    """
    deadline = time.monotonic() + timeout

    while time.monotonic() < deadline:
        if not process.is_alive():
            raise Failed(
                f"{process.name} exited immediately. The end of its log:\n"
                f"{_indent(_tail(LOG_DIR / f'{process.name}.log'))}"
            )

        if port_taken(port):
            return

        time.sleep(0.25)

    raise Failed(
        f"{process.name} started (pid {process.pid}) but nothing is listening on "
        f"port {port} after {timeout:.0f}s. The end of its log:\n"
        f"{_indent(_tail(LOG_DIR / f'{process.name}.log'))}"
    )


def _tail(path: Path, lines: int = 20) -> str:
    try:
        return "\n".join(path.read_text(errors="replace").splitlines()[-lines:])
    except OSError:
        return f"(no log at {path})"


def stop(quiet: bool = False) -> None:
    """
    Stop everything, youngest first, and only after checking each pid is ours.

    SIGTERM, then SIGKILL for anything that has not gone after ten seconds.
    """
    stopped = False

    for name in reversed(_all_names()):
        process = Process.load(name)

        if process is None:
            continue

        if not process.is_alive():
            # Either it exited on its own, or that pid now belongs to something
            # else entirely. Signalling it in the second case is how a script
            # like this kills a stranger's process, so it does not.
            command = command_line(process.pid)
            if command is not None and not quiet:
                print(
                    f"  - {name}: pid {process.pid} is now '{command.split()[0]}', "
                    "not ours - leaving it alone"
                )
            process.pid_file.unlink(missing_ok=True)
            continue

        print(f"  - stopping {name} (pid {process.pid})")
        _signal(process, signal.SIGTERM)

        deadline = time.monotonic() + 10.0
        while time.monotonic() < deadline and process.is_alive():
            time.sleep(0.2)

        if process.is_alive():
            print("    ...still there, sending SIGKILL")
            _signal(process, signal.SIGKILL)
            time.sleep(0.2)

        process.pid_file.unlink(missing_ok=True)
        stopped = True

    if not stopped and not quiet:
        print("Nothing was running.")


def _signal(process: Process, sig: int) -> None:
    """
    Signal the process group if we can, the process itself otherwise.

    The group is the right target: `start_new_session` made each process its own
    group leader, so the group is exactly it and whatever it spawned.
    """
    try:
        os.killpg(os.getpgid(process.pid), sig)
    except (ProcessLookupError, PermissionError, OSError):
        try:
            os.kill(process.pid, sig)
        except OSError:
            pass


# --------------------------------------------------------------------------
# start
# --------------------------------------------------------------------------


def check_python_deps() -> None:
    """
    Fail early, and usefully, if the app's imports are not there.

    Worth pre-empting rather than leaving to uvicorn, whose own failure would be
    a traceback in a log file this script started - so it is out of sight, and
    it names the module without naming the fix.
    """
    missing = [
        module
        for module in ("openportal", "fastapi", "uvicorn")
        if not _importable(module)
    ]

    if missing:
        raise Failed(
            "The FastAPI app cannot start - these Python modules are missing: "
            + ", ".join(missing)
            + f"\n  pip install -r {_short(HERE / 'requirements.txt')}\n"
            + "  (`openportal` is on PyPI and comes with them, so no Rust "
            "toolchain is needed for this;\n"
            + f"   to use one built from this checkout instead: cd {REPO} && "
            "make python)\n"
            + "  The agents need none of them, so `start` gets this far without "
            "them."
        )


def _importable(module: str) -> bool:
    try:
        __import__(module)
        return True
    except ImportError:
        return False


def start_all() -> None:
    """Set up if needed, start the four agents and the app, then explain how to use them."""
    setup()
    check_python_deps()

    print("\nStarting the agents")

    for agent in AGENTS:
        process = start(
            agent.name,
            [str(binary(agent.binary)), "--config-file", str(agent.config), "run"],
            # `<binary> ... <its own config file>` identifies one agent
            # uniquely - two agents share a binary, but never a config.
            match=[agent.binary, str(agent.config)],
            cwd=BASE,
        )
        wait_for_port(agent.ready_port, process)
        print(
            f"  - {agent.name} up on {agent.url}"
            + (f", HTTP API on {agent.http_url}" if agent.http_port else "")
            + f" (pid {process.pid}"
            + (", already running)" if process.resumed else ")")
        )

    print("\nStarting the site portal's FastAPI app")

    app = start(
        APP,
        [
            sys.executable,
            "-m",
            "uvicorn",
            "app:app",
            "--host",
            "127.0.0.1",
            "--port",
            str(APP_PORT),
        ],
        match=["uvicorn", "app:app", str(APP_PORT)],
        cwd=HERE,
        env={
            # Which bridge the app serves - the site's, always. This is the one
            # variable that decides whether app.py is a site portal or a very
            # confused awarding one.
            "OPENPORTAL_CONFIG": str(SITE_BRIDGE.py_config),
            "PORTAL_STATE_DIR": str(STATE_DIR),
            "PORTAL_AWARDING_PORTALS": AWARDING_PORTALS,
        },
    )
    wait_for_port(APP_PORT, app)
    print(
        f"  - app listening on http://127.0.0.1:{APP_PORT} (pid {app.pid}"
        + (", already running)" if app.resumed else ")")
    )

    instructions()


def instructions() -> None:
    """Print what is running and, more usefully, what to do with it."""
    award = f"myaward1.{ALLOCATOR.name}"

    # Step 6 shows both a finalisation that works and one that is refused, so it
    # needs the current month and a month that has ended. Computed here rather
    # than left as `YYYY-MM` because the difference between the two is the point
    # being made, and a placeholder hides it.
    today = datetime.date.today()
    this_month = f"{today.year:04d}-{today.month:02d}"
    previous = today.replace(day=1) - datetime.timedelta(days=1)
    last_month = f"{previous.year:04d}-{previous.month:02d}"
    #: The award as the README's step 2 spells it - same name, same member, same
    #: role - so a reader moving between the two is looking at one award and not
    #: two. Spaces inside the JSON are fine: everything after the identifier is
    #: the details blob.
    details = (
        '{"name":"My First Award","template":"standard",'
        '"members":{"alice@example.com":"Project Lead"}}'
    )
    member = "alice@example.com"
    member2 = "bob@example.com"

    #: The same award with a second member added - what step 7 sends. Note it
    #: repeats the name and template: an update carries the whole of
    #: AwardDetails, not just the parts that changed.
    updated = (
        '{"name":"My First Award","template":"standard",'
        '"members":{"alice@example.com":"Project Lead",'
        '"bob@example.com":"Project Member"}}'
    )

    print(
        f"""
Everything is up.

  {ALLOCATOR.name:<17}  awards portal        {ALLOCATOR.url}
  {SITE.name:<17}  site portal          {SITE.url}
  {ALLOCATOR_BRIDGE.name:<17}  awards bridge API    {ALLOCATOR_BRIDGE.http_url}
  {SITE_BRIDGE.name:<17}  site bridge API      {SITE_BRIDGE.http_url}
  {'app.py':<17}  the site portal      http://127.0.0.1:{APP_PORT}  (docs at /docs)

1. **The site has to offer a resource first.** A fresh portal advertises
   nothing, so there is nothing for the awards portal to address yet - and a
   request for a resource that is not advertised is *held*, not refused, which
   looks exactly like nothing happening. So start here:

     curl -X POST http://127.0.0.1:{APP_PORT}/offerings \\
          -H 'content-type: application/json' \\
          -d '{{"name": "{OFFERING}", "templates": ["standard", "large"]}}'

   That registers `{OFFERING}.{SITE.name}.{ALLOCATOR.name}` with the agents: a
   virtual agent on `{SITE.name}` that `{ALLOCATOR.name}` may address directly.
   `templates` is required and has no default - which templates a resource
   accepts is the site's decision, so it has to be stated rather than guessed.
   The other two endpoints are

     curl http://127.0.0.1:{APP_PORT}/offerings                    # what is offered
     curl -X DELETE http://127.0.0.1:{APP_PORT}/offerings/{OFFERING}   # withdraw it

   and withdrawing keeps the awards made on it, it just stops them being
   reachable. Add a second resource the same way -

     -d '{{"name": "cluster2", "templates": ["standard"]}}'

   and README step 8 becomes visible too: asking `cluster2` about an award that
   lives on `{OFFERING}` answers with an empty report rather than an error, which
   is what lets an awards portal sweep every resource to find one.

2. Ask the awards portal to place an award on that resource, from Python:

     import openportal
     openportal.load_config("{_short(ALLOCATOR_BRIDGE.py_config)}")

     job = openportal.run(
         '{ALLOCATOR.name}.{SITE.name}.{OFFERING} create_award {award} '
         '{details}',
         30000)
     print(job.state, job.error_message or job.result)

   The first answer is an error - ManagedProjectPendingError - and that is
   correct: this site wants a human to look at every award first (README steps 2
   and 3). The awards portal would simply keep asking. (Read `job.result` on an
   errored job and it raises that error as an exception, which is why the line
   above looks at `error_message` first.)

   If it hangs for 30 seconds and comes back unfinished instead, the resource in
   the destination is not one the site offers - go back to step 1.

3. Approve it, as the site's operators would:

     curl http://127.0.0.1:{APP_PORT}/awards

     curl -X POST http://127.0.0.1:{APP_PORT}/awards/{OFFERING}/{award}/approve \\
          -H 'content-type: application/json' \\
          -d '{{"project": "myproject1", "reason": "approved by the panel"}}'

4. Repeat step 2. It now succeeds, and returns the mapping both portals hold
   from here on: {award}:myproject1.{SITE.name}

5. Push today's usage in - against *our* project identifier, not the award's:

     TODAY=$(date +%F)

     curl -X PUT http://127.0.0.1:{APP_PORT}/projects/myproject1.{SITE.name}/usage \\
          -H 'content-type: application/json' \\
          -d "{{\\"hours\\": {{\\"$TODAY\\": {{\\"{member}\\": 12.5}}}}}}"

   Two things to notice. The figures are filed under `myproject1.{SITE.name}`,
   because your accounting produces numbers for your own projects and has never
   heard of `{award}` - the mapping from step 4 is what joins the two.

   And the day has to be **today**, which is why it comes from `date` rather than
   being typed. A day's usage is billed to whichever award the project was
   attached to during that day, and it was attached in step 3 - so a date before
   that belongs to no award, and step 6 would come back empty and leave you
   wondering why. The reply names the award the day will be billed to
   (`billing_to`), which is the quickest way to notice that it is not the one
   you expected.

6. Ask the awards portal for it. This is where the translation shows:

     import openportal
     openportal.load_config("{_short(ALLOCATOR_BRIDGE.py_config)}")

     report = openportal.run(
         '{ALLOCATOR.name}.{SITE.name}.{OFFERING} get_usage_report {award} today',
         30000).result

     print(report)
     print("complete?", report.is_complete)
     for user in report.users:
         print(user, report.usage(user), report.user_mapping[user])

   `today` is a keyword the date-range grammar understands, so nothing needs
   substituting on this side. What comes back is in the **awards portal's**
   namespace:

     {award}
     <today>
       alice.{award}: 12.500 hours
     Daily total: 12.500 hours

   The usage went in against `{member}` on `myproject1.{SITE.name}`
   and comes out as `alice.{award}` - while `user_mapping` still carries the
   email, because that is the same person either way.

   `complete?` is False, and it decides whether the awards portal asks about this
   month again. Declaring a month settled is what stops it:

     curl -X POST http://127.0.0.1:{APP_PORT}/projects/myproject1.{SITE.name}/usage/finalise \\
          -H 'content-type: application/json' \\
          -d '{{"month": "{last_month}"}}'

   `{last_month}` is last month, and this is the honest demonstration available
   today: the current month cannot be finalised, and this app refuses it -

     -d '{{"month": "{this_month}"}}'   ->  400, "{this_month} is the current month,
                                       so its figures can still change"

   "these figures will not change" cannot be true while the month is still
   running. So the month you have usage in is the one you cannot close, and the
   ones you can close have nothing in them - which is worth noticing rather than
   working around, because a finalised empty month tells the awards portal
   "nothing was used, and that is settled" and is believed. README step 7 is the
   one worth reading twice.

7. Later, the awards portal adds someone to the award:

     job = openportal.run(
         '{ALLOCATOR.name}.{SITE.name}.{OFFERING} update_award {award} '
         '{updated}',
         30000)
     print(job.state, job.error_message or job.result)

   That answers with the mapping again, and needs no approval: the award was
   approved in step 3, and this changes its metadata rather than its attachment.

     curl http://127.0.0.1:{APP_PORT}/awards/{OFFERING}/{award}

   now shows both members with their roles. (The `/awards` listing summarises -
   it names the members but not their roles; the single-award view above holds
   the details exactly as the awards portal sent them.)

   Two things about it are easy to get wrong. The member list is the **whole**
   set, not a delta - send an update without `{member2}` and they have been
   removed, because there is no "remove_member". And the awards portal must name
   the right template every time, on an update exactly as on a create: a missing
   or unoffered one is a ManagedProjectRejectedError, which is terminal, so the
   award is recorded as errored rather than retried.

   Then walk the rest of the README - moving the award to another project, and
   removing it.

To drive the *site's* own bridge from Python instead, load the other config:

     openportal.load_config("{_short(SITE_BRIDGE.py_config)}")

Logs are in {_short(LOG_DIR)}/ - one per process, and worth watching:

     tail -f {_short(LOG_DIR)}/*.log

  python example.py status    what is running
  python example.py stop      stop it all
  python example.py start     start anything that is not running
  python example.py clean     stop it all and delete {_short(BASE)}
"""
    )


def status() -> None:
    """One line per process, whether it is up or not."""
    if not BASE.exists():
        print(f"Nothing set up - {_short(BASE)} does not exist.")
        return

    print(f"{'process':<20} {'pid':>8}  state")

    for name in _all_names():
        process = Process.load(name)

        if process is None:
            print(f"{name:<20} {'-':>8}  not started")
        elif process.is_alive():
            print(f"{name:<20} {process.pid:>8}  running")
        else:
            print(f"{name:<20} {process.pid:>8}  gone (stale pid file)")

    print(f"\nconfigured: {'yes' if is_configured() else 'no'}   base: {_short(BASE)}")


def clean() -> None:
    """Stop everything, then delete the whole base directory."""
    stop(quiet=True)

    if BASE.exists():
        shutil.rmtree(BASE)
        print(f"Removed {_short(BASE)}")
    else:
        print(f"{_short(BASE)} does not exist - nothing to remove.")


def _short(path: Path) -> str:
    """A path relative to the example directory, for readable output."""
    try:
        return str(path.relative_to(HERE))
    except ValueError:
        return str(path)


# --------------------------------------------------------------------------


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    commands = parser.add_subparsers(dest="command", required=True)

    setup_parser = commands.add_parser("setup", help="write the config files")
    setup_parser.add_argument(
        "--force",
        action="store_true",
        help="throw away the existing configs (and keys) and write them again",
    )

    commands.add_parser("start", help="set up if needed, then start everything")
    commands.add_parser("status", help="show what is running")
    commands.add_parser("stop", help="stop everything")
    commands.add_parser("clean", help="stop everything and delete ./data")

    args = parser.parse_args()

    try:
        if args.command == "setup":
            setup(force=args.force)
        elif args.command == "start":
            start_all()
        elif args.command == "status":
            status()
        elif args.command == "stop":
            stop()
        elif args.command == "clean":
            clean()
    except Failed as e:
        print(f"\n{e}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        return 130

    return 0


if __name__ == "__main__":
    sys.exit(main())
