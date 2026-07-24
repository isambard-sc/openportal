// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Context;
use orion::{aead, auth, hazardous::kdf::hkdf, kdf};
use secrecy::{zeroize::Zeroize, CloneableSecret, SecretBox, SerializableSecret};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_with::serde_as;
use std::fmt::Display;
use std::{fmt, str, vec};

use crate::error::Error;

pub const KEY_SIZE: usize = 32;
pub const SALT_SIZE: usize = KEY_SIZE;

/// Argon2 parameters for versioned (v1) password-based secret encryption
/// (`from_password_with_salt`). Far above orion's minimum (`MIN_MEMORY = 8`
/// KiB, `MIN_ITERATIONS = 3`): 19 MiB / 3 passes matches the current OWASP
/// Argon2 floor. Used only for config-at-rest secrets, derived rarely (at
/// startup and when the operator sets a `secret`), so the cost is never on a
/// hot path. See docs/specifications/security-review.md (finding F2).
const SECRET_KDF_MEMORY_KIB: u32 = 19456;
const SECRET_KDF_ITERATIONS: u32 = 3;

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

pub fn random_bytes(size: usize) -> Result<Vec<u8>, Error> {
    let mut data: Vec<u8> = vec![0; size];
    orion::util::secure_rand_bytes(&mut data).context("Failed to generate random bytes.")?;
    Ok(data)
}

///
/// Constant-time equality check for two byte slices, using orion's vetted
/// `secure_cmp`. Returns `true` iff the slices are equal. Use this instead of
/// `==` (or a hand-rolled loop) when comparing authentication tags, tokens, or
/// other secret-derived values, to avoid a timing side-channel. Slices of
/// differing length compare unequal (the length itself is not treated as
/// secret).
///
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    orion::util::secure_cmp(a, b).is_ok()
}

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Salt {
    #[serde_as(as = "serde_with::hex::Hex")]
    data: vec::Vec<u8>,
}

impl Display for Salt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(&self.data))
    }
}

impl Salt {
    pub fn generate() -> Result<Salt, Error> {
        let mut data: Vec<u8> = [0u8; SALT_SIZE].to_vec();
        orion::util::secure_rand_bytes(&mut data).context("Failed to generate a salt.")?;

        Ok(Salt { data })
    }

    pub fn xor(self: &Salt, key: &Key) -> Salt {
        let data: Vec<u8> = self
            .data
            .iter()
            .zip(key.data.iter())
            .map(|(&x1, &x2)| x1 ^ x2)
            .collect();

        Salt { data }
    }
}

impl std::str::FromStr for Salt {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = hex::decode(s).with_context(|| "Failed to decode the salt.")?;
        Ok(Salt { data: bytes })
    }
}

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Key {
    #[serde_as(as = "serde_with::hex::Hex")]
    data: vec::Vec<u8>,
}

impl Zeroize for Key {
    fn zeroize(&mut self) {
        self.data.zeroize();
    }
}

/// Permits cloning, Debug printing as [[REDACTED]] and serialising
impl CloneableSecret for Key {}
impl SerializableSecret for Key {}

