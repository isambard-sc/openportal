#!/bin/bash
# SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
# SPDX-FileCopyrightText: © 2024 Matt Williams <matt.williams@bristol.ac.uk>
# SPDX-License-Identifier: MIT
set -euo pipefail

# Build the project and create an OCI image containing it.

function artifact_path {
  echo "${1}" | jq --raw-output 'select(.reason == "compiler-artifact") | select(.target.name == "'"${2}"'") | .executable'
}

out=$(cargo build --package op-clusters --target=x86_64-unknown-linux-musl --message-format=json ${@-})
cp "$(artifact_path "${out}" "op-clusters")" oci/clusters

cd oci/clusters

version=$(./op-clusters --version | tail -n1 | cut -d' ' -f 2)
image_id=$(
  podman build . --tag=op-clusters:latest --tag=op-clusters:"${version}" \
    --annotation="org.opencontainers.image.source=https://github.com/isambard-sc/openportal" \
    --annotation="org.opencontainers.image.description=OpenPortal" \
    --annotation="org.opencontainers.image.licenses=MIT" \
    | tee /dev/fd/2 \
    | tail -n1
)
rm op-clusters
echo "Built op-clusters image:" 1>&2
echo "${image_id}"
