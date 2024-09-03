// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::crypto::{Error as CryptoError, Key, SecretKey};
use crate::invite::Invite;
use anyhow::Context;
use anyhow::Error as AnyError;
use iptools::iprange::IpRange;
use serde::{Deserialize, Serialize};
use std::fmt::{self, Display};
use std::net::IpAddr;
use std::path;
use thiserror::Error;
use url::{ParseError as UrlParseError, Url};

pub fn load<T: serde::de::DeserializeOwned + serde::Serialize>(
    config_file: &path::PathBuf,
) -> Result<T, Error> {
    // see if this config_file exists - return an error if it doesn't
    let config_file = path::absolute(config_file)?;

    if !config_file.try_exists()? {
        return Err(Error::NotExists(config_file));
    }

    // read the config file
    let config = std::fs::read_to_string(&config_file)
        .with_context(|| format!("Could not read config file: {:?}", config_file))?;

    // parse the config file
    let config: T = toml::from_str(&config)
        .with_context(|| format!("Could not parse config file fron toml: {:?}", config_file))?;

    Ok(config)
}

pub fn save<T: serde::de::DeserializeOwned + serde::Serialize>(
    config: T,
    config_file: &path::PathBuf,
) -> Result<(), Error> {
    // write the config to a json file
    // write the config to a toml file
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

    std::fs::write(config_file, config_toml)
        .with_context(|| format!("Could not write config file: {:?}", config_file_string))?;

    Ok(())
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Defaults {
    pub name: String,
    pub config_file: std::path::PathBuf,
    pub url: String,
    pub ip: String,
    pub port: u16,
}

impl Defaults {
    pub fn parse(
        name: Option<String>,
        config_file: Option<std::path::PathBuf>,
        url: Option<String>,
        ip: Option<String>,
        port: Option<u16>,
    ) -> Self {
        let config_file = config_file.unwrap_or(
            dirs::config_local_dir()
                .unwrap_or(
                    ".".parse()
                        .expect("Could not parse fallback config directory."),
                )
                .join("openportal")
                .join("service.toml"),
        );

        Self {
            name: name.unwrap_or("default_service".to_owned()),
            config_file,
            url: url.unwrap_or("http://localhost:8000".to_owned()),
            ip: ip.unwrap_or("127.0.0.1".to_owned()),
            port: port.unwrap_or(8042),
        }
    }
}

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

fn create_websocket_url(url: &str) -> Result<String, Error> {
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
    pub fn new(name: String, url: String) -> Self {
        ServerConfig {
            name: name.to_string(),
            url: create_websocket_url(&url).unwrap_or_else(|e| {
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

    pub fn get_websocket_url(&self) -> Result<String, Error> {
        if self.url.is_empty() {
            tracing::warn!("No URL provided.");
            return Err(Error::Null("No URL provided.".to_string()));
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
    pub fn new(ip: &str) -> Result<Self, Error> {
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
pub struct ServiceConfig {
    pub name: String,
    pub url: String,
    pub ip: IpAddr,
    pub port: u16,

    pub servers: Vec<ServerConfig>,
    pub clients: Vec<ClientConfig>,
    pub encryption: Option<String>,
}

impl ServiceConfig {
    pub fn parse(name: String, url: String, ip: IpAddr, port: u16) -> Result<Self, Error> {
        Ok(ServiceConfig {
            name: name.to_string(),
            url: create_websocket_url(&url)?,
            ip,
            port,
            servers: Vec::new(),
            clients: Vec::new(),
            encryption: None,
        })
    }

    pub fn add_client(&mut self, name: &String, ip: &String) -> Result<Invite, Error> {
        let ip = IpOrRange::new(ip)
            .with_context(|| format!("Could not parse into an IP address or IP range: {}", ip))?;

        if name.is_empty() {
            return Err(Error::Peer("No client name provided.".to_string()));
        }

        // check if we already have a client with this name
        for c in self.clients.iter() {
            if c.name == Some(name.clone()) {
                return Err(Error::Peer(format!(
                    "Client with name '{}' already exists.",
                    name
                )));
            }
        }

        let client = ClientConfig::new(name, &ip);

        self.clients.push(client.clone());

        Ok(Invite {
            name: self.name.clone(),
            url: self.url.clone(),
            inner_key: client.inner_key.clone(),
            outer_key: client.outer_key.clone(),
        })
    }

    pub fn remove_client(&mut self, name: &str) -> Result<(), Error> {
        self.clients = self
            .clients
            .iter()
            .filter(|client| client.name != Some(name.to_string()))
            .cloned()
            .collect();

        Ok(())
    }

    pub fn add_server(&mut self, invite: Invite) -> Result<(), Error> {
        for server in self.servers.iter() {
            if server.name == invite.name {
                return Err(Error::Peer(format!(
                    "Server with name '{}' already exists.",
                    invite.name
                )));
            }
        }

        let server = ServerConfig {
            name: invite.name.clone(),
            url: create_websocket_url(&invite.url)?,
            inner_key: invite.inner_key,
            outer_key: invite.outer_key,
        };

        if server.url.is_empty() {
            tracing::warn!("No valid URL provided for server {}.", server.name);
            return Err(Error::Null("No URL provided.".to_string()));
        }

        self.servers.push(server.clone());

        Ok(())
    }

    pub fn remove_server(&mut self, name: &str) -> Result<(), Error> {
        self.servers = self
            .servers
            .iter()
            .filter(|server| server.name != name)
            .cloned()
            .collect();

        Ok(())
    }

    pub fn create(
        config_file: &path::PathBuf,
        name: String,
        url: String,
        ip: IpAddr,
        port: u16,
    ) -> Result<ServiceConfig, Error> {
        // see if this config_dir exists - return an error if it does
        let config_file = path::absolute(config_file).with_context(|| {
            format!(
                "Could not get absolute path for config file: {:?}",
                config_file
            )
        })?;

        if config_file.try_exists()? {
            return Err(Error::Exists(config_file));
        }

        let config = ServiceConfig::parse(name, url, ip, port)?;
        save::<ServiceConfig>(config.clone(), &config_file)?;

        // check we can read the config and return it
        let config = load::<ServiceConfig>(&config_file)?;

        Ok(config)
    }
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
