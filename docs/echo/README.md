<!--
SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# Echo Service Example

This is a simple echo demo between two paddington Services. It demonstrates:

1. How to create paddington Services
2. How to introduce one Service to another via an Invite
3. How to set the message handler for each service
4. How to send and receive messages between the services

## Compiling the Example

You have two choices for compiling the example:

1. Compile everything, by going into the top-level directory and
   running `make` (or `cargo build`). This will produce an executable
   called `example-echo` in the `target/debug` directory.

2. Compile only this example by navigating to this directory and
   running `cargo build`. To run, you will need to use `cargo run`

## Running the Example

This example implements two services:

1. The `echo-server` service, which listens for a connection from
   the `echo-client` service,

2. and the `echo-client` service, which initiates a connection to
   the `echo-server` service.

You fist need to start the `echo-server` service. You can do this using
the `server` argument. Either;

```bash
./target/debug/example-echo server
```

or

```bash
cargo run -- server
```

This will start the `echo-server` service and will write an invitation
file to invite the client. By default this invitation will be in the
current directory and called `invitation.toml`.

Next, you need to start the `echo-client` service. You can do this using
the `client` argument, specifyig the path to the invitation file
via the `--invitation` argument. Either;

```bash
./target/debug/example-echo client --invitation invitation.toml
```

or

```bash
cargo run -- client --invitation invitation.toml
```

## Expected Behaviour

What should happen is that the `echo-client` service will connect to the
`echo-server` service. Both service will then become peers, able to send
and receive messages to each other.

First, each service will receive a "control message", that tells them
that a peer service has connected. For example, you should see

```
echo-server received: Control message: {"Connected":{"agent":"echo-client"}}
```

written to the log for the `echo-server` service (with a similar message
written to the log for the `echo-client` service).

> [!NOTE]
> paddington implements a peer-to-peer network, so there is no concept of a
> "server" or "client" once the connection is established. The only
> distinction is that the "client" is the process that initiates the
> new connection to the "server"

The `echo-client` service ignores control messages, so doesn't do anything.

However, the `echo-server` service, on receiving the control message,
sends a message to `echo-client` with the content `1000`.

The `echo-client` service is configured to just echo back any messages it
receives from peer services. So, on receiving the message `1000` from
`echo-server`, it just echos back `1000` to `echo-server`.

The `echo-server` service is configured to interpret any message received
from a peer as an integer. On receiving the message, it decrements one from
that number and sends it back to the `echo-client` service.

This process continues until the number reaches `0`. At this point,
both services exit - but not before the `echo-server` prints
"Blast off!".

So you should see a countdown from 1000 to 0 in the logs of both services,
with "Blast off!" written into the log of `echo-server` at the end.

## How was this implemented?

Both the `echo-client` and `echo-server` services are implemented in the
`src/main.rs` file. This file is mostly parsing command line arguments,
followed by a call to either `run_server` or `run_client` depending on
the service.

Each of these functions defines the service, sets the message handler,
and then enters the service event loop. For example, here is
`run_client`:

```rust
async fn run_client(invitation: &Path) -> Result<(), Error> {
    // load the invitation from the file
    let invite: Invite = Invite::load(invitation)?;

    // create the echo-client service - note that the url, ip and
    // port aren't used, as this service won't be listening for any
    // connecting clients
    let mut service: ServiceConfig =
        ServiceConfig::new("echo-client", "http://localhost:6502", "127.0.0.1", &6502)?;

    // now give the invitation to connect to the server to the client
    service.add_server(invite)?;

    // set the handler for the echo-client service
    set_handler(echo_client_handler).await?;

    // run the echo-client service
    run(service).await?;

    Ok(())
}
```

We see that first we read the invitation from the passed file. We then create
a new service configuration (`ServiceConfig`). This configures the name of the
service (`echo-client`) and connection details if this service is expecting
to be connected to by other client services.

