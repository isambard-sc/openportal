// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;

// import freeipa directory as a module
mod freeipa;
use freeipa::IPAGroup;

mod cache;

use templemeads::agent::account::{process_args, run, Defaults};
use templemeads::agent::Type as AgentType;
use templemeads::async_runnable;
use templemeads::grammar::Instruction::{AddUser, RemoveUser, UpdateHomeDir};
use templemeads::job::{Envelope, Job};
use templemeads::Error;

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
        None,
        None,
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

    // get the details about the FreeIPA server - this must be set
    let freeipa_server = config.option("freeipa-server", "");
    let freeipa_user: String = config.option("freeipa-user", "admin");
    let system_groups: Vec<IPAGroup> = IPAGroup::parse(&config.option("system-groups", ""))?;

    if freeipa_server.is_empty() {
        return Err(anyhow::anyhow!(
            "No FreeIPA server specified. Please set this in the freeipa-server option."
        ));
    }

    let freeipa_password = match config.secret("freeipa-password") {
        Some(password) => password,
        None => {
            return Err(anyhow::anyhow!(
                "No FreeIPA password specified. Please set this in the freeipa-password option.",
            ));
        }
    };

    cache::set_system_groups(&system_groups).await?;

    // connect the single shared FreeIPA client - this will be used in the
    // async function (we can't bind variables to async functions, or else
    // we would just pass the client with the environment)
    freeipa::connect(&freeipa_server, &freeipa_user, &freeipa_password).await?;

    // we need to bind the FreeIPA client into the freeipa_runner
    async_runnable! {
        ///
        /// Runnable function that will be called when a job is received
        /// by the agent
        ///
        pub async fn freeipa_runner(envelope: Envelope) -> Result<Job, templemeads::Error>
        {
            let job = envelope.job();

            match job.instruction() {
                AddUser(user) => {
                    let user = freeipa::add_user(&user).await?;
                    let job = job.completed(user.mapping()?)?;
                    Ok(job)
                },
                RemoveUser(user) => {
                    Err(Error::IncompleteCode(
                        format!("RemoveUser instruction not implemented yet - cannot remove {}", user),
                    ))
                },
                UpdateHomeDir(user, homedir) => {
                    let _ = freeipa::update_homedir(&user, &homedir).await?;
                    let job = job.completed(homedir)?;
                    Ok(job)
                },
                _ => {
                    Err(Error::InvalidInstruction(
                        format!("Invalid instruction: {}. FreeIPA only supports add_user and remove_user", job.instruction()),
                    ))
                }
            }
        }
    }

    run(config, freeipa_runner).await?;

    Ok(())
}
