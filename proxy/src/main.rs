// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! `op-proxy` - a blind relay proxy for OpenPortal agents that can only
//! make outbound connections (e.g. neither side of a pair can open a
//! listening port reachable by the other). See
//! `docs/plans/archive/blind-relay-proxy-design.md` for the full design.
//!
//! Unlike every other `op-*` agent, this binary depends only on
//! `paddington`, not `templemeads` - it has no Jobs, no Boards, no
//! `Domain`, and never sees plaintext. It relays [`paddington::relay::RelayEnvelope`]
//! payloads between the agents connected to it, exactly as configured by
//! its [`paddington::relay::RelayPolicy`] - nothing more.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use serde::{Deserialize, Serialize};

use paddington::config::{self, ServiceConfig};
use paddington::invite::Invite;
use paddington::relay::{self, RelayPolicy};

mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

fn version() -> &'static str {
    built_info::GIT_VERSION.unwrap_or(built_info::PKG_VERSION)
}

fn default_config_file() -> PathBuf {
    dirs::config_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("openportal")
        .join("proxy-config.toml")
}

/// The proxy's config file - the `ServiceConfig` (name, url, servers,
/// clients, ...) flattened at the top level exactly as any other agent's
/// config, plus a `[policy]` table holding the `RelayPolicy` (the
/// `(from, to)` allow-list `allow` manages) - one file, like every other
/// agent, rather than a separate policy file to keep track of.
#[derive(Serialize, Deserialize, Clone, Debug)]
struct ProxyConfig {
    #[serde(flatten)]
    service: ServiceConfig,

    #[serde(default)]
    policy: RelayPolicy,
}

#[derive(Parser)]
#[command(version = version(), about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialise the proxy service
    Init {
        #[arg(long, short = 'u', help = "URL of the proxy, including port and route")]
        url: Option<String>,

        #[arg(
            long,
            short = 'i',
            help = "IP address on which to listen for connections"
        )]
        ip: Option<String>,

        #[arg(long, short = 'p', help = "Port on which to listen for connections")]
        port: Option<u16>,

        #[arg(long, short = 'c', help = "Path to the config file to create")]
        config_file: Option<PathBuf>,
    },

    /// Introduce an agent that will connect to this proxy directly (i.e.
    /// an ordinary paddington client of the proxy - one of the two real
    /// hops in a relayed pair). This is *not* the relay policy itself -
    /// see the `allow` subcommand for which pairs of introduced agents may
    /// actually be relayed between each other.
    Client {
        #[arg(long, short = 'a', help = "Name of the agent to introduce")]
        add: String,

        #[arg(
            long,
            short = 'i',
            help = "IP address or CIDR range the agent will connect from (required)"
        )]
        ip: Option<String>,

        #[arg(long, short = 'z', help = "Zone the agent connects in")]
        zone: Option<String>,

        #[arg(
            long,
            short = 'o',
            help = "Path to write the invitation file for the agent"
        )]
        invitation: Option<PathBuf>,

        #[arg(long, short = 'c', help = "Path to the config file")]
        config_file: Option<PathBuf>,
    },

    /// Allow two introduced agents to be relayed between each other.
    /// Default-deny: no pair is relayed unless explicitly allowed here.
    Allow {
        #[arg(help = "First agent name")]
        a: String,

        #[arg(help = "Second agent name")]
        b: String,

        #[arg(long, short = 'c', help = "Path to the config file")]
        config_file: Option<PathBuf>,
    },

    /// Run the proxy
    Run {
        #[arg(long, short = 'c', help = "Path to the config file")]
        config_file: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    initialise_tracing();

    let args: Args = Args::parse();

    match &args.command {
        Some(Commands::Init {
            url,
            ip,
            port,
            config_file,
        }) => {
            init(
                url.clone().unwrap_or("http://localhost:8060".to_string()),
                ip.clone().unwrap_or("127.0.0.1".to_string()),
                port.unwrap_or(8060),
                config_file.clone().unwrap_or_else(default_config_file),
            )?;
        }
        Some(Commands::Client {
            add,
            ip,
            zone,
            invitation,
            config_file,
        }) => {
            let ip = ip.clone().ok_or_else(|| {
                anyhow::anyhow!("No IP address or IP range provided for client '{}'.", add)
            })?;

            add_client(
                add,
                ip,
                zone.clone(),
                invitation
                    .clone()
                    .unwrap_or_else(|| PathBuf::from(format!("invite_{}.toml", add))),
                config_file.clone().unwrap_or_else(default_config_file),
            )?;
        }
        Some(Commands::Allow { a, b, config_file }) => {
            allow(
                a,
                b,
                config_file.clone().unwrap_or_else(default_config_file),
            )?;
        }
        Some(Commands::Run { config_file }) => {
            run(config_file.clone().unwrap_or_else(default_config_file)).await?;
        }
        _ => {
            let _ = Args::command().print_help();
        }
    }

    Ok(())
}

fn initialise_tracing() {
    use tracing_subscriber::prelude::*;

    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "INFO");
    }

    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .with(tracing_subscriber::fmt::layer());

    if tracing::subscriber::set_global_default(subscriber).is_err() {
        tracing::warn!("Tracing subscriber already set - ignoring.");
    }
}

