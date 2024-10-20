# Agents Sending Jobs Example

This is a demo of two templemeads Agents that send jobs to each other. The
aims of this example are to show you;

1. How to create templemeads Agents
2. How to introduce Agents to each other
3. How to create an send jobs between Agents, including returning results
4. How to supply a custom job handler to process jobs

## Compiling the Example

You have two choices for compiling the example:

1. Compile everything, by going into the top-level directory and
   running `make` (or `cargo build`). This will produce an executable
   called `example-job` in the `target/debug` directory.

2. Compile only this example by navigating to this directory and
   running `cargo build`. To run, you will need to use `cargo run`

## Running the Example

This example implements two Agents;

1. The `portal` agent, which sends jobs to the `cluster` agent. These
   jobs tell the `cluster` to add and remove users from projects,

2. and the `cluster` agent, which receives jobs from the `portal`,
   and which would implement the business logic of adding and removing
   users from projects.

Both agents are implemented in the same executable. You choose which agent
to run by passing either `portal` or `cluster` as an argument.

You first need to start the `portal` agent. You can do this using
the `portal` argument. Either;

```bash
./target/debug/example-job portal
```

or

```bash
cargo run -- portal
```

This will start the `portal` agent and will write an invitation
file to invite the `cluster`. By default this invitation will be in the
current directory and called `invitation.toml`.

Next, you need to start the `cluster` agent. You can do this using
the `cluster` argument, specifyig the path to the invitation file
via the `--invitation` argument. Either;

```bash
./target/debug/example-job cluster --invitation invitation.toml
```

or

```bash
cargo run -- cluster --invitation invitation.toml
```

## Expected Behaviour

What should happen is that you will see that the `cluster` agent connects
to the `portal` agent via a paddington peer-to-peer connection. Once
connected, both agents will register the other agent as a peer to whom
they can communicate.

Next, the `portal` will send a job to add the user `fred` to the
project `proj`, with this project managed by the organisation portal
called `org`. This job is represented by the string;

```
add_user fred.proj.org
```

This job is sent from the `portal` to the `cluster`, so the command is
prefixed with the addressing details, i.e. `portal.cluster` (meaning
sent from the `portal` to the `cluster`). The full job commmand is thus;

```
portal.cluster add_user fred.proj.org
```

In the `cluster` agent, you should see that this job is "put" onto the
`cluster`. The `cluster` agent then processes this job, and when it has
finished, it updates the job with the result.

```
Put job: {portal.cluster add_user fred.proj.org}: version=2, created=2024-10-18 11:46:09 UTC, changed=2024-10-18 11:46:09 UTC, state=Pending to cluster from portal
Adding fred.proj.org to cluster
Here we would implement the business logic to add the user to the cluster
Job has finished: {portal.cluster add_user fred.proj.org}: version=1002, created=2024-10-18 11:46:09 UTC, changed=2024-10-18 11:46:09.470340 UTC, state=Complete
```

You should see that the job state has been updated from `Pending` to `Complete`,
and the version number of the job increased.

On the `portal` agent you will next see that the job has been updated with
this new version.

```
 Update job: {portal.cluster add_user fred.proj.org}: version=1002, created=2024-10-18 11:46:09 UTC, changed=2024-10-18 11:46:09 UTC, state=Complete to portal from cluster
 ```

 This update enables the `portal` agent to get the result of the job.

 ```
 Result: Some("account created")
 ```

 This process is repeated, except now the user `fred` is removed from the
 `proj` project that is managed by the `org` organisation portal.

 The job is

```
portal.cluster remove_user fred.proj.org
```

This is "put" onto the system, meaning that it is communicated from
`portal` to `cluster`, and `cluster` is the agent responsible for
processing the job. Once complete, `cluster` updates the job with the
result, and then enacts an "update" on the job, which communicates the
new version back from `cluster` to `portal`. From here, `portal` can get
the result.

Finally, `portal` tries to remove a user from the `admin` project, by
"putting" this job into the system;

```
portal.cluster remove_user jane.admin.org
```

However, in this example, `cluster` is hard-coded to prevent the removal
of admin users. So, instead an error is generated, and this is used to
"update" the job. This is "updated" on the system, which communicates it
back to `portal`. On trying to get the result, `portal` receives the
error instead. As this is a Rust error that `portal` isn't programmed
to handle, it exits with the error

