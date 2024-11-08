// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::crypto::SecretKey;
use crate::error::Error;
use anyhow::Context;
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use std::fmt::{self, Display};
use std::path;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Invite {
    name: String,
    url: String,
    zone: String,
    inner_key: SecretKey,
    outer_key: SecretKey,
}

impl Invite {
    pub fn new(
        name: &str,
        url: &str,
        zone: &str,
        inner_key: &SecretKey,
        outer_key: &SecretKey,
    ) -> Self {
        Invite {
            name: name.to_string(),
            url: url.to_string(),
            zone: zone.to_string(),
            inner_key: inner_key.clone(),
            outer_key: outer_key.clone(),
        }
    }

    pub fn name(&self) -> String {
        self.name.clone()
    }

    pub fn url(&self) -> String {
        self.url.clone()
    }

    pub fn zone(&self) -> String {
        self.zone.clone()
    }

    pub fn inner_key(&self) -> SecretKey {
        self.inner_key.clone()
    }

    pub fn outer_key(&self) -> SecretKey {
        self.outer_key.clone()
    }

    pub fn assert_valid(&self) -> Result<(), Error> {
        if self.name.is_empty() {
            return Err(Error::Null("Invite name is empty".to_string()));
        }

        // check the name is alphanumeric
        if !self
            .name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            return Err(Error::InvalidPeer(format!(
                "Name '{}' contains invalid characters. It must be alphanumeric or - _",
                self.name
            )));
        }

        if self.url.is_empty() {
            return Err(Error::Null("Invite url is empty".to_string()));
        }

        if self.zone.is_empty() {
            return Err(Error::Null("Invite zone is empty".to_string()));
        }

        // check the zone is alphanumeric
        if !self
            .zone
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            return Err(Error::InvalidPeer(format!(
                "Zone '{}' contains invalid characters. It must be alphanumeric or - _",
                self.zone
            )));
        }

        if self.inner_key.expose_secret().is_null() {
            return Err(Error::InvalidPeer("Invite inner key is null".to_string()));
        }

        if self.outer_key.expose_secret().is_null() {
            return Err(Error::InvalidPeer("Invite outer key is null".to_string()));
        }

        Ok(())
    }

    pub fn load(filename: &path::Path) -> Result<Self, Error> {
        let invite = std::fs::read_to_string(filename)
            .with_context(|| format!("Could not read invite file: {:?}", filename))?;

        let invite: Invite = toml::from_str(&invite)
            .with_context(|| format!("Could not parse invite file from toml: {:?}", filename))?;

        invite.assert_valid()?;

        Ok(invite)
    }

    pub fn save(&self, filename: &path::Path) -> Result<(), Error> {
        self.assert_valid()?;

        let invite_toml =
            toml::to_string(&self).with_context(|| "Could not serialise invite to toml")?;

        let invite_file_string = filename.to_string_lossy();

        let prefix = filename.parent().with_context(|| {
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

        std::fs::write(filename, invite_toml)
            .with_context(|| format!("Could not write invite file: {:?}", filename))?;

        Ok(())
    }
}

impl Display for Invite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Invite {{ name: {}, url: {}, zone: {} }}",
            self.name, self.url, self.zone
        )
    }
}

pub fn load(invite_file: &path::PathBuf) -> Result<Invite, Error> {
    // read the invite file
    let invite = std::fs::read_to_string(invite_file)
        .with_context(|| format!("Could not read invite file: {:?}", invite_file))?;

    // parse the invite file
    let invite: Invite = toml::from_str(&invite)
        .with_context(|| format!("Could not parse invite file from toml: {:?}", invite_file))?;

    // assert valid after loading, in case it has been tampered with
    invite.assert_valid()?;

    Ok(invite)
}

pub fn save(invite: &Invite, invite_file: &path::PathBuf) -> Result<(), Error> {
    // only save valid invites
    invite.assert_valid()?;

    // serialise to toml
    let invite_toml =
        toml::to_string(invite).with_context(|| "Could not serialise invite to toml")?;

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
