// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::crypto::{CryptoError, Key, SecretKey};
use anyhow::Context;
use anyhow::Error as AnyError;
use serde::{Deserialize, Serialize};
use serde_json;
use std::fmt::{self, Display};
use std::net::IpAddr;
use std::path;
use thiserror::Error;
use toml;
use url::Url;

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("{0}")]
    IOError(#[from] std::io::Error),

    #[error("{0}")]
    AnyError(#[from] AnyError),

    #[error("{0}")]
    SerdeError(#[from] serde_json::Error),

    #[error("{0}")]
    CryptoError(#[from] CryptoError),

    #[error("Config directory already exists: {0}")]
    ExistsError(path::PathBuf),

    #[error("Config directory does not exist: {0}")]
    NotExistsError(path::PathBuf),

    #[error("Unknown config error")]
    Unknown,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ConnectionInvite {
    pub name: String,
    pub url: url::Url,
    pub inner_key: SecretKey,
    pub outer_key: SecretKey,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ServerConfig {
    pub name: String,
    pub url: Url,
    pub inner_key: SecretKey,
    pub outer_key: SecretKey,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ClientConfig {
    pub ip: Option<IpAddr>,
    pub netmask: Option<IpAddr>,
    pub inner_key: SecretKey,
    pub outer_key: SecretKey,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ServiceConfig {
    pub name: Option<String>,
    pub url: Option<Url>,
    pub servers: Option<Vec<ServerConfig>>,
    pub clients: Option<Vec<ClientConfig>>,
    pub encryption: Option<String>,
}

impl Display for ServiceConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt(f)
    }
}

impl ServiceConfig {
    pub fn new(name: String, url: Option<Url>) -> Self {
        ServiceConfig {
            name: Some(name),
            url,
            servers: None,
            clients: None,
            encryption: None,
        }
    }

    pub fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.is_valid() {
            return write!(f, "ServiceConfig {{ name: null }}");
        } else if self.url.is_some() {
            write!(
                f,
                "ServiceConfig {{ name: {}, url: {} }}",
                self.name.as_ref().unwrap(),
                self.url.as_ref().unwrap()
            )
        } else {
            write!(
                f,
                "ServiceConfig {{ name: {} }}",
                self.name.as_ref().unwrap()
            )
        }
    }

    pub fn is_null(&self) -> bool {
        self.name.is_none()
    }

    pub fn is_valid(&self) -> bool {
        !self.is_null()
    }

    pub fn create_null() -> Self {
        ServiceConfig {
            name: None,
            url: None,
            servers: None,
            clients: None,
            encryption: None,
        }
    }

    pub fn num_clients(&self) -> usize {
        if self.clients.is_none() {
            return 0;
        }

        self.clients.as_ref().unwrap().len()
    }

    pub fn num_servers(&self) -> usize {
        if self.servers.is_none() {
            return 0;
        }

        self.servers.as_ref().unwrap().len()
    }

    pub fn get_clients(&self) -> Vec<ClientConfig> {
        if self.clients.is_none() {
            return Vec::new();
        }

        self.clients.as_ref().unwrap().clone()
    }

    pub fn get_servers(&self) -> Vec<ServerConfig> {
        if self.servers.is_none() {
            return Vec::new();
        }

        self.servers.as_ref().unwrap().clone()
    }
}

pub fn create(
    config_file: &path::PathBuf,
    service_name: &String,
    url: &Option<Url>,
) -> Result<ServiceConfig, ConfigError> {
    // see if this config_dir exists - return an error if it does
    let config_file = path::absolute(config_file).with_context(|| {
        format!(
            "Could not get absolute path for config file: {:?}",
            config_file
        )
    })?;

    if config_file.try_exists()? {
        return Err(ConfigError::ExistsError(config_file));
    }

    let config = ServiceConfig::new(service_name.clone(), url.clone());

    // write the config to a json file
    let config_toml =
        toml::to_string(&config).with_context(|| "Could not serialise config to toml")?;

    let config_file_string = config_file.to_string_lossy();

    let prefix = config_file.parent().with_context(|| {
        format!(
            "Could not get parent directory for config file: {:?}",
            config_file_string
        )
    })?;

    std::fs::create_dir_all(prefix).with_context(|| {
        format!(
            "Could not create parent directory for config file: {:?}",
            config_file_string
        )
    })?;

    std::fs::write(&config_file, config_toml)
        .with_context(|| format!("Could not write config file: {:?}", config_file_string))?;

    // read the config and return it
    let config = load(&config_file)?;

    Ok(config)
}

///
/// Load the full service configuration from the passed config file.
/// This will return an error if the config file does not exist
/// or if the data within cannot be read.
///
/// # Arguments
///
/// * `config_file` - The file containing the service configuration.
///
/// # Returns
///
/// The full service configuration.
///
/// # Errors
///
/// This function will return an error if the config file does not exist
/// or if the data within cannot be read.
///
/// # Example
///
/// ```
/// use paddington::config;
///
/// let config = config::load("/path/to/config_file")?;
///
/// println!("Service name: {}", config.name);
/// ```
///
pub fn load(config_file: &path::PathBuf) -> Result<ServiceConfig, ConfigError> {
    // see if this config_dilw exists - return an error if it doesn't
    let config_file = path::absolute(config_file)?;

    if !config_file.try_exists()? {
        return Err(ConfigError::NotExistsError(config_file));
    }

    // read the config file
    let config = std::fs::read_to_string(&config_file)
        .with_context(|| format!("Could not read config file: {:?}", config_file))?;

    // parse the config file
    let config: ServiceConfig = toml::from_str(&config)
        .with_context(|| format!("Could not parse config file fron toml: {:?}", config_file))?;

    Ok(config)
}