```
Error: You are not allowed to remove the account for "jane"
```

with `cluster` exiting shortly afterwards.

## How was this implemented?

Both the `portal` and `cluster` agents are implemented in the
`src/main.rs` file. This file is mostly parsing command line arguments,
followed by a call to either `run_portal` or `run_cluster` depending on
the agent. Note that this is very similar to the previous `echo-client`
example, as `templemeads` agents build on the `paddington` peer-to-peer
services.

Each of these functions defines the agent, sets the job handler,
enters the agent's event loop in a background task, and then in the
foreground task, the `portal` sends jobs, while the `cluster` runs
a timer to automatically shut down after a couple of seconds.

For example, here is the `run_cluster` function.

```rust
async fn run_cluster(invitation: &Path) -> Result<(), Error> {
    // load the invitation from the file
    let invite: Invite = Invite::load(invitation)?;

    // create the paddington service for the cluster agent
    // - note that the url, ip and port aren't used, as this
    // agent won't be listening for any connecting clients
    let mut service: ServiceConfig =
        ServiceConfig::new("cluster", "http://localhost:6502", "127.0.0.1", &6502)?;

    // now give the invitation to connect to the server to the client
    service.add_server(invite)?;

    // now create the config for this agent - this combines
    // the paddington service configuration with the Agent::Type
    // for the agent
    let config = agent::custom::Config::new(service, agent::Type::Instance);

    // now start the agent, passing in the message handler for the agent
    // We will start this in a background task, so that we can close the
    // program after a few seconds
    tokio::spawn(async move {
        agent::custom::run(config, cluster_runner)
            .await
            .unwrap_or_else(|e| {
                tracing::error!("Error running cluster: {}", e);
            });
    });

    // wait for a few seconds before exiting
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    std::process::exit(0);
}
```

It starts in a very similar way to the `echo-client` example. It loads
the invitation, and then defines a `paddington` service configuration
(`ServiceConfig`) based on it's own details and the contents of the
invitation.

Next, it passes the `ServiceConfig`, together with the agent type of
`Instance` to the `agent::custom::Config::new` function. This creates
a new custom agent configuration. We are using a custom configuration here
as we want to supply a custom job handler to the agent. There are already
hard-coded agents that could be used, e.g. `agent::portal::Config`,
`agent::cluster::Config`, etc.

Now that the agent has been configured, we run its event loop via the
`agent::custom::run` function, passing in the agent configuration,
and the `cluster_runner` async function that will be used to process jobs.
This is spawned into a background task in this example, so that we can
start a timer in the foreground task to automatically shut down the
program after a couple of seconds. Normally, you would run the agent
event loop in the foreground task, and would not manually exit the process.

### Job Handlers

The `cluster_runner` function is the job handler for the `cluster` agent.

The handler function is called whenever a `Job` is `put` onto the system
for this agent. The possible actions for a `Job` are;

1. `put` - this adds a new `Job` to the distributed system of agents. The
   `Job` will be communicated to the destination agent, and it will be
   responsible for doing the work.

2. `update` - this will update the contents of a `Job` that already exists
   in the distributed system of agents. All agents that hold a copy of this
   job will receive the update.

3. `delete` - this will remove a `Job` from the distributed system of agents.
   All agents that hold a copy of this job will remove it. If the destination
   agent is processing the job, then the job will be cancelled.

Here is the `cluster_runner` function. Note that, as with the `echo-client`
the function has to be made `async_runnable`, to help Rust hold pointers
to async functions.

```rust
async_runnable! {
    ///
    /// Runnable function that will be called when a job is received
    /// by the cluster agent
    ///
    pub async fn cluster_runner(envelope: Envelope) -> Result<Job, Error>
    {
        let mut job = envelope.job();

        match job.instruction() {
            AddUser(user) => {
                // add the user to the cluster
                tracing::info!("Adding {} to cluster", user);

                tracing::info!("Here we would implement the business logic to add the user to the cluster");

                job = job.completed("account created")?;
            }
            RemoveUser(user) => {
                // remove the user from the cluster
                tracing::info!("Removing {} from the cluster", user);

                tracing::info!("Here we would implement the business logic to remove the user from the cluster");

                if user.project() == "admin" {
                    job = job.errored(&format!("You are not allowed to remove the account for {:?}",
                                      user.username()))?;
                } else {
                    job = job.completed("account removed")?;
                }
            }
            _ => {
                tracing::error!("Unknown instruction: {:?}", job.instruction());
                return Err(Error::UnknownInstruction(
                    format!("Unknown instruction: {:?}", job.instruction()).to_string(),
                ));
            }
        }

        Ok(job)
    }
}
```

