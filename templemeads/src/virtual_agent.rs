// SPDX-FileCopyrightText: © 2025 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use paddington::message::Message;

use crate::agent;
use crate::destination::Destination;
use crate::error::Error;
use crate::handler::process_message;

pub async fn send(destination: &Option<Destination>, message: Message) -> Result<(), Error> {
    let my_name = agent::name().await;

    let mut message = message;

    if message.recipient() == my_name {
        if let Some(dest) = destination {
            // the sender is the last agent in the destination path,
            // as we must be on the return journey of the message,
            // and virtual agents are always the last agent in the path
            message.set_sender(&dest.last());
        } else {
            tracing::error!(
                "Virtual agent cannot send message to itself without a different destination"
            );
            return Err(Error::Run(
                "Virtual agent cannot send message to itself".to_string(),
            ));
        }
    } else {
        message.set_sender(&my_name);
    }

    tracing::info!(
        "Virtual agent sending message: {:?} to destination: {}",
        message,
        destination
            .as_ref()
            .map_or("None".to_string(), |d| d.to_string())
    );

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
