#!/bin/sh
# SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
# SPDX-License-Identifier: MIT
#
# A stand-in for `lfs`, used by the Lustre engine tests. It keeps each
# directory's `lfs project -d` line in a file under a state directory, and logs
# every recursive assignment it is asked to make.
#
# usage: fake_lfs.sh <state-dir> seed <dir> <id> <flag>
#        fake_lfs.sh <state-dir> project -d <dir>
#        fake_lfs.sh <state-dir> project -srp <id> <dir>

state="$1"
shift

key() { echo "$1" | tr '/' '_'; }

if [ "$1" = "seed" ]; then
    echo "$3 $4 $2" > "$state/$(key "$2")"
elif [ "$1" = "project" ] && [ "$2" = "-d" ]; then
    cat "$state/$(key "$3")"
elif [ "$1" = "project" ] && [ "$2" = "-srp" ]; then
    echo "$3 P $4" > "$state/$(key "$4")"
    echo "$3 $4" >> "$state/assigned.log"
else
    echo "fake_lfs: unexpected arguments: $*" >&2
    exit 1
fi
