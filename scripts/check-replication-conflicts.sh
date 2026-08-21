#!/usr/bin/env bash
# SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
# SPDX-License-Identifier: CC0-1.0
#
# Look for LDAP replication conflicts on the FreeIPA servers that op-freeipa
# writes to.
#
# FreeIPA's multi-master replication cannot reconcile two independent ADDs of
# the same DN. When it meets a pair, 389-ds keeps one copy, renames the other
# to `nsuniqueid=<uuid>+uid=<user>,...` and flags it `nsds5ReplConflict`. Those
# entries are marked `ldapsubentry`, which excludes them from ordinary LDAP
# searches (RFC 3672), so they are invisible to `ipa user-find` and to every
# tool that goes through the IPA framework. A three-master site accumulated 67
# of them over 11 months without anything noticing, and two of the affected
# accounts had home directories owned by a UID that replication had discarded.
#
# op-freeipa no longer creates them (writes go to one master, and it asks every
# master before concluding that a user or group does not exist), but nothing
# self-heals the ones already there and the IPA framework cannot delete them -
# it cannot address a DN containing `nsuniqueid=`. So this exists to make them
# visible: it reports, and never modifies anything.
#
# The search has to name `objectclass=ldapsubentry` explicitly, because that is
# what makes 389-ds return subentries at all.
#
# Usage:
#   scripts/check-replication-conflicts.sh ldaps://ipa1.example.com [ldaps://ipa2...]
#
#   -D <bind-dn>   simple bind as this DN, reading the password from
#                  $LDAP_PASSWORD if set, otherwise prompting. Without -D the
#                  search uses GSSAPI, so `kinit admin` first.
#   -b <base-dn>   search base. Defaults to the server's own namingContexts.
#
# Conflict entries may not be readable by an ordinary account. If a run comes
# back empty and you expect otherwise, try `-D "cn=Directory Manager"`.
#
# Exit status: 0 nothing found, 1 conflicts found, 2 could not check.

set -euo pipefail

bind_dn=""
base_dn=""

usage() {
    sed -n '4,37p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
}

while getopts ":D:b:h" opt; do
    case "${opt}" in
        D) bind_dn="${OPTARG}" ;;
        b) base_dn="${OPTARG}" ;;
        h) usage ;;
        *) usage ;;
    esac
done
shift $((OPTIND - 1))

if [ "$#" -eq 0 ]; then
    usage
fi

if ! command -v ldapsearch >/dev/null 2>&1; then
    echo "ERROR: ldapsearch not found - install openldap-clients." >&2
    exit 2
fi

auth=(-Y GSSAPI -Q)

if [ -n "${bind_dn}" ]; then
    if [ -z "${LDAP_PASSWORD:-}" ]; then
        read -r -s -p "Password for ${bind_dn}: " LDAP_PASSWORD
        echo
    fi
    # -y reads the password from a file, which keeps it off the process
    # command line where `ps` would show it
    password_file="$(mktemp)"
    chmod 600 "${password_file}"
    trap 'rm -f "${password_file}"' EXIT
    printf '%s' "${LDAP_PASSWORD}" > "${password_file}"
    auth=(-x -D "${bind_dn}" -y "${password_file}")
fi

# The attributes worth seeing on a conflict: which object it is, when it was
# created (identical timestamps on a user and its private group are the
# signature of one logical add executed twice), and the POSIX IDs, because each
# master draws them from a disjoint DNA range - so the discarded copy's UID may
# own files on disk.
attributes=(
    nsds5ReplConflict createTimestamp uid cn uidNumber gidNumber
    homeDirectory description userClass
)

found=0
failed=0

for server in "$@"; do
    echo "=== ${server}"

    server_base="${base_dn}"

    if [ -z "${server_base}" ]; then
        if ! server_base="$(ldapsearch -LLL -o ldif-wrap=no -H "${server}" \
                                       "${auth[@]}" -s base -b "" namingContexts 2>/dev/null \
                                | sed -n 's/^namingContexts: //p' | head -n 1)"; then
            server_base=""
        fi

        if [ -z "${server_base}" ]; then
            echo "  ERROR: could not read namingContexts - pass -b <base-dn>." >&2
            failed=1
            continue
        fi
    fi

    if ! result="$(ldapsearch -LLL -o ldif-wrap=no -H "${server}" "${auth[@]}" \
                              -b "${server_base}" \
                              '(&(objectclass=ldapsubentry)(nsds5ReplConflict=*))' \
                              "${attributes[@]}" 2>&1)"; then
        echo "  ERROR: search failed:" >&2
        echo "${result}" | sed 's/^/    /' >&2
        failed=1
        continue
    fi

    count="$(printf '%s\n' "${result}" | grep -c '^dn: ' || true)"

    if [ "${count}" -eq 0 ]; then
        echo "  no replication conflicts"
        continue
    fi

    found=1

    # Which of them are ours: op-freeipa stamps users with
    # `userClass: openportal` and puts "OpenPortal-managed" in a group's
    # description.
    ours="$(printf '%s\n' "${result}" \
            | awk 'BEGIN { RS = ""; } /userClass: openportal|OpenPortal-managed/ { print }' \
            | grep -c '^dn: ' || true)"

    echo "  ${count} replication conflict(s), ${ours} of them created by OpenPortal:"
    printf '%s\n' "${result}" | sed 's/^/    /'

    losing_ids="$(printf '%s\n' "${result}" \
                  | sed -n 's/^uidNumber: //p' | sort -u | tr '\n' ' ')"

    if [ -n "${losing_ids}" ]; then
        cat <<GUIDANCE

  Check whether any of these UIDs own files before cleaning up - a home
  directory created against the copy replication discarded will be owned by
  an ID that no longer resolves:

    find /home -xdev \\( $(for id in ${losing_ids}; do printf -- '-uid %s -o ' "${id}"; done | sed 's/ -o $//') \\) -print

  Cleanup itself cannot be done with \`ipa user-del\` or \`ipa group-del\`: the
  framework cannot address a DN containing \`nsuniqueid=\`. It needs raw LDAP as
  Directory Manager - compare both copies attribute by attribute, decide which
  to keep, chown anything owned by the losing IDs, strip Managed Entries
  linkage (the plugin refuses the delete otherwise), then delete in that order.
GUIDANCE
    fi
done

if [ "${failed}" -ne 0 ]; then
    exit 2
fi

if [ "${found}" -ne 0 ]; then
    exit 1
fi

exit 0
