// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;

use templemeads::agent::portal::{process_args, run, Defaults, Type as AgentType};

///
/// Main function for the bridge application
///
/// The purpose of this application is to bridge between the user portal
/// (e.g. Waldur) and OpenPortal.
///
/// It does this by providing a "Client" agent in OpenPortal that can be
/// used to make requests over the OpenPortal protocol.
///
/// It also provides a web API that can be called by the user portal to
/// submit and get information about those requests. This API is designed
/// to be called via, e.g. the openportal Python client.
///
#[tokio::main]
async fn main() -> Result<()> {
    // start tracing
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber)?;

    // create the OpenPortal paddington defaults
    let defaults = Defaults::parse(
        Some("portal".to_owned()),
        Some(
            dirs::config_local_dir()
                .unwrap_or(
                    ".".parse()
                        .expect("Could not parse fallback config directory."),
                )
                .join("openportal")
                .join("portal-config.toml"),
        ),
        Some("ws://localhost:8040".to_owned()),
        Some("127.0.0.1".to_owned()),
        Some(8040),
        Some(AgentType::Portal),
    );

    // now parse the command line arguments to get the service configuration
    let config = match process_args(&defaults).await? {
        Some(config) => config,
        None => {
            // Not running the service, so can safely exit
            return Ok(());
        }
    };

    // run the portal agent
    run(config).await?;

    Ok(())
}