The `Job` is passed to the handler function in an `Envelope`. The `Envelope`
contains the `Job`, as well as metadata about the communication of that
job (e.g. the sender, intended recipient etc).

The aim of the `cluster_runner` function is to process the job, and then
return the new, updated version of the job.

First, it extracts the `Job` from the `Envelope` into a mutable variable.
This is mutable as we will be updating the job through the function.

We then match on the instruction of the job. The instruction is the
command that the job is supposed to carry out. There is a whole grammar,
described later, that maps string commands (e.g. `add_user`) to Rust
instruction Enums (e.g. `AddUser`). This ensures that command parsing
is robust and that the risk of command injection attacks is minimised.

There are three arms to this match statement. The first is for the
`AddUser` instruction. This implements the business logic to add a user
to the cluster. In this example, it just logs that it is adding the user,
and then marks the job as completed with the result "account created".

The second arm is for the `RemoveUser` instruction. This implements the
business logic to remove a user from the cluster. In this example, it
logs that it is removing the user, and then checks if the user is an
admin user. If it is, then it marks the job as errored with the message
"You are not allowed to remove the account for `username`". If it is not
an admin user, then it marks the job as completed with the result
"account removed".

The final arm is for any other instruction. This logs an error message
saying that the `cluster` agent cannot process any other instruction. This
is also a security design feature - we ensure that each agent contains
only the business logic of the instructions that it is supposed to process.
It doesn't contain any other code, and so cannot be manipulated to do
things that it is not permitted to do.

Finally, the function returns the updated job.

In constrast, the job handler for the `portal` agent is much simpler. It
is not responsible for processing any jobs, and so its handler simply
triggers an error if it is ever called.

```rust
async_runnable! {
    ///
    /// Runnable function that will be called when a job is received
    /// by the portal agent
    ///
    pub async fn portal_runner(envelope: Envelope) -> Result<Job, Error>
    {
        let job = envelope.job();

        tracing::error!("Unknown instruction: {:?}", job.instruction());

        return Err(Error::UnknownInstruction(
            format!("Unknown instruction: {:?}", job.instruction()).to_string(),
        ));
    }
}
```

### Submitting Jobs

The `portal` agent is responsible for submitting jobs to the `cluster` agent.
This is done in the `run_portal` function, after the `portal` agent has
been configured and its event loop started in a background task.

```rust
async fn run_portal(
    url: &str,
    ip: &str,
    port: &u16,
    range: &str,
    invitation: &Path,
) -> Result<(), Error> {
    // create a paddington service configuration for the portal agent
    let mut service = ServiceConfig::new("portal", url, ip, port)?;

    // add the cluster to the portal, returning an invitation
    let invite = service.add_client("cluster", range)?;

    // save the invitation to the requested file
    invite.save(invitation)?;

    // now create the config for this agent - this combines
    // the paddington service configuration with the Agent::Type
    // for the agent
    let config = agent::custom::Config::new(service, agent::Type::Portal);

    // now start the agent, passing in the message handler for the agent
    // Do this in a background task, so that we can send jobs to the cluster
    // here - normally jobs will come from the bridge
    tokio::spawn(async move {
        agent::custom::run(config, portal_runner)
            .await
            .unwrap_or_else(|e| {
                tracing::error!("Error running portal: {}", e);
            });
    });

    // wait until the cluster has connected...
    let mut clusters = agent::get_all(&agent::Type::Instance).await;

    while clusters.is_empty() {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        clusters = agent::get_all(&agent::Type::Instance).await;
    }

    let cluster = clusters.pop().unwrap_or_else(|| {
        tracing::error!("No cluster connected to the portal");
        std::process::exit(1);
    });

    // create a job to add a user to the cluster
    let mut job = Job::parse("portal.cluster add_user fred.proj.org")?;

    // put this job to the cluster
    job = job.put(&cluster).await?;

    // get the result - note that calling 'result' on its own would
    // just look to see if the result exists now. To actually wait
    // for the result to arrive we need to use the 'wait' function,
    // await on that, and then call 'result'
    let result: Option<String> = job.wait().await?.result()?;

    tracing::info!("Result: {:?}", result);

    // create a job to remove a user from the cluster
    let mut job = Job::parse("portal.cluster remove_user fred.proj.org")?;

    // put this job to the cluster
    job = job.put(&cluster).await?;

    // get the result
    let result: Option<String> = job.wait().await?.result()?;

    tracing::info!("Result: {:?}", result);

    // try to remove a user who should not be removed
    let mut job = Job::parse("portal.cluster remove_user jane.admin.org")?;

    // put this job to the cluster
    job = job.put(&cluster).await?;

    // get the result - this should exit with an error
    let result: Option<String> = job.wait().await?.result()?;

    tracing::info!("Result: {:?}", result);

    Ok(())
}
```