Next, we add a new server to the service configuration by passing in the
invitation. Then, we set the handler function for the service, before calling
the `run` function to enter the service's event loop.

The handler function is called whenever the service receives any message.
It is set via the `set_handler` function. Here is the handler function for
the `echo-client` service:

```rust
async_message_handler! {
    ///
    /// This is the function that will be called on the echo-client
    /// service whenever it receives a message
    ///
    async fn echo_client_handler(message: Message) -> Result<(), Error> {
        tracing::info!("echo-client received: {}", message);

        // we will ignore control messages
        if message.is_control() {
            return Ok(())
        }

        // just echo the message back to the sender
        send(Message::new(message.sender(), message.payload())).await?;

        // exit if the message is "0"
        if message.payload() == "0" {
            std::process::exit(0);
        }

        Ok(())
    }
}
```

> [!NOTE]
> Note that the handler function is defined using the `async_message_handler!`
> as rust needs help to use async function pointers.

In this case, we check to see if the message is a control message.
If it is, we ignore it. Otherwise, we send the message back to the
sender. If the message is "0", we exit the service.

The `echo-server` service is similar, but it sends a message to the
`echo-client` service when it receives a control message, and decrements
the number of the message it receives from the `echo-client` service.

Here is it's message handler;

```rust
    async fn echo_server_handler(message: Message) -> Result<(), Error> {
        tracing::info!("echo-server received: {}", message);

        // there are two types of message - control messages that
        // tell us that, e.g. services have connected, and normal
        // messages that come from those services. Here, as the
        // echo-server, we will start the echo exchange whenever
        // we receive a control message telling us the echo-client
        // service has connected
        match message.is_control() {
            true => {
                // start the echo exchange
                send(Message::new("echo-client", "1000")).await?;
            }
            false => {
                // the message should be a number - we will decrement
                // it and echo it back
                let number = message.payload().parse::<i32>().with_context(|| {
                    format!("Could not parse message payload as i32: {}", message.payload())
                })?;

                // echo the decremented number
                send(Message::new(message.sender(), &(number - 1).to_string())).await?;

                if number <= 1 {
                    // blast off!
                    tracing::info!("Blast off!");

                    // exit the program gracefully
                    // (this will eventually flush all caches / queues,
                    //  and exit once all messages sent, blocking sending
                    //  of any new messages - for now, we will just sleep
                    // for a short time before calling exit...)
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    std::process::exit(0);
                }
            }
        }

        Ok(())
    }
```

## Under the hood

The `run` function is the main entry point for the service. It creates an
event loop the is responsible for listening for new connections from
client services, connecting to new server services, and handling the sending
and receiving of messages between services.

> [!NOTE]
> An individual service can be connected to an arbitrary number of peer
> services. It can connect to them both as a client that initiates the
> connection, and a server than listens for new connections. In this way,
> a distributed network of peer-to-peer communicating services can be
> orchestrated.

Services can only connect to each other if they have been properly introduced.
To introduce a service to another, you first need to ask the "server" service
to create an invitation file that will be passed to the "client" service.

This request tells the "server" service to create a pair of secure
symmetric cryptographic keys that will be used for the connection. It also lets
the "server" know the name of the "client" service, plus the expected
IP address or IP range from which the "client" will connect. The "server"
then encodes information about how to connect to itself (e.g. the protocol,
URL, port, etc) and the cryptographic keys into an invitation file, which
is written in the toml format.

You then manually (out of band) give this invitation file to the
"client" service. This tells the "client" that it should initiate the
connection to the "server" service.

### Initiating the connection

If a service knows that it has clients, then it will start a HTTP server
listening for new connections as specified in its configuration.

If a service knows that it has servers, then it will start a HTTP client
to try to connect to that server.

When the "client" service connects to the "server" service, the two engage
in a handshake to securely upgrade the connection to a peer-to-peer
websocket connection.

### Handshake

The handshake starts from the "client" service. It generates a new
symmetric cryptographic key and encrypts this with the two keys contained
in the invititation file from the "server" service. It sends this encrypted
message to the "server".

