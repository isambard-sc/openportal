// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;

paddington::async_message_handler! {
    async fn process_message(message: paddington::Message) -> Result<(), paddington::Error> {
        tracing::info!(
            "Received message: {} from: {}",
            message.message,
            message.from
        );

        paddington::send(&message.from, "Hello from the portal!!!").await?;

        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // construct a subscriber that prints formatted traces to stdout
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    // use that subscriber to process traces emitted after this point
    tracing::subscriber::set_global_default(subscriber)?;

    let defaults = paddington::Defaults::new(
        Some("portal".to_string()),
        Some(
            "portal.toml"
                .parse()
                .expect("Could not parse default config file."),
        ),
        Some("ws://localhost:8042".to_string()),
        Some("127.0.0.1".to_string()),
        Some(8042),
    );

    paddington::set_handler(process_message).await?;
    paddington::run(defaults).await?;

    Ok(())
}
