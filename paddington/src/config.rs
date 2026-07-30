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
    config_file: &path::Path,
) -> Result<(), Error> {
    // write the config to a toml file
    let config_toml =
        toml::to_string(&config).with_context(|| "Could not serialise config to toml")?;

    let config_file_string = config_file.to_string_lossy();

    // `write_secret_file` creates the parent directory itself (owner-only),
    // so there is no separate `create_dir_all` here - doing it separately
    // created the directory at the process umask instead.
    write_secret_file(config_file, &config_toml)
        .with_context(|| format!("Could not write config file: {:?}", config_file_string))?;

    Ok(())
}

/// Write `contents` to `path` as an owner-only (mode 0600) file, creating the
/// parent directory owner-only (mode 0700) if it does not exist.
///
/// **This is the only function that should ever write a file containing key
/// material.** The service config, the paddington invite files, and the bridge
/// invite all contain plaintext keys; written with a plain `std::fs::write`
/// they land at the process umask, which is commonly group/world-readable
/// (0644), leaving long-term keys readable by any local user. See
/// docs/specifications/security-review.md (finding F9) and
/// docs/specifications/security-review-2.md (finding R9 - the bridge invite,
/// which is in `templemeads` and so could not previously call this).
///
/// The mode is set **at creation** rather than with a `set_permissions` call
/// afterwards: the latter leaves a window in which the secret is already on
/// disk at the umask, and does not lower the mode of a pre-existing file at
/// all (`std::fs::write` preserves it).
pub fn write_secret_file(path: &path::Path, contents: &str) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Could not create parent directory for file: {:?}", path)
            })?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
                    .with_context(|| {
                        format!("Could not restrict permissions on directory: {:?}", parent)
                    })?;
            }
        }
    }

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("Could not open file for writing: {:?}", path))?;

        file.write_all(contents.as_bytes())
            .with_context(|| format!("Could not write file: {:?}", path))?;

        // A pre-existing file keeps its own mode when reopened, so lower it
        // explicitly too - this is the case `mode()` above does not cover.
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("Could not restrict permissions on file: {:?}", path))?;
    }

    #[cfg(not(unix))]
    std::fs::write(path, contents).with_context(|| format!("Could not write file: {:?}", path))?;

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
    /// Name of a `servers` entry (on this same `ServiceConfig`) to reach
    /// this peer via, instead of dialling `url` directly - set when this
    /// peer can only be reached through a blind relay proxy (see
    /// `docs/plans/archive/blind-relay-proxy-design.md`). `url` is ignored when
    /// this is set.
    #[serde(default)]
    proxy: Option<String>,
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
        "ws" if port == 80 => {
            return Ok(format!("{}://{}", scheme, host));
        }
        "wss" if port == 443 => {
            return Ok(format!("{}://{}", scheme, host));
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
            proxy: None,
            zone: zone.to_string(),
            inner_key: Key::generate(),
            outer_key: Key::generate(),
        }
    }

    pub fn from_invite(invite: &Invite) -> Result<Self, Error> {
        Ok(ServerConfig {
            name: invite.name(),
            url: create_websocket_url(&invite.url())?,
            proxy: None,
            zone: invite.zone(),
            inner_key: invite.inner_key(),
            outer_key: invite.outer_key(),
        })
    }

    ///
    /// Create a `ServerConfig` for a peer reached only via a blind relay
    /// proxy (see `docs/plans/archive/blind-relay-proxy-design.md`) - `relay` names
    /// an existing entry in this same service's `servers` list. The
    /// invite's `url` is ignored - this peer is never dialled directly.
    ///
    pub fn from_relayed_invite(relay: &str, invite: &Invite) -> Result<Self, Error> {
        if relay.trim().is_empty() {
            return Err(Error::Peer("No relay name provided.".to_string()));
        }

        Ok(ServerConfig {
            name: invite.name(),
            url: "".to_string(),
            proxy: Some(relay.trim().to_string()),
            zone: invite.zone(),
            inner_key: invite.inner_key(),
            outer_key: invite.outer_key(),
        })
    }

    pub fn create_null() -> Self {
        ServerConfig {
            name: "".to_string(),
            url: "".to_string(),
            proxy: None,
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

    ///
    /// The name of the `servers` entry to reach this peer via, if it can
    /// only be reached through a blind relay proxy rather than directly.
    ///
    pub fn proxy(&self) -> Option<String> {
        self.proxy.clone()
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

    pub fn rotate_keys(&mut self, invite: &Invite) -> Result<(), Error> {
        // verify that the name and zone match the invite
        if self.name != invite.name() || self.zone != invite.zone() {
            return Err(Error::Peer(format!(
                "Server name and zone do not match invite: {} {}",
                self.name, self.zone
            )));
        }

        // copy the keys from the invite
        self.inner_key = invite.inner_key();
        self.outer_key = invite.outer_key();

        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum IpOrRange {
    IP(IpAddr),
    Range(String),
    /// Several addresses and/or ranges, any one of which is allowed - built
    /// from a comma-separated `--ip` value (e.g.
    /// `"127.0.0.1,10.0.0.0/24,2001:db8::/32"`), see `IpOrRange::new`. A
    /// single-entry input never produces this variant (it returns the
    /// plain `IP`/`Range` directly), so existing single-address configs
    /// are unaffected.
    List(Vec<IpOrRange>),
}

/// Whether `range` is CIDR notation for "every IPv4 address" (prefix `/0`,
/// e.g. `"0.0.0.0/0"` - the network bits are irrelevant when the prefix
/// length is zero, so any address before the `/0` means the same thing).
/// Handled entirely without constructing an `iptools::iprange::IpRange`:
/// that type's `IPv4::new` computes `len = end - start + 1` as a `u32`,
/// which overflows (and panics in debug builds) for the full
/// 2^32-address range, since `4294967295 - 0 + 1` doesn't fit in a `u32`.
/// `iptools::ipv4::validate_ip` is safe to call here - it's a plain regex
/// + octet-range check with no arithmetic on the full address span.
fn is_full_ipv4_range(range: &str) -> bool {
    match range.split_once('/') {
        Some((ip, prefix)) => prefix.parse::<u32>() == Ok(0) && iptools::ipv4::validate_ip(ip),
        None => false,
    }
}

/// As `is_full_ipv4_range`, for IPv6: `IpRange::<IPv6>::new`'s `len = end -
/// start + 1` is a `u128`, which overflows identically for the full
/// 2^128-address range (`"::/0"`, i.e. `u128::MAX - 0 + 1`).
fn is_full_ipv6_range(range: &str) -> bool {
    match range.split_once('/') {
        Some((ip, prefix)) => prefix.parse::<u32>() == Ok(0) && iptools::ipv6::validate_ip(ip),
        None => false,
    }
}

/// Whether `range` is IPv6 CIDR notation rather than IPv4. A colon appears in
/// every IPv6 form (including the IPv4-mapped `::ffff:a.b.c.d/96`) and in no
/// IPv4 form, so this is an exact discriminator. Used by `IpOrRange::matches`
/// to refuse to compare a range against an address of the other family - see
/// `docs/specifications/security-review-2.md` (finding R8).
fn range_is_ipv6(range: &str) -> bool {
    range.contains(':')
}

/// Validates `range` as CIDR notation for either address family, trying
/// IPv4 first so behaviour and error messages are unchanged for anything
/// that already parses as an IPv4 range - see
/// `docs/plans/ipv6-support-design.md` §4.2.
fn validate_ip_range(range: &str) -> Result<(), String> {
    if is_full_ipv4_range(range) || is_full_ipv6_range(range) {
        return Ok(());
    }

    let v4_err = match IpRange::<iptools::iprange::IPv4>::new(range, "") {
        Ok(_) => return Ok(()),
        Err(err) => err,
    };

    match IpRange::<iptools::iprange::IPv6>::new(range, "") {
        Ok(_) => Ok(()),
        Err(v6_err) => Err(format!(
            "not a valid IPv4 range ({}) or IPv6 range ({})",
            v4_err, v6_err
        )),
    }
}

impl<'de> Deserialize<'de> for IpOrRange {
    /// Validates a `Range` variant's CIDR syntax at load time, rather than
    /// only discovering it is unparseable later, silently, at connection
    /// time (`matches()` below just logs a warning and treats an
    /// unparseable range as "does not match" - by the time that happens,
    /// there is no config-loading error to surface to the operator, only
    /// a confusing "no matching peer found" in the connection log).
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::de::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            // A plain string - a single IP, a CIDR range, or a
            // comma-separated list of either (e.g. "127.0.0.1" or
            // "127.0.0.1,10.0.0.0/24") - parsed via `IpOrRange::new`, the
            // same convenience parser used by the `--ip`/`set_trusted_proxy`
            // CLI/API path, so config files and code accept identical
            // syntax.
            String(String),
            Tagged(Tagged),
        }

        #[derive(Deserialize)]
        enum Tagged {
            IP(IpAddr),
            Range(String),
            // Each element is deserialised (and so validated) via this
            // same `Deserialize` impl, recursively - no separate
            // validation needed here.
            List(Vec<IpOrRange>),
        }

        match Raw::deserialize(deserializer)? {
            Raw::String(s) => IpOrRange::new(&s).map_err(serde::de::Error::custom),
            Raw::Tagged(Tagged::IP(ip)) => Ok(IpOrRange::IP(ip)),
            Raw::Tagged(Tagged::Range(range)) => {
                validate_ip_range(&range).map_err(|err| {
                    serde::de::Error::custom(format!(
                        "Could not parse IP range: {}, error {}",
                        range, err
                    ))
                })?;
                Ok(IpOrRange::Range(range))
            }
            Raw::Tagged(Tagged::List(list)) => Ok(IpOrRange::List(list)),
        }
    }
}

impl Display for IpOrRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IpOrRange::IP(ip) => write!(f, "{}", ip),
            IpOrRange::Range(range) => write!(f, "{}", range),
            IpOrRange::List(entries) => {
                let joined = entries
                    .iter()
                    .map(|entry| entry.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                write!(f, "{}", joined)
            }
        }
    }
}

