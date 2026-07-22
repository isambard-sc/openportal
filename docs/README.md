<!--
SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# OpenPortal Design and Examples

OpenPortal is designed to sit between user management portals (e.g. Waldur)
and digital research infrastructure (e.g. the Isambard supercomputers).

Its aim is to separate the communication of user and infrastructure management
tasks (e.g. "create an account on a cluster for a user who has been added
to a project") from the business logic of actually implementing those
tasks (e.g. logging into FreeIPA and adding accounts, getting root access
to the filesystem to create user and project directories etc).

Separation is necessary, as the business logic requires privileged access
to the infrastructure. When this is merged with communication, it forces
either that the user management portal implements the business logic directly,
and so needs "god keys" that grant full access to the infrastructure, or
this requires the infrastructure provider to implements the business logic
in a way that is tightly coupled to the user management portal, with a
corresponding "god key" that grants API access to the portal to enable it
to respond to and manage the communication.

In contrast, OpenPortal aims to implement an infrastructure management
protocol that is separate from the business logic. The protocol comprises
messages that describe standard infrastructure management tasks, e.g.
"add user to cluster", "create notebook service" etc., and a secure
distributed peer-to-peer communications layer that enable trusted and
authenticated communication of those messages between services that implement
the business logic.

Each service is represented by an Agent that is solely responsible for a
single aspect of the business logic. This means that there is no
need for a single executable to have "god keys" that
grant full access to the infrastructure. Instead, Agents coordinate to
implement the business logic in a distributed, secure manner, with full
audit tracing of all actions, and a complete trust hierarchy that ensures
that only the correct Agents can perform the correct actions based on
messages received from trusted sources.

## Design

OpenPortal is a collection of Agents, each of which are implemented in their
own statically compiled executable. Each executable links to a common set
of OpenPortal crates (libraries).

The three main components of OpenPortal are:

1. `paddington` - this is a crate that implements the secure websocket
   peer-to-peer protocol that lets services communicate with each other.
   Using `paddington`, you can create a distributed, secure,
   real-time, peer-to-peer network of services. The source code
   is in the [paddington](../paddington) directory.

2. `templemeads` - this is a crate that implements the "Agent" concept.
   Agents are services that have particular roles in OpenPortal. Agents
   communicate with each other, sending Jobs (tasks) to each other which
   are managed on Job Boards. The Agents collectively manage a distributed
   set of Jobs in a robust manner, such that the system can recover from
   the failure of individual Agents. In addition to Jobs, `templemeads`
   provides a lightweight fire-and-forget **Notification** system for
   signalling events between agents without requiring acknowledgement or
   a result. Deliberately, `templemeads` has no opinion on what a Job
   actually asks an Agent to do - it is generic over a `Domain` (a Rust
   trait), which supplies the concrete instruction and notification
   vocabulary. The source code is in the
   [templemeads](../templemeads) directory.

3. `greatwestern` - this is the `Domain` that every built-in OpenPortal
   Agent is compiled against: the HPC/Waldur command vocabulary (`add_user`,
   `get_usage_report`, `set_project_quota`, and so on), along with the
   identifier types and usage/storage report types those instructions
   operate on. It is the *reference implementation* of a Domain, not a
   privileged part of the protocol - anyone wanting to reuse
   `paddington`/`templemeads` for a completely different kind of
   infrastructure can write their own crate in its place. See
   [Writing your own Domain](specifications/writing-a-domain.md) for how.
   The source code is in the [greatwestern](../greatwestern) directory.

### Agents

There are many types of Agent in OpenPortal. Each Agent operates as a single
executable process. These processes can be distributed, with `paddington`
used to securely connect them together over the internet.

The key types of Agent are:

1. `portal` - this is an Agent that represents user portals. Portals
   are the entry point for users to interact with the OpenPortal network.
   For example, a user who is an owner of a project can request that
   one of their colleagues joins their project. This request is processed
   via the user management software (e.g. Waldur), which then passes that
   request as a Job to the `portal` Agent. The `portal` Agent works out
   which other Agents need to be involved in the request, and sends Jobs
   to those Agents. The `op-portal` executable implements the `portal`
   Agent, with source code in the [portal](../portal) directory.

2. `provider` - this is an Agent that represents an infrastructure provider,
   e.g. single supercomputing centre. The `provider` Agent is responsible
   for receiving requests from the `portal` Agent, and then deciding how
   those requests should be fulfilled. For example, the project owner's
   request to add a colleague to their project may require the `provider`
   to add the colleague to the slurm cluster that is part of that project.
   The `op-provider` executable implements the `provider` Agent,
   with source code in the [provider](../provider) directory.

3. `platform` - these are Agents that represent a class of infrastructure
   platforms that are provided by the infrastructure `provider`.
   For example, the infrastructure `provider` may provide slurm clusters
   and Jupyter notebooks. In this case, there will be one `platform` agent
   responsible for managing the slurm clusters, and another `platform`
   agent responsible for managing the Jupyter notebook. For example,
   adding the colleague to the project may require adding them to a slurm
   cluster. So the `provider` Agent will send a Job to the `platform`
   Agent to tell it to add the colleague to the slurm cluster.
   The `op-clusters` executable implements the `platform` Agent
   for clusters, with source code in the [clusters](../clusters) directory.

4. `instance` - these are agents that represent individual instances of
   a platform. For example, each individual slurm cluster or Jupyter notebook
   service would have an accompanying `instance` Agent. The `instance`
   Agent is responsible for managing the lifecycle of the instance and
   tasks relating to that instance. For example, the `provider` Agent for
   slurm clusters would pass on the request to add the colleague to the
   individual `instance` Agent that is responsible for managing the
   specific slurm cluster to which the colleague is being added.
   The `op-cluster` executable implements the `instance` Agent for
   clusters, with source code in the [cluster](../cluster) directory.

5. `account` - these are Agents that interface with user account management
   services, e.g. LDAP, FreeIPA etc. There is one `account` Agent per
   account management service, e.g. the `freeipa` Agent manages
   accounts in FreeIPA. The `account` Agent is responsible for acting
   on Jobs that request the addition or deletion of user accounts, and
   for actually connecting to, e.g. FreeIPA, and performing the
   necessary operations. The `op-freeipa` executable implements the
   `account` Agent for FreeIPA, with source code in the [freeipa](../freeipa)
   directory.

6. `filesystem` - these are Agents that manage files and directories on the
   infrastructure. The `filesystem` Agent running on a cluster would be responsible
   for running the actual commands to create project and user directories
   when it receives a request to add a new user to a project. Or,
   for deleting a user's files when they are removed from a project.
   The `op-filesystem` executable implements the `filesystem` Agent,
   with source code in the [filesystem](../filesystem) directory.

7. `slurm` - this is an Agent that interfaces directly with the Slurm
   workload manager. It can be configured to use either the `sacctmgr`
   command-line tools or the Slurm REST API. The `slurm` Agent handles
   Jobs relating to Slurm account management (creating and removing
   accounts, setting resource limits etc). The `op-slurm` executable
   implements the `slurm` Agent, with source code in the
   [slurm](../slurm) directory.

8. `cloudaccount` - this is an Agent that represents a single cloud account
   (e.g. one AWS account) assigned to a project. It is a prototype agent
   that merges the `instance` and `slurm`-style roles into one process:
   there is no cloud-side API yet to record which projects/users have been
   assigned to the account, so this Agent is the source of truth for that,
   and it turns whatever cost-report files the cloud operators drop into a
   directory into a usage report. The `op-cloudaccount` executable
   implements this Agent, with source code in the
   [cloudaccount](../cloudaccount) directory. See
   [docs/plans/archive/op-cloudaccount-design.md](plans/archive/op-cloudaccount-design.md)
   for the full design.

9. `cloudportal` - this is an Agent that acts as a self-contained "cloud"
   portal in a portal-to-portal relationship - for example, a central
   portal creating Awards on it, the same way any OpenPortal portal can
   create Awards on a downstream portal. Like `cloudaccount`, it is a
   deliberately rough prototype agent: there is no real portal management
   software (no Waldur) behind it, so it stores Award state itself,
   requires a human operator to approve an Award (via CLI subcommands)
   before anything is provisioned, and then provisions the approved Award
   on whichever `cloudaccount` Agent its `template` field maps to. The
   `op-cloudportal` executable implements this Agent, with source code in
   the [cloudportal](../cloudportal) directory. See
   [docs/plans/archive/op-cloudportal-design.md](plans/archive/op-cloudportal-design.md)
   for the full design.

10. `bridge` - OpenPortal is implemented in Rust, while portals are typically
   implemented in other languages (e.g. Python). The `bridge` Agent is
   responsible for bridging between the Rust-based OpenPortal network and
   the actual code in the portal. For example, the `op-bridge` executable
   implements a bridge for Python-based portals, with source code in the
   [bridge](../bridge) directory. It runs a simple local
   HTTP server that listens for API calls from the OpenPortal Python
   library, and then translates those API calls into OpenPortal Jobs
   that can be sent, via the `portal` Agent, to the rest of the
   OpenPortal network. It then caches the real-time responses of those
   calls, and makes them available to the Python library via Python
   function calls. In this way, the `bridge` not only translates between
   programming languages, but it also provides a caching layer that
   enables imperative or polling based programming models typically
   used for portals, to be used with the asynchronous, real-time communication
   that is the basis of OpenPortal.

Finally, the `python` crate provides a Python library written in rust
(via the [pyo3](https://pyo3.rs/v0.22.5/) crate) that provides a simple Python
API to call into the OpenPortal network via the `bridge` Agent. The source
code is in the [python](../python) directory.

## Examples

These examples aim to both demonstrate and teach the components that make
up OpenPortal. Each example is self-contained and demonstrates one aspect
of OpenPortal.

It is best to work through the examples in order, as the concepts in
OpenPortal build on each other.

### Example 1: echo

The [echo](echo) example demonstrates a pair of simple `paddington` services
which echo messages between each other. This example introduces the key
concepts in `paddington`, such as how to create services, how to introduce
them to each other, how to send messages, and how to implement your own
message handler to process messages.

### Example 2: job

The [job](job) example demonstrates a pair of simple `templemeads` Agents
which send `Jobs` between each other. This example introduces the key
concepts in `templemeads`, such as how to create Agents, how to introduce
them to each other, how to send `Jobs`, and how to implement your own
job handler to implement the business logic.

### Example 3: command line and config files

The [command line](cmdline) example demonstrates how to create agents
in a more standardised way, e.g. with a standardised command line interface
and configuration file. This example shows how to easily create new agents
that will look and feel like other agents in the OpenPortal system.

### Example 4: bridge and Python

The [bridge](bridge) example demonstrates how to connect the OpenPortal
agent network to portal software written in Python using the `op-bridge`
agent and the `openportal` Python library.

## Protocol Specifications

The [specifications](specifications) directory contains formal protocol
and API reference documentation covering the wire protocol, security model,
bridge HTTP API, and agent configuration reference - all of which apply to
any OpenPortal `Domain` - plus the instruction grammar, notification event
grammar, and JSON result types of `greatwestern`, the reference `Domain`.

If you want to bring your own instruction vocabulary instead of
`greatwestern`'s, see
[Writing your own Domain](specifications/writing-a-domain.md).
