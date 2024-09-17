// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Context;
use orion::{aead, auth};
use secrecy::{CloneableSecret, DebugSecret, Secret, SerializableSecret, Zeroize};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_with::serde_as;
use std::fmt::Display;
use std::{fmt, str, vec};

use crate::error::Error;

#[derive(Debug, Clone, PartialEq)]
pub struct Signature {
    sig: orion::auth::Tag,
}

impl Serialize for Signature {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::ser::Serializer,
    {
        hex::encode(self.sig.unprotected_as_bytes()).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Signature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::de::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let bytes = hex::decode(s).map_err(serde::de::Error::custom)?;
        match orion::auth::Tag::from_slice(&bytes) {
            Ok(sig) => Ok(Signature { sig }),
            Err(_) => Err(serde::de::Error::custom("Failed to create Signature.")),
        }
    }
}

impl Display for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(self.sig.unprotected_as_bytes()))
    }
}

impl Signature {
    pub fn from_string(s: &str) -> Result<Signature, Error> {
        let bytes = hex::decode(s).with_context(|| "Failed to decode the signature.")?;
        Ok(Signature {
            sig: orion::auth::Tag::from_slice(&bytes)
                .with_context(|| "Failed to create signature.")?,
        })
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
    /// This will return the encrypted data as a hex-encoded string.
    ///
    /// Note that the data to encrypt must be serialisable and deserialisable
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
    /// use paddington::crypto::Key;
    ///
    /// let key = Key::generate();
    ///
    /// let encrypted_data = key.expose_secret().encrypt("Hello, World!".to_string());
    /// ```
    pub fn encrypt<T>(&self, data: T) -> Result<String, Error>
    where
        T: Serialize,
    {
        let orion_key = aead::SecretKey::from_slice(&self.data)
            .with_context(|| "Failed to create a secret key from the secret key data.")?;
        let json_data = serde_json::to_string(&data).with_context(|| {
            "Failed to serialise the data to JSON. Ensure that the data is serialisable by serde."
        })?;

        let encrypted_data = aead::seal(&orion_key, json_data.as_bytes())
            .with_context(|| "Failed to encrypt the data.")?;

        Ok(hex::encode(encrypted_data))
    }

    ///
    /// Decrypt the passed data with this key.
    /// This will return the decrypted data.
    ///
    /// Arguments
    ///
    /// * `data` - The data to decrypt (hex-encoded string)
    ///
    /// Returns
    ///
    /// The decrypted data.
    ///
    /// Example
    ///
    /// ```
    /// use paddington::crypto::Key;
    ///
    /// let key = Key::generate();
    ///
    /// let encrypted_data = key.expose_secret().encrypt("Hello, World!".to_string());
    /// let decrypted_data = key.expose_secret().decrypt(&encrypted_data).unwrap();
    ///
    /// assert_eq!(decrypted_data, "Hello, World!".to_string());
    /// ```
    pub fn decrypt<T>(&self, data: &str) -> Result<T, Error>
    where
        T: DeserializeOwned,
    {
        let orion_key = aead::SecretKey::from_slice(&self.data)
            .with_context(|| "Failed to create a secret key from the secret key data.")?;

        let data = hex::decode(data).with_context(|| "Failed to decode the hex-encoded data.")?;

        let decrypted_data =
            aead::open(&orion_key, &data).with_context(|| "Failed to decrypt the data.")?;

        let decrypted_string: String = String::from_utf8(decrypted_data)
            .with_context(|| "Failed to convert the decrypted data to a string.")?;

        let obj: T = serde_json::from_str(&decrypted_string)
            .with_context(|| "Failed to deserialise the decrypted data from JSON.")?;

        Ok(obj)
    }

    ///
    /// Sign (authenticate) the passed data with this key.
    /// This will return the signed data as a hex-encoded string.
    ///
    /// Arguments
    ///
    /// * `data` - The data to sign.
    ///
    /// Returns
    ///
    /// A Signature object containing the signature.
    ///
    /// Example
    ///
    /// ```
    /// use paddington::crypto::Key;
    ///
    /// let key = Key::generate();
    ///
    /// let signature = key.expose_secret().sign("Hello, World!".to_string());
    /// ```
    ///
    pub fn sign<T>(&self, data: T) -> Result<Signature, Error>
    where
        T: Serialize,
    {
        let orion_key = aead::SecretKey::from_slice(&self.data)
            .with_context(|| "Failed to create a secret key from the secret key data.")?;
        let json_data = serde_json::to_string(&data).with_context(|| {
            "Failed to serialise the data to JSON. Ensure that the data is serialisable by serde."
        })?;

        let signature = auth::authenticate(&orion_key, json_data.as_bytes())
            .with_context(|| "Failed to sign the data.")?;

        Ok(Signature { sig: signature })
    }

    ///
    /// Verify the passed data matches the signature with this key.
    /// This will return true if the data matches the signature, false otherwise.
    ///
    /// Arguments
    ///
    /// * `data` - The data to verify.
    /// * `signature` - The signature to verify against.
    ///
    /// Returns
    ///
    /// An error if the data does not match the signature, else Ok(())
    ///
    /// Example
    ///
    /// ```
    /// use paddington::crypto::Key;
    ///
    /// let key = Key::generate();
    ///
    /// let signature = key.expose_secret().sign("Hello, World!".to_string());
    ///
    /// key.expose_secret().verify("Hello, World!".to_string(), &signature)?;
    /// ```
    ///
    pub fn verify<T>(&self, data: T, signature: &Signature) -> Result<(), Error>
    where
        T: Serialize,
    {
        let orion_key = aead::SecretKey::from_slice(&self.data)
            .with_context(|| "Failed to create a secret key from the secret key data.")?;

        let data = serde_json::to_string(&data).with_context(|| {
            "Failed to serialise the data to JSON. Ensure that the data is serialisable by serde."
        })?;

        auth::authenticate_verify(&signature.sig, &orion_key, data.as_bytes())
            .with_context(|| "Failed to verify the data.")?;

        Ok(())
    }
}