pub type SecretKey = SecretBox<Key>;

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
    /// use paddington::{Key, SecretKey};
    ///
    /// let key = Key::generate();
    /// ```
    pub fn generate() -> SecretKey {
        Box::new(Key {
            data: aead::SecretKey::default().unprotected_as_bytes().to_vec(),
        })
        .into()
    }

    ///
    /// Derive a new secret key from this key, the passed salt, and
    /// the (optional) additional information
    ///
    pub fn derive(
        self: &Key,
        salt: &Salt,
        additional_info: Option<&[u8]>,
    ) -> Result<SecretKey, Error> {
        let mut new_key = self.data.clone();

        hkdf::sha512::derive_key(&salt.data, &self.data, additional_info, &mut new_key)
            .context("Failed to derive key.")?;

        Ok(Box::new(Key { data: new_key }).into())
    }

    ///
    /// Derive a secret key from a password and an explicit salt, using strong
    /// Argon2 parameters (`SECRET_KDF_*`). This is the derivation used by the
    /// versioned (v1) config-secret encryption: a fresh random salt is stored
    /// alongside each ciphertext, so identical passwords no longer produce
    /// identical keys and the derivation cost is realistic. See
    /// `ServiceConfig::encrypt`/`decrypt` and
    /// docs/specifications/security-review.md (finding F2).
    ///
    /// # Arguments
    ///
    /// * `password` - The password to derive the key from.
    /// * `salt` - A per-secret random salt (see `SECRET_SALT_SIZE`).
    ///
    pub fn from_password_with_salt(password: &str, salt: &[u8]) -> Result<SecretKey, Error> {
        let salt = kdf::Salt::from_slice(salt)
            .context("Failed to create a salt from the supplied salt bytes.")?;

        Ok(Box::new(Key {
            data: kdf::derive_key(
                &kdf::Password::from_slice(password.as_bytes())
                    .context("Failed to build a KDF password from the supplied secret")?,
                &salt,
                SECRET_KDF_ITERATIONS,
                SECRET_KDF_MEMORY_KIB,
                KEY_SIZE as u32,
            )
            .context("Failed to derive key from password and salt.")?
            .unprotected_as_bytes()
            .to_vec(),
        })
        .into())
    }

    ///
    /// Generate a new secret key from the supplied password - this will
    /// reproducibly generate the same key from the same password.
    ///
    /// **Legacy.** This uses a fixed application-defined salt and orion's
    /// minimum Argon2 cost, so it is deterministic and weak. It is retained
    /// only to decrypt pre-existing (v0) config secrets; new secrets are
    /// written with `from_password_with_salt`. See
    /// docs/specifications/security-review.md (finding F2).
    ///
    /// # Arguments
    ///
    /// * `password` - The password to generate the key from.
    ///
    /// # Returns
    ///
    /// The secret key.
    ///
    /// # Example
    ///
    /// ```
    /// use paddington::{Key, SecretKey};
    /// use secrecy::ExposeSecret;
    ///
    /// let key = Key::from_password("password");
    /// ```
    pub fn from_password(password: &str) -> Result<SecretKey, Error> {
        // we need to use an application-defined salt to ensure that we always
        // get the same key from the same password

        // this is a random salt of 16 bytes
        let salt = vec![
            0x3a, 0x7f, 0x1b, 0x4c, 0x5d, 0x6e, 0x2f, 0x8a, 0x9b, 0xac, 0xbd, 0xce, 0xdf, 0xef,
            0xf0, 0x01,
        ];

        let salt =
            kdf::Salt::from_slice(&salt).context("Failed to create a salt from the salt data.")?;

        Ok(Box::new(Key {
            data: kdf::derive_key(
                // Do not interpolate `password` into this context: it is the
                // secret (the OPENPORTAL_SECRET_KEY value under the Environment
                // scheme) and would otherwise be leaked into logs/stderr.
                &kdf::Password::from_slice(password.as_bytes())
                    .context("Failed to build a KDF password from the supplied secret")?,
                &salt,
                3,
                8,
                KEY_SIZE as u32,
            )
            .context("Failed to derive key from password.")?
            .unprotected_as_bytes()
            .to_vec(),
        })
        .into())
    }

    ///
    /// Create and return a null key - this should not be used
    ///
    pub fn null() -> SecretKey {
        Box::new(Key {
            data: vec![0; KEY_SIZE],
        })
        .into()
    }

    ///
    /// Return whether or not this key is null
    ///
    pub fn is_null(&self) -> bool {
        self.data.is_empty() || self.data.iter().all(|&x| x == 0)
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
    /// use paddington::{Key, SecretKey};
    /// use secrecy::ExposeSecret;
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
    /// use paddington::{Key, SecretKey};
    /// use secrecy::ExposeSecret;
    ///
    /// let key = Key::generate();
    ///
    /// let encrypted_data = key.expose_secret().encrypt("Hello, World!").unwrap();
    /// let decrypted_data: String = key.expose_secret().decrypt(&encrypted_data).unwrap();
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
    /// # Key domain separation
    ///
    /// `sign`/`verify` (BLAKE2b MAC) and `encrypt`/`decrypt` (XChaCha20-Poly1305
    /// AEAD) use the key bytes directly, with no domain-separating sub-key
    /// derivation. This is safe **only because a given `Key` is never used for
    /// both purposes** in OpenPortal: the AEAD is used for wire messages and
    /// config-at-rest secrets, and the MAC is used solely for the `op-bridge`
    /// HTTP request signatures - always distinct keys. If a future caller ever
    /// needs to use one key for both, it must first derive purpose-specific
    /// sub-keys (e.g. `derive` with an `"enc"`/`"mac"` `info`) to avoid
    /// cross-protocol key reuse. See docs/specifications/security-review.md
    /// (finding F15).
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
    /// use paddington::{Key, SecretKey};
    /// use secrecy::ExposeSecret;
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
    /// use paddington::{Key, SecretKey};
    /// use secrecy::ExposeSecret;
    ///
    /// let key = Key::generate();
    ///
    /// let signature = key.expose_secret().sign("Hello, World!".to_string()).unwrap();
    ///
    /// key.expose_secret().verify("Hello, World!".to_string(), &signature).unwrap();
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

#[cfg(test)]
mod tests {
    use secrecy::ExposeSecret;

    use super::*;

    #[test]
    fn test_key_generate() {
        let key = Key::generate();
        assert_eq!(key.expose_secret().data.len(), KEY_SIZE);
    }

    #[test]
    fn test_key_from_password() {
        let key: SecretBox<Key> = Key::from_password("password").unwrap_or_else(|err| {
            unreachable!("Failed to create key from password: {}", err);
        });

        assert_eq!(key.expose_secret().data.len(), KEY_SIZE);

        let key2: SecretKey = Key::from_password("password").unwrap_or_else(|err| {
            unreachable!("Failed to create key from password: {}", err);
        });

        assert_eq!(key.expose_secret().data, key2.expose_secret().data);
    }

    #[test]
    fn test_salt_xor_roundtrip() {
        // The legacy salt masking relies on XOR being self-inverse:
        // (salt XOR key) XOR key == salt. The server un-masks an old client's
        // salts this way (finding F15).
        let key = Key::generate();
        let salt = Salt::generate().unwrap_or_else(|err| {
            unreachable!("Failed to generate salt: {}", err);
        });

        let masked = salt.xor(key.expose_secret());
        let unmasked = masked.xor(key.expose_secret());

        assert_eq!(salt.to_string(), unmasked.to_string());
        // Masking actually changes the value (keys are non-null random).
        assert_ne!(salt.to_string(), masked.to_string());
    }

    #[test]
    fn test_constant_time_eq() {
        assert!(constant_time_eq(b"abcdef", b"abcdef"));
        assert!(!constant_time_eq(b"abcdef", b"abcdeg"));
        assert!(!constant_time_eq(b"abc", b"abcdef"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn test_key_encrypt_decrypt() {
        let key: SecretBox<Key> = Key::generate();

        let encrypted_data: String = key
            .expose_secret()
            .encrypt("Hello, World!".to_string())
            .unwrap_or_else(|err| {
                unreachable!("Failed to encrypt data: {}", err);
            });

        let decrypted_data: String =
            key.expose_secret()
                .decrypt(&encrypted_data)
                .unwrap_or_else(|err| {
                    unreachable!("Failed to decrypt data: {}", err);
                });

        assert_eq!(decrypted_data, "Hello, World!".to_string());
    }

    #[test]
    fn test_key_sign_verify() {
        let key: SecretBox<Key> = Key::generate();

        let signature: Signature = key
            .expose_secret()
            .sign("Hello, World!".to_string())
            .unwrap_or_else(|err| {
                unreachable!("Failed to sign data: {}", err);
            });

        key.expose_secret()
            .verify("Hello, World!".to_string(), &signature)
            .unwrap_or_else(|err| {
                unreachable!("Failed to verify data: {}", err);
            });
    }
}
