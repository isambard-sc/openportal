// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;

use templemeads::agent::platform::{process_args, run, Defaults};
use templemeads::agent::Type as AgentType;

///
/// Main function for the cluster platform agent
///
/// This purpose of this agent is to manage clusters, defined
/// as HPC batch clusters. It will manage the lifecycle of
/// the cluster, including creating and deleting the cluster
///
#[tokio::main]
async fn main() -> Result<()> {
    // start tracing
    templemeads::config::initialise_tracing();

    // create the OpenPortal paddington defaults
    let defaults = Defaults::parse(
        Some("clusters".to_owned()),
        Some(
            dirs::config_local_dir()
                .unwrap_or(
                    ".".parse()
                        .expect("Could not parse fallback config directory."),
                )
                .join("openportal")
                .join("clusters-config.toml"),
        ),
        Some("ws://localhost:8045".to_owned()),
        Some("127.0.0.1".to_owned()),
        Some(8045),
        None,
        None,
        Some(AgentType::Platform),
    );

    // now parse the command line arguments to get the service configuration
    let config = match process_args(&defaults).await? {
        Some(config) => config,
        None => {
            // Not running the service, so can safely exit
            return Ok(());
        }
    };

    // run the agent
    run(config).await?;

    Ok(())
}
