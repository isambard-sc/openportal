// SPDX-FileCopyrightText: © 2025 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use paddington::message::Message;

use crate::agent;
use crate::error::Error;
use crate::handler::process_message;

pub async fn send(message: Message) -> Result<(), Error> {
    tracing::info!("Virtual agent sending message: {:?}", message);

    let mut message = message;
    message.set_sender(&agent::name().await);

    match process_message(message).await {
        Ok(_) => Ok(()),
        Err(e) => {
            tracing::error!("Error processing message in virtual agent: {}", e);
            Err(Error::Run(format!(
                "Error processing message in virtual agent: {}",
                e
            )))
        }
    }
}
