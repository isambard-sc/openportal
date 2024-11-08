// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use serde::{Deserialize, Serialize};
use std::fmt::Display;

use crate::error::Error;

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    sender: String,
    recipient: String,
    zone: String,
    payload: String,
}

impl Display for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.is_control() {
            true => write!(f, "Control message: {}", self.payload),
            false => write!(
                f,
                "Message in zone {} from {} to {}: {}",
                self.zone, self.sender, self.recipient, self.payload
            ),
        }
    }
}

pub enum MessageType {
    Control,
    KeepAlive,
    Message,
}

impl Message {
    pub fn received_from(sender: &str, zone: &str, payload: &str) -> Self {
        Self {
            sender: sender.trim().to_owned(),
            recipient: "".to_owned(),
            zone: zone.trim().to_owned(),
            payload: payload.to_owned(),
        }
    }

    pub fn send_to(recipient: &str, zone: &str, payload: &str) -> Self {
        Self {
            sender: "".to_owned(),
            recipient: recipient.trim().to_owned(),
            zone: zone.trim().to_owned(),
            payload: payload.to_owned(),
        }
    }

    pub fn control(payload: &str) -> Self {
        Self {
            sender: "".to_owned(),
            recipient: "".to_owned(),
            zone: "".to_owned(),
            payload: payload.to_owned(),
        }
    }

    pub fn is_control(&self) -> bool {
        self.sender.is_empty() && self.zone.is_empty()
    }

    pub fn keepalive(recipient: &str, zone: &str) -> Self {
        Self {
            sender: "".to_owned(),
            recipient: recipient.trim().to_owned(),
            zone: zone.trim().to_owned(),
            payload: "KEEPALIVE".to_owned(),
        }
    }

    pub fn is_keepalive(&self) -> bool {
        self.payload == "KEEPALIVE"
    }

    pub fn is_message(&self) -> bool {
        !self.is_control() && !self.is_keepalive()
    }

    pub fn typ(&self) -> MessageType {
        if self.is_control() {
            MessageType::Control
        } else if self.is_keepalive() {
            MessageType::KeepAlive
        } else {
            MessageType::Message
        }
    }

    pub fn set_recipient(&mut self, recipient: &str) {
        self.recipient = recipient.to_owned();
    }

    pub fn set_sender(&mut self, sender: &str) {
        self.sender = sender.to_owned();
    }

    pub fn sender(&self) -> &str {
        &self.sender
    }

    pub fn recipient(&self) -> &str {
        &self.recipient
    }

    pub fn payload(&self) -> &str {
        &self.payload
    }

    pub fn zone(&self) -> &str {
        &self.zone
    }

    pub fn assert_in_zone(&self, zone: &str) -> Result<(), Error> {
        match self.zone == zone.trim() {
            true => Ok(()),
            false => Err(Error::InvalidPeer(format!(
                "Message zone {} does not match expected zone {}",
                self.zone, zone
            ))),
        }
    }
}
