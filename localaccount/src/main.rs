// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;
use chrono::Utc;

mod localaccount;

use templemeads::agent::account::{process_args, run, Defaults};
use templemeads::agent::{Peer, Type as AgentType};
use templemeads::async_runnable;
use templemeads::grammar::Instruction::{
    AddProject, AddUser, GetProjectMapping, GetProjects, GetUserMapping, GetUsers,
    IsExistingProject, IsExistingUser, IsProtectedUser, RemoveProject, RemoveUser, UpdateHomeDir,
};
use templemeads::grammar::UserMapping;
use templemeads::job::{assert_not_expired, Envelope, Job};
use templemeads::Error;

///
/// Main function for the localaccount agent.
///
/// This agent implements the Account agent interface using standard Unix
/// commands (useradd, groupadd, etc.).  Each command is configurable so
/// that, for example, commands can be prefixed with "docker exec slurmctld"
/// to run inside a container without requiring local root access.
///
#[tokio::main]
async fn main() -> Result<()> {
    // start tracing
    templemeads::config::initialise_tracing();

    // start system monitoring
    templemeads::spawn_system_monitor();

    // create the OpenPortal paddington defaults
    let defaults: Defaults = Defaults::parse(
        Some("localaccount".to_owned()),
        Some(
            dirs::config_local_dir()
                .unwrap_or(
                    ".".parse()
                        .expect("Could not parse fallback config directory."),
                )
                .join("openportal")
                .join("localaccount-config.toml"),
        ),
        Some("ws://localhost:8047".to_owned()),
        Some("127.0.0.1".to_owned()),
        Some(8047),
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

    // Read the configurable command strings. Each defaults to the standard
    // system binary name, which works when running as root locally. For
    // containerised use, set e.g.:
    //   useradd = "docker exec slurmctld useradd"
    let useradd = config.option("useradd", "useradd");
    let userdel = config.option("userdel", "userdel");
    let groupadd = config.option("groupadd", "groupadd");
    let groupdel = config.option("groupdel", "groupdel");
    let usermod = config.option("usermod", "usermod");
    let getent = config.option("getent", "getent");

    // The managed group is used to distinguish users created by this agent
    // from pre-existing system users. All managed users are added to it.
    let managed_group = config.option("managed-group", "openportal");

    localaccount::initialise_commands(localaccount::Commands::new(
        &useradd,
        &userdel,
        &groupadd,
        &groupdel,
        &usermod,
        &getent,
        &managed_group,
    ))?;

    async_runnable! {
        ///
        /// Runnable function that will be called when a job is received
        /// by the agent
        ///
        pub async fn localaccount_runner(envelope: Envelope) -> Result<Job, templemeads::Error>
        {
            let job = envelope.job();
            let sender = envelope.sender();
            let me = envelope.recipient();

            match job.instruction() {
                GetProjects(portal) => {
                    let mappings = localaccount::get_groups(&portal, job.expires()).await?;
                    job.completed(mappings)
                },
                AddProject(project) => {
                    let mapping = localaccount::add_project(&project, job.expires()).await?;
                    job.completed(mapping)
                },
                RemoveProject(project) => {
                    let mapping = localaccount::remove_project(&project, job.expires()).await?;
                    job.completed(mapping)
                },
                GetUsers(project) => {
                    let mappings = localaccount::get_users(&project, job.expires()).await?;
                    job.completed(mappings)
                },
                AddUser(user) => {
                    let local_user = localaccount::identifier_to_userid(&user);
                    let local_group = localaccount::get_primary_group_name(&user);
                    let mapping = UserMapping::new(&user, &local_user, &local_group)?;
                    let homedir = get_home_dir(me.name(), &sender, &mapping, job.expires()).await?;
                    let mapping = localaccount::add_user(&user, &Some(homedir), job.expires()).await?;
                    job.completed(mapping)
                },
                RemoveUser(user) => {
                    let mapping = localaccount::remove_user(&user, job.expires()).await?;
                    job.completed(mapping)
                },
                UpdateHomeDir(user, homedir) => {
                    localaccount::update_homedir(&user, &homedir, job.expires()).await?;
                    job.completed(homedir)
                },
                GetProjectMapping(project) => {
                    let mapping = localaccount::get_project_mapping(&project, job.expires()).await?;
                    job.completed(mapping)
                },
                GetUserMapping(user) => {
                    let mapping = localaccount::get_user_mapping(&user, job.expires()).await?;
                    job.completed(mapping)
                },
                IsProtectedUser(user) => {
                    let is_protected = localaccount::is_protected_user(&user, job.expires()).await?;
                    job.completed(is_protected)
                },
                IsExistingUser(user) => {
                    let exists = localaccount::is_existing_user(&user, job.expires()).await?;
                    job.completed(exists)
                },
                IsExistingProject(project) => {
                    let exists = localaccount::is_existing_project(&project, job.expires()).await?;
                    job.completed(exists)
                },
                _ => {
                    Err(Error::InvalidInstruction(
                        format!("Invalid instruction: {}. LocalAccount only supports account management instructions", job.instruction()),
                    ))
                }
            }
        }
    }

    run(config, localaccount_runner).await?;

    Ok(())
}

///
/// Ask the instance agent for the home directory that should be assigned
/// to this user. This follows the same protocol used by op-freeipa.
///
async fn get_home_dir(
    me: &str,
    sender: &Peer,
    mapping: &UserMapping,
    expires: &chrono::DateTime<Utc>,
) -> Result<String, Error> {
    assert_not_expired(expires)?;

    let job = Job::parse(
        &format!("{}.{} get_local_home_dir {}", me, sender.name(), mapping),
        false,
    )?;

    let job = job.put(sender).await?;

    assert_not_expired(expires)?;

    let mut home_dir = job.wait().await?.result::<String>()?;

    assert_not_expired(expires)?;

    while home_dir.is_none() {
        let job = job.wait().await?;
        assert_not_expired(expires)?;
        home_dir = job.result::<String>()?;
    }

    if let Some(homedir) = home_dir {
        Ok(homedir)
    } else {
        Err(Error::InvalidInstruction(
            "No home directory found".to_string(),
        ))
    }
}
