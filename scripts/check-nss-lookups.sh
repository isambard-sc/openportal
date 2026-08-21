#!/usr/bin/env bash
# SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
# SPDX-License-Identifier: CC0-1.0
#
# Guard against a libc user/group lookup creeping back into the workspace.
#
# Release binaries are statically linked against musl (see
# .github/workflows/build.yml), and musl has no NSS implementation at all.
# `getpwnam_r`/`getgrnam_r` - which is what `nix::unistd::User::from_name` and
# `Group::from_name` call - therefore read /etc/passwd and /etc/group and, on a
# miss, make a single attempt over musl's own minimal nscd-protocol client.
# There is no nsswitch.conf, no sss module and no fallback, so in production:
#
#   - with nscd not running, musl reports the failed connect() as *not found*,
#     and every directory-backed group looks as though it does not exist;
#   - with nscd merely busy, an incomplete exchange is reported as EIO.
#
# Neither reaches SSSD, so neither appears in its logs. This cost us weeks of
# intermittent, unreproducible-by-hand job failures, and the call site looks
# entirely correct at review time - `from_name` is the obvious, idiomatic thing
# to reach for, and it works fine in a debug build on a dev machine, which is
# exactly why this needs to be structural rather than remembered.
#
# Resolve names through `crate::nameservice` (filesystem/src/nameservice.rs),
# which shells out to the host's glibc-dynamic `getent` and so consults every
# source in nsswitch.conf whether or not nscd is healthy.

set -euo pipefail

cd "$(dirname "$0")/.."

# The libc lookups, by any of the spellings that reach them.
pattern='(User|Group)::from_name|\bgetpwnam|\bgetgrnam|\bgetpwuid|\bgetgrgid'

# Comment lines are excluded: the modules that explain *why* this rule exists
# necessarily name the functions it forbids.
matches=$(grep -rnE --include='*.rs' "${pattern}" . \
    | grep -v '^\./target/' \
    | grep -vE '^[^:]+:[0-9]+:[[:space:]]*(//|/\*|\*)' \
    || true)

if [ -n "${matches}" ]; then
    cat >&2 <<MSG
error: user/group lookup through libc

${matches}

Release binaries are statically linked against musl, which has no NSS
implementation, so these calls see only /etc/passwd and /etc/group plus a
single fragile nscd attempt. A directory-backed name then looks absent
whenever nscd is down, and reports EIO whenever nscd is merely busy.

Resolve names with \`crate::nameservice::resolve_uid\` /
\`resolve_gid\` instead - see filesystem/src/nameservice.rs, which documents
this in full, and the "Resolve users and groups via getent" entry in
CHANGELOG.md.
MSG
    exit 1
fi

echo "ok: no user/group lookups through libc"
