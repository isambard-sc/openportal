// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;

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
        Some("ws://localhost:8043".to_string()),
        Some("127.0.0.1".to_string()),
        Some(8043),
    );

    eventloop::run(defaults).await?;

    Ok(())
}
