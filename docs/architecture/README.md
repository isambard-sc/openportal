<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# How OpenPortal works

A guided tour of OpenPortal: why it exists, how its agent network is shaped,
how the three layers underneath it fit together, and what operating it looks
like.

This sits between the short tour in the [top-level README](../../README.md) and
the [specifications](../specifications/README.md). The README tells you *what*
the pieces are; the specifications are the reference for each one. This guide
is about *why* they are shaped the way they are, and how they behave together.
Each page links to the specification that covers its topic in full.

## Reading order

The pages build on each other, so the first time through, read them in order.

| | Page | What it covers |
|---|---|---|
| 1 | [Why OpenPortal](01-why-openportal.md) | The problem it solves, and the idea it is built on |
| 2 | [The agent network](02-the-agent-network.md) | How a deployment is shaped, and what that shape buys you |
| 3 | [Portal to portal](03-portal-to-portal.md) | One portal awarding resources on another |
| 4 | [paddington](04-paddington.md) | Layer 1: who can talk to whom, and how securely |
| 5 | [templemeads](05-templemeads.md) | Layer 2: how work moves, and survives failure |
| 6 | [greatwestern](06-greatwestern.md) | Layer 3: what agents say to each other |
| 7 | [Running OpenPortal](07-running-openportal.md) | Distribution, availability and versioning |
| 8 | [Connecting to an awarding portal](08-connecting-to-an-awarding-portal.md) | A site operator's walkthrough |

And two agents in more depth, because they are the ones that hold the most
privileged credentials:

- [op-freeipa](agents/op-freeipa.md) - accounts, and staying safe against a
  multi-master directory
- [op-slurm](agents/op-slurm.md) - Slurm accounts, limits and accounting

## Two ways in

**If you run a site**, pages 1, 2 and 4 explain what you are deploying and why
it is safe to, page 8 is the walkthrough for connecting to an awarding portal,
and the [site portal example](../../python/examples/site_portal) does it all on
your laptop.

**If you develop portal software**, pages 1, 2 and 6 cover what your portal
needs to know - and, as importantly, what it does not - and the
[Python API](../specifications/python-api.md) and
[bridge API](../specifications/bridge-api.md) are how you talk to the network.

## The names used in this guide

The examples follow one fictional deployment throughout, so that each page can
build on the last.

| Name | What it is |
|---|---|
| `site` | A site's portal agent - its entry point into the network |
| `bridge` | The bridge agent its portal software (for example Waldur) talks to |
| `hpc1`, `hpc2` | Two supercomputers the site runs, each a *provider* |
| `data` | A data service the site also runs, another provider |
| `clusters`, `storage` | Platforms: the kinds of service a provider offers |
| `shared`, `dedicated`, `projects` | Instances: the individual clusters or services |
| `freeipa`, `filesystem`, `slurm` | The agents that do the privileged work |
| `alice.demo.site` | A user, `alice`, in project `demo`, owned by portal `site` |
| `allocator` | An *awarding* portal, which awards time on `site`'s resources |
| `cluster1` | A resource `site` offers to `allocator` |

The awarding portal names - `allocator`, `site`, `cluster1`, `myaward1` - are
the same ones the [site portal example](../../python/examples/site_portal)
uses, so what you read here is what you see when you run it.