Once its event loop starts, the `run_portal` function waits until an
agent of type `Instance` has connected.

```rust
    // wait until the cluster has connected...
    let mut clusters = agent::get_all(&agent::Type::Instance).await;

    while clusters.is_empty() {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        clusters = agent::get_all(&agent::Type::Instance).await;
    }

    let cluster = clusters.pop().unwrap_or_else(|| {
        tracing::error!("No cluster connected to the portal");
        std::process::exit(1);
    });
```

It does this by calling the `agent::get_all` function, to find all agents
connected with a specified agent type. Once connected, it stores the name
of the connected agent in the `cluster` variable.

Next, the `portal` agent creates a job to add a user to the cluster.

```rust
    // create a job to add a user to the cluster
    let mut job = Job::parse("portal.cluster add_user fred.proj.org")?;
```

This job is created by parsing the string `portal.cluster add_user fred.proj.org`.

Parsing uses the job grammar mentioned earlier to securely parse the
command string into a `Job` object. A simple grammar is used, comprising
three parts:

1. The full path between the sending agent, in this case `portal`, and the
   destination agent, in this case `cluster`. The names are separated by
   dots, e.g. `portal.cluster`. In this case, we only have a couple of agents,
   so it is a simple path. But, for more complex networks, the path
   can be longer, e.g. `waldur.brics.notebook.shared` would be the path
   from the `portal` agent called `waldur`, to the `provider` agent called
   `brics`, to the `platform` agent called `notebook`, to the individual
   `instance` agent of a notebook platform called `shared`.

2. The instruction, e.g. `add_user`. This is the command that the job is
   supposed to carry out. Instructions include `add_user`, `remove_user`,
   etc.

3. The argument(s) to the instruction. In this case, the argument is a
   user identifier. In OpenPortal, users are identified by a unique
   triple - the username, the project that they are a member of, and the
   name of the portal that manages that project. For example,
   `dave.demo.brics` would refer to the user called `dave` who is a member
   of the project called `demo` that is managed by the portal called `brics`.
   The user identifier triple should uniquely identify the user across
   the full distributed OpenPortal network of agents. In the case of this
   example, the user identifier is `fred.proj.org`.

Next, the `Job` is `put` onto the distributed system of agents.

```rust
    // put this job to the cluster
    job = job.put(&cluster).await?;
```

This sends the `Job` to the backend distributed peer-to-peer communications
network implemented by the `paddington` crate. Each `paddington::Connection`
between pairs of agents is given a job `Board`. The `Board` is responsible
for recording the state of jobs on either side of the `paddington::Connection`.
When a job is `put` onto the system, the agent that `put` the job identifies
the next agent in the route. For example, if the `waldur` portal `put` a job
to `waldur.brics.notebook.shared`, then the `waldur` portal would `put`
the job onto its job `Board` that is associated with its connection to the
`brics` agent. Because the `Board` is made the same on both sides of the
connection, this would copy the `Job` onto the `Board` of the `brics` agent.
The `brics` agent would see the job, and would detect that it isn't the
final destination (that is `shared`). So, it `puts` the job onto the next
connection in the route, which is the connection to the `notebook` agent.
This `puts` the `Job` onto the `Board` of the `notebook` agent,
which copies it across the `Connection` between `brics` and `notebook`,
meaning that `notebook` now has a copy. Noticing that it isn't the final
destination of the job, it `puts` the job onto the `Connection` between
itself and the `shared` agent. This `puts` the job onto the `Board` of the
`shared` agent, which is the final destination of the job. The `shared`
agent, recognising that it is the final destination, processes the job
via its job handling function.

