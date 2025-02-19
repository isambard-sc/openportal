// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;
use std::collections::HashMap;

// import freeipa directory as a module
mod freeipa;
use freeipa::IPAGroup;

mod cache;

use templemeads::agent::account::{process_args, run, Defaults};
use templemeads::agent::{Peer, Type as AgentType};
use templemeads::async_runnable;
use templemeads::grammar::Instruction::{
    AddProject, AddUser, GetProjectMapping, GetProjects, GetUserMapping, GetUsers, IsProtectedUser,
    RemoveProject, RemoveUser, UpdateHomeDir,
};
use templemeads::grammar::UserMapping;
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
    templemeads::config::initialise_tracing();

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
    let system_groups: Vec<IPAGroup> =
        IPAGroup::parse_system_groups(&config.option("system-groups", ""))?;
    let instance_groups: HashMap<Peer, Vec<IPAGroup>> =
        IPAGroup::parse_instance_groups(&config.option("instance-groups", ""))?;

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
    cache::set_instance_groups(&instance_groups).await?;

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
            let sender = envelope.sender();
            let me = envelope.recipient();

            match job.instruction() {
                GetProjects(portal) => {
                    let groups = freeipa::get_groups(&portal).await?;
                    job.completed(groups.iter().map(|g| g.mapping()).collect::<Result<Vec<_>, _>>()?)
                },
                AddProject(project) => {
                    let project = freeipa::add_project(&project).await?;
                    job.completed(project.mapping()?)
                },
                RemoveProject(project) => {
                    let project = freeipa::remove_project(&project, &sender).await?;
                    job.completed(project.mapping()?)
                },
                GetUsers(project) => {
                    let users = freeipa::get_users(&project, &sender).await?;
                    job.completed(users.iter().map(|u| u.mapping()).collect::<Result<Vec<_>, _>>()?)
                },
                AddUser(user) => {
                    let local_user = freeipa::identifier_to_userid(&user).await?;
                    let local_group = freeipa::get_primary_group_name(&user).await?;
                    let mapping = UserMapping::new(&user, &local_user, &local_group)?;

                    let homedir = get_home_dir(me.name(), &sender, &mapping).await?;

                    if homedir.is_none() {
                        tracing::warn!("No home directory preferred for user: {}", user);
                    }

                    let user = freeipa::add_user(&user, &sender, &homedir).await?;
                    job.completed(user.mapping()?)
                },
                RemoveUser(user) => {
                    let user = freeipa::remove_user(&user, &sender).await?;
                    job.completed(user.mapping()?)
                },
                UpdateHomeDir(user, homedir) => {
                    let _ = freeipa::update_homedir(&user, &homedir).await?;
                    job.completed(homedir)
                },
                GetProjectMapping(project) => {
                    let mapping = freeipa::get_project_mapping(&project).await?;
                    job.completed(mapping)
                },
                GetUserMapping(user) => {
                    let mapping = freeipa::get_user_mapping(&user).await?;
                    job.completed(mapping)
                },
                IsProtectedUser(user) => {
                    let is_protected = freeipa::is_protected_user(&user).await?;
                    job.completed(is_protected)
                }
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

async fn get_home_dir(
    me: &str,
    sender: &Peer,
    mapping: &UserMapping,
) -> Result<Option<String>, Error> {
    let job = Job::parse(
        &format!("{}.{} get_local_home_dir {}", me, sender.name(), mapping),
        false,
    )?;

    let job = job.put(sender).await?;

    // wait for the job to complete - get the result
    job.wait().await?.result::<String>()
}
