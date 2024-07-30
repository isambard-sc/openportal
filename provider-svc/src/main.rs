// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;
use tracing;
use tracing_subscriber;

use paddington::args::ArgDefaults;
use paddington::eventloop;

#[tokio::main]
async fn main() -> Result<()> {
    // construct a subscriber that prints formatted traces to stdout
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    // use that subscriber to process traces emitted after this point
    tracing::subscriber::set_global_default(subscriber)?;

    let defaults = ArgDefaults::new(
        Some("provider".to_string()),
        Some(
            "provider.toml"
                .parse()
                .expect("Could not parse default config file."),
        ),
    );

    let _ = eventloop::run(defaults).await?;

    Ok(())
}
