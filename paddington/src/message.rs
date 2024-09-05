// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub peer: String,
    pub payload: String,
}

impl Message {
    pub fn new(peer: &str, payload: &str) -> Self {
        Self {
            peer: peer.to_owned(),
            payload: payload.to_owned(),
        }
    }

    pub fn control(payload: &str) -> Self {
        Self {
            peer: "".to_owned(),
            payload: payload.to_owned(),
        }
    }

    pub fn is_control(&self) -> bool {
        self.peer.is_empty()
    }
}
