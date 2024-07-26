// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Context;
use anyhow::Error as AnyError;
use orion::aead;
use secrecy::{CloneableSecret, DebugSecret, Secret, SerializableSecret, Zeroize};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json;
use serde_with::serde_as;
use std::{fmt, str, vec};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum CryptoError {
    #[error("{0}")]
    IOError(#[from] std::io::Error),

    #[error("{0}")]
    UnknownCryptoError(#[from] orion::errors::UnknownCryptoError),

    #[error("{0}")]
    AnyError(#[from] AnyError),

    #[error("{0}")]
    SerdeError(#[from] serde_json::Error),

    #[error("Unsupported version: {0}")]
    VersionError(u8),

    #[error("Unknown config error")]
    Unknown,
}

#[serde_as]
#[derive(Clone, Serialize, Deserialize)]
pub struct EncryptedData {
    #[serde_as(as = "serde_with::hex::Hex")]
    pub data: vec::Vec<u8>,
    pub version: u8,
}

impl fmt::Debug for EncryptedData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "EncryptedData {{ data: [REDACTED] length {} bytes, version: {} }}",
            self.data.len(),
            self.version
        )
    }
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
    /// Create and return a null key - this should not be used
    ///
    pub fn null() -> SecretKey {
        Key { data: vec![0; 32] }.into()
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
    /// let encrypted_data = key.expose_secret().encrypt("Hello, World!".to_string());
    /// ```
    pub fn encrypt<T>(&self, data: T) -> Result<EncryptedData, CryptoError>
    where
        T: Serialize,
    {
        let orion_key = aead::SecretKey::from_slice(&self.data)
            .with_context(|| "Failed to create a secret key from the secret key data.")?;
        let json_data = serde_json::to_string(&data).with_context(|| {
            "Failed to serialise the data to JSON. Ensure that the data is serialisable by serde."
        })?;

        Ok(EncryptedData {
            data: aead::seal(&orion_key, json_data.as_bytes())
                .with_context(|| "Failed to encrypt the data.")?,
            version: 1,
        })
    }

    ///
    /// Decrypt the passed data with this key.
    /// This will return the decrypted data.
    ///
    /// Arguments
    ///
    /// * `data` - The data to decrypt.
    ///
    /// Returns
    ///
    /// The decrypted data.
    ///
    /// Example
    ///
    /// ```
    /// use paddington::crypto::{Key, SecretKey};
    ///
    /// let key = Key::generate();
    ///
    /// let encrypted_data = key.expose_secret().encrypt("Hello, World!".to_string());
    /// let decrypted_data = key.expose_secret().decrypt(&encrypted_data).unwrap();
    ///
    /// assert_eq!(decrypted_data, "Hello, World!".to_string());
    /// ```
    pub fn decrypt<T>(&self, data: &EncryptedData) -> Result<T, CryptoError>
    where
        T: DeserializeOwned,
    {
        if data.version != 1 {
            return Err(CryptoError::VersionError(data.version));
        }

        let orion_key = aead::SecretKey::from_slice(&self.data)
            .with_context(|| "Failed to create a secret key from the secret key data.")?;

        let decrypted_data =
            aead::open(&orion_key, &data.data).with_context(|| "Failed to decrypt the data.")?;

        let decrypted_string: String = String::from_utf8(decrypted_data)
            .with_context(|| "Failed to convert the decrypted data to a string.")?;

        let obj: T = serde_json::from_str(&decrypted_string)
            .with_context(|| "Failed to deserialise the decrypted data from JSON.")?;

        Ok(obj)
    }
}
