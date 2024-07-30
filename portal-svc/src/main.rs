// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::{Context, Result};

use paddington;

#[tokio::main]
async fn main() -> Result<()> {
    let defaults = paddington::args::ArgDefaults::new(
        Some("portal".to_string()),
        Some(
            "portal.toml"
                .parse()
                .expect("Could not parse default config file."),
        ),
    );

    let config = paddington::args::process_args(&defaults).await?;

    if config.is_null() {
        anyhow::bail!("No configuration provided.");
    }

    let mut server_handles = vec![];
    let mut client_handles = vec![];

    let clients = config.get_clients();

    if config.has_clients() {
        let my_config = config.clone();
        server_handles.push(tokio::spawn(async move {
            paddington::server::run(my_config);
        }));
    }

    for client in clients {
        let my_config = config.clone();
        client_handles.push(tokio::spawn(async move {
            paddington::client::run(
                my_config.clone(),
                paddington::config::PeerConfig::from_client(&client),
            )
        }));
    }

    for handle in server_handles {
        handle.await?;
    }

    for handle in client_handles {
        handle.await?;
    }

    Ok(())
}