In our example case, we just have two agents, `portal` and `cluster`, and
so the route is just `portal.cluster`. The `portal` agent `puts` the job
onto the `Board` for the `Connection` between `portal` and `cluster`, which
copies the `Job` across to `cluster`. The `cluster` agent, recognising that
it is the final destination, processes the job via the job handling function
we discussed above.

Once finished, it returns an updated version of the `Job`. This goes back onto
the `Board` for the `Connection` between `portal` and `cluster`. As the
system recognises that the version of the `Job` has changed, it will call
`update` to `update` to the `Job` across the `Connection`. This copies the
new job back to the `Board` on the side of the `Connection` belonging to
`portal`, meaning that `portal` now has the result.

The `portal` agent has been waiting for the result by calling
`wait().await` on the job that it originally `put`.

```rust
    // get the result - note that calling 'result' on its own would
    // just look to see if the result exists now. To actually wait
    // for the result to arrive we need to use the 'wait' function,
    // await on that, and then call 'result'
    let result: Option<String> = job.wait().await?.result()?;

    tracing::info!("Result: {:?}", result);
```

Once updated, the `portal` agent can get the result of the job by calling
`result()` on the job. This returns an `Option<String>`, which is the
result of the job. In this case, the result is "account created".

> [!NOTE]
> A `Job` can return any type - it is up to the definition of the
> grammar of the individual `Instruction` to define what information
> should be returned as the result.

Note that calling `result()?` will return a `templemeads::Error` if anything
went wrong with processing the job. This is why the `run_portal` function
exited when the `remove_user jane.admin.org` job failed to process.

```rust
    // try to remove a user who should not be removed
    let mut job = Job::parse("portal.cluster remove_user jane.admin.org")?;

    // put this job to the cluster
    job = job.put(&cluster).await?;

    // get the result - this should exit with an error
    let result: Option<String> = job.wait().await?.result()?;

    tracing::info!("Result: {:?}", result);
```

## Parallelism

Jobs can be `put`, `update`, and `delete` in parallel. This is because
the underlying peer-to-peer `paddington` network is fully duplex,
and websockets communicate in real time.

There are no consistency guarantees on the order of jobs. You can though
rely on the fact that the state of each job will update atomically, and
its state will be communicated as soon as possible to all agents that
hold a copy. You can also rely on the fact that only a single agent
will process each job, and only that agent (the job's destination) can
update the job.

## Idempotency

Jobs are idempotent. This means that if a job is `put` onto the system
multiple times, then only the first `put` will change the state of the
service. Subsequent `puts` will not change the state. This is a key
design feature, enabling the system to robustly implement error recovery
by simply re-sending jobs. For example, it is safe to re-send the
job to `add_user` `fred.proj.org` to the `cluster` agent. The first
time this is processed it will do all of the business logic to add
`fred` to the `proj` project on `cluster`. Subsequent `puts` of
this job will be ignored, as `fred` is already a member of the `proj`
project on `cluster`.

## Robustness

Multiple copies of a `Job` are held across the distributed peer-to-peer
network of Agents. The `Job` is held on a `Board` on each side of a
`Connection` between pairs of Agents along the communication path from
the sender to the receiver. For example, the `Job` sent via `portal.cluster`
would be stored twice (once of each side of the single `Connection`),
while the `Job` sent via `waldur.brics.notebook.shared` would be stored six
times (once on each side of the three `Connections`).

This means that the system is robust to failures. If, for example, the
`notebook` agent goes down, then the `Job` still exists on `Boards` on the
`waldur`, `brics` and `shared` agents. When the `notebook` agent comes back,
its first task is to restore the `Boards` for each of its `Connections`,
by asking that the agent on the other side sends across all of its jobs.
This is because the system ensures that the `Boards` are kept in sync
on both sides of a `Connection`. If, for example, `shared` went down while
processing a `Job`, then on coming back, it would receive the `Job` again
from `notebook`, and would then process it again. As `Jobs` are idempotent,
this is a safe operation.
