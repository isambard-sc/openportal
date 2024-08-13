// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::crypto::{CryptoError, Key, SecretKey};
use anyhow::Context;
use anyhow::Error as AnyError;
use iptools::iprange::IpRange;
use serde::{Deserialize, Serialize};
use std::fmt::{self, Display};
use std::net::IpAddr;
use std::path;
use thiserror::Error;
use url::{ParseError as UrlParseError, Url};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ServerConfig {
    pub name: String,
    pub url: String,
    pub inner_key: SecretKey,
    pub outer_key: SecretKey,
}

impl Display for ServerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ServerConfig {{ name: {}, url: {} }}",
            self.name, self.url
        )
    }
}

fn create_websocket_url(url: &str) -> Result<String, ConfigError> {
    let url = url
        .parse::<Url>()
        .with_context(|| format!("Could not parse URL: {}", url))?;

    let scheme = match url.scheme() {
        "ws" => "ws",
        "wss" => "wss",
        "http" => "ws",
        "https" => "wss",
        _ => "wss",
    };

    let host = url.host_str().unwrap_or("localhost");
    let port = url.port().unwrap_or(8080);
    let path = url.path();

    Ok(format!("{}://{}:{}{}", scheme, host, port, path))
}

impl ServerConfig {
    pub fn new(name: &str, url: &str) -> Self {
        ServerConfig {
            name: name.to_string(),
            url: create_websocket_url(url).unwrap_or_else(|e| {
                tracing::warn!("Could not create websocket URL {}: {:?}", url, e);
                "".to_string()
            }),
            inner_key: Key::generate(),
            outer_key: Key::generate(),
        }
    }

    pub fn create_null() -> Self {
        ServerConfig {
            name: "".to_string(),
            url: "".to_string(),
            inner_key: Key::null(),
            outer_key: Key::null(),
        }
    }

    pub fn is_null(&self) -> bool {
        self.name.is_empty()
    }

    pub fn to_peer(&self) -> PeerConfig {
        PeerConfig::from_server(self)
    }

    pub fn get_websocket_url(&self) -> Result<String, ConfigError> {
        if self.url.is_empty() {
            tracing::warn!("No URL provided.");
            return Err(ConfigError::Null("No URL provided.".to_string()));
        }

        Ok(self.url.clone())
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
    pub fn new(ip: &str) -> Result<Self, ConfigError> {
        match ip.parse() {
            Ok(ip) => Ok(IpOrRange::IP(ip)),
            Err(_) => Ok(IpOrRange::Range(ip.to_string())),
        }
    }

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
    pub name: Option<String>,
    pub ip: Option<IpOrRange>,
    pub inner_key: SecretKey,
    pub outer_key: SecretKey,
}

impl Display for ClientConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ip = match &self.ip {
            Some(ip) => format!("{}", ip),
            None => "None".to_string(),
        };

        match &self.name {
            Some(name) => write!(f, "ClientConfig {{ name: {}, ip: {} }}", name, ip),
            None => write!(f, "ClientConfig {{ name: null, ip: {} }}", ip),
        }
    }
}

impl ClientConfig {
    pub fn new(name: &str, ip: &IpOrRange) -> Self {
        ClientConfig {
            name: Some(name.to_string()),
            ip: Some(ip.clone()),
            inner_key: Key::generate(),
            outer_key: Key::generate(),
        }
    }