On receiving the message, the "server" checks to see if it is expecting a
connection from any client with the IP address in one of the IP ranges
expected for the clients. If not, it drops the connection.

Next, the "server" checks to see if the name of the connecting "client"
service matches the name expected for a service from the connecting IP address.
If not, it drops the connection.

Next, the "server" checks to see if it has an existing connection from the
named "client" service. If it does, it drops the connection. This ensures
that there is only a single peer-to-peer connection between any two services
at any one time.

Next, it tries to decrypt the message from the "client" service using the
pair of symmetric keys that it generated when it created the invitation file
for the "client". If it can't decrypt the message, it drops the connection.

However, if all of the above succeeded, then the "server" can be confident
that the "client" is authenticated (it connected with the right name from
the right location and knew both the secret symmetric keys). Given this,
the "server" saves the (now decrypted) new symmetric key from the client,
and then generates its own, new symmetric key. These two new symmetric keys
will become the session keys for the connection.

The "server" encrypts its new symmetric key with both one of the invitation
symmetric keys, and the new symmetric key from the "client". It sends the
result back as a message to the "client".

The "client" receives the message, and checks that it can decrypt it using
one of the invitation symmetric keys, and the new symmetric key that it
generated and sent to the "server". If it can, then it can be confident
that it is truly communicating with the "server" service, as only the
"server" is able to respond to the request with the correct IP parameters,
service name and demonstration of the ability to both decrypt and encrypt
messages using the invitiation symmetric keys. If anything here fails,
then the client will drop the connection.

Now that the "server" and "client" have both authenticated each other,
they both now share a pair of newly generated "session" symmetric keys.
The connection is fully upgraded to a peer-to-peer websocket connection,
and all messages sent during this session between the two services are
double-encrypted using the two session symmetric keys. This ensures that
even if the underlying communication protocol is insecure (e.g. standard
websockets over HTTP, rather than secure websockets over HTTPS), then
communication is authenticated and protected from tampering or eavesdropping.

> [!NOTE]
> The pair of invitation symmetric keys are only used in the handshake
> to set up the initial connection between the two services. Once the
> handshake is complete, all encryption is done using the pair of session
> keys.

### Encryption standard