impl IpOrRange {
    ///
    /// Parses a single IP address or CIDR range, or a comma-separated
    /// list of several (e.g. `"127.0.0.1,10.0.0.0/24,2001:db8::/32"`),
    /// any one of which is allowed to match. A single entry (no comma)
    /// returns the plain `IP`/`Range` directly rather than a one-element
    /// `List`, so existing single-address configuration is unaffected.
    ///
    pub fn new(ip: &str) -> Result<Self, Error> {
        let entries = ip.split(',').collect::<Vec<_>>();

        if let [single] = entries.as_slice() {
            return Self::new_single(single.trim());
        }

        Ok(IpOrRange::List(
            entries
                .into_iter()
                .map(|entry| Self::new_single(entry.trim()))
                .collect::<Result<Vec<_>, _>>()?,
        ))
    }

    fn new_single(ip: &str) -> Result<Self, Error> {
        match ip.parse() {
            Ok(ip) => Ok(IpOrRange::IP(ip)),
            Err(_) => match validate_ip_range(ip) {
                Ok(()) => Ok(IpOrRange::Range(ip.to_string())),
                Err(err) => Err(Error::Parse(format!(
                    "Could not parse IP address or range: {}, error {}",
                    ip, err
                ))),
            },
        }
    }

    /// Whether `addr` is matched by this address, range, or list.
    ///
    /// An IPv4-mapped IPv6 address (`::ffff:a.b.c.d`) is canonicalised to its
    /// plain IPv4 form first: on a dual-stack listener an IPv4 peer arrives in
    /// the mapped form, and an IPv4 allow-list entry should still match it.
    ///
    /// A range is then only ever matched against an address of its *own*
    /// family. That check is load-bearing, not cosmetic: `iptools`'s IPv4
    /// `IpRange::contains` parses an IPv6 argument into a `u128` and then
    /// compares it as `addr as u32`, i.e. it silently truncates to the low 32
    /// bits. Without the family check, any IPv6 address whose last 32 bits
    /// happen to fall inside an IPv4 CIDR matched that CIDR - so a peer at
    /// `2001:db8::7f00:1` satisfied a `127.0.0.0/8` rule, defeating both the
    /// client IP allow-list and `trusted_proxy`. See
    /// `docs/specifications/security-review-2.md` (finding R8).
    pub fn matches(&self, addr: &IpAddr) -> bool {
        let addr = addr.to_canonical();

        match self {
            IpOrRange::IP(ip) => ip.to_canonical() == addr,
            IpOrRange::List(entries) => entries.iter().any(|entry| entry.matches(&addr)),
            IpOrRange::Range(range) if is_full_ipv4_range(range) => addr.is_ipv4(),
            IpOrRange::Range(range) if is_full_ipv6_range(range) => addr.is_ipv6(),
            IpOrRange::Range(range) => match (range_is_ipv6(range), addr) {
                (false, IpAddr::V4(_)) => match IpRange::<iptools::iprange::IPv4>::new(range, "") {
                    Ok(parsed) => parsed.contains(&addr.to_string()).unwrap_or(false),
                    Err(_) => {
                        tracing::warn!("Could not parse IPv4 range: {}", range);
                        false
                    }
                },
                (true, IpAddr::V6(_)) => match IpRange::<iptools::iprange::IPv6>::new(range, "") {
                    Ok(parsed) => parsed.contains(&addr.to_string()).unwrap_or(false),
                    Err(_) => {
                        tracing::warn!("Could not parse IPv6 range: {}", range);
                        false
                    }
                },
                // Range and address are of different families - never a match.
                _ => false,
            },
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ClientConfig {
    name: String,
    /// `None` for a client reached only via a blind relay proxy - see
    /// `proxy` below. Always `Some` for a directly-connecting client.
    #[serde(default)]
    ip: Option<IpOrRange>,
    /// Name of a `servers` entry (on this same `ServiceConfig`) this
    /// client is expected to arrive via, instead of being IP-allowlisted -
    /// set when this peer can only reach us through a blind relay proxy
    /// (see `docs/plans/archive/blind-relay-proxy-design.md`). Authentication then
    /// comes from completing the relayed handshake, not from `ip`.
    #[serde(default)]
    proxy: Option<String>,
    #[serde(default = "default_zone")]
    zone: String,
    inner_key: SecretKey,
    outer_key: SecretKey,
}

impl Display for ClientConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.proxy {
            Some(proxy) => write!(
                f,
                "ClientConfig {{ name: {}, proxy: {}, zone: {} }}",
                self.name, proxy, self.zone
            ),
            None => write!(
                f,
                "ClientConfig {{ name: {}, ip: {}, zone: {} }}",
                self.name,
                self.ip
                    .as_ref()
                    .map(|ip| ip.to_string())
                    .unwrap_or_default(),
                self.zone
            ),
        }
    }
}

impl ClientConfig {
    pub fn new(name: &str, ip: &IpOrRange, zone: &str) -> Self {
        ClientConfig {
            name: name.to_string(),
            ip: Some(ip.clone()),
            proxy: None,
            zone: zone.to_string(),
            inner_key: Key::generate(),
            outer_key: Key::generate(),
        }
    }

    ///
    /// Create a `ClientConfig` for a peer that can only reach us via a
    /// blind relay proxy (see `docs/plans/archive/blind-relay-proxy-design.md`) -
    /// `relay` names an existing entry in this same service's `servers`
    /// list.
    ///
    pub fn new_relayed(name: &str, relay: &str, zone: &str) -> Result<Self, Error> {
        if relay.trim().is_empty() {
            return Err(Error::Peer("No relay name provided.".to_string()));
        }

        Ok(ClientConfig {
            name: name.to_string(),
            ip: None,
            proxy: Some(relay.trim().to_string()),
            zone: zone.to_string(),
            inner_key: Key::generate(),
            outer_key: Key::generate(),
        })
    }

