// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;

use templemeads::agent::portal::{process_args, run, Defaults};
use templemeads::agent::Type as AgentType;
use templemeads::async_runnable;
use templemeads::job::{Envelope, Job};
use templemeads::Error;

use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    templemeads::config::initialise_tracing();

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
    run(config, portal_runner).await?;

    Ok(())
}

async_runnable! {
    ///
    /// Runnable function that is called when the portal needs
    /// to issue a job
    ///
    pub async fn portal_runner(envelope: Envelope) -> Result<Job, Error>
    {
        let job = envelope.job();

        tracing::error!("Unknown instruction: {:?}", job.instruction());
        return Err(Error::UnknownInstruction(
            format!("Unknown instruction: {:?}", job.instruction()).to_string(),
        ));
    }
}
