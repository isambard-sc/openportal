// SPDX-FileCopyrightText: © 2025 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use paddington::message::Message;

use crate::error::Error;

pub async fn send(message: Message) -> Result<(), Error> {
    tracing::info!("Virtual agent sending message: {:?}", message);
    tracing::error!("Virtual agent cannot send messages yet");
    Ok(())
}
