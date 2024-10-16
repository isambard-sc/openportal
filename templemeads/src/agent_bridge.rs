// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent::Type as AgentType;
use crate::bridge_server::{
    spawn, Config as BridgeConfig, Defaults as BridgeDefaults, Invite as BridgeInvite,
};
use crate::error::Error;
use crate::handler::{process_message, set_service_details};

use anyhow::Context;
use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use paddington::config::{
    load as load_config, save as save_config, Defaults as ServiceDefaults, ServiceConfig,
};
use paddington::invite::{load as load_invite, save as save_invite, Invite};
use paddington::Key;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::path::PathBuf;

///
/// Run the Bridge Agent.
/// This listens for requests from the bridge http server and
/// bridges those to the other Agents in the OpenPortal system.
///
pub async fn run(config: Config) -> Result<(), Error> {
    if config.service.name().is_empty() {
        return Err(Error::Misconfigured("Service name is empty".to_string()));
    }

    if config.agent != AgentType::Bridge {
        return Err(Error::Misconfigured(
            "Service agent is not a Bridge".to_string(),
        ));
    }

    // pass the service details onto the handler
    set_service_details(&config.service.name(), &config.agent, None).await?;

    // spawn the bridge server
    spawn(config.bridge).await?;

    // now run the bridge OpenPortal agent
    paddington::set_handler(process_message).await?;
    paddington::run(config.service).await?;

    Ok(())
}

// Configuration

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Config {
    pub service: ServiceConfig,
    pub bridge: BridgeConfig,
    pub agent: AgentType,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Defaults {
    pub service: ServiceDefaults,
    pub bridge: BridgeDefaults,
}

impl Defaults {
    #[allow(clippy::too_many_arguments)]
    pub fn parse(
        name: Option<String>,
        config_file: Option<PathBuf>,
        url: Option<String>,
        ip: Option<String>,
        port: Option<u16>,
        bridge_url: Option<String>,
        bridge_ip: Option<String>,
        bridge_port: Option<u16>,
    ) -> Self {
        Self {
            service: ServiceDefaults::parse(name, config_file, url, ip, port),
            bridge: BridgeDefaults::parse(bridge_url, bridge_ip, bridge_port),
        }
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
            bridge_ip,
            bridge_port,
            force,
        }) => {
            let config = Config {
                service: {
                    ServiceConfig::parse(
                        &service.clone().unwrap_or(defaults.service.name()),
                        &url.clone().unwrap_or(defaults.service.url()),
                        &ip.clone()
                            .unwrap_or(defaults.service.ip())
                            .parse::<IpAddr>()?
                            .to_string(),
                        port.unwrap_or_else(|| defaults.service.port()),
                    )?
                },
                bridge: BridgeConfig::parse(
                    &bridge_ip.clone().unwrap_or(defaults.bridge.url()),
                    bridge_ip
                        .clone()
                        .unwrap_or(defaults.bridge.ip())
                        .parse::<IpAddr>()?,
                    bridge_port.unwrap_or_else(|| defaults.bridge.port()),
                ),
                agent: AgentType::Bridge,
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
            match list {
                true => {
                    let config = load_config::<Config>(&config_file)?;
                    for client in config.service.clients() {
                        println!("{}", client);
                    }
                    return Ok(None);
                }
                false => {}
            }

            match add {
                Some(client) => {
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
                None => {}
            }

            match remove {
                Some(client) => {
                    let mut config = load_config::<Config>(&config_file)?;
                    config.service.remove_client(client)?;
                    save_config(config, &config_file)?;
                    tracing::info!("Client '{}' removed.", client);
                    return Ok(None);
                }
                None => {}
            }
        }
        Some(Commands::Server { add, list, remove }) => {
            match list {
                true => {
                    let config = load_config::<Config>(&config_file)?;
                    for server in config.service.servers() {
                        println!("{}", server);
                    }
                    return Ok(None);
                }
                false => {}
            }

            match add {
                Some(server) => {
                    // read the invitation from the passed toml file
                    let invite = load_invite::<Invite>(server)?;
                    let mut config = load_config::<Config>(&config_file)?;
                    config.service.add_server(invite)?;
                    save_config(config, &config_file)?;
                    tracing::info!("Server '{}' added.", server.display());
                    return Ok(None);
                }
                None => {}
            }

            match remove {
                Some(server) => {
                    let mut config = load_config::<Config>(&config_file)?;
                    config.service.remove_server(server)?;
                    save_config(config, &config_file)?;
                    tracing::info!("Server '{}' removed.", server);
                    return Ok(None);
                }
                None => {
                    let _ = Args::command().print_help();
                    return Ok(None);
                }
            }
        }
        Some(Commands::Bridge { config, regenerate }) => {
            match config {
                Some(py_config_file) => {
                    let config = load_config::<Config>(&config_file)?;
                    let py_config = BridgeInvite::parse(&config.bridge.url, &config.bridge.key);
                    save_invite(py_config, py_config_file)?;
                    tracing::info!(
                        "Python configuration file written to {}",
                        py_config_file.display()
                    );
                    return Ok(None);
                }
                None => {}
            }

            match regenerate {
                true => {
                    let mut config = load_config::<Config>(&config_file)?;
                    config.bridge.key = Key::generate();
                    save_config(config, &config_file)?;
                    tracing::info!("API key regenerated.");
                    return Ok(None);
                }
                false => {}
            }
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
            short = 'b',
            help = "IP address on which to listen for bridge connections (e.g. '::')"
        )]
        bridge_ip: Option<String>,

        #[arg(
            long,
            short = 'q',
            help = "Port on which to listen for bridge connections (e.g. 3000)"
        )]
        bridge_port: Option<u16>,

        #[arg(long, short = 'f', help = "Force reinitialisation")]
        force: bool,
    },

    /// Handling connections to the bridge API webserver
    Bridge {
        #[arg(
            long,
            short = 'c',
            help = "File name in which to write the configuration file for a Python client that wants to connect to the bridge."
        )]
        config: Option<std::path::PathBuf>,

        #[arg(
            long,
            short = 'r',
            help = "Re-generate the API key used by bridge clients to connect to the service. Note you will need to generate a new configuration file for any Python clients."
        )]
        regenerate: bool,
    },

    /// Run the service
    Run {},
}
