#!/bin/bash
# SPDX-FileCopyrightText: © 2026 David John <David.John@bristol.ac.uk>
# SPDX-License-Identifier: MIT
set -euo pipefail

# Build the project and create an OCI image containing it.

function artifact_path {
  echo "${1}" | jq --raw-output 'select(.reason == "compiler-artifact") | select(.target.name == "'"${2}"'") | .executable'
}

out=$(cargo build --package op-k8s --target=x86_64-unknown-linux-musl --message-format=json ${@-})
cp "$(artifact_path "${out}" "op-k8s")" oci/k8s

cd oci/k8s

version=$(./op-k8s --version | tail -n1 | cut -d' ' -f 2)
image_id=$(
  podman build . --tag=op-k8s:latest --tag=op-k8s:"${version}" \
    --annotation="org.opencontainers.image.source=https://github.com/isambard-sc/openportal" \
    --annotation="org.opencontainers.image.description=OpenPortal" \
    --annotation="org.opencontainers.image.licenses=MIT" \
    | tee /dev/fd/2 \
    | tail -n1
)
rm op-k8s
echo "Built op-k8s image:" 1>&2
echo "${image_id}"
