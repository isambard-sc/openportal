// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Error as AnyError;

paddington::async_message_handler! {
    async fn process_message(message: paddington::Message) -> Result<(), paddington::Error> {
        tracing::info!(
            "Received message: {} from: {}",
            message.message,
            message.from
        );

        // sleep for 1 second
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        paddington::send(&message.from, "Hello from an agent!").await?;

        Ok(())
    }
}

pub async fn run(defaults: paddington::Defaults) -> Result<(), AnyError> {
    paddington::set_handler(process_message).await?;
    paddington::run(defaults).await?;

    Ok(())
}
