// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::crypto::SecretKey;
use crate::error::Error;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::fmt::{self, Display};
use std::path;

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
