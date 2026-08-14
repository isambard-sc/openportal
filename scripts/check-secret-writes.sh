#!/usr/bin/env bash
# SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
# SPDX-License-Identifier: CC0-1.0
#
# Guard against a plain file write creeping back onto a path that handles key
# material.
#
# Every file OpenPortal writes that contains a key - the service config, the
# paddington invites, the bridge invite - must go through
# `paddington::config::write_secret_file`, which creates the file at mode 0600
# rather than at the process umask. Two rounds of security review found the same
# regression independently (findings F9 and R9): a new call site reached for
# `std::fs::write`, and the key landed group- and world-readable.
#
# `write_secret_file` is easy to bypass by accident and the mistake is invisible
# at review time, so this asserts structurally that no *new* bare write appears.
# The allowlist below is the complete set of writes that legitimately do not
# handle secrets; adding to it should be a deliberate, reviewed decision.

set -euo pipefail

cd "$(dirname "$0")/.."

# path:reason - each of these has been checked and writes no key material
ALLOWED=(
    "paddington/src/config.rs"          # write_secret_file's own non-unix fallback
    "cloudaccount/src/state.rs"         # project/user assignment state
    "cloudportal/src/state.rs"          # award state
    "filesystem/src/fakequotaengine.rs" # test double for a quota backend
)

pattern='(std|tokio)::fs::write\('

matches=$(grep -rnE --include='*.rs' "${pattern}" . \
    | grep -v '^\./target/' \
    | grep -v '/tests/' \
    || true)

violations=""

while IFS= read -r line; do
    [ -z "${line}" ] && continue
    file="${line%%:*}"
    file="${file#./}"

    allowed=false
    for a in "${ALLOWED[@]}"; do
        if [ "${file}" = "${a}" ]; then
            allowed=true
            break
        fi
    done

    if [ "${allowed}" = false ]; then
        violations="${violations}${line}"$'\n'
    fi
done <<< "${matches}"

if [ -n "${violations}" ]; then
    cat >&2 <<EOF
error: bare file write outside the allowlist

${violations}
Files containing key material must be written with
\`paddington::config::write_secret_file\`, which creates them at mode 0600
instead of at the process umask. See findings F9 and R9 in
docs/specifications/security-review.md and
docs/specifications/security-review-2.md.

If this write genuinely handles no secrets, add it to ALLOWED in
scripts/check-secret-writes.sh with a note saying why.
EOF
    exit 1
fi

echo "ok: no bare file writes outside the allowlist"
