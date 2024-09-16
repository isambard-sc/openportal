// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;

// import freeipa directory as a module
mod freeipa;
use freeipa::IPAGroup;

mod db;

use templemeads::agent::account::{process_args, run, Defaults};
use templemeads::agent::Type as AgentType;
use templemeads::async_runnable;
use templemeads::grammar::Instruction::{AddUser, RemoveUser};
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
    let freeipa_password: String = config.option("freeipa-password", "");
    let system_groups: Vec<IPAGroup> = IPAGroup::parse(&config.option("system-groups", ""))?;

    if freeipa_server.is_empty() {
        return Err(anyhow::anyhow!(
            "No FreeIPA server specified. Please set this in the freeipa-server option."
        ));
    }

    if freeipa_password.is_empty() {
        return Err(anyhow::anyhow!(
            "No FreeIPA password specified. Please set this in the freeipa-password option."
        ));
    }

    db::set_system_groups(system_groups).await?;

    // connect the single shared FreeIPA client - this will be used in the
    // async function (we can't bind variables to async functions, or else
    // we would just pass the client with the environment)
    freeipa::connect(&freeipa_server, &freeipa_user, &freeipa_password).await?;

    // now get all of the users from FreeIPA that are part of the OpenPortal
    // group (these are the users that we are managing)
    let users = freeipa::get_users_in_group("openportal").await?;

    // add all of the users to the database
    db::set_existing_users(users).await?;

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
                    // have we already added this user? - check the list
                    let user = freeipa::add_user(&user).await?;

                    tracing::info!("User {:?} added", user);

                    // update the job with the new user
                    job.completed(format!("User {:?} added", user))?;

                    Ok(job)
                },
                RemoveUser(user) => {
                    Err(Error::IncompleteCode(
                        format!("RemoveUser instruction not implemented yet - cannot remove {}", user),
                    ))
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
