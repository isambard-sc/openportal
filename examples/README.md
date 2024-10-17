# OpenPortal Examples

These examples aim to both demonstrate and teach the components that make
up OpenPortal. Each example is self-contained and demonstrates one aspect
of OpenPortal.

It is best to work through the examples in order, as the concepts in
OpenPortal build on each other.

## Concepts

OpenPortal is a collection of executables that all link to a common set
of OpenPortal crates (libraries).

The two main components of OpenPortal are:

1. `paddington` - this is a crate that implements the secure websocket
   peer-to-peer protocol that lets services communicate with each other.
   Using `paddington`, you can create a distributed, secure peer-to-peer
   network of services.

2. `templemeads` - this is a crate that implements the "Agent" concept.
   Agents are services that have particular roles in OpenPortal. Agents
   communicate with each other, sending Jobs (tasks) to each other which
   are managed on Job Boards. The Agents collectively manage a distributed
   set of Jobs in a robust manner, such that the system can recover from
   the failure of individual Agents.

## Agents

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
   Agent.

2. `provider` - this is an Agent that represents an infrastructure provider,
   e.g. single supercomputing centre. The `provider` Agent is responsible
   for receiving requests from the `portal` Agent, and then deciding how
   those requests should be fulfilled. For example, the project owner's
   request to add a colleague to their project may require the `provider`
   to add the colleague to the slurm cluster that is part of that project.
   The `op-provider` executable implements the `provider` Agent.

3. `platform` - these are Agents that represent a class of infrastructure
   platforms that are provided by the infrastructure `provider`.
   For example, the infrastructure `provider` may provide slurm clusters
   and Jupyter notebooks. In this case, there will be one `platform` agent
   responsible for managing the slurm clusters, and another `platform`
   agent responsible for managing the Jupyter notebook. For example,
   adding the colleague to the project may require adding them to a slurm
   cluster. So the `provider` Agent will send a Job to the `platform`
   Agent to tell it to add the colleague to the slurm cluster.
   The `op-platform` executable implements the `platform` Agent.

4. `instance` - these are agents that represent individual instances of
   a platform. For example, each indvidual slurm cluster or Jupyter notebook
   service would have an accompanying `instance` Agent. The `instance`
   Agent is responsible for managing the lifecycle of the instance and
   tasks relating to that instance. For example, the `provider` Agent for
   slurm clusters would pass on the request to add the colleague to the
   individual `instance` Agent that is responsible for managing the
   specific slurm cluster to which the colleague is being added.
   The `op-slurm` executable implements the `instance` Agent for slurm
   clusters.

5. `account` - these are Agents that interface with user account management
   services, e.g. LDAP, FreeIPA etc. There is one `account` Agent per
   account management service, e.g. the `freeipa` Agent manages
   accounts in FreeIPA. The `account` Agent is responsible for acting
   on Jobs that request the addition or deletion of user accounts, and
   for actually connecting to, e.g. FreeIPA, and performing the
   necessary operations. The `op-freeipa` executable implements the
   `account` Agent for FreeIPA.

6. `filesystem` - these are Agents that manage files and directories on the
   infrastructure. The `filesystem` Agent running on a cluster would be responsible
   for running the actual commands to create project and user directories
   when it receives a request to add a new user to a project. Or,
   for deleting a user's files when they are removed from a project.
   The `op-filesystem` executable implements the `filesystem` Agent.

7. `bridge` - OpenPortal is implemented in Rust, while portals are typically
   implemented in other languages (e.g. Python). The `bridge` Agent is
   responsible for bridging between the Rust-based OpenPortal network and
   the actual code in the portal. For example, the `op-bridge` executable
   implements a bridge for Python-based portals. It runs a simple local
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
(via the `pyo3` crate) that provides a simple Python API to call into
the OpenPortal network via the `bridge` Agent.

## Examples: paddington

To start, it is best to look first at the `paddington` crate. These examples
show how `paddington` works, and the key concepts of how it is used to
create a secure peer-to-peer network.

### Example 1: echo

The [echo](echo) example demonstrates the simplest possible OpenPortal

## Examples: templemeads

### Example 1: ping-pong
