<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# 7. Running OpenPortal

The aim is to deploy once, and then have the network run without interruption
and heal itself.

## Distribution

- **All Rust.** Asynchronous and memory-safe, with `unsafe` code forbidden
  across the workspace.
- **Static binaries.** Each agent is a single musl-linked executable for Linux
  on x86-64 and aarch64: copy one file and run it.
- **Containers and Helm.** Every release also publishes a container image for
  each agent, and Helm charts for the main ones.
- **Standard logging.** Structured tracing runs through the whole chain, ready
  to ship to a log aggregator or SIEM.
- **High availability.** Client-side high availability uses standby replicas;
  server-side high availability comes from running several replicas behind
  [`op-proxy`](04-paddington.md#when-neither-side-can-listen). See
  [high availability](../specifications/highavailability.md).
- **Health and diagnostics.** The portal can ask any agent in its chain for its
  health and diagnostics, through its bridge.

Releases are built entirely by GitHub Actions: the binaries, the images and the
charts. [Agent configuration](../specifications/agent-configuration.md) covers
setting each agent up and peering it.

## Versioning and compatibility

- **Semantic versioning.** A minor release adds functionality or grammar; a
  major release would signal a breaking change. Every change is recorded in the
  [changelog](../../CHANGELOG.md).
- **Agents introduce themselves.** On connecting, each agent announces its role,
  its engine version, and the name and version of its Domain.
- **Features are negotiated.** Newer behaviour - portal routes, or structured
  errors on a failed Job, for example - is only used with peers that advertise
  support for it.
- **Older peers keep working.** New fields are optional on the wire, so a
  message from an older peer still deserialises, and a network can be upgraded
  one agent at a time.

The one exception is the vocabulary itself: an agent that has to *parse* a new
instruction in order to forward it needs a version that knows that instruction.
See [greatwestern](06-greatwestern.md#adding-new-instructions).