    pub fn create_null() -> Self {
        ClientConfig {
            name: "".to_string(),
            #[allow(clippy::unwrap_used)]
            ip: Some(IpOrRange::IP("127.0.0.1".parse().unwrap())),
            proxy: None,
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

    ///
    /// Whether this client is reached via a blind relay proxy rather than
    /// a direct, IP-allowlisted connection.
    ///
    pub fn is_relayed(&self) -> bool {
        self.proxy.is_some()
    }

    ///
    /// Returns `false` for a relayed client - it is never matched by IP,
    /// only by successfully completing the relayed handshake.
    ///
    pub fn matches(&self, addr: IpAddr) -> bool {
        self.ip.as_ref().is_some_and(|ip| ip.matches(&addr))
    }

    pub fn to_peer(&self) -> PeerConfig {
        PeerConfig::from_client(self)
    }

    pub fn name(&self) -> String {
        self.name.clone()
    }

    pub fn ip(&self) -> Option<IpOrRange> {
        self.ip.clone()
    }

    ///
    /// The name of the `servers` entry to expect this client to arrive
    /// via, if it can only reach us through a blind relay proxy.
    ///
    pub fn proxy(&self) -> Option<String> {
        self.proxy.clone()
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

    pub fn rotate_keys(&mut self) {
        self.inner_key = Key::generate();
        self.outer_key = Key::generate();
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

/// Prefix marking a versioned (v1) encrypted secret: strong, salted Argon2
/// derivation (`Key::from_password_with_salt`). A stored value lacking this
/// prefix is a legacy (v0) secret - fixed-salt, minimal-Argon2 - which is
/// still decryptable for backward compatibility but never written any more.
/// Re-running the `secret` CLI command re-encrypts a value in the v1 format.
/// See docs/specifications/security-review.md (finding F2).
const SECRET_V1_PREFIX: &str = "op-secret-v1:";

/// Length (bytes) of the per-secret random salt used by v1 encryption.
const SECRET_SALT_SIZE: usize = 16;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum EncryptionScheme {
    /// Derive the at-rest secret-encryption key from the value of a named
    /// environment variable (read at startup). This is the recommended
    /// production scheme - its strength is that of the operator-supplied
    /// secret, combined with the strong salted Argon2 derivation used for
    /// v1 secrets.
    Environment { key: String },
    /// Derive the key from the service's own (non-secret) name.
    ///
    /// **Not for production.** The service name is not secret - it is stored
    /// in this same config file, embedded in every issued invite, and printed
    /// to logs - so anyone who can read the config can re-derive the key. This
    /// scheme is **obfuscation, not encryption**, intended only for
    /// development or low-security deployments. Use `Environment` in
    /// production. See docs/specifications/security-review.md (finding F2).
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

    /// IP address(es)/range(s) of reverse proxies whose `proxy_header` (e.g.
    /// `X-Forwarded-For`) may be trusted to carry the real client address.
    /// A forwarded client address is only honoured when the actual TCP peer
    /// matches one of these entries; otherwise the header is ignored and the
    /// real peer address is used. Without this, `proxy_header` alone would let
    /// any direct connector spoof an allow-listed IP. Accepts the same
    /// comma-separated IP/CIDR syntax as a client's `ip` (see `IpOrRange`) -
    /// e.g. a Cloudflare tunnel daemon on loopback is `"127.0.0.0/8"`. See
    /// docs/specifications/security-review.md (finding F6).
    #[serde(default)]
    trusted_proxy: Option<IpOrRange>,

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
            trusted_proxy: None,
            servers: Vec::new(),
            clients: Vec::new(),
            encryption: None,
        })
    }

    /// Return the password the configured encryption scheme derives its key
    /// from. The returned value is a secret (the environment variable's
    /// contents under `Environment`) and must never be logged or interpolated
    /// into error messages.
    fn get_password(&self) -> Result<String, Error> {
        match self.encryption.clone() {
            Some(EncryptionScheme::Environment { key }) => {
                // `key` is the *name* of the environment variable; the value it
                // holds is the secret. Only ever interpolate the name into
                // error contexts - the value would leak the secret to logs.
                let value = std::env::var(&key)
                    .with_context(|| format!("Could not get environment variable: {}", key))?;
                Ok(value)
            }
            Some(EncryptionScheme::Simple {}) => {
                // The service name is not secret; `Simple` is obfuscation only
                // (see `EncryptionScheme::Simple`).
                Ok(self.name.clone())
            }
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

    /// Encrypt a secret value for storage in the config (`extras`), using the
    /// versioned (v1) format: a fresh random salt, a strong salted Argon2
    /// derivation, then XChaCha20-Poly1305 AEAD. The salt is stored alongside
    /// the ciphertext (`op-secret-v1:<hex salt>:<hex ciphertext>`). See
    /// docs/specifications/security-review.md (finding F2).
    pub fn encrypt<T>(&self, data: &T) -> Result<String, Error>
    where
        T: Serialize,
    {
        let password = self.get_password()?;
        let salt = crate::crypto::random_bytes(SECRET_SALT_SIZE)?;
        let key = Key::from_password_with_salt(&password, &salt)?;
        let ciphertext = key.expose_secret().encrypt(data)?;
        Ok(format!(
            "{}{}:{}",
            SECRET_V1_PREFIX,
            hex::encode(&salt),
            ciphertext
        ))
    }

    /// Decrypt a secret value previously stored by `encrypt`. Values carrying
    /// the `op-secret-v1:` prefix use the salted strong derivation; any other
    /// value is treated as a legacy (v0) secret and decrypted with the old
    /// fixed-salt derivation, so pre-existing config files keep working.
    pub fn decrypt<T>(&self, data: &str) -> Result<T, Error>
    where
        T: for<'de> Deserialize<'de>,
    {
        let password = self.get_password()?;

        if let Some(rest) = data.strip_prefix(SECRET_V1_PREFIX) {
            let (salt_hex, ciphertext) = rest.split_once(':').ok_or_else(|| {
                Error::Parse(
                    "Malformed versioned secret: missing salt/ciphertext separator".to_string(),
                )
            })?;
            let salt = hex::decode(salt_hex).with_context(|| "Could not decode secret salt")?;
            let key = Key::from_password_with_salt(&password, &salt)?;
            key.expose_secret().decrypt::<T>(ciphertext)
        } else {
            // Legacy (v0) secret: fixed-salt, minimal-Argon2 derivation.
            let key = Key::from_password(&password)?;
            key.expose_secret().decrypt::<T>(data)
        }
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

    pub fn trusted_proxy(&self) -> Option<IpOrRange> {
        self.trusted_proxy.clone()
    }

    /// Set (or clear, with `None`) the trusted-proxy IP/range allow-list. The
    /// value uses the same comma-separated IP/CIDR syntax as a client's `ip`.
    pub fn set_trusted_proxy(&mut self, value: Option<&str>) -> Result<(), Error> {
        self.trusted_proxy = match value {
            Some(value) => Some(IpOrRange::new(value)?),
            None => None,
        };
        Ok(())
    }

    /// Whether an inbound TCP connection from `peer_ip` should be allowed to
    /// even *attempt* authentication. True when the address matches any
    /// configured client's allow-listed IP, or the `trusted_proxy` allow-list
    /// (for `proxy_header` deployments, where the real client IP arrives in a
    /// header after the connection is up and the TCP peer is the proxy).
    ///
    /// This is a cheap pre-handshake filter used to fail-fast a connection
    /// flood before any WebSocket-upgrade or cryptographic work is done, sharply
    /// reducing the cost an unauthenticated attacker can impose. See
    /// docs/specifications/security-review.md (finding F11).
    ///
    /// Note: relayed clients (whose `ip` is `None`) are reached via the blind
    /// relay proxy, never via this inbound listener, so their absence from this
    /// set is correct - they never connect here directly.
    pub fn may_attempt_connection(&self, peer_ip: &IpAddr) -> bool {
        if let Some(trusted) = &self.trusted_proxy {
            if trusted.matches(peer_ip) {
                return true;
            }
        }

        self.clients.iter().any(|client| client.matches(*peer_ip))
    }

    ///
    /// Checks that `relay` names an existing `servers` entry - every
    /// relayed peer names, independently, which of this service's
    /// `servers` entries relays it (see `docs/plans/archive/blind-relay-proxy-design.md`
    /// §4.3), so a service can freely use different proxies for different
    /// peers (as well as mix relayed and directly-connected peers) - there
    /// is no requirement to pick just one.
    ///
    fn check_relay_exists(&self, relay: &str) -> Result<(), Error> {
        let relay = relay.trim();

        if relay.is_empty() {
            return Err(Error::Peer("No relay name provided.".to_string()));
        }

        if !self.servers.iter().any(|s| s.name == relay) {
            return Err(Error::Peer(format!(
                "Relay '{}' is not a known server - add it as a server first.",
                relay
            )));
        }

        Ok(())
    }

    fn clean_zone(&self, zone: &Option<String>) -> Result<String, Error> {
        let zone = zone.clone().unwrap_or_else(default_zone);
        let zone = zone.trim();

        if zone.is_empty() {
            return Err(Error::Peer("No zone provided.".to_string()));
        }

        // make sure that zone is [a-zA-Z0-9_<>]
        if !zone
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '<' || c == '>')
        {
            return Err(Error::Peer(format!(
                "Zone '{}' contains invalid characters. It must be alphanumeric or - _ < >",
                zone
            )));
        }

        Ok(zone.to_string())
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

        let zone = self.clean_zone(zone)?;

        // check if we already have a client with this name in this zone
        for c in self.clients.iter() {
            if c.name == name && c.zone == zone {
                return Err(Error::Peer(format!(
                    "Client with name '{}' already exists in zone {}.",
                    name, zone
                )));
            }
        }

        let client = ClientConfig::new(name, &ip, &zone);

        self.clients.push(client.clone());

        Ok(Invite::new(
            &self.name,
            &self.url,
            &zone,
            &client.inner_key,
            &client.outer_key,
            &None,
        ))
    }

    ///
    /// Add a client that can only reach us via the blind relay proxy
    /// named `relay` (an existing `servers` entry), rather than a direct,
    /// IP-allowlisted connection - see
    /// `docs/plans/archive/blind-relay-proxy-design.md`. Returns an `Invite` to
    /// pass to the client exactly as `add_client` does.
    ///
    pub fn add_relayed_client(
        &mut self,
        name: &str,
        relay: &str,
        zone: &Option<String>,
    ) -> Result<Invite, Error> {
        if name.is_empty() {
            return Err(Error::Peer("No client name provided.".to_string()));
        }

        let zone = self.clean_zone(zone)?;

        self.check_relay_exists(relay)?;

        // check if we already have a client with this name in this zone
        for c in self.clients.iter() {
            if c.name == name && c.zone == zone {
                return Err(Error::Peer(format!(
                    "Client with name '{}' already exists in zone {}.",
                    name, zone
                )));
            }
        }

        let client = ClientConfig::new_relayed(name, relay, &zone)?;

        self.clients.push(client.clone());

        Ok(Invite::new(
            &self.name,
            &self.url,
            &zone,
            &client.inner_key,
            &client.outer_key,
            &Some(relay.to_string()),
        ))
    }

    ///
    /// Remove the client named `name`. If `zone` is given, only a client in
    /// that exact zone is removed (erroring if none matches). If `zone` is
    /// not given, the name alone must be unambiguous: exactly one client
    /// with that name, in any zone, is removed; zero or multiple matches
    /// (i.e. the same name added in more than one zone) is an error asking
    /// the caller to disambiguate with `--zone`.
    ///
    pub fn remove_client(&mut self, name: &str, zone: &Option<String>) -> Result<(), Error> {
        let matching_zones: Vec<String> = self
            .clients
            .iter()
            .filter(|c| c.name == name)
            .map(|c| c.zone.clone())
            .collect();

        let zone_to_remove = match zone {
            Some(_) => {
                let zone = self.clean_zone(zone)?;
                if !matching_zones.iter().any(|z| z == &zone) {
                    return Err(Error::Peer(format!(
                        "Client with name '{}' not found in zone {}.",
                        name, zone
                    )));
                }
                zone
            }
            None => match matching_zones.as_slice() {
                [] => {
                    return Err(Error::Peer(format!(
                        "Client with name '{}' not found.",
                        name
                    )));
                }
                [only] => only.clone(),
                _ => {
                    return Err(Error::Peer(format!(
                        "Multiple clients named '{}' exist (zones: {}) - pass --zone to \
                         disambiguate.",
                        name,
                        matching_zones.join(", ")
                    )));
                }
            },
        };

        self.clients
            .retain(|client| !(client.name == name && client.zone == zone_to_remove));

        Ok(())
    }

    ///
    /// Add a server from an invite. If the invite names a blind relay
    /// proxy (see `docs/plans/archive/blind-relay-proxy-design.md`) - i.e. it was
    /// created by `add_relayed_client` on the issuing side - this peer is
    /// automatically added as relayed, reached via that proxy, rather than
    /// dialling `invite.url()` directly. Nothing extra needs to be passed
    /// in: the invite is self-describing.
    ///
    pub fn add_server(&mut self, invite: &Invite) -> Result<(), Error> {
        for server in self.servers.iter() {
            if server.name == invite.name() && server.zone == invite.zone() {
                return Err(Error::Peer(format!(
                    "Server with name '{}' already exists in zone {}.",
                    invite.name(),
                    invite.zone()
                )));
            }
        }

        let server = match invite.proxy() {
            Some(relay) => {
                self.check_relay_exists(&relay)?;
                ServerConfig::from_relayed_invite(&relay, invite)?
            }
            None => {
                let server = ServerConfig::from_invite(invite)?;

                if server.url.is_empty() {
                    tracing::warn!("No valid URL provided for server {}.", server.name());
                    return Err(Error::Null("No URL provided.".to_string()));
                }

                server
            }
        };

        self.servers.push(server);

        Ok(())
    }

    ///
    /// Remove the server named `name`. If `zone` is given, only a server in
    /// that exact zone is removed (erroring if none matches). If `zone` is
    /// not given, the name alone must be unambiguous: exactly one server
    /// with that name, in any zone, is removed; zero or multiple matches
    /// (i.e. the same name added in more than one zone) is an error asking
    /// the caller to disambiguate with `--zone`.
    ///
    pub fn remove_server(&mut self, name: &str, zone: &Option<String>) -> Result<(), Error> {
        let matching_zones: Vec<String> = self
            .servers
            .iter()
            .filter(|s| s.name == name)
            .map(|s| s.zone.clone())
            .collect();

        let zone_to_remove = match zone {
            Some(_) => {
                let zone = self.clean_zone(zone)?;
                if !matching_zones.iter().any(|z| z == &zone) {
                    return Err(Error::Peer(format!(
                        "Server with name '{}' not found in zone {}.",
                        name, zone
                    )));
                }
                zone
            }
            None => match matching_zones.as_slice() {
                [] => {
                    return Err(Error::Peer(format!(
                        "Server with name '{}' not found.",
                        name
                    )));
                }
                [only] => only.clone(),
                _ => {
                    return Err(Error::Peer(format!(
                        "Multiple servers named '{}' exist (zones: {}) - pass --zone to \
                         disambiguate.",
                        name,
                        matching_zones.join(", ")
                    )));
                }
            },
        };

        self.servers
            .retain(|server| !(server.name == name && server.zone == zone_to_remove));

        Ok(())
    }

    pub fn rotate_client_keys(
        &mut self,
        name: &str,
        zone: &Option<String>,
    ) -> Result<Invite, Error> {
        let zone = self.clean_zone(zone)?;

        // find the client with the given name and zone
        let client = self
            .clients
            .iter_mut()
            .find(|c| c.name == name && c.zone == zone)
            .ok_or_else(|| {
                Error::Peer(format!(
                    "Client with name '{}' not found in zone {}.",
                    name, zone
                ))
            })?;

        // rotate the keys
        client.rotate_keys();

        // save as a new invite
        Ok(Invite::new(
            &self.name,
            &self.url,
            &zone,
            &client.inner_key,
            &client.outer_key,
            &client.proxy,
        ))
    }

    pub fn rotate_server_keys(&mut self, invite: &Invite) -> Result<(), Error> {
        // find the server with the given name and zone
        let server = self
            .servers
            .iter_mut()
            .find(|s| s.name == invite.name() && s.zone == invite.zone())
            .ok_or_else(|| {
                Error::Peer(format!(
                    "Server with name '{}' not found in zone {}.",
                    invite.name(),
                    invite.zone()
                ))
            })?;

        // rotate the keys
        server.rotate_keys(invite)?;

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

    fn simple_config() -> ServiceConfig {
        let mut config = ServiceConfig::new(
            "test-service",
            "http://localhost:8000",
            "127.0.0.1",
            &8042,
            &None,
            &None,
        )
        .unwrap_or_else(|e| unreachable!("Could not create config: {:?}", e));
        config
            .set_simple_encryption()
            .unwrap_or_else(|e| unreachable!("Could not set encryption: {:?}", e));
        config
    }

    #[test]
    fn test_secret_encrypt_roundtrip_is_versioned() {
        let config = simple_config();

        let encrypted = config
            .encrypt(&"hunter2".to_string())
            .unwrap_or_else(|e| unreachable!("Could not encrypt: {:?}", e));

        // New secrets are written in the versioned (v1) format with a salt.
        assert!(encrypted.starts_with(SECRET_V1_PREFIX));

        let decrypted: String = config
            .decrypt(&encrypted)
            .unwrap_or_else(|e| unreachable!("Could not decrypt: {:?}", e));
        assert_eq!(decrypted, "hunter2");

        // A fresh random salt each time -> ciphertext differs for the same input.
        let encrypted2 = config
            .encrypt(&"hunter2".to_string())
            .unwrap_or_else(|e| unreachable!("Could not encrypt: {:?}", e));
        assert_ne!(encrypted, encrypted2);
    }

    #[test]
    fn test_secret_decrypt_reads_legacy_v0() {
        let config = simple_config();

        // Simulate a value written by the old (v0) code path: legacy
        // fixed-salt derivation, stored as bare hex with no version prefix.
        let legacy = Key::from_password("test-service")
            .unwrap_or_else(|e| unreachable!("Could not derive legacy key: {:?}", e))
            .expose_secret()
            .encrypt("legacy-secret".to_string())
            .unwrap_or_else(|e| unreachable!("Could not encrypt legacy: {:?}", e));

        assert!(!legacy.starts_with(SECRET_V1_PREFIX));

        let decrypted: String = config
            .decrypt(&legacy)
            .unwrap_or_else(|e| unreachable!("Could not decrypt legacy: {:?}", e));
        assert_eq!(decrypted, "legacy-secret");
    }

    #[test]
    fn test_may_attempt_connection() {
        let mut config = simple_config();

        // No clients, no trusted proxy: nothing may connect.
        assert!(!config.may_attempt_connection(&IpAddr::from([10, 0, 0, 5])));

        // A configured client's IP may connect; other addresses may not.
        config
            .add_client("peer", "10.0.0.5", &None)
            .unwrap_or_else(|e| unreachable!("Could not add client: {:?}", e));
        assert!(config.may_attempt_connection(&IpAddr::from([10, 0, 0, 5])));
        assert!(!config.may_attempt_connection(&IpAddr::from([10, 0, 0, 6])));

        // With a trusted proxy set, the proxy's address may also connect (the
        // real client IP is validated later, from the forwarded header).
        config
            .set_trusted_proxy(Some("127.0.0.0/8"))
            .unwrap_or_else(|e| unreachable!("Could not set trusted proxy: {:?}", e));
        assert!(config.may_attempt_connection(&IpAddr::from([127, 0, 0, 1])));
        assert!(config.may_attempt_connection(&IpAddr::from([10, 0, 0, 5])));
        assert!(!config.may_attempt_connection(&IpAddr::from([192, 168, 1, 1])));
    }

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
    fn test_ip_or_range_rejects_invalid_range_on_deserialize() {
        // A hand-edited config file with an unparseable range (e.g.
        // "0.0.0.0/0.0.0.0" - not valid CIDR) must be rejected when the
        // config is loaded, not silently accepted and only discovered
        // later, as a connection-time "no matching peer" warning.
        let bad_toml = r#"Range = "0.0.0.0/0.0.0.0""#;
        let result: Result<IpOrRange, _> = toml::from_str(bad_toml);
        assert!(result.is_err());

        let good_toml = r#"Range = "10.0.0.0/24""#;
        let result: Result<IpOrRange, _> = toml::from_str(good_toml);
        assert!(result.is_ok());
    }

    #[test]
    fn test_ip_or_range_accepts_plain_string_on_deserialize() {
        // A hand-edited config may use the same plain, comma-separated
        // syntax accepted by `IpOrRange::new`/`set_trusted_proxy`, not just
        // the tagged `{ IP = ... }` / `{ Range = ... }` / `{ List = [...] }`
        // form - so `trusted_proxy = "127.0.0.0/24"` works directly.
        #[derive(Deserialize)]
        struct Holder {
            value: IpOrRange,
        }

        let holder: Holder = toml::from_str(r#"value = "127.0.0.0/24""#)
            .unwrap_or_else(|e| unreachable!("Could not deserialise plain range string: {}", e));
        assert_eq!(holder.value, IpOrRange::Range("127.0.0.0/24".to_string()));
        assert!(holder.value.matches(&IpAddr::from([127, 0, 0, 1])));
        assert!(!holder.value.matches(&IpAddr::from([128, 0, 0, 1])));

        let holder: Holder = toml::from_str(r#"value = "127.0.0.1""#)
            .unwrap_or_else(|e| unreachable!("Could not deserialise plain IP string: {}", e));
        assert_eq!(holder.value, IpOrRange::IP(IpAddr::from([127, 0, 0, 1])));

        let holder: Holder = toml::from_str(r#"value = "127.0.0.1,10.0.0.0/24""#)
            .unwrap_or_else(|e| unreachable!("Could not deserialise plain list string: {}", e));
        assert!(holder.value.matches(&IpAddr::from([127, 0, 0, 1])));
        assert!(holder.value.matches(&IpAddr::from([10, 0, 0, 1])));
        assert!(!holder.value.matches(&IpAddr::from([11, 0, 0, 1])));

        // still rejected at load time, not just when first typed on the CLI
        let result: Result<Holder, _> = toml::from_str(r#"value = "not-an-ip""#);
        assert!(result.is_err());
    }

    #[test]
    fn test_ip_range_never_matches_across_address_families() {
        // Regression test for finding R8. `iptools`'s IPv4 `IpRange::contains`
        // truncates an IPv6 argument to its low 32 bits, so before the
        // address-family check in `matches` every one of these returned
        // `true` - letting any IPv6 peer whose last 32 bits fell inside an
        // IPv4 CIDR satisfy a client `ip` allow-list or `trusted_proxy` rule.
        let v4_cases = [
            ("127.0.0.0/8", "2001:db8::7f00:1"),
            ("10.0.0.0/24", "2001:db8::a00:5"),
            ("10.0.0.0/24", "::a00:5"),
            ("192.168.0.0/16", "fe80::c0a8:1"),
        ];

        for (range, addr) in v4_cases {
            let range_parsed = IpOrRange::new(range)
                .unwrap_or_else(|e| unreachable!("Could not parse range {}: {}", range, e));
            let addr_parsed: IpAddr = addr
                .parse()
                .unwrap_or_else(|e| unreachable!("Could not parse address {}: {}", addr, e));

            assert!(
                !range_parsed.matches(&addr_parsed),
                "IPv4 range {} must not match IPv6 address {}",
                range,
                addr
            );
        }

        // ...and the mirror image: an IPv6 range must not match an IPv4 peer.
        let v6_range = IpOrRange::new("2001:db8::/32")
            .unwrap_or_else(|e| unreachable!("Could not parse IPv6 range: {}", e));
        assert!(!v6_range.matches(&IpAddr::from([127, 0, 0, 1])));
        assert!(!v6_range.matches(&IpAddr::from([10, 0, 0, 5])));

        // A list is only as strict as its entries, so check it too.
        let list = IpOrRange::new("127.0.0.0/8,10.0.0.0/24")
            .unwrap_or_else(|e| unreachable!("Could not parse list: {}", e));
        let v6: IpAddr = "2001:db8::7f00:1"
            .parse()
            .unwrap_or_else(|e| unreachable!("Could not parse address: {}", e));
        assert!(!list.matches(&v6));

        // Same-family matching must still work exactly as before.
        let v4_range = IpOrRange::new("127.0.0.0/8")
            .unwrap_or_else(|e| unreachable!("Could not parse range: {}", e));
        assert!(v4_range.matches(&IpAddr::from([127, 0, 0, 1])));
        assert!(!v4_range.matches(&IpAddr::from([128, 0, 0, 1])));
        assert!(v6_range.matches(
            &"2001:db8::1"
                .parse::<IpAddr>()
                .unwrap_or_else(|e| unreachable!("Could not parse address: {}", e))
        ));
    }

    #[test]
    fn test_ipv4_mapped_ipv6_address_matches_ipv4_rules() {
        // On a dual-stack listener an IPv4 peer arrives as ::ffff:a.b.c.d, so
        // canonicalising before matching is what keeps IPv4 allow-lists
        // working there. This is the one cross-notation match that *should*
        // succeed - it is the same host, written differently (finding R8).
        let mapped: IpAddr = "::ffff:127.0.0.1"
            .parse()
            .unwrap_or_else(|e| unreachable!("Could not parse mapped address: {}", e));

        let range = IpOrRange::new("127.0.0.0/8")
            .unwrap_or_else(|e| unreachable!("Could not parse range: {}", e));
        assert!(range.matches(&mapped));

        let single = IpOrRange::new("127.0.0.1")
            .unwrap_or_else(|e| unreachable!("Could not parse address: {}", e));
        assert!(single.matches(&mapped));

        // But a mapped address outside the range still must not match.
        let elsewhere: IpAddr = "::ffff:203.0.113.9"
            .parse()
            .unwrap_or_else(|e| unreachable!("Could not parse mapped address: {}", e));
        assert!(!range.matches(&elsewhere));
    }

    #[test]
    fn test_ip_or_range_full_range_does_not_panic() {
        // "0.0.0.0/0" is the canonical CIDR "match everything" range, but
        // constructing it via `iptools::iprange::IpRange` panics (v0.3.0
        // overflows computing `len` for the full 2^32-address span) - it
        // must be special-cased entirely without ever calling into
        // `iptools::iprange`.
        let ip = IpOrRange::new("0.0.0.0/0").unwrap_or_else(|e| {
            unreachable!("Could not create full-range IpOrRange: {:?}", e);
        });

        assert!(ip.matches(&IpAddr::from([1, 2, 3, 4])));
        assert!(ip.matches(&IpAddr::from([255, 255, 255, 255])));
        assert!(!ip.matches(
            &"::1"
                .parse()
                .unwrap_or_else(|e| { unreachable!("Could not parse IPv6 address: {:?}", e) })
        ));

        // any prefix before /0 means the same thing
        let ip = IpOrRange::new("10.1.2.3/0").unwrap_or_else(|e| {
            unreachable!("Could not create full-range IpOrRange: {:?}", e);
        });
        assert!(ip.matches(&IpAddr::from([8, 8, 8, 8])));

        let result: Result<IpOrRange, _> = toml::from_str(r#"Range = "0.0.0.0/0""#);
        assert!(result.is_ok());
    }

    #[test]
    fn test_ip_or_range_ipv6() {
        // mirrors test_ip_or_range, but for IPv6 - see
        // docs/plans/ipv6-support-design.md §4.2.
        let mut ip = IpOrRange::new("::1").unwrap_or_else(|e| {
            unreachable!("Could not create IPv6 address: {:?}", e);
        });

        assert_eq!(format!("{}", ip), "::1");

        assert!(ip.matches(
            &"::1"
                .parse()
                .unwrap_or_else(|e| { unreachable!("Could not parse IPv6 address: {:?}", e) })
        ));
        assert!(!ip.matches(
            &"::2"
                .parse()
                .unwrap_or_else(|e| { unreachable!("Could not parse IPv6 address: {:?}", e) })
        ));
        assert!(!ip.matches(&IpAddr::from([127, 0, 0, 1])));

        assert!(IpOrRange::new("::zzzz").is_err());

        ip = IpOrRange::new("2001:db8::/32").unwrap_or_else(|e| {
            unreachable!("Could not create IPv6 range: {:?}", e);
        });

        assert_eq!(format!("{}", ip), "2001:db8::/32");

        assert!(ip.matches(
            &"2001:db8::1"
                .parse()
                .unwrap_or_else(|e| { unreachable!("Could not parse IPv6 address: {:?}", e) })
        ));
        assert!(ip.matches(
            &"2001:db8:0:ffff::1"
                .parse()
                .unwrap_or_else(|e| { unreachable!("Could not parse IPv6 address: {:?}", e) })
        ));
        assert!(!ip.matches(
            &"2001:db9::1"
                .parse()
                .unwrap_or_else(|e| { unreachable!("Could not parse IPv6 address: {:?}", e) })
        ));
        assert!(!ip.matches(&IpAddr::from([127, 0, 0, 1])));
    }

    #[test]
    fn test_ip_or_range_rejects_invalid_range_on_deserialize_ipv6() {
        let bad_toml = r#"Range = "2001:db8::/zz""#;
        let result: Result<IpOrRange, _> = toml::from_str(bad_toml);
        assert!(result.is_err());

        let good_toml = r#"Range = "2001:db8::/32""#;
        let result: Result<IpOrRange, _> = toml::from_str(good_toml);
        assert!(result.is_ok());
    }

    #[test]
    fn test_ip_or_range_full_ipv6_range_does_not_panic() {
        // "::/0" is IPv6's equivalent of "0.0.0.0/0" - and hits the exact
        // same `iptools::iprange::IPv6::new` `len` overflow (u128::MAX -
        // 0 + 1) that the IPv4 case works around - see
        // test_ip_or_range_full_range_does_not_panic and
        // docs/plans/ipv6-support-design.md §3.
        let ip = IpOrRange::new("::/0").unwrap_or_else(|e| {
            unreachable!("Could not create full-range IpOrRange: {:?}", e);
        });

        assert!(ip.matches(
            &"::1"
                .parse()
                .unwrap_or_else(|e| { unreachable!("Could not parse IPv6 address: {:?}", e) })
        ));
        assert!(ip.matches(
            &"2001:db8::1"
                .parse()
                .unwrap_or_else(|e| { unreachable!("Could not parse IPv6 address: {:?}", e) })
        ));
        assert!(!ip.matches(&IpAddr::from([1, 2, 3, 4])));

        // any prefix before /0 means the same thing
        let ip = IpOrRange::new("2001:db8::1/0").unwrap_or_else(|e| {
            unreachable!("Could not create full-range IpOrRange: {:?}", e);
        });
        assert!(ip.matches(
            &"::1"
                .parse()
                .unwrap_or_else(|e| { unreachable!("Could not parse IPv6 address: {:?}", e) })
        ));

        let result: Result<IpOrRange, _> = toml::from_str(r#"Range = "::/0""#);
        assert!(result.is_ok());
    }

    #[test]
    fn test_ip_or_range_single_entry_is_not_wrapped_in_a_list() {
        // a single entry (no comma) must produce the plain IP/Range
        // variant directly, not a one-element List - so existing
        // single-address configuration round-trips byte-for-byte.
        assert_eq!(
            IpOrRange::new("127.0.0.1").unwrap_or_else(|e| unreachable!("{:?}", e)),
            IpOrRange::IP(
                "127.0.0.1"
                    .parse()
                    .unwrap_or_else(|e| unreachable!("{:?}", e))
            )
        );
        assert_eq!(
            IpOrRange::new("10.0.0.0/24").unwrap_or_else(|e| unreachable!("{:?}", e)),
            IpOrRange::Range("10.0.0.0/24".to_string())
        );
    }

    #[test]
    fn test_ip_or_range_list() {
        let ip = IpOrRange::new("127.0.0.1,10.0.0.0/24,2001:db8::/32")
            .unwrap_or_else(|e| unreachable!("Could not create IpOrRange list: {:?}", e));

        assert!(matches!(ip, IpOrRange::List(ref entries) if entries.len() == 3));

        // matches any one of the three entries...
        assert!(ip.matches(&IpAddr::from([127, 0, 0, 1])));
        assert!(ip.matches(&IpAddr::from([10, 0, 0, 42])));
        assert!(ip.matches(
            &"2001:db8::1"
                .parse()
                .unwrap_or_else(|e| { unreachable!("Could not parse IPv6 address: {:?}", e) })
        ));

        // ...but nothing outside all three
        assert!(!ip.matches(&IpAddr::from([127, 0, 0, 2])));
        assert!(!ip.matches(&IpAddr::from([10, 0, 1, 1])));
        assert!(!ip.matches(
            &"2001:db9::1"
                .parse()
                .unwrap_or_else(|e| { unreachable!("Could not parse IPv6 address: {:?}", e) })
        ));

        // Display round-trips back to a comma-separated string `new` can
        // re-parse identically.
        assert_eq!(format!("{}", ip), "127.0.0.1,10.0.0.0/24,2001:db8::/32");
        let reparsed = IpOrRange::new(&format!("{}", ip))
            .unwrap_or_else(|e| unreachable!("Could not re-parse: {:?}", e));
        assert_eq!(reparsed, ip);
    }

    #[test]
    fn test_ip_or_range_list_tolerates_whitespace_around_entries() {
        // `client --add name --ip "127.0.0.1, 10.0.0.0/24"` - a space
        // after the comma (or around any entry) is a natural thing for an
        // operator to type and must not be treated as part of the address.
        let ip = IpOrRange::new(" 127.0.0.1 , 10.0.0.0/24 ")
            .unwrap_or_else(|e| unreachable!("Could not create IpOrRange list: {:?}", e));

        assert!(ip.matches(&IpAddr::from([127, 0, 0, 1])));
        assert!(ip.matches(&IpAddr::from([10, 0, 0, 1])));
        assert!(!ip.matches(&IpAddr::from([8, 8, 8, 8])));
    }

    #[test]
    fn test_ip_or_range_list_rejects_any_invalid_entry() {
        // one bad entry in an otherwise-valid list must fail the whole
        // thing at parse/load time, not silently drop just that entry.
        assert!(IpOrRange::new("127.0.0.1,not-an-ip,10.0.0.0/24").is_err());
    }

    #[test]
    fn test_ip_or_range_list_round_trips_through_toml() {
        // confirms List survives a real serialise/deserialise cycle - not
        // just the in-memory Display/new round-trip above.
        let ip = IpOrRange::new("127.0.0.1,10.0.0.0/24")
            .unwrap_or_else(|e| unreachable!("Could not create IpOrRange list: {:?}", e));

        let toml_str =
            toml::to_string(&ip).unwrap_or_else(|e| unreachable!("Could not serialise: {:?}", e));

        let reloaded: IpOrRange = toml::from_str(&toml_str)
            .unwrap_or_else(|e| unreachable!("Could not deserialise: {:?}", e));

        assert_eq!(reloaded, ip);
        assert!(reloaded.matches(&IpAddr::from([127, 0, 0, 1])));
        assert!(reloaded.matches(&IpAddr::from([10, 0, 0, 1])));
    }

    #[test]
    fn test_ip_or_range_list_rejects_invalid_entry_on_deserialize() {
        // a hand-edited config with one bad entry inside a saved List
        // must be rejected at load time too, not just when first typed on
        // the CLI - each element is validated recursively via the same
        // Deserialize impl.
        let bad_toml = r#"List = [{ IP = "127.0.0.1" }, { Range = "0.0.0.0/0.0.0.0" }]"#;
        let result: Result<IpOrRange, _> = toml::from_str(bad_toml);
        assert!(result.is_err());
    }

    #[test]
    fn test_service_config_ipv6_listen_address() {
        // `ServiceConfig.ip` must accept an IPv6 address, and produce a
        // `SocketAddr` that round-trips correctly - see
        // docs/plans/ipv6-support-design.md §4.1 (the actual `bind()` call
        // this feeds lives in `server.rs`, not exercised by this crate's
        // unit tests, but the `SocketAddr` construction it depends on is).
        let config =
            ServiceConfig::new("test-ipv6", "http://localhost", "::1", &6010, &None, &None)
                .unwrap_or_else(|e| unreachable!("Cannot create service config: {}", e));

        assert!(config.ip().is_ipv6());

        let addr = std::net::SocketAddr::new(config.ip(), config.port());
        assert_eq!(addr.ip(), config.ip());
        assert_eq!(addr.port(), 6010);
    }

    #[test]
    fn test_create_websocket_url_ipv6_host() {
        // The client-dial-out path (`ServerConfig::get_websocket_url`)
        // goes through the `url` crate rather than the ad-hoc string
        // formatting `server.rs`'s own bind address used to use (§4.1) -
        // this confirms that path already handles a bracketed IPv6 host
        // literal correctly, end to end through to the same
        // `IntoClientRequest` conversion `make_connection` performs, per
        // docs/plans/ipv6-support-design.md §5 ("expected to work" should
        // be verified, not just assumed).
        use tungstenite::client::IntoClientRequest;

        let url = create_websocket_url("http://[2001:db8::1]:8080").unwrap_or_else(|e| {
            unreachable!("Could not create websocket URL: {:?}", e);
        });

        assert_eq!(url, "ws://[2001:db8::1]:8080/");

        let request = url.into_client_request().unwrap_or_else(|e| {
            unreachable!("Bracketed IPv6 URL rejected by IntoClientRequest: {:?}", e);
        });
        assert!(request
            .uri()
            .host()
            .unwrap_or_default()
            .contains("2001:db8::1"));
    }

    #[test]
    fn test_client_config() {
        let ip = IpOrRange::new("127.0.0.1").unwrap_or_else(|e| {
            unreachable!("Could not create IP address: {:?}", e);
        });

        let client = ClientConfig::new("test", &ip, &default_zone());

        assert_eq!(client.name, "test".to_string());
        assert_eq!(client.ip, Some(ip));

        let peer = PeerConfig::from_client(&client);

        assert!(peer.is_client());
        assert!(!peer.is_server());
        assert!(!peer.is_null());
    }

    #[test]
    fn test_remove_client_and_server_require_a_match() {
        // Removing a peer that was added in a different zone (or never
        // added at all) must error clearly, not silently leave the peer
        // list unchanged - `remove_client`/`remove_server` filter on
        // (name, zone) together, so a zone mismatch used to look
        // indistinguishable from a successful removal.
        let mut service =
            ServiceConfig::new("airr", "http://localhost", "127.0.0.1", &5560, &None, &None)
                .unwrap_or_else(|e| unreachable!("Cannot create service config: {}", e));

        service
            .add_client("brics", "127.0.0.1", &None)
            .unwrap_or_else(|e| unreachable!("Cannot add client: {}", e));

        // wrong zone - must error, not silently no-op
        assert!(service
            .remove_client("brics", &Some("other-zone".to_string()))
            .is_err());
        assert_eq!(service.clients().len(), 1);

        // unknown name - must error
        assert!(service.remove_client("nonexistent", &None).is_err());
        assert_eq!(service.clients().len(), 1);

        // correct name and zone - actually removes it
        service
            .remove_client("brics", &None)
            .unwrap_or_else(|e| unreachable!("Cannot remove client: {}", e));
        assert_eq!(service.clients().len(), 0);

        // same behaviour for servers
        let mut proxy = ServiceConfig::new(
            "proxy",
            "http://localhost",
            "127.0.0.1",
            &5561,
            &None,
            &None,
        )
        .unwrap_or_else(|e| unreachable!("Cannot create service config: {}", e));

        let invite = proxy
            .add_client("airr", "127.0.0.1", &None)
            .unwrap_or_else(|e| unreachable!("Cannot add client: {}", e));
        service
            .add_server(&invite)
            .unwrap_or_else(|e| unreachable!("Cannot add server: {}", e));

        assert!(service
            .remove_server("proxy", &Some("other-zone".to_string()))
            .is_err());
        assert_eq!(service.servers().len(), 1);

        service
            .remove_server("proxy", &None)
            .unwrap_or_else(|e| unreachable!("Cannot remove server: {}", e));
        assert_eq!(service.servers().len(), 0);
    }

    #[test]
    fn test_remove_client_ambiguous_name_requires_zone() {
        // A name added in more than one zone is genuinely ambiguous without
        // --zone - unlike the single-match case, which now removes without
        // requiring --zone at all.
        let mut service =
            ServiceConfig::new("airr", "http://localhost", "127.0.0.1", &5562, &None, &None)
                .unwrap_or_else(|e| unreachable!("Cannot create service config: {}", e));

        service
            .add_client("brics", "127.0.0.1", &None)
            .unwrap_or_else(|e| unreachable!("Cannot add client: {}", e));
        service
            .add_client("brics", "127.0.0.1", &Some("other-zone".to_string()))
            .unwrap_or_else(|e| unreachable!("Cannot add client in second zone: {}", e));

        assert_eq!(service.clients().len(), 2);

        // ambiguous - must error rather than guessing which one
        assert!(service.remove_client("brics", &None).is_err());
        assert_eq!(service.clients().len(), 2);

        // disambiguated with --zone - removes only that one
        service
            .remove_client("brics", &Some("other-zone".to_string()))
            .unwrap_or_else(|e| unreachable!("Cannot remove client: {}", e));
        assert_eq!(service.clients().len(), 1);

        // now unambiguous again - removes without --zone
        service
            .remove_client("brics", &None)
            .unwrap_or_else(|e| unreachable!("Cannot remove client: {}", e));
        assert_eq!(service.clients().len(), 0);
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
        secondary.add_server(&invite).unwrap_or_else(|e| {
            unreachable!("Cannot add primary to secondary: {}", e);
        });

        assert_eq!(primary.clients().len(), 1);
        assert_eq!(secondary.servers().len(), 1);

        assert_eq!(primary.clients()[0].name(), "secondary".to_string());
        assert_eq!(secondary.servers()[0].name(), "primary".to_string());
    }

    #[test]
    fn test_relayed_peer_requires_known_relay() {
        let mut airr =
            ServiceConfig::new("airr", "http://localhost", "127.0.0.1", &5546, &None, &None)
                .unwrap_or_else(|e| unreachable!("Cannot create service config: {}", e));

        // "proxy" is not yet a known server - must be rejected
        assert!(airr.add_relayed_client("brics", "proxy", &None).is_err());
    }

    #[test]
    fn test_relayed_peer_introduction() {
        let mut airr =
            ServiceConfig::new("airr", "http://localhost", "127.0.0.1", &5547, &None, &None)
                .unwrap_or_else(|e| unreachable!("Cannot create service config: {}", e));
        let mut brics = ServiceConfig::new(
            "brics",
            "http://localhost",
            "127.0.0.1",
            &5548,
            &None,
            &None,
        )
        .unwrap_or_else(|e| unreachable!("Cannot create service config: {}", e));
        let mut proxy = ServiceConfig::new(
            "proxy",
            "http://localhost",
            "127.0.0.1",
            &5549,
            &None,
            &None,
        )
        .unwrap_or_else(|e| unreachable!("Cannot create service config: {}", e));

        // both airr and brics have an ordinary, direct relationship with
        // the proxy - unaffected by anything relay-related
        let invite = proxy
            .add_client("airr", "127.0.0.1", &None)
            .unwrap_or_else(|e| unreachable!("Cannot add airr to proxy: {}", e));
        airr.add_server(&invite)
            .unwrap_or_else(|e| unreachable!("Cannot add proxy to airr: {}", e));

        let invite = proxy
            .add_client("brics", "127.0.0.1", &None)
            .unwrap_or_else(|e| unreachable!("Cannot add brics to proxy: {}", e));
        brics
            .add_server(&invite)
            .unwrap_or_else(|e| unreachable!("Cannot add proxy to brics: {}", e));

        // airr is the relayed "server" - it authorises brics to reach it
        // via the proxy
        let invite = airr
            .add_relayed_client("brics", "proxy", &None)
            .unwrap_or_else(|e| unreachable!("Cannot add relayed brics to airr: {}", e));

        // brics is the relayed "client" - it reaches airr via the same
        // proxy, auto-detected from the invite (no separate relay name
        // needs to be passed in - the invite is self-describing)
        brics
            .add_server(&invite)
            .unwrap_or_else(|e| unreachable!("Cannot add relayed airr to brics: {}", e));

        let relayed_client = airr
            .clients()
            .into_iter()
            .find(|c| c.name() == "brics")
            .unwrap_or_else(|| unreachable!("brics should be a client of airr"));
        assert!(relayed_client.is_relayed());
        assert_eq!(relayed_client.proxy(), Some("proxy".to_string()));
        assert!(relayed_client.ip().is_none());

        let relayed_server = brics
            .servers()
            .into_iter()
            .find(|s| s.name() == "airr")
            .unwrap_or_else(|| unreachable!("airr should be a server of brics"));
        assert_eq!(relayed_server.proxy(), Some("proxy".to_string()));
        assert_eq!(relayed_server.url(), "".to_string());

        // the shared key material must actually match between the two sides
        use secrecy::ExposeSecret;
        assert_eq!(
            format!("{:?}", relayed_client.inner_key().expose_secret()),
            format!("{:?}", relayed_server.inner_key().expose_secret())
        );
        assert_eq!(
            format!("{:?}", relayed_client.outer_key().expose_secret()),
            format!("{:?}", relayed_server.outer_key().expose_secret())
        );
    }

    #[test]
    fn test_relayed_peers_can_use_different_proxies() {
        // A service can relay different peers via different proxies at
        // the same time - there is no requirement to pick just one (see
        // `check_relay_exists`). Mixing relayed and direct peers already
        // worked; this proves mixing *relayed peers reached via different
        // proxies* works too.
        let mut airr =
            ServiceConfig::new("airr", "http://localhost", "127.0.0.1", &5550, &None, &None)
                .unwrap_or_else(|e| unreachable!("Cannot create service config: {}", e));

        // introduce two candidate relays as ordinary direct servers
        let mut proxy1 = ServiceConfig::new(
            "proxy1",
            "http://localhost",
            "127.0.0.1",
            &5551,
            &None,
            &None,
        )
        .unwrap_or_else(|e| unreachable!("Cannot create service config: {}", e));
        let mut proxy2 = ServiceConfig::new(
            "proxy2",
            "http://localhost",
            "127.0.0.1",
            &5552,
            &None,
            &None,
        )
        .unwrap_or_else(|e| unreachable!("Cannot create service config: {}", e));

        let invite = proxy1
            .add_client("airr", "127.0.0.1", &None)
            .unwrap_or_else(|e| unreachable!("Cannot add airr to proxy1: {}", e));
        airr.add_server(&invite)
            .unwrap_or_else(|e| unreachable!("Cannot add proxy1 to airr: {}", e));

        let invite = proxy2
            .add_client("airr", "127.0.0.1", &None)
            .unwrap_or_else(|e| unreachable!("Cannot add airr to proxy2: {}", e));
        airr.add_server(&invite)
            .unwrap_or_else(|e| unreachable!("Cannot add proxy2 to airr: {}", e));

        // relayed clients via two *different* proxies must both succeed
        airr.add_relayed_client("brics", "proxy1", &None)
            .unwrap_or_else(|e| unreachable!("Cannot add relayed brics via proxy1: {}", e));
        airr.add_relayed_client("someone_else", "proxy2", &None)
            .unwrap_or_else(|e| unreachable!("Cannot add relayed peer via proxy2: {}", e));

        let brics = airr
            .clients()
            .into_iter()
            .find(|c| c.name() == "brics")
            .unwrap_or_else(|| unreachable!("brics should be a client of airr"));
        assert_eq!(brics.proxy(), Some("proxy1".to_string()));

        let someone_else = airr
            .clients()
            .into_iter()
            .find(|c| c.name() == "someone_else")
            .unwrap_or_else(|| unreachable!("someone_else should be a client of airr"));
        assert_eq!(someone_else.proxy(), Some("proxy2".to_string()));
    }
}