The paddington protocol currently uses the `XChaCha20Poly1305` algorithm
implemented in the [orion crate](https://github.com/orion-rs/orion). Orion
is a performant, pure rust encryption crate.

All symmetric keys are 256bits, and are randomly generated using the secure
functions provided by orion.

### Sending a Message

The message protocol is simple. A `Message` object contains three fields;

```rust
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    sender: String,
    recipient: String,
    payload: String,
}
```

The `sender` is the name of the service that sent the message. The
`recipent` is the name of the service that should receive the message.
And the `payload` is a string containing the message itself.

Typically, the `payload` will be a string. However, you can send anything
that can be serialised to a string, e.g. an object serialised to JSON via
[serde_json](https://docs.rs/serde_json/latest/serde_json/index.html).

The `sender` and `recipient` need to be specified because each service can
be connected to multiple peer services. Every connection between a pair
of services is managed via a `Connection` object. All of the connections
for a service are managed via the `Exchange`.

To send a message, you call the `send` function, which is defined as part
of the `Exchange`. This implements a set of functions that handle the
exchange of messages between services. The `Exchange` looks at the `sender` and
`recipient` and works out if it knows how to send the message to one of its
connected peer services. If it does, it passes the payload of the message
to the `Connection` object that manages the peer-to-peer connection between
the services.

### Transmitting the Payload

The `Connection` object is responsible for sending the actual message over the
websocket stream. First, it envelopes the payload using the `envelope_message`
function. This encrypts the payload string using the two session symmetric keys.
The result is passed into [tokio::tungstenite](https://docs.rs/tokio-tungstenite/latest/tokio_tungstenite/) as a `tokio_tungstenite::tungstenite::protocol::Message as TokioMessage`
(as a "text" message). The `tokio::tungstenite` crate handes the websocket
connection and is responsible for the underlying wire protocol, chunking
of the message into packets etc.

The actual string message that is sent is the result of enveloping
(double-encrypting) the payload. Encrypting the data is handled by
the `paddington::crypto` module, using a `paddington::crypto::Key`.
This first uses [serde_json](https://docs.rs/serde_json/latest/serde_json/index.html)
to convert the data to be encrypted into a UTF-8 encoded JSON string. This string
is then byte encrypted using [orion](https://docs.rs/orion/latest/orion/index.html)
into a binary array. This binary array is converted back into a string
using hexadecimal encoding. This process is performed twice, once for
each of the two session keys. The result is that the payload is converted
into a long(ish) secure hex-encoded string, which is passed to
[tokio::tungstenite](https://docs.rs/tokio-tungstenite/latest/tokio_tungstenite/)
to transmit over websockets using the text protocol implemented in that crate.

### Receiving the Message

On receiving, the whole process is repeated in reverse. The
[tokio::tungstenite](https://docs.rs/tokio-tungstenite/latest/tokio_tungstenite/)
crate is responsible for decoding and reassembling the websocket packets
back into a single string. This is a hex-encoded UTF-8 string, which is
de-enveloped by `Connection` via double-decrypting using the two
session keys via `crypto::Key::descrypt`. The result is a UTF-8 encoded
string which is the original payload. This is combined by the `Exchange`
with the sender and recipient data to create a `paddington::message::Message`
object that is passed to the async message handling function that is set
for the service (via the `paddington::set_handler` function).

### Parallelism

The event loop handling all communication is implemented using
[tokio](https://docs.rs/tokio/latest/tokio/index.html). This allows
for parallel, asynchronous handling of messages. The peer-to-peer
connecton is fully duplex, meaning that messages can be sent and
received at the same time. To keep the code responsive, and to manage
memory and resource usage, both sending and receiving of messages uses
rust channels to queue messages. In code, sending a message merely pushes
the message onto a channel, returning immediately. In the background,
the event loop's large number of parallel tokio tasks are pulling
messages off of this channel and doing the work of sending them
over the "send" websocket connection. Similarly, incoming
messages are read from the websocket connection and pushed onto a
receive channel. The event loop's parallel tokio tasks are pulling
received messages from this channel, processing them, and then
calling the message handler function. In this way, sending and
receiving of messages is not blocked by message processing,
and the service should scale as the number of messages sent
and received increases.

### Error handling

The event loop and tokio tasks handling the sending and receiving of
messages also handle all errors encountered during the process. If
any errors are encountered, then these are logged via the
[tracing](https://docs.rs/tracing/latest/tracing/) crate, and
subsequent processing of that message is cancelled. The event loop
and its worker tasks then move on to processing the next message.

In this way, errors should not cause the service to crash or for
message processing to be blocked.

In addition, the event loop catches errors that occur for the
actual websocket connection. If the connection is lost, then
the event loop will automatically try to reconnect to the peer
service (i.e. the "server" will automatically try to restart the
HTTP servre, and the "client" will automatically try to reconnect via
a HTTP client, re-trying to connect every 5 seconds). In this way,
any disruptions or outages in the connection should be automatically
recovered from. Messages that were in the process of being sent
may be lost, so it is up to a higher level protocol (e.g. that
implemented in templemeads) to handle recovery.

Finally, if the service does exit, e.g. by crashing or being killed,
then on restarting, it will automatically try to reconnect to all
of its peer services. This means that a keepalive process, e.g.
using kubernetes pods or systemd daeamons, could be used to ensure
that the service automatically restarts and recovers from most
outages.

## What next?

Now that you've seen how paddington peer-to-peer services can be created,
and how they communicate with one another, the next step is to see how
templemeads builds on this to create a distributed network of Agents.
We will do this in the [job example](../job/README.md).

