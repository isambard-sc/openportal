// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::crypto::{Key, SecretKey};
use crate::error::Error;
use crate::invite::Invite;

use anyhow::Context;
use iptools::iprange::IpRange;
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use std::fmt::Display;
use std::net::IpAddr;
use std::path;
use url::Url;

fn default_zone() -> String {
    "default".to_string()
}

pub fn load<T: serde::de::DeserializeOwned + serde::Serialize>(
    config_file: &path::PathBuf,
) -> Result<T, Error> {
    // see if this config_file exists - return an error if it doesn't
    let config_file = path::absolute(config_file)?;

    if !config_file.try_exists()? {
        return Err(Error::NotExists(config_file.to_string_lossy().to_string()));
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
    config: &T,
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
    name: String,
    config_file: std::path::PathBuf,
    url: String,
    ip: String,
    port: u16,
    healthcheck_port: Option<u16>,
    proxy_header: Option<String>,
}

impl Defaults {
    pub fn parse(
        name: Option<String>,
        config_file: Option<std::path::PathBuf>,
        url: Option<String>,
        ip: Option<String>,
        port: Option<u16>,
        healthcheck_port: Option<u16>,
        proxy_header: Option<String>,
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
            healthcheck_port,
            proxy_header,
        }
    }

    pub fn name(&self) -> String {
        self.name.clone()
    }

    pub fn config_file(&self) -> std::path::PathBuf {
        self.config_file.clone()
    }

    pub fn url(&self) -> String {
        self.url.clone()
    }

    pub fn ip(&self) -> String {
        self.ip.clone()
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn healthcheck_port(&self) -> Option<u16> {
        self.healthcheck_port
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ServerConfig {
    name: String,
    url: String,
    #[serde(default = "default_zone")]
    zone: String,
    inner_key: SecretKey,
    outer_key: SecretKey,
}

impl Display for ServerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ServerConfig {{ name: {}, url: {}, zone: {} }}",
            self.name, self.url, self.zone
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
    let port = url.port().unwrap_or(match scheme {
        "ws" => 80,
        "wss" => 443,
        _ => 443,
    });
    let path = url.path();

    // don't specify the port if it's the default for the protocol
    match scheme {
        "ws" => {
            if port == 80 {
                return Ok(format!("{}://{}", scheme, host));
            }
        }
        "wss" => {
            if port == 443 {
                return Ok(format!("{}://{}", scheme, host));
            }
        }
        _ => {}
    }

    Ok(format!("{}://{}:{}{}", scheme, host, port, path))
}

impl ServerConfig {
    pub fn new(name: &str, url: &str, zone: &str) -> Self {
        ServerConfig {
            name: name.to_string(),
            url: create_websocket_url(url).unwrap_or_else(|e| {
                tracing::warn!("Could not create websocket URL {}: {:?}", url, e);
                "".to_string()
            }),
            zone: zone.to_string(),
            inner_key: Key::generate(),
            outer_key: Key::generate(),
        }
    }

    pub fn from_invite(invite: &Invite) -> Result<Self, Error> {
        Ok(ServerConfig {
            name: invite.name(),
            url: create_websocket_url(&invite.url())?,
            zone: invite.zone(),
            inner_key: invite.inner_key(),
            outer_key: invite.outer_key(),
        })
    }

    pub fn create_null() -> Self {
        ServerConfig {
            name: "".to_string(),
            url: "".to_string(),
            zone: "".to_string(),
            inner_key: Key::null(),
            outer_key: Key::null(),
        }
    }

    pub fn is_null(&self) -> bool {
        self.name.is_empty()
    }

    pub fn is_valid(&self) -> bool {
        !self.is_null()
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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum IpOrRange {
    IP(IpAddr),
    Range(String),
}

impl Display for IpOrRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
            Err(_) => match IpRange::<iptools::iprange::IPv4>::new(ip, "") {
                Ok(_) => Ok(IpOrRange::Range(ip.to_string())),
                Err(err) => Err(Error::Parse(format!(
                    "Could not parse IP address or range: {}, error {}",
                    ip, err
                ))),
            },
        }
    }

    pub fn matches(&self, addr: &IpAddr) -> bool {
        match self {
            IpOrRange::IP(ip) => ip == addr,
            IpOrRange::Range(range) => match IpRange::<iptools::iprange::IPv4>::new(range, "") {
                Ok(range) => range.contains(&addr.to_string()).unwrap_or(false),
                Err(_) => {
                    tracing::warn!("Could not parse IP range: {}", range);
                    false
                }
            },
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ClientConfig {
    name: String,
    ip: IpOrRange,
    #[serde(default = "default_zone")]
    zone: String,
    inner_key: SecretKey,
    outer_key: SecretKey,
}

impl Display for ClientConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ClientConfig {{ name: {}, ip: {}, zone: {} }}",
            self.name, self.ip, self.zone
        )
    }
}

impl ClientConfig {
    pub fn new(name: &str, ip: &IpOrRange, zone: &str) -> Self {
        ClientConfig {
            name: name.to_string(),
            ip: ip.clone(),
            zone: zone.to_string(),
            inner_key: Key::generate(),
            outer_key: Key::generate(),
        }
    }

    pub fn create_null() -> Self {
        ClientConfig {
            name: "".to_string(),
            #[allow(clippy::unwrap_used)]
            ip: IpOrRange::IP("127.0.0.1".parse().unwrap()),
            zone: "".to_string(),
            inner_key: Key::null(),
            outer_key: Key::null(),
        }
    }

    pub fn is_null(&self) -> bool {
        self.name.is_empty()
    }

    pub fn is_valid(&self) -> bool {
        !self.is_null()
    }

    pub fn matches(&self, addr: IpAddr) -> bool {
        self.ip.matches(&addr)
    }

    pub fn to_peer(&self) -> PeerConfig {
        PeerConfig::from_client(self)
    }

    pub fn name(&self) -> String {
        self.name.clone()
    }

    pub fn ip(&self) -> IpOrRange {
        self.ip.clone()
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
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum PeerConfig {
    Server(ServerConfig),
    Client(ClientConfig),
    None,
}

impl Display for PeerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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

    pub fn is_valid(&self) -> bool {
        !self.is_null()
    }

    pub fn is_client(&self) -> bool {
        matches!(self, PeerConfig::Client(_))
    }

    pub fn is_server(&self) -> bool {
        matches!(self, PeerConfig::Server(_))
    }

    pub fn is_none(&self) -> bool {
        matches!(self, PeerConfig::None)
    }

    pub fn name(&self) -> String {
        match self {
            PeerConfig::Server(server) => server.name.clone(),
            PeerConfig::Client(client) => client.name.clone(),
            PeerConfig::None => "".to_string(),
        }
    }

    pub fn zone(&self) -> String {
        match self {
            PeerConfig::Server(server) => server.zone.clone(),
            PeerConfig::Client(client) => client.zone.clone(),
            PeerConfig::None => "".to_string(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum EncryptionScheme {
    Environment { key: String },
    Simple {},
    /*Vault {
        url: String,
    }*/
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ServiceConfig {
    name: String,
    url: String,
    ip: IpAddr,
    port: u16,
    heathcheck_port: Option<u16>,
    proxy_header: Option<String>,

    servers: Vec<ServerConfig>,
    clients: Vec<ClientConfig>,
    encryption: Option<EncryptionScheme>,
}

impl ServiceConfig {
    pub fn new(
        name: &str,
        url: &str,
        ip: &str,
        port: &u16,
        healthcheck_port: &Option<u16>,
        proxy_header: &Option<String>,
    ) -> Result<Self, Error> {
        let name = name.trim();

        if name.is_empty() {
            return Err(Error::Parse("No service name provided.".to_string()));
        }

        // check that the name is [a-zA-Z0-9_-]
        if !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            return Err(Error::Parse(format!(
                "Service name '{}' contains invalid characters. It must be alphanumeric or - _",
                name
            )));
        }

        Ok(ServiceConfig {
            name: name.to_string(),
            url: create_websocket_url(url)?,
            ip: ip
                .parse()
                .with_context(|| format!("Could not parse IP address: {}", ip))?,
            port: *port,
            heathcheck_port: *healthcheck_port,
            proxy_header: proxy_header.clone(),
            servers: Vec::new(),
            clients: Vec::new(),
            encryption: None,
        })
    }

    fn get_key(&self) -> Result<SecretKey, Error> {
        match self.encryption.clone() {
            Some(EncryptionScheme::Environment { key }) => {
                let key = std::env::var(&key)
                    .with_context(|| format!("Could not get environment variable: {}", key))?;

                Ok(Key::from_password(&key).with_context(|| {
                    format!("Could not parse key from environment variable: {}", key)
                })?)
            }
            Some(EncryptionScheme::Simple {}) => Ok(Key::from_password(&self.name)?),
            None => Err(Error::Null(
                "No encryption in use. Please choose a scheme from the options provided."
                    .to_string(),
            )),
        }
    }

    pub fn set_environment_encryption(&mut self, key: &str) -> Result<(), Error> {
        self.encryption = Some(EncryptionScheme::Environment {
            key: key.to_string(),
        });
        Ok(())
    }

    pub fn set_simple_encryption(&mut self) -> Result<(), Error> {
        self.encryption = Some(EncryptionScheme::Simple {});
        Ok(())
    }

    pub fn encrypt<T>(&self, data: &T) -> Result<String, Error>
    where
        T: Serialize,
    {
        self.get_key()?.expose_secret().encrypt(data)
    }

    pub fn decrypt<T>(&self, data: &str) -> Result<T, Error>
    where
        T: for<'de> Deserialize<'de>,
    {
        self.get_key()?.expose_secret().decrypt::<T>(data)
    }

    pub fn clients(&self) -> Vec<ClientConfig> {
        self.clients.clone()
    }

    pub fn servers(&self) -> Vec<ServerConfig> {
        self.servers.clone()
    }

    pub fn ip(&self) -> IpAddr {
        self.ip
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn healthcheck_port(&self) -> Option<u16> {
        self.heathcheck_port
    }

    pub fn proxy_header(&self) -> Option<String> {
        self.proxy_header.clone()
    }

    pub fn add_client(
        &mut self,
        name: &str,
        ip: &str,
        zone: &Option<String>,
    ) -> Result<Invite, Error> {
        let ip = IpOrRange::new(ip)
            .with_context(|| format!("Could not parse into an IP address or IP range: {}", ip))?;

        if name.is_empty() {
            return Err(Error::Peer("No client name provided.".to_string()));
        }

        let zone = zone.clone().unwrap_or_else(default_zone);
        let zone = zone.trim();

        if zone.is_empty() {
            return Err(Error::Peer("No zone provided.".to_string()));
        }

        // make sure that zone is [a-zA-Z0-9_]
        if !zone
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            return Err(Error::Peer(format!(
                "Zone '{}' contains invalid characters. It must be alphanumeric or - _",
                zone
            )));
        }

        // check if we already have a client with this name in this zone
        for c in self.clients.iter() {
            if c.name == name && c.zone == zone {
                return Err(Error::Peer(format!(
                    "Client with name '{}' already exists in zone {}.",
                    name, zone
                )));
            }
        }

        let client = ClientConfig::new(name, &ip, zone);

        self.clients.push(client.clone());

        Ok(Invite::new(
            &self.name,
            &self.url,
            zone,
            &client.inner_key,
            &client.outer_key,
        ))
    }

    pub fn remove_client(&mut self, name: &str, zone: &Option<String>) -> Result<(), Error> {
        let zone = zone.clone().unwrap_or_else(default_zone);
        let zone = zone.trim();

        if zone.is_empty() {
            return Err(Error::Peer("No zone provided.".to_string()));
        }

        // make sure that zone is [a-zA-Z0-9_-]
        if !zone
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            return Err(Error::Peer(format!(
                "Zone '{}' contains invalid characters. It must be alphanumeric or - _",
                zone
            )));
        }

        self.clients = self
            .clients
            .iter()
            .filter(|client| client.name != name || client.zone != zone)
            .cloned()
            .collect();

        Ok(())
    }

    pub fn add_server(&mut self, invite: Invite) -> Result<(), Error> {
        for server in self.servers.iter() {
            if server.name == invite.name() && server.zone == invite.zone() {
                return Err(Error::Peer(format!(
                    "Server with name '{}' already exists in zone {}.",
                    invite.name(),
                    invite.zone()
                )));
            }
        }

        let server = ServerConfig::from_invite(&invite)?;

        if server.url.is_empty() {
            tracing::warn!("No valid URL provided for server {}.", server.name());
            return Err(Error::Null("No URL provided.".to_string()));
        }

        self.servers.push(server.clone());

        Ok(())
    }

    pub fn remove_server(&mut self, name: &str, zone: &Option<String>) -> Result<(), Error> {
        let zone = zone.clone().unwrap_or_else(default_zone);
        let zone = zone.trim();

        if zone.is_empty() {
            return Err(Error::Peer("No zone provided.".to_string()));
        }

        // make sure that zone is [a-zA-Z0-9_]
        if !zone
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            return Err(Error::Peer(format!(
                "Zone '{}' contains invalid characters. It must be alphanumeric or - _.",
                zone
            )));
        }

        self.servers = self
            .servers
            .iter()
            .filter(|server| server.name != name || server.zone != zone)
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
        healthcheck_port: &Option<u16>,
        proxy_header: &Option<String>,
    ) -> Result<ServiceConfig, Error> {
        // see if this config_dir exists - return an error if it does
        let config_file = path::absolute(config_file).with_context(|| {
            format!(
                "Could not get absolute path for config file: {:?}",
                config_file
            )
        })?;

        if config_file.try_exists()? {
            return Err(Error::NotExists(config_file.to_string_lossy().to_string()));
        }

        let config = ServiceConfig::new(
            &name,
            &url,
            &ip.to_string(),
            &port,
            healthcheck_port,
            proxy_header,
        )?;
        save::<ServiceConfig>(&config, &config_file)?;

        // check we can read the config and return it
        let config = load::<ServiceConfig>(&config_file)?;

        Ok(config)
    }

    pub fn name(&self) -> String {
        self.name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ip_or_range() {
        let mut ip = IpOrRange::new("127.0.0.1").unwrap_or_else(|e| {
            unreachable!("Could not create IP address: {:?}", e);
        });

        assert_eq!(format!("{}", ip), "127.0.0.1");

        assert!(ip.matches(&IpAddr::from([127, 0, 0, 1])));
        assert!(!ip.matches(&IpAddr::from([127, 0, 0, 2])));
        assert!(!ip.matches(&IpAddr::from([129, 0, 0, 1])));

        assert!(IpOrRange::new("127.*.*.*").is_err());

        ip = IpOrRange::new("127.0.0.0/24").unwrap_or_else(|e| {
            unreachable!("Could not create IP range: {:?}", e);
        });

        assert_eq!(format!("{}", ip), "127.0.0.0/24");

        assert!(ip.matches(&IpAddr::from([127, 0, 0, 1])));
        assert!(ip.matches(&IpAddr::from([127, 0, 0, 2])));
        assert!(!ip.matches(&IpAddr::from([129, 0, 0, 1])));
    }

    #[test]
    fn test_client_config() {
        let ip = IpOrRange::new("127.0.0.1").unwrap_or_else(|e| {
            unreachable!("Could not create IP address: {:?}", e);
        });

        let client = ClientConfig::new("test", &ip, &default_zone());

        assert_eq!(client.name, "test".to_string());
        assert_eq!(client.ip, ip);

        let peer = PeerConfig::from_client(&client);

        assert!(peer.is_client());
        assert!(!peer.is_server());
        assert!(!peer.is_null());
    }

    #[test]
    fn test_invitations() {
        let mut primary = ServiceConfig::new(
            "primary",
            "http://localhost",
            "127.0.0.1",
            &5544,
            &None,
            &None,
        )
        .unwrap_or_else(|e| {
            unreachable!("Cannot create service config: {}", e);
        });

        let mut secondary = ServiceConfig::new(
            "secondary",
            "http://localhost",
            "127.0.0.1",
            &5545,
            &None,
            &None,
        )
        .unwrap_or_else(|e| {
            unreachable!("Cannot create service config: {}", e);
        });

        // introduce the secondary to the primary
        let invite = primary
            .add_client(&secondary.name(), "127.0.0.1", &None)
            .unwrap_or_else(|e| {
                unreachable!("Cannot add secondary to primary: {}", e);
            });

        // give the invitation to the secondary
        secondary.add_server(invite).unwrap_or_else(|e| {
            unreachable!("Cannot add primary to secondary: {}", e);
        });

        assert_eq!(primary.clients().len(), 1);
        assert_eq!(secondary.servers().len(), 1);

        assert_eq!(primary.clients()[0].name(), "secondary".to_string());
        assert_eq!(secondary.servers()[0].name(), "primary".to_string());
    }
}
