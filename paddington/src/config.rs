// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::crypto::{CryptoError, Key, SecretKey};
use anyhow::Context;
use anyhow::Error as AnyError;
use serde::{Deserialize, Serialize};
use serde_json;
use std::fmt::{self, Display};
use std::path;
use thiserror::Error;

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

///
/// The full service configuration.
/// This includes the service name, the secret key, the server to bind to and the port to bind to.
/// The secret key is generated when the service is created and is used to encrypt and decrypt messages.
/// The server is the address that the service will bind to and the port is the port that the service will bind to.
/// The service name is the name of the service.
///
/// # Example
///
/// ```
/// use paddington::config::ServiceConfig;
/// use paddington::crypto::Key;
///
/// let config = ServiceConfig {
///    name: "openportal".to_string(),
///    key: Key::generate(),
///    server: "localhost".to_string(),
///    port: 8080,
/// };
/// ```
///
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ServiceConfig {
    pub name: String,
    pub key: SecretKey,
    pub server: String,
    pub port: u16,
}

impl Display for ServiceConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt(f)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PeerConfig {
    pub name: String,
    pub key: SecretKey,
    pub server: String,
    pub port: u16,
}

impl Display for PeerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt(f)
    }
}

impl ServiceConfig {
    pub fn new(name: String, key: SecretKey, server: String, port: u16) -> Self {
        ServiceConfig {
            name,
            key,
            server,
            port,
        }
    }

    pub fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ServiceConfig {{ name: {}, server: {}, port: {} }}",
            self.name, self.server, self.port
        )
    }

    pub fn is_null(&self) -> bool {
        self.name == "" || self.server == ""
    }

    pub fn is_valid(&self) -> bool {
        !self.is_null()
    }

    pub fn default() -> Self {
        ServiceConfig {
            name: "".to_string(),
            key: Key::null(),
            server: "".to_string(),
            port: 0,
        }
    }
}

impl PeerConfig {
    pub fn new(name: String, key: SecretKey, server: String, port: u16) -> Self {
        PeerConfig {
            name,
            key,
            server,
            port,
        }
    }

    pub fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "PeerConfig {{ name: {}, server: {}, port: {} }}",
            self.name, self.server, self.port
        )
    }

    pub fn from_service_config(config: ServiceConfig) -> Self {
        PeerConfig {
            name: config.name,
            key: config.key,
            server: config.server,
            port: config.port,
        }
    }

    pub fn is_null(&self) -> bool {
        self.name == "" || self.server == ""
    }

    pub fn is_valid(&self) -> bool {
        !self.is_null()
    }

    pub fn default() -> Self {
        PeerConfig {
            name: "".to_string(),
            key: Key::null(),
            server: "".to_string(),
            port: 0,
        }
    }
}

///
/// Create a new service configuration in the passed directory.
/// This will return an error if the config directory already exists.
/// The service name, server and port can be passed as arguments.
/// If they are not passed, default values will be used.
/// The service name will default to "openportal", the server will default to "localhost"
/// and the port will default to 8080.
///
/// # Arguments
///
/// * `config_dir` - The directory to create the service configuration in.
/// * `service_name` - The name of the service.
/// * `server` - The server to bind to.
/// * `port` - The port to bind to.
///
/// # Returns
///
/// The full service configuration.
///
/// # Errors
///
/// This function will return an error if the config directory already exists.
///
/// # Example
///
/// ```
/// use paddington::config;
///
/// let config = config::create("/path/to/config", "service_name",
///                             "https://service_url", 8000)?;
///
/// println!("Service name: {}", config.name);
/// ```
///
pub fn create(
    config_dir: &path::PathBuf,
    service_name: &Option<String>,
    server: &Option<String>,
    port: &Option<u16>,
) -> Result<ServiceConfig, ConfigError> {
    // see if this config_dir exists - return an error if it does
    let config_dir = path::absolute(config_dir).with_context(|| {
        format!(
            "Could not get absolute path for config directory: {:?}",
            config_dir
        )
    })?;

    if config_dir.try_exists()? {
        return Err(ConfigError::ExistsError(config_dir));
    }

    // create the config directory
    std::fs::create_dir_all(&config_dir).with_context(|| {
        format!(
            "Could not create config directory: {:?}",
            config_dir.to_string_lossy()
        )
    })?;

    let service_name = service_name.clone().unwrap_or("openportal".to_string());
    let server = server.clone().unwrap_or("localhost".to_string());
    let port = port.unwrap_or(8080);

    let config = ServiceConfig {
        name: service_name.clone(),
        key: Key::generate(),
        server: server.clone(),
        port,
    };

    // write the config to a json file
    let config_file = config_dir.join("service.json");
    let config_file_string = config_dir.to_string_lossy();

    let config_json = serde_json::to_string(&config).with_context(|| {
        format!(
            "Could not serialise config to json: {:?}",
            config_file_string
        )
    })?;

    std::fs::write(config_file, config_json)
        .with_context(|| format!("Could not write config file: {:?}", config_file_string))?;

    // read the config and return it
    let config = load(&config_dir)?;

    Ok(config)
}

///
/// Load the full service configuration from the passed directory.
/// This will return an error if the config directory does not exist
/// or if the data within cannot be read.
///
/// # Arguments
///
/// * `config_dir` - The directory containing the service configuration.
///
/// # Returns
///
/// The full service configuration.
///
/// # Errors
///
/// This function will return an error if the config directory does not exist
/// or if the data within cannot be read.
///
/// # Example
///
/// ```
/// use paddington::config;
///
/// let config = config::load("/path/to/config")?;
///
/// println!("Service name: {}", config.name);
/// ```
///
pub fn load(config_dir: &path::PathBuf) -> Result<ServiceConfig, ConfigError> {
    // see if this config_dir exists - return an error if it doesn't
    let config_dir = path::absolute(config_dir)?;

    if !config_dir.try_exists()? {
        return Err(ConfigError::NotExistsError(config_dir));
    }

    // look for a json config file called "service.json" in the config directory
    let config_file = config_dir.join("service.json");
    let config_string = config_file.to_string_lossy();

    // read the config file
    let config = std::fs::read_to_string(&config_file)
        .with_context(|| format!("Could not read config file: {:?}", config_string))?;

    // parse the config file
    let config: ServiceConfig = serde_json::from_str(&config)
        .with_context(|| format!("Could not parse config file fron json: {:?}", config_string))?;

    Ok(config)
}
