#!/usr/bin/env bash
# SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
# SPDX-License-Identifier: CC0-1.0
#
# Install musl-tools (musl-gcc, needed to link the static musl binaries) on a
# CI runner, falling back to other Ubuntu mirrors if the runner's own are
# unreachable.
#
# A GitHub-hosted runner is configured with a short mirror list that is all
# under ubuntu.com. When those cannot be resolved the plain `apt install`
# fails, and with it every check and every release build. The fallbacks below
# are tried one at a time, each as the *only* apt source for that attempt, so a
# dead mirror cannot stall an attempt that would otherwise succeed.
#
# Using a third-party mirror is safe: apt still verifies everything it
# downloads against the Ubuntu archive key already on the runner.
#
# MUSL_FALLBACK_MIRRORS (space separated) overrides the built-in list.

set -uo pipefail

packages=(musl-tools)

apt_opts=(
    -o Acquire::Retries=1
    -o Acquire::http::Timeout=20
    -o Acquire::https::Timeout=20
)

if command -v musl-gcc >/dev/null 2>&1; then
    echo "musl-gcc is already installed"
    exit 0
fi

echo "::group::Install ${packages[*]} from the runner's own mirrors"
if sudo apt-get install -y "${apt_opts[@]}" "${packages[@]}"; then
    echo "::endgroup::"
    exit 0
fi
echo "::endgroup::"

echo "::warning::Could not install ${packages[*]} from the runner's own mirrors - trying fallback mirrors"

# shellcheck disable=SC1091
. /etc/os-release
codename="${VERSION_CODENAME}"
arch="$(dpkg --print-architecture)"

case "${arch}" in
    amd64)
        default_mirrors=(
            "https://archive.ubuntu.com/ubuntu"
            "https://mirrors.edge.kernel.org/ubuntu"
            "https://mirror.leaseweb.com/ubuntu"
            "https://mirrors.ocf.berkeley.edu/ubuntu"
        )
        ;;
    *)
        default_mirrors=(
            "https://ports.ubuntu.com/ubuntu-ports"
            "https://mirrors.ocf.berkeley.edu/ubuntu-ports"
            "https://ftp.tu-chemnitz.de/pub/linux/ubuntu-ports"
        )
        ;;
esac

if [ -n "${MUSL_FALLBACK_MIRRORS:-}" ]; then
    read -r -a mirrors <<< "${MUSL_FALLBACK_MIRRORS}"
else
    mirrors=("${default_mirrors[@]}")
fi

# Say what the runner can and cannot resolve, so that a failure here can be
# told apart from a problem with one mirror.
echo "::group::Name resolution"
for file in /etc/apt/apt-mirrors.txt; do
    [ -f "${file}" ] && { echo "${file}:"; cat "${file}"; }
done
for mirror in "${mirrors[@]}"; do
    host="$(echo "${mirror}" | sed -E 's#^[a-z]+://([^/]+).*#\1#')"
    if getent hosts "${host}" >/dev/null 2>&1; then
        echo "resolves:         ${host}"
    else
        echo "does not resolve: ${host}"
    fi
done
echo "::endgroup::"

list="$(mktemp --suffix=.list)"
trap 'rm -f "${list}"' EXIT

for mirror in "${mirrors[@]}"; do
    echo "::group::Install ${packages[*]} from ${mirror}"

    for suite in "${codename}" "${codename}-updates" "${codename}-security"; do
        echo "deb [signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] ${mirror} ${suite} main universe"
    done > "${list}"
    chmod 644 "${list}"

    # this mirror is the only source for this attempt
    only=(
        -o "Dir::Etc::sourcelist=${list}"
        -o Dir::Etc::sourceparts=-
        -o APT::Get::List-Cleanup=0
    )

    if sudo apt-get update "${apt_opts[@]}" "${only[@]}" \
        && sudo apt-get install -y "${apt_opts[@]}" "${only[@]}" "${packages[@]}"; then
        echo "::endgroup::"
        echo "Installed ${packages[*]} from ${mirror}"
        exit 0
    fi

    echo "::endgroup::"
    echo "::warning::Could not install ${packages[*]} from ${mirror}"
done

echo "::error::Could not install ${packages[*]} from the runner's mirrors or any fallback"
exit 1
