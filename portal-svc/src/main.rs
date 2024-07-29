// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::{Context, Result};

use paddington;

#[tokio::main]
async fn main() -> Result<()> {
    let config = paddington::args::process_args()
        .await
        .context("Error processing arguments")?;

    if (config.is_null()) {
        eprintln!("No configuration provided.");
        std::process::exit(1);
    }

    let mut handles = vec![];

    if config.has_clients() {
        handles.append(tokio::spawn(async move {
            paddington::server::run(config).context("Error running server")?;
        }));
    }

    for client in config.get_clients().iter() {
        handles.append(tokio::spawn(async move {
            paddington::client::run(client).context("Error running client")?
        }));
    }

    for handle in handles {
        handle.await?;
    }

    Ok(())
}
