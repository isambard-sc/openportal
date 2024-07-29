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

    paddington::client::run(config)
        .await
        .context("Error running client")?;

    Ok(())
}
