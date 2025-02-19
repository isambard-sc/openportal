<!--
SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# Command Line and Config Files Example

This is a demo of two templemeads Agents that have been set up to use
the standard command line argument parser and configuration file
handling built into the `templemeads` crate. This example extends
the previous example, in that we the `portal` and `cluster`
agents are now created using a more standardised route. The aims of this
example are to show you how to;

1. Use default options to create standard command line arguments
2. How to create, update and use agent configuration files
3. How to introduce agents using command line options
4. How to run an agent in a standardised way

## Compiling the Example

There are two subdirectories in this example, each containing a
Rust executable. The `portal` agent is in the `portal/src` directory,
while the `cluster` agent is in the `cluster/src` directory.

You have two choices for compiling the example:

1. Compile everything, by going into the top-level directory and
   running `make` (or `cargo build`). This will produce executables
   called `example-portal` and `example-cluster` in the `target/debug`
   directory.

2. Compile only this example by navigating to each of the `portal`
   and `cluster` subdirectories and running `cargo build`. To run,
   you will need to use `cargo run` in each.

However, it is much easier now to use `make` and work with the executables
in the `target/debug` directory. The commands below assume you are using
these executables.

## Running the Example

This example implements two "standardised" Agents;

1. The `portal` agent, which sends jobs to the `cluster` agent. These
   jobs tell the `cluster` to add and remove users from projects,

2. and the `cluster` agent, which receives jobs from the `portal`,
   and which would implement the business logic of adding and removing
   users from projects.

## Running the portal

Because this is now standardised, the `portal` agent has a number of
command line options. You can see these by running the `example-portal`
executable with the `--help` option:

```shell
$ ./target/debug/example-portal --help

A library for interfacing OpenPortal with specific portals

Usage: example-portal [OPTIONS] [COMMAND]

Commands:
  client      Adding and removing clients
  server      Adding and removing servers
  init        Initialise the Service
  extra       Add extra configuration options
  secret      Add secret configuration options
  encryption  Add commands to control encryption of the config file and secrets
  run         Run the service
  help        Print this message or the help of the given subcommand(s)

Options:
  -c, --config-file <CONFIG_FILE>  Path to the configuration file
  -h, --help                       Print help
  -V, --version                    Print version
```

To start, we have to initiailise the `portal`. We do this with the
`init` command. There are optional arguments to this that can be used
to configure how the agent will behave. These can be seen by running
the `init` command with the `--help` option:

```shell
$ ./target/debug/example-portal init --help

Initialise the Service

Usage: example-portal init [OPTIONS]

Options:
  -n, --service <SERVICE>  Name of the service to initialise
  -u, --url <URL>          URL of the service including port and route (e.g. http://localhost:8080)
  -i, --ip <IP>            IP address on which to listen for connections (e.g. 127.0.0.1)
  -p, --port <PORT>        Port on which to listen for connections (e.g. 8042)
  -f, --force              Force reinitialisation
  -h, --help               Print help
```

For now, we will accept all of the default, so just run;

```shell
$ ./target/debug/example-portal init
```

You should see something like this printed out;

```
Service initialised. Config file written to example-portal.toml
```

This shows that the `portal` agent has been initialised, and its
configuration has been written to the file `example-portal.toml`.

Take a look at the file - it should look something like this;

```toml
agent = "Portal"

[service]
name = "portal"
url = "ws://localhost:8090/"
ip = "127.0.0.1"
port = 8090
servers = []
clients = []

[extras]
```

This shows that the type of this agent is `Portal`, and its name is
`portal`. It also gives the URL on which the agent can be contacted,
and the IP address and port on which it will listen for connections.

At the end it gives the list of server and client agents that it will
try to connect to. As there are none at the moment, these lists are
empty.

### Under the hood

These defaults were set in the code in the `portal/src/main.rs` file.

```rust
use anyhow::Result;

use templemeads::agent::portal::{process_args, run, Defaults};
use templemeads::agent::Type as AgentType;

use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber)?;

    // create the default options for a portal
    let defaults = Defaults::parse(
        Some("portal".to_owned()),
        Some(PathBuf::from("example-portal.toml")),
        Some("ws://localhost:8090".to_owned()),
        Some("127.0.0.1".to_owned()),
        Some(8090),
        Some(AgentType::Portal),
    );

    // now parse the command line arguments to get the service configuration
    let config = match process_args(&defaults).await? {
        Some(config) => config,
        None => {
            // Not running the service, so can safely exit
            return Ok(());
        }
    };

    // run the portal agent
    run(config).await?;

    Ok(())
}
```

