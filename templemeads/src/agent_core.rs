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
use paddington::invite::{load as load_invite, save as save_invite, Invite};
use secrecy::Secret;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;

// Configuration

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Config {
    service: ServiceConfig,
    agent: AgentType,

    #[serde(default)]
    extras: HashMap<String, String>,
}

impl Config {
    pub fn new(service: ServiceConfig, agent: AgentType) -> Self {
        Self {
            service,
            agent,
            extras: HashMap::new(),
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

    pub fn secret(&self, key: &str) -> Option<Secret<String>> {
        match self.extras.get(key) {
            Some(value) => match self.service.decrypt::<String>(value) {
                Ok(secret) => Some(Secret::<String>::new(secret)),
                Err(e) => {
                    tracing::error!("Failed to decrypt secret for key '{}': {:?}", key, e);
                    None
                }
            },
            None => None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Defaults {
    pub service: ServiceDefaults,
    pub agent: AgentType,
    pub extras: HashMap<String, String>,
}

impl Defaults {
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
pub async fn process_args(defaults: &Defaults) -> Result<Option<Config>, Error> {
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
            force,
        }) => {
            let local_healthcheck_port;

            if let Some(healthcheck_port) = healthcheck_port {
                local_healthcheck_port = Some(*healthcheck_port);
            } else {
                local_healthcheck_port = defaults.service.healthcheck_port();
            }

            let config = Config {
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
                extras: defaults.extras.clone(),
            };

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
            save_config(config, &config_file)?;

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
        }) => {
            if *list {
                let config = load_config::<Config>(&config_file)?;
                for client in config.service.clients() {
                    println!("{}", client);
                }
                return Ok(None);
            }

            if let Some(client) = add {
                if ip.is_none() {
                    return Err(Error::PeerEdit(format!(
                        "No IP address or IP range provided for client {}.",
                        client
                    )));
                }

                let mut config = load_config::<Config>(&config_file)?;

                let invite = config
                    .service
                    .add_client(client, &ip.clone().unwrap_or_else(|| "".to_string()))?;

                save_config(config, &config_file)?;
                save_invite(invite, &PathBuf::from(format!("./invite_{}.toml", client)))?;

                tracing::info!("Client '{}' added.", client);
                return Ok(None);
            }

            if let Some(client) = remove {
                let mut config = load_config::<Config>(&config_file)?;
                config.service.remove_client(client)?;
                save_config(config, &config_file)?;
                tracing::info!("Client '{}' removed.", client);
                return Ok(None);
            }

            let _ = Args::command().print_help();

            return Ok(None);
        }
        Some(Commands::Server { add, list, remove }) => {
            if *list {
                let config = load_config::<Config>(&config_file)?;
                for server in config.service.servers() {
                    println!("{}", server);
                }
                return Ok(None);
            }

            if let Some(server) = add {
                // read the invitation from the passed toml file
                let invite = load_invite::<Invite>(server)?;
                let mut config = load_config::<Config>(&config_file)?;
                config.service.add_server(invite)?;
                save_config(config, &config_file)?;
                tracing::info!("Server '{}' added.", server.display());
                return Ok(None);
            }

            if let Some(server) = remove {
                let mut config = load_config::<Config>(&config_file)?;
                config.service.remove_server(server)?;
                save_config(config, &config_file)?;
                tracing::info!("Server '{}' removed.", server);
                return Ok(None);
            }

            let _ = Args::command().print_help();

            return Ok(None);
        }
        Some(Commands::Encryption {
            simple,
            environment,
        }) => {
            let mut config = load_config::<Config>(&config_file)?;

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
            save_config(config, &config_file)?;
            return Ok(None);
        }
        Some(Commands::Secret { key, value }) => {
            let mut config = load_config::<Config>(&config_file)?;
            let value = config.service().encrypt(value)?;
            config.extras.insert(key.clone(), value.clone());
            save_config(config, &config_file)?;
            return Ok(None);
        }
        Some(Commands::Extra { key, value }) => {
            let mut config = load_config::<Config>(&config_file)?;
            config.extras.insert(key.clone(), value.clone());
            save_config(config, &config_file)?;
            return Ok(None);
        }
        Some(Commands::Run {}) => {
            let config = load_config::<Config>(&config_file)?;
            tracing::info!("Loaded config from {}", &config_file.display());
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
    Run {},
}