fn init(url: String, ip: String, port: u16, config_file: PathBuf) -> Result<()> {
    if config_file.try_exists()? {
        return Err(anyhow::anyhow!(
            "Config file already exists: {:?}",
            config_file
        ));
    }

    let service = ServiceConfig::new("proxy", &url, &ip, &port, &None, &None)
        .with_context(|| "Could not create proxy service config")?;

    let config = ProxyConfig {
        service,
        policy: RelayPolicy::new(),
    };

    config::save(&config, &config_file)
        .with_context(|| format!("Could not save proxy config to {:?}", config_file))?;

    println!(
        "Proxy initialised. Config file written to {:?}",
        config_file
    );
    println!("Service name: {}", config.service.name());

    Ok(())
}

fn add_client(
    name: &str,
    ip: String,
    zone: Option<String>,
    invitation: PathBuf,
    config_file: PathBuf,
) -> Result<()> {
    let mut config: ProxyConfig = config::load(&config_file)
        .with_context(|| format!("Could not load proxy config from {:?}", config_file))?;

    // A relayed peer is addressed by name alone, so this proxy must not connect two
    // peers with the same name even in different zones. `ServiceConfig::add_client`
    // only rejects a duplicate (name, zone), so check the name here - the operator
    // finds out now rather than at the far side's first envelope. See
    // docs/specifications/security-review-2.md (finding R33).
    if let Some(existing) = config.service.clients().iter().find(|c| c.name() == name) {
        return Err(anyhow::anyhow!(
            "This proxy already has a client named '{}' (in zone '{}'). Relayed peers \
             are addressed by name alone, so a proxy cannot connect two peers with the \
             same name - choose a different name.",
            name,
            existing.zone()
        ));
    }

    let invite: Invite = config
        .service
        // A proxy is a blind relay, not an agent: it has no `agent::Type` of its
        // own to declare, and it never inspects the roles of the peers it
        // relays between - its authorization is the explicit `allow` pair list.
        // So no agent type is recorded here or written into the invite.
        .add_client(name, &ip, &zone, &None)
        .with_context(|| format!("Could not add client '{}'", name))?;

    // Write the invite *first*. Saving the config first meant a failed invite write
    // left the proxy holding a client entry - and a freshly generated key pair - that
    // the peer could never receive, so the operator had to notice and remove it by
    // hand. See docs/specifications/security-review-2.md (finding R33).
    invite
        .save(&invitation)
        .with_context(|| format!("Could not save invitation to {:?}", invitation))?;

    config::save(&config, &config_file)
        .with_context(|| format!("Could not save proxy config to {:?}", config_file))?;

    println!(
        "Client '{}' introduced. Invitation written to {:?} - \
         give this file to '{}' so it can connect to this proxy.",
        name, invitation, name
    );

    Ok(())
}

fn allow(a: &str, b: &str, config_file: PathBuf) -> Result<()> {
    let mut config: ProxyConfig = config::load(&config_file)
        .with_context(|| format!("Could not load proxy config from {:?}", config_file))?;

    // Trim, and require both names to be clients this proxy actually knows. An
    // untrimmed or misspelled name previously produced a policy entry that silently
    // matched nothing, so the operator believed a pair was allowed when it was not.
    // See docs/specifications/security-review-2.md (finding R33).
    let a = a.trim();
    let b = b.trim();

    if a.is_empty() || b.is_empty() {
        return Err(anyhow::anyhow!("Both agent names must be non-empty"));
    }

    if a == b {
        return Err(anyhow::anyhow!(
            "Cannot allow '{}' to be relayed to itself",
            a
        ));
    }

    let known: Vec<String> = config.service.clients().iter().map(|c| c.name()).collect();

    for name in [a, b] {
        if !known.iter().any(|k| k == name) {
            return Err(anyhow::anyhow!(
                "'{}' is not a client of this proxy, so it cannot be relayed. Add it \
                 first with `client --add {}`. Known clients: {}",
                name,
                name,
                match known.is_empty() {
                    true => "(none)".to_string(),
                    false => known.join(", "),
                }
            ));
        }
    }

    if config.policy.permits(a, b) {
        println!("'{}' and '{}' are already allowed to be relayed.", a, b);
        return Ok(());
    }

    config.policy.allow(a, b);

    config::save(&config, &config_file)
        .with_context(|| format!("Could not save proxy config to {:?}", config_file))?;

    println!(
        "'{}' and '{}' may now be relayed between each other via this proxy.",
        a, b
    );

    Ok(())
}

async fn run(config_file: PathBuf) -> Result<()> {
    let config: ProxyConfig = config::load(&config_file)
        .with_context(|| format!("Could not load proxy config from {:?}", config_file))?;

    relay::set_proxy_policy(config.policy).await;
    relay::configure_proxy(&config.service)
        .await
        .with_context(|| "This proxy's client configuration cannot be relayed")?;

    paddington::set_handler(relay::proxy_handler).await?;

    tracing::info!("Starting op-proxy '{}'", config.service.name());

    paddington::run(config.service).await?;

    Ok(())
}
