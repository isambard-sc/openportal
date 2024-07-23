// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use orion::aead;
use secrecy::{CloneableSecret, DebugSecret, Secret, SerializableSecret, Zeroize};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use std::vec;

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Key {
    #[serde_as(as = "serde_with::hex::Hex")]
    pub data: vec::Vec<u8>,
}

impl Zeroize for Key {
    fn zeroize(&mut self) {
        self.data.zeroize();
    }
}

/// Permits cloning, Debug printing as [[REDACTED]] and serialising
impl CloneableSecret for Key {}
impl DebugSecret for Key {}
impl SerializableSecret for Key {}

pub type SecretKey = Secret<Key>;

pub fn generate_key() -> SecretKey {
    Key {
        data: aead::SecretKey::default().unprotected_as_bytes().to_vec(),
    }
    .into()
}
