// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use std::path::{Path, PathBuf};

use anyhow::Context;
use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};

use paddington::async_message_handler;
use paddington::config::ServiceConfig;
use paddington::invite::Invite;
use paddington::message::Message;
use paddington::{run, send, set_handler, Error};

mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

fn version() -> &'static str {
    built_info::GIT_VERSION.unwrap_or(built_info::PKG_VERSION)
}

// simple command line options - either this is the
// "server" or the "client"
#[derive(Parser)]
#[command(version = version(), about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Invitation file from the server to allow the client to connect
    Client {
        #[arg(long, short = 'c', help = "Path to the invitation file")]
        invitation: Option<std::path::PathBuf>,
    },

    /// Specify the port to listen on for the server
    Server {
        #[arg(
            long,
            short = 'u',
            help = "URL of the service including port and route (e.g. http://localhost:8080)"
        )]
        url: Option<String>,

        #[arg(
            long,
            short = 'i',
            help = "IP address on which to listen for connections (e.g. 127.0.0.1)"
        )]
        ip: Option<String>,

        #[arg(
            long,
            short = 'p',
            help = "Port on which to listen for connections (e.g. 8042)"
        )]
        port: Option<u16>,

        #[arg(
            long,
            short = 'r',
            help = "IP range on which to accept connections from the client"
        )]
        range: Option<String>,

        #[arg(
            long,
            short = 'c',
            help = "Config file to which to write the invitation for the client"
        )]
        invitation: Option<std::path::PathBuf>,
    },
}

///
/// Simple main function - this will parse the command line arguments
/// and either call run_client or run_server depending on whether this
/// is the client or server
///
#[tokio::main]
async fn main() -> Result<()> {
    // start tracing
    templemeads::config::initialise_tracing();

    // parse the command line arguments
    let args: Args = Args::parse();

    match &args.command {
        Some(Commands::Client { invitation }) => {
            let invitation = match invitation {
                Some(invitation) => invitation,
                None => {
                    return Err(anyhow::anyhow!("No invitation file specified."));
                }
            };

            run_client(invitation).await?;
        }
        Some(Commands::Server {
            url,
            ip,
            port,
            range,
            invitation,
        }) => {
            let url = match url {
                Some(url) => url.clone(),
                None => "http://localhost:6501".to_string(),
            };

            let ip = match ip {
                Some(ip) => ip.clone(),
                None => "127.0.0.1".to_string(),
            };

            let port = match port {
                Some(port) => port,
                None => &6501,
            };

            let range = match range {
                Some(range) => range.clone(),
                None => "0.0.0.0/0.0.0.0".to_string(),
            };

            let invitation = match invitation {
                Some(invitation) => invitation.clone(),
                None => PathBuf::from("invitation.toml"),
            };

            run_server(&url, &ip, port, &range, &invitation).await?;
        }
        _ => {
            let _ = Args::command().print_help();
        }
    }

    Ok(())
}

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
        send(Message::send_to(message.sender(), message.zone(), message.payload())).await?;

        // exit if the message is "0"
        if message.payload() == "0" {
            std::process::exit(0);
        }

        Ok(())
    }
}

///
/// This function creates a service called 'echo-client' that will connect
/// to the 'echo-server' service and will echo back any messages that
/// it receives
///
async fn run_client(invitation: &Path) -> Result<(), Error> {
    // load the invitation from the file
    let invite: Invite = Invite::load(invitation)?;

    // create the echo-client service - note that the url, ip and
    // port aren't used, as this service won't be listening for any
    // connecting clients
    let mut service: ServiceConfig = ServiceConfig::new(
        "echo-client",
        "http://localhost:6502",
        "127.0.0.1",
        &6502,
        &None,
        &None,
    )?;

    // now give the invitation to connect to the server to the client
    service.add_server(invite)?;

    // set the handler for the echo-client service
    set_handler(echo_client_handler).await?;

    // run the echo-client service
    run(service).await?;

    Ok(())
}

async_message_handler! {
    ///
    /// This is the function that will be called on the echo-server
    /// service whenever it receives a message
    ///
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
                send(Message::send_to("echo-client", message.zone(), "1000")).await?;
            }
            false => {
                // the message should be a number - we will decrement
                // it and echo it back
                let number = message.payload().parse::<i32>().with_context(|| {
                    format!("Could not parse message payload as i32: {}", message.payload())
                })?;

                // echo the decremented number
                send(Message::send_to(message.sender(), message.zone(), &(number - 1).to_string())).await?;

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
}

///
/// This function creates a service called 'echo-server' that will
/// listen for a connection from 'echo-client' and will then start
/// sending messages to it
///
async fn run_server(
    url: &str,
    ip: &str,
    port: &u16,
    range: &str,
    invitation: &Path,
) -> Result<(), Error> {
    // create the echo-server service
    let mut service = ServiceConfig::new("echo-server", url, ip, port, &None, &None)?;

    let invite = service.add_client("echo-client", range, &None)?;

    // save the invitation to the requested file
    invite.save(invitation)?;

    // set the handler for the echo-server service
    set_handler(echo_server_handler).await?;

    // run the echo-server service
    run(service).await?;

    Ok(())
}