This line;

```rust
use templemeads::agent::portal::{process_args, run, Defaults};
```

imports the `process_args` and `run` function for `portal` agents, as
well as the default configuration options for portals, in `Defaults`.

This is used in these lines to let us specify the defaults for our
example portal;

```rust
    // create the default options for a portal
    let defaults = Defaults::parse(
        Some("portal".to_owned()),
        Some(PathBuf::from("example-portal.toml")),
        Some("ws://localhost:8090".to_owned()),
        Some("127.0.0.1".to_owned()),
        Some(8090),
        Some(AgentType::Portal),
    );
```

Here, we set the default name of the agent to `portal`, the default
configuration file path to `example-portal.toml`, the default URL to
`ws://localhost:8090`, the default IP address to `127.0.0.1` and
the default port to `8090`. We also set the type of the agent to
`Portal`.

> [!NOTE]
> With the exception of the agent type, all of these options
> can be overridden by command line arguments or values set in the
> configuration file.

Now that that defaults have been set, these lines set up and parse
the command line arguments;

```rust
    // now parse the command line arguments to get the service configuration
    let config = match process_args(&defaults).await? {
        Some(config) => config,
        None => {
            // Not running the service, so can safely exit
            return Ok(());
        }
    };
```

Finally, we enter the event loop for our example portal agent
via;

```rust
    // run the portal agent
    run(config).await?;
```

## Running the cluster

The `cluster` agent is set up in a similar way to the `portal` agent.
You can see the command line options by running the `example-cluster`
executable with the `--help` option:

```shell
A library for interfacing OpenPortal with specific portals

Usage: example-cluster [OPTIONS] [COMMAND]

Commands:
  client      Adding and removing clients
  server      Adding and removing servers
  init        Initialise the Service
  extra       Add extra configuration options
  secret      Add secret configuration options
  encryption  Add commands to control encryption of the config file and secrets
  run         Run the service
  help        Print this message or the help of the given subcommand(s)

Options:
  -c, --config-file <CONFIG_FILE>  Path to the configuration file
  -h, --help                       Print help
  -V, --version                    Print version
```

These are identical to those of the portal. Using the standardised functions
helps maintain consistency between all of the executables that implement
the agents.

We will now initialise the `cluster` agent. As before, we will accept
all of the defaults, so just run;

```shell
$ ./target/debug/example-cluster init
```

You should see that this has initialised the `cluster` agent, and
written its configuration to the file `example-cluster.toml`. This file
should look very similar to the `example-portal.toml` file, e.g.

```toml
agent = "Instance"

[service]
name = "example-cluster"
url = "ws://localhost:8091/"
ip = "127.0.0.1"
port = 8091
servers = []
clients = []

[extras]
```

### Under the hood

Looking in the `cluster/src/main.rs` file, we see that the code is very
similar to that of the `portal` agent.

```rust
// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;

use std::path::PathBuf;
use templemeads::agent::instance::{process_args, run, Defaults};
use templemeads::agent::Type as AgentType;
use templemeads::async_runnable;
use templemeads::grammar::Instruction::{AddUser, RemoveUser};
use templemeads::job::{Envelope, Job};
use templemeads::Error;

#[tokio::main]
async fn main() -> Result<()> {
    // start tracing
    templemeads::config::initialise_tracing();

    // create the OpenPortal paddington defaults
    let defaults = Defaults::parse(
        Some("example-cluster".to_owned()),
        Some(PathBuf::from("example-cluster.toml")),
        Some("ws://localhost:8091".to_owned()),
        Some("127.0.0.1".to_owned()),
        Some(8091),
        Some(AgentType::Instance),
    );

    // now parse the command line arguments to get the service configuration
    let config = match process_args(&defaults).await? {
        Some(config) => config,
        None => {
            // Not running the service, so can safely exit
            return Ok(());
        }
    };

    // run the portal agent
    run(config, cluster_runner).await?;

    Ok(())
}
```

The main differences are that we are importing `process_args`,
`run` and `Defaults` from `templemeads::agent::instance`, and that
we pass in the `cluster_runner` function to the `run` function,
so that it can be used to handle the jobs that are sent to the
`cluster` agent.

