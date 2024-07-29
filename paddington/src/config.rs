// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::crypto::{CryptoError, Key, SecretKey};
use anyhow::Context;
use anyhow::Error as AnyError;
use iptools::iprange::IpRange;
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

impl Display for ServerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ServerConfig {{ name: {} }}", self.name)
    }
}

impl ServerConfig {
    pub fn new(name: &str, url: &Url) -> Self {
        ServerConfig {
            name: name.to_string(),
            url: url.clone(),
            inner_key: Key::generate(),
            outer_key: Key::generate(),
        }
    }

    pub fn create_null() -> Self {
        ServerConfig {
            name: "".to_string(),
            url: Url::parse("http://localhost").unwrap(),
            inner_key: Key::null(),
            outer_key: Key::null(),
        }
    }

    pub fn is_null(&self) -> bool {
        self.name.is_empty()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum IpOrRange {
    IP(IpAddr),
    Range(String),
}

impl Display for IpOrRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IpOrRange::IP(ip) => write!(f, "{}", ip),
            IpOrRange::Range(range) => write!(f, "{}", range),
        }
    }
}

impl IpOrRange {
    pub fn matches(&self, addr: &IpAddr) -> bool {
        match self {
            IpOrRange::IP(ip) => ip == addr,
            IpOrRange::Range(range) => match IpRange::new(range, "") {
                Ok(range) => range.contains(&addr.to_string()).unwrap_or(false),
                Err(_) => false,
            },
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ClientConfig {
    pub ip: Option<IpOrRange>,
    pub inner_key: SecretKey,
    pub outer_key: SecretKey,
}

impl Display for ClientConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.ip {
            Some(ip) => write!(f, "ClientConfig {{ ip: {} }}", ip),
            None => write!(f, "ClientConfig {{ ip: None }}"),
        }
    }
}

impl ClientConfig {
    pub fn new(ip: &Option<IpOrRange>) -> Self {
        ClientConfig {
            ip: ip.clone(),
            inner_key: Key::generate(),
            outer_key: Key::generate(),
        }
    }

    pub fn create_null() -> Self {
        ClientConfig {
            ip: None,
            inner_key: Key::null(),
            outer_key: Key::null(),
        }
    }

    pub fn is_null(&self) -> bool {
        self.ip.is_none()
    }

    pub fn matches(&self, addr: IpAddr) -> bool {
        match &self.ip {
            Some(ip) => ip.matches(&addr),
            None => false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum PeerConfig {
    Server(ServerConfig),
    Client(ClientConfig),
    None,
}

impl Display for PeerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PeerConfig::Server(server) => write!(f, "{}", server),
            PeerConfig::Client(client) => write!(f, "{}", client),
            PeerConfig::None => write!(f, "PeerConfig {{ None }}"),
        }
    }
}

impl PeerConfig {
    pub fn from_server(server: &ServerConfig) -> Self {
        PeerConfig::Server(server.clone())
    }

    pub fn from_client(client: &ClientConfig) -> Self {
        PeerConfig::Client(client.clone())
    }

    pub fn create_null() -> Self {
        PeerConfig::None
    }

    pub fn is_null(&self) -> bool {
        match self {
            PeerConfig::Server(server) => server.is_null(),
            PeerConfig::Client(client) => client.is_null(),
            PeerConfig::None => true,
        }
    }

    pub fn is_client(&self) -> bool {
        matches!(self, PeerConfig::Client(_))
    }

    pub fn is_server(&self) -> bool {
        matches!(self, PeerConfig::Server(_))
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ServiceConfig {
    pub name: Option<String>,
    pub url: Option<Url>,
    pub ip: Option<IpAddr>,
    pub port: Option<u16>,

    servers: Option<Vec<ServerConfig>>,
    clients: Option<Vec<ClientConfig>>,
    encryption: Option<String>,
}

impl Display for ServiceConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt(f)
    }
}

impl ServiceConfig {
    pub fn new(name: &str, url: &Option<Url>) -> Self {
        ServiceConfig {
            name: Some(name.to_string()),
            url: url.clone(),
            ip: None,
            port: None,
            servers: None,
            clients: None,
            encryption: None,
        }
    }

    pub fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name {
            Some(ref name) => write!(f, "ServiceConfig {{ name: {} }}", name),
            None => write!(f, "ServiceConfig {{ name: null }}"),
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
            ip: None,
            port: None,
            servers: None,
            clients: None,
            encryption: None,
        }
    }

    pub fn has_clients(&self) -> bool {
        match &self.clients {
            Some(clients) => !clients.is_empty(),
            None => false,
        }
    }

    pub fn has_servers(&self) -> bool {
        match &self.servers {
            Some(servers) => !servers.is_empty(),
            None => false,
        }
    }

    pub fn num_clients(&self) -> usize {
        match &self.clients {
            Some(clients) => clients.len(),
            None => 0,
        }
    }

    pub fn num_servers(&self) -> usize {
        match &self.servers {
            Some(servers) => servers.len(),
            None => 0,
        }
    }

    pub fn get_clients(&self) -> Vec<ClientConfig> {
        match &self.clients {
            Some(clients) => clients.clone(),
            None => Vec::new(),
        }
    }

    pub fn get_servers(&self) -> Vec<ServerConfig> {
        match &self.servers {
            Some(servers) => servers.clone(),
            None => Vec::new(),
        }
    }
}

pub fn create(
    config_file: &path::PathBuf,
    service_name: &str,
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

    let config = ServiceConfig::new(service_name, url);

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
