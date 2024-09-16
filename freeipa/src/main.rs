// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;

// import freeipa directory as a module
mod freeipa;
use freeipa::FreeIPA;

use templemeads::agent::account::{process_args, run, Defaults};
use templemeads::agent::Type as AgentType;
use templemeads::async_runnable;
use templemeads::job::{Envelope, Job};

///
/// Main function for the freeipa-account application
///
/// The main purpose of this program is to relay account creation and
/// deletion instructions to freeipa, and to provide a way to query the
/// status of accounts.
///
#[tokio::main]
async fn main() -> Result<()> {
    // start tracing
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber)?;

    tracing::info!("FreeIPA account service starting up...");
    let freeipa = FreeIPA::connect("https://ipa.demo1.freeipa.org", "admin", "Secret123").await?;

    let user = freeipa.user("admin").await?;

    tracing::info!("User: {:?}", user);

    let users = freeipa.users_in_group("admins").await?;

    tracing::info!("Users in the admins group: {:?}", users);

    // get a list of all users in the "openportal" group
    let users = freeipa.users_in_group("openportal").await?;
    tracing::info!("Users in the openportal group: {:?}", users);

    let user = freeipa.user("chris").await?;

    tracing::info!("User: {:?}", user);

    let users = freeipa.users().await?;

    tracing::info!("All users: {:?}", users);

    let groups = freeipa.groups().await?;

    tracing::info!("All groups: {:?}", groups);

    // create the OpenPortal paddington defaults
    let defaults = Defaults::parse(
        Some("freeipa".to_owned()),
        Some(
            dirs::config_local_dir()
                .unwrap_or(
                    ".".parse()
                        .expect("Could not parse fallback config directory."),
                )
                .join("openportal")
                .join("freeipa-config.toml"),
        ),
        Some("ws://localhost:8046".to_owned()),
        Some("127.0.0.1".to_owned()),
        Some(8046),
        Some(AgentType::Account),
    );

    // now parse the command line arguments to get the service configuration
    let config = match process_args(&defaults).await? {
        Some(config) => config,
        None => {
            // Not running the service, so can safely exit
            return Ok(());
        }
    };

    run(config, freeipa_runner).await?;

    Ok(())
}

async_runnable! {
    ///
    /// Runnable function that will be called when a job is received
    /// by the agent
    ///
    pub async fn freeipa_runner(envelope: Envelope) -> Result<Job, templemeads::Error>
    {
        tracing::info!("Using the freeipa runner for job from {} to {}", envelope.sender(), envelope.recipient());
        let result = envelope.job().execute().await?;

        Ok(result)
    }
}
