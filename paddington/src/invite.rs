// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::crypto::{Error as CryptoError, SecretKey};
use anyhow::Context;
use anyhow::Error as AnyError;
use serde::{Deserialize, Serialize};
use std::fmt::{self, Display};
use std::path;
use thiserror::Error;
use url::ParseError as UrlParseError;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Invite {
    pub name: String,
    pub url: String,
    pub inner_key: SecretKey,
    pub outer_key: SecretKey,
}

impl Display for Invite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Invite {{ name: {}, url: {} }}", self.name, self.url)
    }
}

pub fn load<T: serde::de::DeserializeOwned + serde::Serialize>(
    invite_file: &path::PathBuf,
) -> Result<T, Error> {
    // read the invite file
    let invite = std::fs::read_to_string(invite_file)
        .with_context(|| format!("Could not read invite file: {:?}", invite_file))?;

    // parse the invite file
    let invite: T = toml::from_str(&invite)
        .with_context(|| format!("Could not parse invite file from toml: {:?}", invite_file))?;

    Ok(invite)
}

pub fn save<T: serde::de::DeserializeOwned + serde::Serialize>(
    config: T,
    invite_file: &path::PathBuf,
) -> Result<(), Error> {
    // serialise to toml
    let invite_toml =
        toml::to_string(&config).with_context(|| "Could not serialise invite to toml")?;

    let invite_file_string = invite_file.to_string_lossy();

    let prefix = invite_file.parent().with_context(|| {
        format!(
            "Could not get parent directory for invite file: {:?}",
            invite_file_string
        )
    })?;

    std::fs::create_dir_all(prefix).with_context(|| {
        format!(
            "Could not create parent directory for invite file: {:?}",
            invite_file_string
        )
    })?;

    std::fs::write(invite_file, invite_toml)
        .with_context(|| format!("Could not write invite file: {:?}", invite_file))?;

    Ok(())
}

/// Errors

#[derive(Error, Debug)]
pub enum Error {
    #[error("{0}")]
    IO(#[from] std::io::Error),

    #[error("{0}")]
    Any(#[from] AnyError),

    #[error("{0}")]
    Serde(#[from] serde_json::Error),

    #[error("{0}")]
    Crypto(#[from] CryptoError),

    #[error("{0}")]
    UrlParse(#[from] UrlParseError),

    #[error("{0}")]
    Peer(String),

    #[error("Config directory already exists: {0}")]
    Exists(path::PathBuf),

    #[error("Config directory does not exist: {0}")]
    NotExists(path::PathBuf),

    #[error("Config file is null: {0}")]
    Null(String),

    #[error("Unknown config error")]
    Unknown,
}