> [!NOTE]
> The `cluster_runner` function is not shown here as it
> is identical to the `cluster_runner` function from the previous
> example.

## Introducing the agents

We can now introduce the `portal` and `cluster` agents by asking
the `portal` to create an invitation for the `cluster`.

We do this use the `client` command line option of the `portal` agent.

```shell
$ ./target/debug/example-portal client --help

Adding and removing clients

Usage: example-portal client [OPTIONS]

Options:
  -a, --add <ADD>        Name of a client to add to the service
  -r, --remove <REMOVE>  Name of a client to remove from the service
  -i, --ip <IP>          IP address or IP range that the client can connect from
  -l, --list             List all clients added to the service
  -h, --help             Print help
```

In this case we want to add the `cluster` agent as a new client, and will
say that it will connect to the `portal` only from the localhost IP address
(as we are running everything locally).

```shell
$ ./target/debug/example-portal client -a cluster -i 127.0.0.1
```

This will produce an invitation file called `invite_cluster.toml` in the
current directory.

> [!NOTE]
> The invitation file is called `invite_{name}.toml', where
> `{name}` is the name of the agent being invited.

Now that we have this invitation, we can pass it to the `cluster` agent.
To do this, we use the `server` command line option of the `cluster` agent.

```shell
$ ./target/debug/example-cluster server --help

Adding and removing servers

Usage: example-cluster server [OPTIONS]

Options:
  -a, --add <ADD>        File containing an invite from a server to add to the service
  -r, --remove <REMOVE>  Name of a server to remove from the service
  -l, --list             List all servers added to the service
  -h, --help             Print help
```

In this case, we just need to add the invitation file.

```shell
$ ./target/debug/example-cluster server -a invite_cluster.toml
```

Running this, you should see that the portal has been added.

### Under the hood

Calling the above functions has modified the configuration files for
the `portal` and `cluster` agents. Information about the agents are
added to these files, including the secret pair of synmmetric keys
used for the handshake between the two agents. For example, here is
the `example-portal.toml` file after the `cluster` agent has been
added;

```toml
agent = "Instance"

[service]
name = "example-cluster"
url = "ws://localhost:8091/"
ip = "127.0.0.1"
port = 8091
clients = []

[[service.servers]]
name = "portal"
url = "ws://localhost:8090/"

[service.servers.inner_key]
data = "2c79e38168ef4b4b323415f88a5f9872cf2d40bc324ed9f30ed3b38fb22542de"

[service.servers.outer_key]
data = "c27997a3e2c4e745d16a7b57e4ad19b242afb1ce02e129e267b9e6645b9725cd"

[extras]
```

and here is the `example-cluster.toml` file.

```toml
agent = "Instance"

[service]
name = "example-cluster"
url = "ws://localhost:8091/"
ip = "127.0.0.1"
port = 8091
clients = []

[[service.servers]]
name = "portal"
url = "ws://localhost:8090/"

[service.servers.inner_key]
data = "2c79e38168ef4b4b323415f88a5f9872cf2d40bc324ed9f30ed3b38fb22542de"

[service.servers.outer_key]
data = "c27997a3e2c4e745d16a7b57e4ad19b242afb1ce02e129e267b9e6645b9725cd"

[extras]
```

You can see that the key pairs match up.

> [!NOTE]
> The data in this configuration file is currently *not* encrypted.
> The keys are very sensitive data, so please make sure to keep the
> configuration files of the agent secure. We are working on a way to
> encrypt the configuration file using a secret, and will update this
> example when the code is available. Note also that the above keys are
> examples, and are not in production use anywhere.

## Running the agents

You can now run the two agents using the `run` command line argument.

```shell
$ ./target/debug/example-portal run
```

and

```shell
$ ./target/debug/example-cluster run
```

You should see that they both start and connect to each other. Then, nothing
happens, because no-one is sending any jobs. You can stop the agents by
pressing `Ctrl-C`. If you stop the `portal` first, you will see that the
`cluster` agent will keep retrying to connect, and will automatically
reconnect if the `portal` restarts.

## What next?

Now that you've seen how to write standardised `templemeads` agents,
we will next look at how to connect these agents, via a bridge,
to a Python script.

We will do this in the [bridge example](../bridge/README.md).