    pub fn create_null() -> Self {
        ClientConfig {
            name: None,
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

    pub fn to_peer(&self) -> PeerConfig {
        PeerConfig::from_client(self)
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

    pub fn name(&self) -> Option<String> {
        match self {
            PeerConfig::Server(server) => Some(server.name.clone()),
            PeerConfig::Client(client) => client.name.clone(),
            PeerConfig::None => None,
        }
    }
}

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

impl Invite {
    pub fn save(&self) -> Result<String, ConfigError> {
        // serialise to toml
        let invite_toml =
            toml::to_string(&self).with_context(|| "Could not serialise invite to toml")?;

        // write this to a file in the current directory named "invite_<name>.toml"
        let filename = format!("invite_{}.toml", self.name);

        std::fs::write(&filename, invite_toml)
            .with_context(|| format!("Could not write invite file: {:?}", filename))?;

        Ok(filename)
    }

    pub fn load(filename: &str) -> Result<Self, ConfigError> {
        // read the invite file
        let invite = std::fs::read_to_string(filename)
            .with_context(|| format!("Could not read invite file: {:?}", filename))?;

        // parse the invite file
        let invite: Invite = toml::from_str(&invite)
            .with_context(|| format!("Could not parse invite file from toml: {:?}", filename))?;

        Ok(invite)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ServiceConfig {
    pub name: Option<String>,
    pub url: Option<String>,
    pub ip: Option<IpAddr>,
    pub port: Option<u16>,

    servers: Option<Vec<ServerConfig>>,
    clients: Option<Vec<ClientConfig>>,
    encryption: Option<String>,
}

impl Display for ServiceConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let url = match &self.url {
            Some(url) => url.clone(),
            None => "None".to_string(),
        };

        match &self.name {
            Some(name) => write!(f, "ServiceConfig {{ name: {}, url: {} }}", name, url),
            None => write!(f, "ServiceConfig {{ name: null, url: {} }}", url),
        }
    }
}

impl ServiceConfig {
    pub fn new(name: &str, url: &str, ip: &IpAddr, port: u16) -> Result<Self, ConfigError> {
        Ok(ServiceConfig {
            name: Some(name.to_string()),
            url: Some(create_websocket_url(url)?),
            ip: Some(*ip),
            port: Some(port),
            servers: None,
            clients: None,
            encryption: None,
        })
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

    pub fn get_ip(&self) -> IpAddr {
        match &self.ip {
            Some(ip) => *ip,
            None => IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
        }
    }

    pub fn get_port(&self) -> u16 {
        match &self.port {
            Some(port) => *port,
            None => 8080,
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

    pub fn add_client(&mut self, name: &String, ip: &String) -> Result<Invite, ConfigError> {
        let ip = IpOrRange::new(ip)
            .with_context(|| format!("Could not parse into an IP address or IP range: {}", ip))?;

        if name.is_empty() {
            return Err(ConfigError::Peer("No client name provided.".to_string()));
        }

        let self_name = match &self.name {
            Some(name) => name.clone(),
            None => {
                return Err(ConfigError::Null(
                    "Cannot add a client to a null server (no name).".to_string(),
                ))
            }
        };

        let self_url = match &self.url {
            Some(url) => url.clone(),
            None => {
                return Err(ConfigError::Null(
                    "Cannot add a client to a null server (no URL).".to_string(),
                ))
            }
        };

        match &mut self.clients {
            Some(clients) => {
                // check if we already have a client with this name
                for c in clients.iter() {
                    if c.name == Some(name.clone()) {
                        return Err(ConfigError::Peer(format!(
                            "Client with name '{}' already exists.",
                            name
                        )));
                    }
                }
            }
            None => {}
        };

        let client = ClientConfig::new(name, &ip);

        match &mut self.clients {
            Some(clients) => clients.push(client.clone()),
            None => {
                self.clients = Some(vec![client.clone()]);
            }
        };

        Ok(Invite {
            name: self_name,
            url: self_url,
            inner_key: client.inner_key.clone(),
            outer_key: client.outer_key.clone(),
        })
    }

    pub fn remove_client(&mut self, name: &str) -> Result<(), ConfigError> {
        match &mut self.clients {
            Some(clients) => {
                let mut new_clients = Vec::new();

                for client in clients.iter() {
                    if client.name != Some(name.to_string()) {
                        new_clients.push(client.clone());
                    }
                }

                self.clients = Some(new_clients);
            }
            None => {}
        };

        Ok(())
    }

    pub fn add_server(&mut self, invite: Invite) -> Result<(), ConfigError> {
        // make sure there is no server with this name
        match &self.servers {
            Some(servers) => {
                for server in servers.iter() {
                    if server.name == invite.name {
                        return Err(ConfigError::Peer(format!(
                            "Server with name '{}' already exists.",
                            invite.name
                        )));
                    }
                }
            }
            None => {}
        };

        let server = ServerConfig {
            name: invite.name,
            url: create_websocket_url(&invite.url).unwrap_or_else(|e| {
                tracing::warn!("Could not create websocket URL {:?}: {:?}", invite.url, e);
                "".to_string()
            }),
            inner_key: invite.inner_key,
            outer_key: invite.outer_key,
        };

        if server.url.is_empty() {
            tracing::warn!("No valid URL provided for server {}.", server.name);
            return Err(ConfigError::Null("No URL provided.".to_string()));
        }

        match &mut self.servers {
            Some(servers) => servers.push(server.clone()),
            None => {
                self.servers = Some(vec![server.clone()]);
            }
        };

        Ok(())
    }

    pub fn remove_server(&mut self, name: &str) -> Result<(), ConfigError> {
        match &mut self.servers {
            Some(servers) => {
                let mut new_servers = Vec::new();

                for server in servers.iter() {
                    if server.name != name {
                        new_servers.push(server.clone());
                    }
                }

                self.servers = Some(new_servers);
            }
            None => {}
        };

        Ok(())
    }

    pub fn create(
        config_file: &path::PathBuf,
        service_name: &str,
        url: &str,
        ip: &IpAddr,
        port: u16,
    ) -> Result<ServiceConfig, ConfigError> {
        // see if this config_dir exists - return an error if it does
        let config_file = path::absolute(config_file).with_context(|| {
            format!(
                "Could not get absolute path for config file: {:?}",
                config_file
            )
        })?;

        if config_file.try_exists()? {
            return Err(ConfigError::Exists(config_file));
        }

        let config = ServiceConfig::new(service_name, url, ip, port)?;
        config.save(&config_file)?;

        // check we can read the config and return it
        let config = ServiceConfig::load(&config_file)?;

        Ok(config)
    }

    pub fn load(config_file: &path::PathBuf) -> Result<ServiceConfig, ConfigError> {
        // see if this config_dilw exists - return an error if it doesn't
        let config_file = path::absolute(config_file)?;

        if !config_file.try_exists()? {
            return Err(ConfigError::NotExists(config_file));
        }

        // read the config file
        let config = std::fs::read_to_string(&config_file)
            .with_context(|| format!("Could not read config file: {:?}", config_file))?;

        // parse the config file
        let config: ServiceConfig = toml::from_str(&config)
            .with_context(|| format!("Could not parse config file fron toml: {:?}", config_file))?;

        Ok(config)
    }

    pub fn save(self, config_file: &path::PathBuf) -> Result<(), ConfigError> {
        // write the config to a json file
        // write the config to a toml file
        let config_toml =
            toml::to_string(&self).with_context(|| "Could not serialise config to toml")?;

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

        std::fs::write(config_file, config_toml)
            .with_context(|| format!("Could not write config file: {:?}", config_file_string))?;

        Ok(())
    }
}

/// Errors

#[derive(Error, Debug)]
pub enum ConfigError {
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
