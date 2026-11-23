#!/bin/sh
# SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
# SPDX-License-Identifier: MIT
#
# A stand-in for `lfs`, used by the Lustre engine tests. It keeps each
# directory's `lfs project -d` line in a file under a state directory, keeps
# each quota ID's block limit, and logs every recursive walk and every limit it
# is asked to set.
#
# A file `<state-dir>/delay` makes each recursive walk sleep that many seconds
# first; a file `<state-dir>/fail` makes each recursive walk fail.
#
# usage: fake_lfs.sh <state-dir> seed <dir> <id> <flag>
#        fake_lfs.sh <state-dir> project -d <dir>
#        fake_lfs.sh <state-dir> project -srp <id> <dir>
#        fake_lfs.sh <state-dir> project -Cr <dir>
#        fake_lfs.sh <state-dir> setquota -p <id> -B <limit> -I <inodes> <mount>
#        fake_lfs.sh <state-dir> quota -p <id> <mount>

state="$1"
shift

key() { echo "$1" | tr '/' '_'; }

walk() {
    if [ -f "$state/delay" ]; then
        sleep "$(cat "$state/delay")"
    fi
    if [ -f "$state/fail" ]; then
        echo "fake_lfs: walk failed" >&2
        exit 1
    fi
}

if [ "$1" = "seed" ]; then
    echo "$3 $4 $2" > "$state/$(key "$2")"
elif [ "$1" = "project" ] && [ "$2" = "-d" ]; then
    cat "$state/$(key "$3")"
elif [ "$1" = "project" ] && [ "$2" = "-srp" ]; then
    # like lfs, the top directory is tagged before the tree is walked
    echo "$3 P $4" > "$state/$(key "$4")"
    walk
    echo "$3 $4" >> "$state/assigned.log"
elif [ "$1" = "project" ] && [ "$2" = "-Cr" ]; then
    echo "0 - $3" > "$state/$(key "$3")"
    walk
    echo "$3" >> "$state/cleared.log"
elif [ "$1" = "setquota" ] && [ "$2" = "-p" ]; then
    echo "$5" > "$state/limit_$3"
    echo "$3 $5" >> "$state/setquota.log"
elif [ "$1" = "quota" ] && [ "$2" = "-p" ]; then
    limit=0
    if [ -f "$state/limit_$3" ]; then
        limit=$(cat "$state/limit_$3")
    fi
    case "$limit" in
        *M) limit=$(( ${limit%M} * 1024 )) ;;
    esac
    echo "Disk quotas for prj $3 (pid $3):"
    echo "     Filesystem  kbytes   quota   limit   grace   files   quota   limit   grace"
    echo "$4 0 0 $limit - 0 0 0 -"
else
    echo "fake_lfs: unexpected arguments: $*" >&2
    exit 1
fi
