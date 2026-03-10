// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;
use chrono::Utc;
use std::collections::HashMap;

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

    // The managed group distinguishes users created by this agent from
    // pre-existing system users.  All managed users are added to it.
    let managed_group = config.option("managed-group", "openportal");

    // Optional extra groups added to every managed user, regardless of instance.
    // Format: comma-separated group names, e.g. "users,staff"
    let system_groups = parse_group_list(&config.option("system-groups", ""));

    // Optional extra groups added per instance.
    // Format: comma-separated "instance:group" pairs, e.g.
    //   "slurmcluster:slurm,slurmcluster:mpi,othercluster:gpu"
    let instance_groups = parse_instance_groups(&config.option("instance-groups", ""));

    localaccount::initialise_commands(localaccount::Commands::new(
        &useradd,
        &userdel,
        &groupadd,
        &groupdel,
        &usermod,
        &getent,
        &managed_group,
        system_groups,
        instance_groups,
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
                    let mapping = localaccount::add_user(&user, &sender, &Some(homedir), job.expires()).await?;
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

///
/// Parse a comma-separated list of group names.
/// e.g. "users,staff,wheel" → vec!["users", "staff", "wheel"]
///
fn parse_group_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(|g| g.trim().to_owned())
        .filter(|g| !g.is_empty())
        .collect()
}

///
/// Parse instance-specific group assignments.
/// Format: "instance:group" pairs separated by commas.
/// e.g. "slurmcluster:slurm,slurmcluster:mpi,gpu-cluster:cuda"
/// → {"slurmcluster": ["slurm", "mpi"], "gpu-cluster": ["cuda"]}
///
fn parse_instance_groups(s: &str) -> HashMap<String, Vec<String>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for entry in s.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        if let Some(colon) = entry.find(':') {
            let instance = entry[..colon].trim().to_owned();
            let group = entry[colon + 1..].trim().to_owned();
            if !instance.is_empty() && !group.is_empty() {
                map.entry(instance).or_default().push(group);
            }
        }
    }
    map
}
