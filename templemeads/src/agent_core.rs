// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent::Type as AgentType;
use crate::error::Error;

use anyhow::Context;
use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use paddington::config::{
    load as load_config, save as save_config, Defaults as ServiceDefaults, ServiceConfig,
};
use paddington::invite::{load as load_invite, save as save_invite};
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;

// Configuration

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(bound(deserialize = "T: for<'de2> Deserialize<'de2>"))]
pub struct Config<T = ()>
where
    T: Serialize + Clone + std::fmt::Debug,
{
    service: ServiceConfig,
    agent: AgentType,

    #[serde(flatten)]
    pub agent_config: T,

    #[serde(default)]
    extras: HashMap<String, String>,

    #[serde(skip)]
    one_shot_commands: Option<Vec<String>>,

    #[serde(skip)]
    one_shot_sender: Option<String>,

    #[serde(skip)]
    one_shot_zone: Option<String>,
}

impl<T> Config<T>
where
    T: Serialize + for<'de> Deserialize<'de> + Clone + std::fmt::Debug + Default,
{
    pub fn new(service: ServiceConfig, agent: AgentType) -> Self {
        Self {
            service,
            agent,
            agent_config: T::default(),
            extras: HashMap::new(),
            one_shot_commands: None,
            one_shot_sender: None,
            one_shot_zone: None,
        }
    }

    pub fn service(&self) -> ServiceConfig {
        self.service.clone()
    }

    pub fn agent(&self) -> AgentType {
        self.agent.clone()
    }

    pub fn option(&self, key: &str, default: &str) -> String {
        match self.extras.get(key) {
            Some(value) => value.clone(),
            None => default.to_string(),
        }
    }

    pub fn secret(&self, key: &str) -> Option<SecretString> {
        match self.extras.get(key) {
            Some(value) => match self.service.decrypt::<String>(value) {
                Ok(secret) => Some(secret.into()),
                Err(e) => {
                    tracing::error!("Failed to decrypt secret for key '{}': {:?}", key, e);
                    None
                }
            },
            None => None,
        }
    }

    pub fn one_shot_commands(&self) -> &Option<Vec<String>> {
        &self.one_shot_commands
    }

    pub fn one_shot_sender(&self) -> String {
        match &self.one_shot_sender {
            Some(sender) => sender.clone(),
            None => "oneshot".to_string(),
        }
    }

    pub fn one_shot_zone(&self) -> String {
        match &self.one_shot_zone {
            Some(zone) => zone.clone(),
            None => "one-shot".to_string(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(bound(deserialize = "T: for<'de2> Deserialize<'de2>"))]
pub struct Defaults<T = ()>
where
    T: Serialize + Clone + std::fmt::Debug,
{
    pub service: ServiceDefaults,
    pub agent: AgentType,
    pub agent_config: T,
    pub extras: HashMap<String, String>,
}

impl<T> Defaults<T>
where
    T: Serialize + for<'de> Deserialize<'de> + Clone + std::fmt::Debug + Default,
{
    #[allow(clippy::too_many_arguments)]
    pub fn parse(
        name: Option<String>,
        config_file: Option<PathBuf>,
        url: Option<String>,
        ip: Option<String>,
        port: Option<u16>,
        healthcheck_port: Option<u16>,
        proxy_header: Option<String>,
        agent: Option<AgentType>,
    ) -> Self {
        Self {
            service: ServiceDefaults::parse(
                name,
                config_file,
                url,
                ip,
                port,
                healthcheck_port,
                proxy_header,
            ),
            agent: agent.unwrap_or(AgentType::Portal),
            agent_config: T::default(),
            extras: HashMap::new(),
        }
    }

    pub fn add_extra(&mut self, key: &str, value: &str) {
        self.extras.insert(key.to_string(), value.to_string());
    }

    pub fn get_extra(&self, key: &str) -> Option<&String> {
        self.extras.get(key)
    }
}

// Command line parsing

mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

fn version() -> &'static str {
    built_info::GIT_VERSION.unwrap_or(built_info::PKG_VERSION)
}

///
/// Process the command line arguments, performing any necessary actions.
/// This will return a Config object that can be used to run the service
/// if this is requested. If nothing is returned then the program can
/// cleanly exit.
///
///
/// Validate a `--type` argument, returning the canonical string form to store in
/// the config (or `None` if the operator did not pass one).
///
/// A typo would otherwise be recorded silently and then discarded at startup as
/// unrecognised, leaving the operator believing a peer was being checked when it
/// was not - so this fails at add-time, when there is someone there to read the
/// error. See `docs/specifications/security-review-2.md` (finding R3).
///
pub(crate) fn validate_agent_type(agent_type: &Option<String>) -> Result<Option<String>, Error> {
    let Some(agent_type) = agent_type else {
        return Ok(None);
    };

    match AgentType::parse(agent_type) {
        Some(typ) => Ok(Some(typ.to_string())),
        None => Err(Error::PeerEdit(format!(
            "'{}' is not a recognised agent type. Valid values are: portal, provider, \
             platform, instance, bridge, account, filesystem, scheduler, virtual.",
            agent_type
        ))),
    }
}

pub async fn process_args<T>(defaults: &Defaults<T>) -> Result<Option<Config<T>>, Error>
where
    T: Serialize + for<'de> Deserialize<'de> + Clone + std::fmt::Debug + Default,
{
    let args = Args::parse();
    let defaults = defaults.clone();

    let config_file = match args.config_file {
        Some(path) => path,
        None => defaults.service.config_file(),
    };

    // see if we need to initialise the config directory
    match &args.command {
        Some(Commands::Init {
            service,
            url,
            ip,
            port,
            healthcheck_port,
            proxy_header,
            trusted_proxy,
            force,
        }) => {
            let local_healthcheck_port;

            if let Some(healthcheck_port) = healthcheck_port {
                local_healthcheck_port = Some(*healthcheck_port);
            } else {
                local_healthcheck_port = defaults.service.healthcheck_port();
            }

            let mut config = Config {
                service: {
                    ServiceConfig::new(
                        &service.clone().unwrap_or(defaults.service.name()),
                        &url.clone().unwrap_or(defaults.service.url()),
                        &ip.clone()
                            .unwrap_or(defaults.service.ip())
                            .parse::<IpAddr>()?
                            .to_string(),
                        &port.unwrap_or_else(|| defaults.service.port()),
                        &local_healthcheck_port,
                        proxy_header,
                    )?
                },
                agent: defaults.agent.clone(),
                agent_config: defaults.agent_config.clone(),
                extras: defaults.extras.clone(),
                one_shot_commands: None,
                one_shot_sender: None,
                one_shot_zone: None,
            };

            config.service.set_trusted_proxy(trusted_proxy.as_deref())?;

            if config_file.try_exists()? {
                if *force {
                    std::fs::remove_file(&config_file)
                        .context("Could not remove existing config file.")?;
                } else {
                    tracing::warn!("Config file already exists: {}", &config_file.display());
                    return Err(Error::ConfigExists(format!(
                        "Config file already exists: {}",
                        &config_file.display()
                    )));
                }
            }

            // save the config to the requested file
            save_config(&config, &config_file)?;

            tracing::info!(
                "Service initialised. Config file written to {}",
                &config_file.display()
            );
            return Ok(None);
        }
        Some(Commands::Client {
            add,
            ip,
            list,
            remove,
            zone,
            rotate,
            proxy,
            r#type,
        }) => {
            if *list {
                let config = load_config::<Config<T>>(&config_file)?;
                for client in config.service.clients() {
                    println!("{}", client);
                }
                return Ok(None);
            }

            if let Some(client) = add {
                let agent_type = validate_agent_type(r#type)?;

                let mut config = load_config::<Config<T>>(&config_file)?;

                let invite = match proxy {
                    Some(relay) => {
                        config
                            .service
                            .add_relayed_client(client, relay, zone, &agent_type)?
                    }
                    None => {
                        if ip.is_none() {
                            return Err(Error::PeerEdit(format!(
                                "No IP address or IP range provided for client {}.",
                                client
                            )));
                        }

                        config.service.add_client(
                            client,
                            &ip.clone().unwrap_or_else(|| "".to_string()),
                            zone,
                            &agent_type,
                        )?
                    }
                };

                // ...and tell the client what *we* are, so its side of the
                // expectation needs no hand-editing. We know our own type for
                // certain, so this is always declared.
                let invite = invite.with_agent_type(Some(defaults.agent.to_string()));

                save_config(&config, &config_file)?;
                save_invite(
                    &invite,
                    &PathBuf::from(format!("./invite_{}_{}.toml", invite.name(), invite.zone())),
                )?;

                match &agent_type {
                    Some(t) => tracing::info!(
                        "Client '{}' added, and must present itself as a '{}' agent.",
                        client,
                        t
                    ),
                    None => tracing::warn!(
                        "Client '{}' added with no expected agent type, so whatever role it \
                         claims will be accepted. Pass --type to check it.",
                        client
                    ),
                }
                return Ok(None);
            }

            if let Some(client) = remove {
                let mut config = load_config::<Config<T>>(&config_file)?;
                config.service.remove_client(client, zone)?;
                save_config(&config, &config_file)?;
                tracing::info!("Client '{}' removed.", client);
                return Ok(None);
            }

            if let Some(client) = rotate {
                let mut config = load_config::<Config<T>>(&config_file)?;
                let invite = config
                    .service
                    .rotate_client_keys(client, zone)?
                    .with_agent_type(Some(defaults.agent.to_string()));

                save_config(&config, &config_file)?;
                save_invite(
                    &invite,
                    &PathBuf::from(format!("./rotate_{}_{}.toml", invite.name(), invite.zone())),
                )?;

                tracing::info!("Client '{}' rotated.", client);
                return Ok(None);
            }

            let _ = Args::command().print_help();

            return Ok(None);
        }
        Some(Commands::Server {
            add,
            list,
            remove,
            rotate,
            zone,
        }) => {
            if *list {
                let config = load_config::<Config<T>>(&config_file)?;
                for server in config.service.servers() {
                    println!("{}", server);
                }
                return Ok(None);
            }

            if let Some(server) = add {
                // read the invitation from the passed toml file
                let invite = load_invite(server)?;

                let zone = zone.clone().unwrap_or_else(|| invite.zone());

                if zone != invite.zone() {
                    return Err(Error::InvalidConfig(format!(
                        "Zone mismatch: invite is for zone '{}', but zone '{}' was specified.",
                        invite.zone(),
                        zone
                    )));
                }

                let mut config = load_config::<Config<T>>(&config_file)?;

                // if the invite names a blind relay proxy (i.e. it was
                // created with `client --add --proxy` on the issuing side),
                // this is added as a relayed server automatically - nothing
                // else needs to be passed in here.
                config.service.add_server(&invite)?;

                save_config(&config, &config_file)?;
                tracing::info!("Server '{}' added.", server.display());
                return Ok(None);
            }

            if let Some(server) = remove {
                let mut config = load_config::<Config<T>>(&config_file)?;
                config.service.remove_server(server, zone)?;
                save_config(&config, &config_file)?;
                tracing::info!("Server '{}' removed.", server);
                return Ok(None);
            }

            if let Some(server) = rotate {
                // read the invitation from the passed toml file
                let invite = load_invite(server)?;

                let mut config = load_config::<Config<T>>(&config_file)?;
                config.service.rotate_server_keys(&invite)?;
                save_config(&config, &config_file)?;
                tracing::info!("Server '{}' rotated.", server.display());
                return Ok(None);
            }

            let _ = Args::command().print_help();

            return Ok(None);
        }
        Some(Commands::Encryption {
            simple,
            environment,
        }) => {
            let mut config = load_config::<Config<T>>(&config_file)?;

            match environment {
                Some(env) => {
                    config.service.set_environment_encryption(env)?;
                }
                None => {
                    if *simple {
                        config.service.set_simple_encryption()?;
                    }
                }
            }
            save_config(&config, &config_file)?;
            return Ok(None);
        }
        Some(Commands::Secret { key, value }) => {
            let mut config = load_config::<Config<T>>(&config_file)?;
            let value = config.service().encrypt(value)?;
            config.extras.insert(key.clone(), value.clone());
            save_config(&config, &config_file)?;
            return Ok(None);
        }
        Some(Commands::Extra { key, value }) => {
            let mut config = load_config::<Config<T>>(&config_file)?;
            config.extras.insert(key.clone(), value.clone());
            save_config(&config, &config_file)?;
            return Ok(None);
        }
        Some(Commands::Run {
            one_shot_commands,
            repeat,
            sender,
            zone,
        }) => {
            let mut config = load_config::<Config<T>>(&config_file)?;
            tracing::info!("Loaded config from {}", &config_file.display());

            if let Some(one_shot_commands) = one_shot_commands {
                let repeat = repeat.unwrap_or(1);
                let mut one_shot_commands = one_shot_commands.clone();
                one_shot_commands = one_shot_commands
                    .into_iter()
                    .flat_map(|cmd| std::iter::repeat_n(cmd, repeat as usize))
                    .collect();

                config.one_shot_commands = Some(one_shot_commands.clone());
                config.one_shot_sender = sender.clone();
                config.one_shot_zone = zone.clone();
            }

            return Ok(Some(config));
        }
        _ => {
            let _ = Args::command().print_help();
        }
    }

    Ok(None)
}

#[derive(Parser)]
#[command(version = version(), about, long_about = None)]
struct Args {
    #[arg(long, short = 'c', help = "Path to the configuration file")]
    config_file: Option<std::path::PathBuf>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Adding and removing clients
    Client {
        #[arg(long, short = 'a', help = "Name of a client to add to the service")]
        add: Option<String>,

        #[arg(
            long,
            short = 'r',
            help = "Name of a client to remove from the service"
        )]
        remove: Option<String>,

        #[arg(
            long,
            short = 'i',
            help = "IP address or IP range that the client can connect from"
        )]
        ip: Option<String>,

        #[arg(long, short = 'l', help = "List all clients added to the service")]
        list: bool,

        #[arg(
            long,
            short = 'z',
            help = "The communication zone to communicate with the service. Only services in the same zone can route messages"
        )]
        zone: Option<String>,

        #[arg(
            long,
            short = 'R',
            help = "Name of the client whose keys are being rotated"
        )]
        rotate: Option<String>,

        #[arg(
            long,
            short = 'p',
            help = "Name of a blind relay proxy (an op-proxy server already added to this \
                    service) this client can only be reached through - if set, --ip is not \
                    required and is ignored"
        )]
        proxy: Option<String>,

        #[arg(
            long,
            short = 't',
            help = "The agent type this client must present itself as, e.g. 'bridge'. \
                    Communicated out-of-band by the operator of that agent - it is never \
                    taken from anything the client sends. If omitted, the client's claimed \
                    type is not checked. One of: portal, provider, platform, instance, \
                    bridge, account, filesystem, scheduler, virtual"
        )]
        r#type: Option<String>,
    },

    /// Adding and removing servers
    Server {
        #[arg(
            long,
            short = 'a',
            help = "File containing an invite from a server to add to the service"
        )]
        add: Option<PathBuf>,

        #[arg(
            long,
            short = 'r',
            help = "Name of a server to remove from the service"
        )]
        remove: Option<String>,

        #[arg(long, short = 'l', help = "List all servers added to the service")]
        list: bool,

        #[arg(
            long,
            short = 'z',
            help = "The communication zone to communicate with the service. Only services in the same zone can route messages"
        )]
        zone: Option<String>,

        #[arg(
            long,
            short = 'R',
            help = "File containing the rotation invite from a server which is rotating keys"
        )]
        rotate: Option<PathBuf>,
    },

    /// Initialise the Service
    Init {
        /// Initialise the service
        #[arg(long, short = 'n', help = "Name of the service to initialise")]
        service: Option<String>,

        #[arg(
            long,
            short = 'u',
            help = "URL of the service including port and route (e.g. http://localhost:8080)"
        )]
        url: Option<String>,

        #[arg(
            long,
            short = 'i',
            help = "IP address on which to listen for connections (e.g. 127.0.0.1)"
        )]
        ip: Option<String>,

        #[arg(
            long,
            short = 'p',
            help = "Port on which to listen for connections (e.g. 8042)"
        )]
        port: Option<u16>,

        #[arg(
            long,
            short = 'k',
            help = "Optional port on which to listen for health checks (e.g. 8080)"
        )]
        healthcheck_port: Option<u16>,

        #[arg(
            long,
            short = 'x',
            help = "Proxy header to use for the client IP address - look here for the client IP address"
        )]
        proxy_header: Option<String>,

        #[arg(
            long,
            help = "IP address(es)/range(s) of trusted reverse proxies whose proxy_header may be \
                    trusted (comma-separated, CIDR allowed, e.g. 127.0.0.0/8). Required for \
                    proxy_header to have any effect."
        )]
        trusted_proxy: Option<String>,

        #[arg(long, short = 'f', help = "Force reinitialisation")]
        force: bool,
    },

    /// Add extra configuration options
    Extra {
        #[arg(long, short = 'k', help = "Key for the extra configuration option")]
        key: String,

        #[arg(long, short = 'v', help = "Value for the extra configuration option")]
        value: String,
    },

    /// Add secret configuration options
    Secret {
        #[arg(long, short = 'k', help = "Key for the secret configuration option")]
        key: String,

        #[arg(long, short = 'v', help = "Value for the secret configuration option")]
        value: String,
    },

    /// Add commands to control encryption of the config file and secrets
    Encryption {
        #[arg(
            long,
            short = 's',
            help = "Use very simple encryption. This should not be used in production."
        )]
        simple: bool,

        #[arg(
            long,
            short = 'e',
            help = "Use the value of the specified environment variable as the encryption password."
        )]
        environment: Option<String>,
    },

    /// Run the service
    Run {
        #[arg(
            long = "one-shot",
            short = 'o',
            help = "One-shot command - run the service once, execute these command(s), then exit."
        )]
        one_shot_commands: Option<Vec<String>>,
        #[arg(
            long = "repeat",
            short = 'r',
            help = "Repeat the one-shot command(s) this number of times (default: 1)."
        )]
        repeat: Option<u32>,
        #[arg(
            long = "sender",
            short = 's',
            help = "The sender to use for the one-shot command(s) (default: oneshot)."
        )]
        sender: Option<String>,
        #[arg(
            long = "zone",
            short = 'z',
            help = "The zone to use for the one-shot command(s) (default: one-shot)."
        )]
        zone: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_type_typos_are_rejected_at_add_time() {
        // A misspelled --type would otherwise be written into the config and
        // then discarded at startup as unrecognised, leaving the operator
        // believing the peer was checked when it was not. Fail while there is
        // someone there to read the error. See finding R3.
        assert_eq!(
            validate_agent_type(&Some("bridge".to_string())).ok(),
            Some(Some("bridge".to_string()))
        );

        // ...and normalise, so `--type BRIDGE` and `--type " bridge "` record
        // the same canonical value the startup check parses.
        assert_eq!(
            validate_agent_type(&Some("  BRIDGE ".to_string())).ok(),
            Some(Some("bridge".to_string()))
        );

        for bad in ["bridges", "brigde", "portal ninja", "", "Portal!"] {
            assert!(
                validate_agent_type(&Some(bad.to_string())).is_err(),
                "{:?} must be rejected",
                bad
            );
        }

        // Omitting it entirely is legitimate - it means "do not check".
        assert_eq!(validate_agent_type(&None).ok(), Some(None));
    }
}
