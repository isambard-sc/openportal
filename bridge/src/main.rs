// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;

use templemeads::agent::bridge::{process_args, run, Defaults};

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
    templemeads::config::initialise_tracing();

    // create the OpenPortal paddington defaults
    let defaults = Defaults::parse(
        Some("bridge".to_owned()),
        Some(
            dirs::config_local_dir()
                .unwrap_or(
                    ".".parse()
                        .expect("Could not parse fallback config directory."),
                )
                .join("openportal")
                .join("bridge-config.toml"),
        ),
        Some("ws://localhost:8044".to_owned()),
        Some("127.0.0.1".to_owned()),
        Some(8044),
        None,
        None,
        Some("http://localhost:3000".to_owned()),
        Some("127.0.0.1".to_owned()),
        Some(3000),
    );

    // now parse the command line arguments to get the service configuration
    let config = match process_args(&defaults).await? {
        Some(config) => config,
        None => {
            // Not running the service, so can safely exit
            return Ok(());
        }
    };

    // run the Bridge agent
    run(config).await?;

    Ok(())
}
