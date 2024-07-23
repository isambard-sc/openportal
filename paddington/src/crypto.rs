// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use orion::aead;
use secrecy::{CloneableSecret, DebugSecret, Secret, SerializableSecret, Zeroize};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use std::{error::Error, fmt, vec};

#[derive(Debug)]
pub struct CryptoError;

impl fmt::Display for CryptoError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Oh no, something bad went down")
    }
}

impl Error for CryptoError {}

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedData {
    #[serde_as(as = "serde_with::hex::Hex")]
    pub data: vec::Vec<u8>,
    pub version: u8,
}

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

impl Key {
    ///
    /// Generate a new secret key.
    /// This will return a new secret key that can be used to encrypt and decrypt messages.
    ///
    /// # Returns
    ///
    /// The secret key.
    ///
    /// # Example
    ///
    /// ```
    /// use paddington::crypto::Key;
    ///
    /// let key = Key::generate();
    /// ```
    pub fn generate() -> SecretKey {
        Key {
            data: aead::SecretKey::default().unprotected_as_bytes().to_vec(),
        }
        .into()
    }

    ///
    /// Encrypt the passed data with this key.
    /// This will return the encrypted data as a struct
    /// that can be serialised and deserialised by serde.
    /// Note that the data must be serialisable and deserialisable
    /// by serde.
    ///
    /// # Arguments
    ///
    /// * `data` - The data to encrypt.
    ///
    /// # Returns
    ///
    /// The encrypted data.
    ///
    /// # Example
    ///
    /// ```
    /// use paddington::crypto::{Key, SecretKey};
    ///
    /// let key = Key::generate();
    ///
    /// let encrypted_data = key.encrypt("Hello, World!".to_string());
    /// ```
    pub fn encrypt<T>(&self, data: T) -> Result<EncryptedData, CryptoError>
    where
        T: Serialize,
    {
        let orion_key = aead::SecretKey::from_slice(&self.data).unwrap();
        let data = serde_json::to_vec(&data).unwrap();
        Ok(EncryptedData {
            data: aead::seal(&orion_key, &data)?,
            version: 1,
        })
    }
}
