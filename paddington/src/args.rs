// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Context;
use anyhow::Error as AnyError;
use clap::CommandFactory;
use clap::{Parser, Subcommand};
use thiserror::Error;
use url::Url;

use crate::config::{ConfigError, Invite, ServiceConfig};

#[derive(Error, Debug)]
pub enum ArgsError {
    #[error("{0}")]
    IOError(#[from] std::io::Error),

    #[error("{0}")]
    ConfigError(#[from] ConfigError),

    #[error("{0}")]
    AnyError(#[from] AnyError),

    #[error("{0}")]
    ServiceNameError(String),

    #[error("{0}")]
    ConfigExistsError(String),

    #[error("{0}")]
    PeerEditError(String),

    #[error("Unknown arguments error")]
    Unknown,
}

pub mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

fn version() -> &'static str {
    built_info::GIT_VERSION.unwrap_or(built_info::PKG_VERSION)
}

#[derive(Debug)]
pub struct ArgDefaults {
    pub service_name: Option<String>,
    pub config_file: Option<std::path::PathBuf>,
}

impl ArgDefaults {
    pub fn new(service_name: Option<String>, config_file: Option<std::path::PathBuf>) -> Self {
        Self {
            service_name,
            config_file,
        }
    }

    pub fn default_config_file(&self) -> std::path::PathBuf {
        dirs::config_local_dir()
            .unwrap_or(
                ".".parse()
                    .expect("Could not parse fallback config directory."),
            )
            .join("openportal")
            .join(match self.config_file {
                Some(ref path) => path.clone(),
                None => "service.toml"
                    .parse()
                    .expect("Could not parse default config file."),
            })
    }

    pub fn default_service_name(&self) -> String {
        match self.service_name.clone() {
            Some(name) => name,
            None => "default_service".to_string(),
        }
    }
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
        add: Option<String>,

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
            help = "URL of the service (e.g. https://localhost:8080)"
        )]
        url: Option<Url>,

        #[arg(long, short = 'f', help = "Force reinitialisation")]
        force: bool,
    },

    /// Run the service
    Run {},
}

pub enum ProcessResult {
    ServiceConfig(ServiceConfig),
    Invite(Invite),
    Message(String),
    None,
}

pub async fn process_args(defaults: &ArgDefaults) -> Result<ProcessResult, ArgsError> {
    let args = Args::parse();

    let config_file = match args.config_file {
        Some(path) => path,
        None => defaults.default_config_file(),
    };

    // see if we need to initialise the config directory
    match &args.command {
        Some(Commands::Init {
            service,
            url,
            force,
        }) => {
            let service_name = match service {
                Some(name) => name.clone(),
                None => defaults.default_service_name(),
            };

            if service_name.is_empty() {
                return Err(ArgsError::ServiceNameError(
                    "No service name provided.".to_string(),
                ));
            }

            if config_file.try_exists()? {
                if *force {
                    std::fs::remove_file(&config_file)
                        .context("Could not remove existing config file.")?;
                } else {
                    return Err(ArgsError::ConfigExistsError(format!(
                        "Config file already exists: {}",
                        &config_file.display()
                    )));
                }
            }

            ServiceConfig::create(&config_file, &service_name, url)?;
            return Ok(ProcessResult::Message("Service initialised.".to_string()));
        }
        Some(Commands::Client {
            add,
            ip,
            list,
            remove,
        }) => {
            match list {
                true => {
                    let config = ServiceConfig::load(&config_file)?;
                    let clients = config.get_clients();
                    for client in clients {
                        println!("{}", client);
                    }
                    return Ok(ProcessResult::None);
                }
                false => {}
            }

            match add {
                Some(client) => {
                    if ip.is_none() {
                        return Err(ArgsError::PeerEditError(format!(
                            "No IP address or IP range provided for client {}.",
                            client
                        )));
                    }

                    let mut config = ServiceConfig::load(&config_file)?;

                    let invite =
                        config.add_client(client, &ip.clone().unwrap_or_else(|| "".to_string()))?;

                    config.save(&config_file)?;

                    return Ok(ProcessResult::Invite(invite));
                }
                None => {}
            }

            match remove {
                Some(client) => {
                    let mut config = ServiceConfig::load(&config_file)?;
                    config.remove_client(client)?;
                    config.save(&config_file)?;
                    return Ok(ProcessResult::Message(format!(
                        "Client '{}' removed.",
                        client
                    )));
                }
                None => {
                    return Ok(ProcessResult::Message(
                        "You need to either add '-a' or remove '-r' a client.".to_string(),
                    ));
                }
            }
        }
        Some(Commands::Server { add, list, remove }) => {
            match list {
                true => {
                    let config = ServiceConfig::load(&config_file)?;
                    let servers = config.get_servers();
                    for server in servers {
                        println!("{}", server);
                    }
                    return Ok(ProcessResult::None);
                }
                false => {}
            }

            match add {
                Some(server) => {
                    // read the invitation from the passed toml file
                    let invite = Invite::load(server)?;
                    let mut config = ServiceConfig::load(&config_file)?;
                    config.add_server(invite)?;
                    config.save(&config_file)?;
                    return Ok(ProcessResult::Message(format!(
                        "Server '{}' added.",
                        server
                    )));
                }
                None => {}
            }

            match remove {
                Some(server) => {
                    let mut config = ServiceConfig::load(&config_file)?;
                    config.remove_server(server)?;
                    config.save(&config_file)?;
                    return Ok(ProcessResult::Message(format!(
                        "Server '{}' removed.",
                        server
                    )));
                }
                None => {
                    return Ok(ProcessResult::Message(
                        "You need to either add '-a' or remove '-r' a server.".to_string(),
                    ));
                }
            }
        }
        Some(Commands::Run {}) => {
            let config = ServiceConfig::load(&config_file)?;
            return Ok(ProcessResult::ServiceConfig(config));
        }
        _ => {
            let _ = Args::command().print_help();
        }
    }

    Ok(ProcessResult::None)
}
