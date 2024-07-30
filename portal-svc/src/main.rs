// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::{Context, Result};
use tracing;
use tracing_subscriber;

use paddington::args::{process_args, ArgDefaults, ProcessResult};
use paddington::{client, server};

#[tokio::main]
async fn main() -> Result<()> {
    // construct a subscriber that prints formatted traces to stdout
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    // use that subscriber to process traces emitted after this point
    tracing::subscriber::set_global_default(subscriber)?;

    let defaults = ArgDefaults::new(
        Some("portal".to_string()),
        Some(
            "portal.toml"
                .parse()
                .expect("Could not parse default config file."),
        ),
    );

    match process_args(&defaults).await? {
        ProcessResult::ServiceConfig(config) => {
            if config.is_null() {
                return Ok(());
            }

            let mut server_handles = vec![];
            let mut client_handles = vec![];

            let clients = config.get_clients();

            if config.has_clients() {
                let my_config = config.clone();
                server_handles.push(tokio::spawn(async move {
                    server::run(my_config);
                }));
            }

            for client in clients {
                let my_config = config.clone();
                client_handles.push(tokio::spawn(async move {
                    client::run(my_config.clone(), client.to_peer())
                }));
            }

            for handle in server_handles {
                handle.await?;
            }

            for handle in client_handles {
                handle.await?;
            }
        }
        ProcessResult::Invite(invite) => {
            // write the invite to a file
            let filename = invite.save()?;
            println!("Invite saved to {}", filename);
            println!(
                "You can load this into the client using the 'server --add {filename}' command."
            );
        }
        ProcessResult::Message(message) => {
            println!("{}", message);
        }
        ProcessResult::None => {
            // this is the exit condition
            return Ok(());
        }
    }

    Ok(())
}
