// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;

use templemeads::agent::portal::{process_args, run, Defaults};
use templemeads::agent::Type as AgentType;

use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber)?;

    // create the default options for a portal
    let defaults = Defaults::parse(
        Some("portal".to_owned()),
        Some(PathBuf::from("example-portal.toml")),
        Some("ws://localhost:8090".to_owned()),
        Some("127.0.0.1".to_owned()),
        Some(8090),
        None,
        None,
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
