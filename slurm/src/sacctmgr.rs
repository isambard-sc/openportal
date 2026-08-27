// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Context;
use anyhow::Result;
use chrono::Utc;
use greatwestern::grammar::{DateRange, ProjectMapping, UserMapping};
use greatwestern::usagereport::{DailyProjectUsageReport, ProjectUsageReport, Usage};
use once_cell::sync::Lazy;
use rand::seq::IteratorRandom;
use rand::SeedableRng;
use std::sync::Arc;
use templemeads::job::assert_not_expired;
use templemeads::Error;
use tokio::sync::Mutex;

use crate::cache;
use crate::slurm::{
    clean_account_name, clean_user_name, get_managed_organization, SlurmAccount, SlurmLimit,
    SlurmUser,
};
use crate::slurm::{SlurmJob, SlurmNodes};

#[derive(Debug, Clone)]
struct SlurmRunner {
    sacct: String,
    sacctmgr: String,
    scontrol: String,
    scancel: String,
}

impl Default for SlurmRunner {
    fn default() -> Self {
        SlurmRunner {
            sacct: "sacct".to_string(),
            sacctmgr: "sacctmgr".to_string(),
            scontrol: "scontrol".to_string(),
            scancel: "scancel".to_string(),
        }
    }
}

static SLURM_RUNNERS: Lazy<Mutex<Vec<Arc<Mutex<SlurmRunner>>>>> =
    Lazy::new(|| Mutex::new(Vec::new()));

// Priority runners are used for time-sensitive commands like adding/removing users,
// getting/setting limits, etc. These are kept separate from the main runners to
// ensure they are not blocked by long-running usage report queries.
static PRIORITY_RUNNERS: Lazy<Mutex<Vec<Arc<Mutex<SlurmRunner>>>>> =
    Lazy::new(|| Mutex::new(Vec::new()));

#[derive(Debug)]
pub struct LockedRunner {
    runner: tokio::sync::OwnedMutexGuard<SlurmRunner>,
}

impl LockedRunner {
    pub fn sacct(&self) -> &str {
        &self.runner.sacct
    }

    pub fn sacctmgr(&self) -> &str {
        &self.runner.sacctmgr
    }

    pub fn scontrol(&self) -> &str {
        &self.runner.scontrol
    }

    pub fn scancel(&self) -> &str {
        &self.runner.scancel
    }

    /// Build a command safely from a vector of arguments
    /// This is the preferred method to avoid command injection
    ///
    /// Handles composite commands (e.g., "docker exec slurmctld sacctmgr") by splitting
    /// the binary string and treating each part as a separate argument
    pub fn build_command(&self, cmd_type: &str, args: Vec<String>) -> Result<Vec<String>, Error> {
        let binary_str = match cmd_type {
            "SACCTMGR" => self.sacctmgr(),
            "SCONTROL" => self.scontrol(),
            "SACCT" => self.sacct(),
            "SCANCEL" => self.scancel(),
            _ => {
                return Err(Error::Call(format!(
                    "Unknown command type: {}. Must be SACCTMGR, SCONTROL, SACCT, or SCANCEL",
                    cmd_type
                )));
            }
        };

        // Split the binary string to handle composite commands like "docker exec slurmctld sacctmgr"
        // Use shlex to properly handle quoted arguments in the command
        let binary_parts = match shlex::split(binary_str) {
            Some(parts) if !parts.is_empty() => parts,
            _ => {
                return Err(Error::Call(format!(
                    "Could not parse command binary: {}",
                    binary_str
                )));
            }
        };

        let mut command = binary_parts;
        command.extend(args);

        // remove any empty arguments
        command.retain(|arg| !arg.trim().is_empty());

        Ok(command)
    }

    pub async fn run(
        &self,
        cmd: &Vec<String>,
        timeout: std::time::Duration,
    ) -> Result<String, Error> {
        if cmd.is_empty() {
            return Err(Error::Call("Empty command vector".to_string()));
        }

        tracing::debug!("Running command: {:?}", cmd);

        let start_time = chrono::Utc::now();
        let Some((program, program_args)) = cmd.split_first() else {
            return Err(Error::Call("Empty command vector".to_string()));
        };

        let output = tokio::process::Command::new(program)
            .args(program_args)
            .kill_on_drop(true)
            .output();

        // use a tokio timeout to ensure we won't block indefinitely
        let output = match tokio::time::timeout(timeout, output).await {
            Ok(output) => output,
            Err(_) => {
                tracing::error!(
                    "Command {:?} timed out after {:?} seconds",
                    cmd,
                    timeout.as_secs()
                );
                return Err(Error::Timeout("Command timed out".to_string()));
            }
        };

        let end_time = chrono::Utc::now();

        let duration_ms = (end_time - start_time).num_milliseconds();

        if duration_ms > 5000 {
            tracing::warn!(
                "Running command {:?} took {} seconds",
                cmd,
                duration_ms as f64 / 1000.0
            );
        }

        let output = match output {
            Ok(output) => output,
            Err(e) => {
                tracing::error!("Could not run command {:?}: {}", cmd, e);
                return Err(Error::Call("Could not run command".to_string()));
            }
        };

        if output.status.success() {
            let output = match String::from_utf8(output.stdout.clone()) {
                Ok(output) => output,
                Err(e) => {
                    tracing::error!("Could not parse output: {}", e);
                    tracing::error!("Output: {:?}", output.stdout);
                    return Err(Error::Call("Could not parse output".to_string()));
                }
            };

            Ok(output)
        } else {
            tracing::error!(
                "Command {:?} failed: {}",
                cmd,
                String::from_utf8(output.stderr.clone()).context("Could not parse error")?
            );
            Err(Error::Call(format!(
                "Command {:?} failed: {}",
                cmd,
                String::from_utf8(output.stderr).context("Could not parse error")?
            )))
        }
    }

    pub async fn run_json(
        &self,
        cmd: &Vec<String>,
        timeout: std::time::Duration,
    ) -> Result<serde_json::Value, Error> {
        let output = self.run(cmd, timeout).await?;

        let start_time = chrono::Utc::now();
        match serde_json::from_str(&output) {
            Ok(output) => {
                let end_time = chrono::Utc::now();
                let duration_ms = (end_time - start_time).num_milliseconds();

                if duration_ms > 5000 {
                    tracing::warn!(
                        "Parsing JSON output of command '{:?}' took {} seconds",
                        cmd,
                        duration_ms as f64 / 1000.0
                    );
                }
                Ok(output)
            }
            Err(e) => {
                tracing::error!("Could not parse json: {}", e);
                tracing::error!("Output: {:?}", output);
                Err(Error::Call("Could not parse json".to_string()))
            }
        }
    }
}

/// The default timeout (30 seconds)
pub const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

// function to return the runner protected by a MutexGuard - this ensures
// that we can only run a small number of slurm commands at a time, thereby not
// overloading the server
pub async fn runner(expires: &chrono::DateTime<Utc>) -> Result<LockedRunner, Error> {
    let runners = SLURM_RUNNERS.lock().await;

    if runners.is_empty() {
        return Err(Error::Call(
            "No Slurm runners have been configured".to_string(),
        ));
    }

    let mut rng = rand::rngs::StdRng::from_os_rng();

    loop {
        // try all the runners in a random order
        for runner in runners.iter().choose_multiple(&mut rng, runners.len()) {
            assert_not_expired(expires)?;

            match runner.clone().try_lock_owned() {
                Ok(guard) => {
                    return Ok(LockedRunner { runner: guard });
                }
                Err(_) => {
                    // the runner is already locked, so try the next one
                    continue;
                }
            }
        }

        // wait a bit before trying again
        assert_not_expired(expires)?;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

// function to return a priority runner - used for time-sensitive commands
// like adding/removing users, getting/setting limits, etc.
pub async fn priority_runner(expires: &chrono::DateTime<Utc>) -> Result<LockedRunner, Error> {
    let runners = PRIORITY_RUNNERS.lock().await;

    if runners.is_empty() {
        return Err(Error::Call(
            "No priority Slurm runners have been configured".to_string(),
        ));
    }

    let mut rng = rand::rngs::StdRng::from_os_rng();

    loop {
        // try all the runners in a random order
        for runner in runners.iter().choose_multiple(&mut rng, runners.len()) {
            assert_not_expired(expires)?;

            match runner.clone().try_lock_owned() {
                Ok(guard) => {
                    return Ok(LockedRunner { runner: guard });
                }
                Err(_) => {
                    // the runner is already locked, so try the next one
                    continue;
                }
            }
        }

        // wait a bit before trying again
        assert_not_expired(expires)?;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

async fn force_add_slurm_account(
    account: &SlurmAccount,
    expires: &chrono::DateTime<Utc>,
) -> Result<SlurmAccount, Error> {
    if account.organization() != get_managed_organization() {
        tracing::warn!(
            "Account {} is not managed by the openportal organization - we cannot manage it.",
            account
        );
        return Err(Error::UnmanagedGroup(format!(
            "Cannot add Slurm account as {} is not managed by openportal",
            account
        )));
    }

    // get the cluster name from the cache
    let cluster = cache::get_cluster().await?;

    // get the parent account name from the cache
    let parent_account = cache::get_parent_account().await?;

    let cmd = priority_runner(expires).await?.build_command(
        "SACCTMGR",
        vec![
            "--immediate".to_string(),
            "add".to_string(),
            "account".to_string(),
            format!("name={}", account.name()),
            format!("cluster={}", cluster),
            format!("parent={}", parent_account),
            format!("organization={}", account.organization()),
            format!("description={}", account.description()),
        ],
    )?;

    priority_runner(expires)
        .await?
        .run(&cmd, DEFAULT_TIMEOUT)
        .await?;

    Ok(account.clone())
}

async fn get_account_from_slurm(
    account: &str,
    expires: &chrono::DateTime<Utc>,
) -> Result<Option<SlurmAccount>, Error> {
    let account = clean_account_name(account)?;

    let cluster = cache::get_cluster().await?;

    let cmd = priority_runner(expires).await?.build_command(
        "SACCTMGR",
        vec![
            "--json".to_string(),
            "list".to_string(),
            "accounts".to_string(),
            "withassoc".to_string(),
            format!("name={}", account),
            format!("cluster={}", cluster),
        ],
    )?;

    let response = match priority_runner(expires)
        .await?
        .run_json(&cmd, DEFAULT_TIMEOUT)
        .await
    {
        Ok(response) => response,
        Err(e) => {
            tracing::warn!("Could not get account {}: {}", account, e);
            return Ok(None);
        }
    };

    // there should be an accounts list, with a single entry for this account
    let accounts = match response.get("accounts") {
        Some(accounts) => accounts,
        None => {
            tracing::warn!("Could not get accounts from response: {:?}", response);
            return Ok(None);
        }
    };

    // this should be an array
    let accounts = match accounts.as_array() {
        Some(accounts) => accounts,
        None => {
            tracing::warn!("Accounts is not an array: {:?}", accounts);
            return Ok(None);
        }
    };

    // there should be an Account object in this array with the right name
    let slurm_account = accounts.iter().find(|a| {
        let name = a.get("name").and_then(|n| n.as_str());
        name == Some(&account)
    });

    let account = match slurm_account {
        Some(account) => account,
        None => {
            tracing::warn!(
                "Could not find account '{}' in response: {:?}",
                account,
                response
            );
            return Ok(None);
        }
    };

    match SlurmAccount::construct(account) {
        Ok(account) => Ok(Some(account)),
        Err(e) => {
            tracing::warn!("Could not construct account from response: {}", e);
            Ok(None)
        }
    }
}

async fn get_account(
    account: &str,
    expires: &chrono::DateTime<Utc>,
) -> Result<Option<SlurmAccount>, Error> {
    // need to GET /slurm/vX.Y.Z/accounts/{account.name}
    // and return the account if it exists
    let cached_account = cache::get_account(account).await?;

    if let Some(cached_account) = cached_account {
        // double-check that the account actually exists...
        let existing_account = match get_account_from_slurm(cached_account.name(), expires).await {
            Ok(account) => account,
            Err(e) => {
                tracing::warn!("Could not get account {}: {}", cached_account.name(), e);
                cache::remove_account(cached_account.name()).await?;
                return Ok(None);
            }
        };

        if let Some(existing_account) = existing_account {
            if cached_account != existing_account {
                tracing::warn!(
                    "Account {} exists, but with different details.",
                    cached_account.name()
                );
                tracing::warn!(
                    "Existing: {:?}, new: {:?}",
                    existing_account,
                    cached_account
                );

                // only this account is known to be stale - see cache::remove_account
                cache::remove_account(cached_account.name()).await?;

                // store the new account
                cache::add_account(&existing_account).await?;

                return Ok(Some(existing_account));
            } else {
                return Ok(Some(cached_account));
            }
        } else {
            // the account doesn't exist
            tracing::warn!(
                "Account {} does not exist - it has been removed from slurm.",
                cached_account.name()
            );
            cache::remove_account(cached_account.name()).await?;
            return Ok(None);
        }
    }

    // see if we can read the account from slurm
    let account = match get_account_from_slurm(account, expires).await {
        Ok(account) => account,
        Err(e) => {
            tracing::warn!("Could not get account {}: {}", account, e);
            return Ok(None);
        }
    };

    if let Some(account) = account {
        cache::add_account(&account).await?;
        Ok(Some(account))
    } else {
        Ok(None)
    }
}

async fn get_account_create_if_not_exists(
    account: &SlurmAccount,
    expires: &chrono::DateTime<Utc>,
) -> Result<SlurmAccount, Error> {
    let existing_account = get_account(account.name(), expires).await?;

    let cluster = cache::get_cluster().await?;

    if let Some(existing_account) = existing_account {
        if existing_account.in_cluster(&cluster) {
            if !account.is_managed() {
                tracing::warn!(
                    "Account {} is not managed by the openportal organization.",
                    account
                );
            }

            tracing::debug!("Using existing slurm account {}", existing_account);
            return Ok(existing_account);
        }
    }

    // it doesn't, so create it
    tracing::info!("Creating new slurm account: {}", account.name());
    let account = force_add_slurm_account(account, expires).await?;

    // get the account as created
    match get_account(account.name(), expires).await {
        Ok(Some(account)) => Ok(account),
        Ok(None) => {
            tracing::error!("Could not get account {}", account.name());
            Err(Error::NotFound(account.name().to_string()))
        }
        Err(e) => {
            tracing::error!("Could not get account {}: {}", account.name(), e);
            Err(e)
        }
    }
}

async fn get_user_from_slurm(
    user: &str,
    expires: &chrono::DateTime<Utc>,
) -> Result<Option<SlurmUser>, Error> {
    let user = clean_user_name(user)?;
    let cluster = cache::get_cluster().await?;

    let cmd = priority_runner(expires).await?.build_command(
        "SACCTMGR",
        vec![
            "--json".to_string(),
            "list".to_string(),
            "users".to_string(),
            "WithAssoc".to_string(),
            format!("name={}", user),
            format!("cluster={}", cluster),
        ],
    )?;

    let response = priority_runner(expires)
        .await?
        .run_json(&cmd, DEFAULT_TIMEOUT)
        .await?;

    // there should be a users list, with a single entry for this user
    let users = match response.get("users") {
        Some(users) => users,
        None => {
            tracing::warn!("Could not get users from response: {:?}", response);
            return Ok(None);
        }
    };

    // this should be an array
    let users = match users.as_array() {
        Some(users) => users,
        None => {
            tracing::warn!("Users is not an array: {:?}", users);
            return Ok(None);
        }
    };

    // there should be an User object in this array with the right name
    let slurm_user = users.iter().find(|u| {
        let name = u.get("name").and_then(|n| n.as_str());
        name == Some(&user)
    });

    let user = match slurm_user {
        Some(user) => user,
        None => {
            tracing::warn!("Could not find user '{}' in response: {:?}", user, response);
            return Ok(None);
        }
    };

    match SlurmUser::construct(user) {
        Ok(user) => Ok(Some(user)),
        Err(e) => {
            tracing::warn!("Could not construct user from response: {}", e);
            Ok(None)
        }
    }
}

async fn get_user(user: &str, expires: &chrono::DateTime<Utc>) -> Result<Option<SlurmUser>, Error> {
    let cached_user = cache::get_user(user).await?;

    if let Some(cached_user) = cached_user {
        // double-check that the user actually exists...
        let existing_user = match get_user_from_slurm(cached_user.name(), expires).await {
            Ok(user) => user,
            Err(e) => {
                tracing::warn!("Could not get user {}: {}", cached_user.name(), e);
                cache::remove_user(cached_user.name()).await?;
                return Ok(None);
            }
        };

        if let Some(existing_user) = existing_user {
            if cached_user != existing_user {
                tracing::warn!(
                    "User {} exists, but with different details.",
                    cached_user.name()
                );
                tracing::warn!("Existing: {:?}, new: {:?}", existing_user, cached_user);

                // only this user is known to be stale - see cache::remove_user
                cache::remove_user(cached_user.name()).await?;

                // store the new user
                cache::add_user(&existing_user).await?;

                return Ok(Some(existing_user));
            } else {
                return Ok(Some(cached_user));
            }
        } else {
            // the user doesn't exist
            tracing::warn!(
                "User {} does not exist - it has been removed from slurm.",
                cached_user.name()
            );
            cache::remove_user(cached_user.name()).await?;
            return Ok(None);
        }
    }

    // see if we can read the user from slurm
    let user = match get_user_from_slurm(user, expires).await {
        Ok(user) => user,
        Err(e) => {
            tracing::warn!("Could not get user {}: {}", user, e);
            return Ok(None);
        }
    };

    if let Some(user) = user {
        cache::add_user(&user).await?;
        Ok(Some(user))
    } else {
        Ok(None)
    }
}

async fn add_account_association(
    account: &SlurmAccount,
    expires: &chrono::DateTime<Utc>,
) -> Result<(), Error> {
    // eventually should check to see if this association already exists,
    // and if so, not to do anything else

    if account.organization() != get_managed_organization() {
        tracing::warn!(
            "Account {} is not managed by the openportal organization - we cannot manage it.",
            account
        );
        return Err(Error::UnmanagedGroup(format!(
            "Cannot add Slurm account as {} is not managed by openportal",
            account
        )));
    }

    // get the cluster name from the cache
    let cluster = cache::get_cluster().await?;

    // get the parent account name from the cache
    let parent_account = cache::get_parent_account().await?;

    let cmd = priority_runner(expires).await?.build_command(
        "SACCTMGR",
        vec![
            "--immediate".to_string(),
            "add".to_string(),
            "account".to_string(),
            format!("name={}", account.name()),
            format!("Clusters={}", cluster),
            format!("parent={}", parent_account),
            format!("Associations={}", account.name()),
            "Comment=Created by OpenPortal".to_string(),
        ],
    )?;

    priority_runner(expires)
        .await?
        .run(&cmd, DEFAULT_TIMEOUT)
        .await?;

    Ok(())
}

async fn add_user_association(
    user: &SlurmUser,
    account: &SlurmAccount,
    make_default: bool,
    expires: &chrono::DateTime<Utc>,
) -> Result<SlurmUser, Error> {
    if !account.is_managed() {
        tracing::error!(
            "Account {} is not managed by the openportal organization!",
            account
        );
    }

    let mut user = user.clone();
    let mut user_changed = false;
    let cluster = cache::get_cluster().await?;

    if user
        .associations()
        .iter()
        .any(|a| a.account() == account.name() && a.cluster() == cluster)
    {
        // the association already exists
        tracing::debug!(
            "User {} already associated with account {} in cluster {}",
            user.name(),
            account.name(),
            cluster
        );
    } else {
        // create the account association first
        add_account_association(account, expires).await?;

        // add the association
        let cmd = priority_runner(expires).await?.build_command(
            "SACCTMGR",
            vec![
                "--immediate".to_string(),
                "add".to_string(),
                "user".to_string(),
                format!("name={}", user.name()),
                format!("Clusters={}", cluster),
                format!("Accounts={}", account.name()),
                "Comment=Created by OpenPortal".to_string(),
            ],
        )?;

        priority_runner(expires)
            .await?
            .run(&cmd, DEFAULT_TIMEOUT)
            .await?;

        // update the user
        user = match get_user_from_slurm(user.name(), expires).await? {
            Some(user) => user,
            None => {
                return Err(Error::Call(format!(
                    "Could not get user that just had its associations updated! '{}'",
                    user.name()
                )))
            }
        };

        user_changed = true;

        tracing::debug!("Updated user: {}", user);
    }

    if make_default && *user.default_account() != Some(account.name().to_string()) {
        tracing::debug!("Will set user default account here");

        let cmd = priority_runner(expires).await?.build_command(
            "SACCTMGR",
            vec![
                "--immediate".to_string(),
                "add".to_string(),
                "user".to_string(),
                format!("name={}", user.name()),
                format!("Clusters={}", cluster),
                format!("DefaultAccount={}", account.name()),
                "Comment=Updated by OpenPortal".to_string(),
            ],
        )?;

        priority_runner(expires)
            .await?
            .run(&cmd, DEFAULT_TIMEOUT)
            .await?;

        // update the user
        user = match get_user_from_slurm(user.name(), expires).await? {
            Some(user) => user,
            None => {
                return Err(Error::Call(format!(
                    "Could not get user that just had its default account updated! '{}'",
                    user.name()
                )))
            }
        };

        user_changed = true;
    }

    if user_changed {
        // now cache the updated user
        cache::add_user(&user).await?;
    } else {
        tracing::debug!("Using existing user: {}", user);
    }

    Ok(user)
}

async fn get_user_create_if_not_exists(
    user: &UserMapping,
    expires: &chrono::DateTime<Utc>,
) -> Result<SlurmUser, Error> {
    // first, make sure that the account exists
    let slurm_account = get_account_create_if_not_exists(
        &SlurmAccount::from_mapping(&user.clone().into())?,
        expires,
    )
    .await?;

    let cluster = cache::get_cluster().await?;

    // now get the user from slurm
    let slurm_user = get_user(user.local_user().unix()?, expires).await?;

    if let Some(slurm_user) = slurm_user {
        // the user exists - check that the account is associated with the user
        if *slurm_user.default_account() == Some(slurm_account.name().to_string())
            && slurm_user
                .associations()
                .iter()
                .any(|a| a.account() == slurm_account.name() && a.cluster() == cluster)
        {
            tracing::debug!("Using existing user {}", slurm_user);
            return Ok(slurm_user);
        } else {
            tracing::warn!(
                "User {} exists, but is not default associated with the requested account '{}' in cluster {}.",
                user,
                slurm_account,
                cluster
            );
        }
    }

    // first, create the user
    let username = clean_user_name(user.local_user().unix()?)?;
    let account = clean_account_name(slurm_account.name())?;

    let cluster = cache::get_cluster().await?;

    let cmd = priority_runner(expires).await?.build_command(
        "SACCTMGR",
        vec![
            "--immediate".to_string(),
            "add".to_string(),
            "user".to_string(),
            format!("name={}", username),
            format!("Clusters={}", cluster),
            format!("Accounts={}", account),
            format!("DefaultAccount={}", account),
            "Comment=Created by OpenPortal".to_string(),
        ],
    )?;

    priority_runner(expires)
        .await?
        .run(&cmd, DEFAULT_TIMEOUT)
        .await?;

    // now load the user from slurm to make sure it exists
    let slurm_user = match get_user(user.local_user().unix()?, expires).await? {
        Some(user) => user,
        None => {
            return Err(Error::Call(format!(
                "Could not get user that was just created! '{}'",
                user.local_user()
            )))
        }
    };

    // now add the association to the account, making it the default
    let slurm_user = add_user_association(&slurm_user, &slurm_account, true, expires).await?;

    let user = SlurmUser::from_mapping(user)?;

    // check we have the user we expected
    if slurm_user != user {
        tracing::warn!("User {} exists, but with different details.", user.name());
        tracing::warn!("Existing: {:?}, new: {:?}", slurm_user, user);
    }

    Ok(slurm_user)
}

pub async fn set_commands(
    sacct: &str,
    sacctmgr: &str,
    scontrol: &str,
    scancel: &str,
    max_slurm_runners: u64,
) {
    tracing::debug!(
        "Using command line slurmd commands: sacctmgr: {}, scontrol: {}, scancel: {}, max_slurm_runners: {}",
        sacctmgr,
        scontrol,
        scancel,
        max_slurm_runners
    );

    // make sure we have at least one runner
    let max_slurm_runners = max_slurm_runners.max(1);

    let mut runners = SLURM_RUNNERS.lock().await;

    runners.clear();

    for _ in 0..max_slurm_runners {
        runners.push(Arc::new(Mutex::new(SlurmRunner {
            sacct: sacct.to_string(),
            sacctmgr: sacctmgr.to_string(),
            scontrol: scontrol.to_string(),
            scancel: scancel.to_string(),
        })));
    }

    // Also set up priority runners for time-sensitive commands
    // that should not be blocked by usage queries
    let mut priority_runners = PRIORITY_RUNNERS.lock().await;

    priority_runners.clear();

    for _ in 0..max_slurm_runners {
        priority_runners.push(Arc::new(Mutex::new(SlurmRunner {
            sacct: sacct.to_string(),
            sacctmgr: sacctmgr.to_string(),
            scontrol: scontrol.to_string(),
            scancel: scancel.to_string(),
        })));
    }
}

pub async fn find_cluster() -> Result<(), Error> {
    // now get the requested cluster from the cache
    let requested_cluster = cache::get_option_cluster().await?;

    let expires = chrono::Utc::now() + chrono::Duration::minutes(1);

    // ask slurm for all of the clusters
    let cmd = priority_runner(&expires).await?.build_command(
        "SACCTMGR",
        vec![
            "--noheader".to_string(),
            "--parsable2".to_string(),
            "list".to_string(),
            "clusters".to_string(),
        ],
    )?;

    let clusters = priority_runner(&expires)
        .await?
        .run(&cmd, DEFAULT_TIMEOUT)
        .await?;

    // the output is the list of clusters, one per line, separated by '|', where
    // the cluster name is the first column
    let clusters: Vec<String> = clusters
        .lines()
        .map(|line| line.split('|').next().unwrap_or_default().to_string())
        .collect();

    tracing::debug!("Clusters: {:?}", clusters);

    if let Some(requested_cluster) = requested_cluster {
        if clusters.contains(&requested_cluster) {
            tracing::debug!("Using requested cluster: {}", requested_cluster);
        } else {
            tracing::warn!(
                "Requested cluster {} not found in list of clusters: {:?}",
                requested_cluster,
                clusters
            );
            return Err(Error::Login("Requested cluster not found".to_string()));
        }
    } else {
        let Some(default_cluster) = clusters.first() else {
            return Err(Error::Login(
                "sacctmgr reported no clusters at all - cannot pick a default".to_string(),
            ));
        };

        tracing::debug!(
            "Using the first cluster available by default: {}",
            default_cluster
        );
        cache::set_cluster(default_cluster).await?;
    }

    Ok(())
}

pub async fn add_project(
    project: &ProjectMapping,
    expires: &chrono::DateTime<Utc>,
) -> Result<(), Error> {
    assert_not_expired(expires)?;

    let account = SlurmAccount::from_mapping(project)?;

    let account = get_account_create_if_not_exists(&account, expires).await?;

    tracing::info!("Added account: {}", account);

    Ok(())
}

pub async fn add_user(user: &UserMapping, expires: &chrono::DateTime<Utc>) -> Result<(), Error> {
    assert_not_expired(expires)?;

    let user: SlurmUser = get_user_create_if_not_exists(user, expires).await?;

    tracing::info!("Added user: {}", user);

    Ok(())
}

///
/// The totals accumulated alongside a `DailyProjectUsageReport`, kept so that
/// what the report says about itself can be checked against what we counted.
///
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReportTotals {
    usage: u64,
    num_jobs: u64,
    wait_seconds: u64,
    runtime_seconds: u64,
    expansion_jobs: u64,
    requeue_usage: u64,
    requeue_events: u64,
    requeue_wait_seconds: u64,
    /// Set when a record counted as a job in this window had not finished when
    /// we asked, so its runtime is not yet final. Such a window must not be
    /// frozen - see `record_job`.
    saw_unfinished_job: bool,
}

impl ReportTotals {
    /// True if a job counted in this window had not finished when we asked, so
    /// its runtime and expansion factor are not in the report. The agent uses
    /// this to decline to cache the window; a tool that reads Slurm directly
    /// uses it to say so in its output.
    pub fn saw_unfinished_job(&self) -> bool {
        self.saw_unfinished_job
    }
}

///
/// Accumulate one Slurm accounting record into `report`.
///
/// A record describing an attempt superseded by a requeue goes into the
/// report's requeue figures; every other record goes into the figures we have
/// always reported. Keeping the two apart is the whole point of requeue
/// accounting - see `docs/plans/slurm-requeue-accounting-design.md`.
///
/// Usage is accumulated for every record overlapping the window, since the
/// record has already been clipped to it. Job and event *counts*, and the wait
/// times that go with them, are accumulated only for records that started
/// inside the window, so that an attempt spanning several windows is counted
/// once rather than once per window.
///
pub fn record_job(
    report: &mut DailyProjectUsageReport,
    job: &SlurmJob,
    window_start: &chrono::DateTime<Utc>,
    totals: &mut ReportTotals,
) {
    let usage = job.billed_node_seconds();
    let wait_seconds = job.wait_time().num_seconds().max(0) as u64;

    // A record that is still running when the window ends reappears in the next
    // window, so counting a *job* needs this guard or a long job is counted
    // once per window it touches.
    let started_in_window = job.original_start_time() >= window_start;

    if job.is_requeued_attempt() {
        let state = job.terminal_state();

        report.add_requeue_usage(job.user(), Usage::new(usage));
        report.add_requeue_state_usage(state, Usage::new(usage));
        totals.requeue_usage = totals.requeue_usage.saturating_add(usage);

        report.add_requeue_component_usage("cpu", job.user(), Usage::new(job.cpu_seconds()));
        report.add_requeue_component_usage("memory", job.user(), Usage::new(job.memory_seconds()));
        report.add_requeue_component_usage("gpu", job.user(), Usage::new(job.gpu_seconds()));
        report.add_requeue_component_usage(
            "billing",
            job.user(),
            Usage::new(job.billing_seconds()),
        );

        // A requeue event needs no such guard, and applying one is actively
        // wrong: a superseded attempt is classified `Requeued` in *at most one*
        // window, so counting every one of them counts each event exactly once.
        //
        // Why at most one. A record is only returned for windows it overlaps,
        // so it can be seen at all only up to the window holding its end. It is
        // only classified `Requeued` when a later attempt is in the same
        // response, and a later attempt cannot start before this one ended - so
        // the window must also reach the successor's start, which is at or
        // after this record's end. The two conditions meet in exactly one
        // window: the one holding the end, which is the moment of the requeue.
        //
        // Requiring the record to have *started* in that window as well asked
        // for something almost no real requeue can satisfy. The attempts that
        // get requeued are the long ones - a job near its wall-clock limit -
        // so the requeue lands on the day after the attempt began, and the two
        // conditions could not both hold. The count came out as very nearly
        // zero while the usage it was counting was correct.
        //
        // The one case still missed is a requeue within seconds of a window
        // boundary, where the successor is submitted on the far side of it and
        // the two records never appear in one response. The count is a lower
        // bound to that extent; nothing is ever counted twice.
        report.add_requeue_events(job.user(), state, 1);
        report.add_requeue_wait_seconds(job.user(), wait_seconds);
        totals.requeue_events = totals.requeue_events.saturating_add(1);
        totals.requeue_wait_seconds = totals.requeue_wait_seconds.saturating_add(wait_seconds);

        // A superseded attempt occupied the reservation's nodes exactly as its
        // replacement did, so it counts towards what the reservation held. The
        // discarded share is recorded alongside it so the two can be separated.
        if job.is_reserved() {
            report.add_reservation_usage(job.reservation(), job.user(), Usage::new(usage));
            report.add_reservation_requeue_usage(job.reservation(), job.user(), Usage::new(usage));
        }

        return;
    }

    report.add_usage(job.user(), Usage::new(usage));
    totals.usage = totals.usage.saturating_add(usage);

    report.add_component_usage("cpu", job.user(), Usage::new(job.cpu_seconds()));
    report.add_component_usage("memory", job.user(), Usage::new(job.memory_seconds()));
    report.add_component_usage("gpu", job.user(), Usage::new(job.gpu_seconds()));
    report.add_component_usage("billing", job.user(), Usage::new(job.billing_seconds()));

    if job.is_reserved() {
        report.add_reservation_usage(job.reservation(), job.user(), Usage::new(usage));
    }

    if started_in_window {
        report.add_jobs(job.user(), 1);
        report.add_wait_seconds(job.user(), wait_seconds);
        totals.num_jobs = totals.num_jobs.saturating_add(1);
        totals.wait_seconds = totals.wait_seconds.saturating_add(wait_seconds);

        // The expansion factor is queue time over runtime, so it uses the job's
        // whole runtime rather than the part that fell inside this window - the
        // ratio is a property of the job, like the wait it is divided into.
        //
        // Which is why a record that has not finished contributes neither. Its
        // `elapsed` is the time it has been running so far, and unlike usage -
        // which this window records its own share of and the next window
        // records the rest - the runtime and the ratio are recorded once, here,
        // and never revisited. Recording them from a job three hours into a
        // thirty-hour run would freeze a runtime of three hours and an
        // expansion factor an order of magnitude too high. The job is still
        // counted, and still contributes its wait, which is already final; the
        // caller declines to cache a window that reaches this branch, so on a
        // later pass the record has finished and the real figures are recorded
        // then.
        let runtime_seconds = job.total_duration().num_seconds().max(0) as u64;

        if job.has_ended() {
            report.add_expansion(job.user(), wait_seconds, runtime_seconds);
            totals.runtime_seconds = totals.runtime_seconds.saturating_add(runtime_seconds);
            if runtime_seconds > 0 {
                totals.expansion_jobs = totals.expansion_jobs.saturating_add(1);
            }
        } else {
            totals.saw_unfinished_job = true;
        }

        // The cores and GPUs the job actually got, not what it asked for - one
        // job's worth however long it ran, so the mean describes the shape of
        // the jobs rather than what the machine was busy with.
        report.add_job_size(job.user(), job.cpus(), job.gpus());

        if job.is_reserved() {
            // counted as `num_jobs` is, so a job spanning several windows is one
            // job in the reservation rather than one per window
            report.add_reservation_jobs(job.reservation(), 1);
        }
    }
}

///
/// Report a node that Slurm blamed for losing a job.
///
/// This is deliberately loud: a node failure destroys a user's work, and on a
/// requeued job it is the difference between "the project spent this" and "the
/// site lost this", which is exactly what a charging dispute turns on. Site
/// monitoring picks these up.
///
/// Called only where a fresh `sacct` response has just been parsed, never when
/// replaying the cache, so that re-reading a cached hour does not re-report a
/// failure that has already been reported.
///
fn report_node_failures(jobs: &[SlurmJob], project: &ProjectMapping) {
    for job in jobs {
        if job.terminal_state() != "NODE_FAIL" {
            continue;
        }

        match job.failed_node().is_empty() {
            true => tracing::error!(
                "Node failure lost job {} of project {} (user {}) after {} seconds \
                 (states: {}). Slurm did not name the node.",
                job.id(),
                project.project(),
                job.user(),
                job.duration().num_seconds(),
                job.states().join(", ")
            ),
            false => tracing::error!(
                "Node failure on {} lost job {} of project {} (user {}) after {} seconds \
                 (states: {}).",
                job.failed_node(),
                job.id(),
                project.project(),
                job.user(),
                job.duration().num_seconds(),
                job.states().join(", ")
            ),
        }
    }
}

///
/// Warn if the report disagrees with what we counted while building it, and
/// return whether it agreed. A mismatch means a bug in the accumulation above,
/// not bad data from Slurm.
///
/// The answer is used to decide whether the day may be cached. A report that
/// does not add up must not be frozen: the caller would serve the bad figures
/// from cache for as long as they are kept, and the next reader would have no
/// way to tell. This is the same treatment the usage mismatch has always had -
/// there is no reason for the job counts, the waits or the requeue figures to
/// be held to a lower standard than the usage beside them.
///
fn check_counter_consistency(
    report: &DailyProjectUsageReport,
    totals: &ReportTotals,
    project: &ProjectMapping,
    day: &greatwestern::grammar::Date,
) -> bool {
    let mut consistent = true;

    if report.total_runtime_seconds() != totals.runtime_seconds {
        consistent = false;
        tracing::warn!(
            "Runtime inconsistency for project {} on {}: local counter ({}s) differs from \
             report total ({}s). This may indicate a bug.",
            project.project(),
            day,
            totals.runtime_seconds,
            report.total_runtime_seconds()
        );
    }

    // The runtime and expansion sums are averaged over this count rather than
    // over the job count, so it has to be checked in its own right - a report
    // whose denominator has drifted reports a plausible figure rather than an
    // obviously broken one.
    if report.expansion_jobs() != totals.expansion_jobs {
        consistent = false;
        tracing::warn!(
            "Expansion denominator inconsistency for project {} on {}: local counter \
             ({} jobs) differs from report total ({} jobs). This may indicate a bug.",
            project.project(),
            day,
            totals.expansion_jobs,
            report.expansion_jobs()
        );
    }

    if report.num_jobs() != totals.num_jobs || report.total_wait_seconds() != totals.wait_seconds {
        consistent = false;
        tracing::warn!(
            "Job count/wait time inconsistency for project {} on {}: \
             local counters ({} jobs, {}s wait) differ from report totals ({} jobs, {}s wait). \
             This may indicate a bug.",
            project.project(),
            day,
            totals.num_jobs,
            totals.wait_seconds,
            report.num_jobs(),
            report.total_wait_seconds()
        );
    }

    if report.num_requeue_events() != totals.requeue_events
        || report.requeue_wait_seconds() != totals.requeue_wait_seconds
        || report.total_requeue_usage().seconds() != totals.requeue_usage
    {
        consistent = false;
        tracing::warn!(
            "Requeue accounting inconsistency for project {} on {}: \
             local counters ({} events, {}s wait, {}s usage) differ from report totals \
             ({} events, {}s wait, {}s usage). This may indicate a bug.",
            project.project(),
            day,
            totals.requeue_events,
            totals.requeue_wait_seconds,
            totals.requeue_usage,
            report.num_requeue_events(),
            report.requeue_wait_seconds(),
            report.total_requeue_usage().seconds()
        );
    }

    // the per-state maps must account for every event and every second of
    // requeue usage - an unrecognised Slurm state is bucketed, never dropped
    if !report.is_consistent() {
        consistent = false;
        tracing::warn!(
            "Report for project {} on {} is internally inconsistent - its per-user or \
             per-state maps do not sum to its own totals. This may indicate a bug.",
            project.project(),
            day
        );
    }

    consistent
}

///
/// How long a day may stay provisional because a job that started in it has not
/// finished, in days.
///
/// A day holding an unfinished job is not cached, so that it is re-read and the
/// job's real runtime recorded once it ends. A record `slurmdbd` never closes
/// would otherwise keep that day being re-queried for ever, so past this age
/// the day is completed with what is known and the gap is logged. Comfortably
/// longer than any cluster's wall-clock limit.
///
const PROVISIONAL_DAY_LIMIT_DAYS: i64 = 30;

///
/// Mark a day complete and cache it, if it is finished and adds up.
///
/// Four things stop a day being frozen, and the first three are bugs: its usage
/// disagreeing with what we counted, its counters disagreeing with its own
/// totals, and the day not being over yet. The fourth is not a bug at all - a
/// job that started in this day is still running, so its runtime and expansion
/// factor are not yet knowable, and caching now would freeze a partial answer
/// that nothing would ever revisit. Leaving the day uncached costs one `sacct`
/// query per pass until the job ends; freezing it costs a wrong figure for as
/// long as the cache is kept.
///
async fn complete_and_cache_if_final(
    daily_report: &mut DailyProjectUsageReport,
    totals: &ReportTotals,
    counters_agree: bool,
    project: &ProjectMapping,
    day: &greatwestern::grammar::Date,
    now: &chrono::DateTime<Utc>,
) {
    if daily_report.total_usage().seconds() != totals.usage {
        // this points to some error when generating the values...
        tracing::error!(
            "Total usage in daily report does not match total usage calculated manually: {} != {}",
            daily_report.total_usage().seconds(),
            totals.usage
        );
        return;
    }

    if !counters_agree {
        // `check_counter_consistency` has already said which of them disagreed
        tracing::error!(
            "Not caching the report for project {} on {}: its counters do not agree with \
             its totals.",
            project.project(),
            day
        );
        return;
    }

    let day_end = day.day().end_time().and_utc();

    if day_end >= *now {
        // the day is not over yet
        return;
    }

    if totals.saw_unfinished_job {
        let age = now.signed_duration_since(day_end);

        if age < chrono::Duration::days(PROVISIONAL_DAY_LIMIT_DAYS) {
            tracing::debug!(
                "Not completing the report for project {} on {}: a job that started that \
                 day had not finished when we asked, so its runtime is not yet known. \
                 The day will be re-read.",
                project.project(),
                day
            );
            return;
        }

        tracing::warn!(
            "Completing the report for project {} on {} even though a job that started \
             that day has still not finished after {} days. Its runtime and expansion \
             factor are not included; its usage is.",
            project.project(),
            day,
            age.num_days()
        );
    }

    daily_report.set_complete();

    match cache::set_report(project.project(), day, daily_report).await {
        Ok(_) => (),
        Err(e) => {
            tracing::error!("Could not cache report for {}: {}", day, e);
        }
    }
}

async fn get_hourly_report(
    expires: &chrono::DateTime<Utc>,
    project: &ProjectMapping,
    day: &greatwestern::grammar::Date,
    account: &SlurmAccount,
    slurm_nodes: &SlurmNodes,
    cluster: &str,
    partition_command: &str,
) -> Result<DailyProjectUsageReport, Error> {
    let now = chrono::Utc::now();
    let mut daily_report = DailyProjectUsageReport::default();
    let mut totals = ReportTotals::default();

    // we need to get the report hour by hour from slurm, as users may have
    // run very large numbers of jobs in a day, and sacct may time out
    for hour in day.hours() {
        if let Some(hourly_report) = cache::get_hourly_report(project.project(), &hour).await? {
            // we have this hour in the cache, so use it
            tracing::debug!(
                "Using cached hourly report for {}. Number of jobs = {}",
                hour,
                hourly_report.len()
            );

            let hour_start_time = hour.start_time().and_utc();

            for job in &hourly_report {
                record_job(&mut daily_report, job, &hour_start_time, &mut totals);
            }

            continue;
        }

        assert_not_expired(expires)?;

        let start_time = hour.start_time().and_utc();
        let end_time = hour.end_time().and_utc();

        if start_time > now {
            // we can't get the usage for this hour yet as it is in the future
            continue;
        }

        let end_time = match now < end_time {
            true => now,
            false => end_time,
        };

        // check that the hour contains <= 3600 seconds
        if end_time.timestamp() - start_time.timestamp() > 3600 {
            tracing::warn!(
                "Hour {} contains more than 1 hour - check this! {} : {}",
                hour,
                start_time,
                end_time
            );
        }

        // now try to get the report for this hour - we use a much longer
        // timeout here as we may be getting a lot of jobs
        let cmd = runner(expires).await?.build_command(
            "SACCT",
            vec![
                "--noconvert".to_string(),
                "--allocations".to_string(),
                "--allusers".to_string(),
                // one record per attempt, not just the last one - without this
                // everything a requeued job consumed before its final attempt
                // is invisible. `get_consumers` classifies them.
                "--duplicates".to_string(),
                format!("--starttime={}", start_time.format("%Y-%m-%dT%H:%M:%S")),
                format!("--endtime={}", end_time.format("%Y-%m-%dT%H:%M:%S")),
                format!("--account={}", account.name()),
                format!("--cluster={}", cluster),
                partition_command.to_string(),
                "--json".to_string(),
            ],
        )?;

        let response = runner(expires)
            .await?
            .run_json(&cmd, std::time::Duration::from_secs(120))
            .await?;

        let jobs = SlurmJob::get_consumers(&response, &start_time, &end_time, slurm_nodes)?;

        tracing::debug!(
            "Got {} jobs for project {} on {}",
            jobs.len(),
            project.project(),
            hour
        );

        // An hour is cached as the records themselves, so an unfinished record
        // would be frozen here with the runtime it had reached at this moment
        // and replayed with that figure for ever - the day-level guard below
        // cannot undo that, because it would be re-reading the cache rather
        // than Slurm. The hour is left uncached until the job it holds ends.
        let hour_has_unfinished_job = jobs
            .iter()
            .any(|job| job.original_start_time() >= &start_time && !job.has_ended());

        // cache this hourly report if it is in the past and final
        if hour.end_time().and_utc() < now && !hour_has_unfinished_job {
            match cache::set_hourly_report(project.project(), &hour, &jobs).await {
                Ok(_) => (),
                Err(e) => {
                    tracing::error!("Could not cache hourly report for {}: {}", hour, e);
                }
            }
        }

        report_node_failures(&jobs, project);

        for job in &jobs {
            record_job(&mut daily_report, job, &start_time, &mut totals);
        }
    }

    tracing::debug!(
        "Got {} jobs consuming {} seconds for project {} on {}, plus {} requeue events \
         consuming {} seconds",
        totals.num_jobs,
        totals.usage,
        project.project(),
        day,
        totals.requeue_events,
        totals.requeue_usage
    );

    // runtime consistency check: local shadow counters must match the report's scalar totals
    let counters_agree = check_counter_consistency(&daily_report, &totals, project, day);

    complete_and_cache_if_final(
        &mut daily_report,
        &totals,
        counters_agree,
        project,
        day,
        &now,
    )
    .await;

    Ok(daily_report)
}

async fn get_daily_report(
    expires: &chrono::DateTime<Utc>,
    project: &ProjectMapping,
    day: &greatwestern::grammar::Date,
    account: &SlurmAccount,
    slurm_nodes: &SlurmNodes,
    cluster: &str,
    partition_command: &str,
) -> Result<DailyProjectUsageReport, Error> {
    // see if we have this report in the cache
    if let Some(report) = cache::get_report(project.project(), day).await? {
        return Ok(report);
    }

    assert_not_expired(expires)?;

    if cache::compute_via_hourly_reports(project.project(), day).await? {
        return get_hourly_report(
            expires,
            project,
            day,
            account,
            slurm_nodes,
            cluster,
            partition_command,
        )
        .await;
    }

    let now = chrono::Utc::now();
    let start_time = day.day().start_time().and_utc();
    let end_time = day.day().end_time().and_utc();

    if start_time > now {
        // we can't get the usage for this day yet as it is in the future
        return Ok(DailyProjectUsageReport::default());
    }

    let end_time = match now < end_time {
        true => now,
        false => end_time,
    };

    // check that the day contains <= 24 hours (86400 seconds)
    if end_time.timestamp() - start_time.timestamp() > 86400 {
        tracing::warn!(
            "Day {} contains more than 24 hours - check this! {} : {}",
            day,
            start_time,
            end_time
        );
    }

    // try to get the daily report from slurm - use a shorter 20 second
    // timeout as we will fall back to hourly reports if this fails
    let cmd = runner(expires).await?.build_command(
        "SACCT",
        vec![
            "--noconvert".to_string(),
            "--allocations".to_string(),
            "--allusers".to_string(),
            // see the note in `get_hourly_report` - one record per attempt
            "--duplicates".to_string(),
            format!("--starttime={}", start_time.format("%Y-%m-%dT%H:%M:%S")),
            format!("--endtime={}", end_time.format("%Y-%m-%dT%H:%M:%S")),
            format!("--account={}", account.name()),
            format!("--cluster={}", cluster),
            partition_command.to_string(),
            "--json".to_string(),
        ],
    )?;

    let response = runner(expires)
        .await?
        .run_json(&cmd, std::time::Duration::from_secs(20))
        .await;

    match response {
        Ok(response) => {
            let jobs = SlurmJob::get_consumers(&response, &start_time, &end_time, slurm_nodes)?;

            tracing::debug!(
                "Got {} jobs for project {} on {}",
                jobs.len(),
                project.project(),
                day
            );

            let mut daily_report = DailyProjectUsageReport::default();
            let mut totals = ReportTotals::default();

            report_node_failures(&jobs, project);

            for job in &jobs {
                record_job(&mut daily_report, job, &start_time, &mut totals);
            }

            // runtime consistency check
            let counters_agree = check_counter_consistency(&daily_report, &totals, project, day);

            complete_and_cache_if_final(
                &mut daily_report,
                &totals,
                counters_agree,
                project,
                day,
                &now,
            )
            .await;

            Ok(daily_report)
        }
        Err(Error::Timeout(_)) => {
            tracing::warn!(
                "Timed out getting usage for project {} on {}. Switching to hourly reporting.",
                project.project(),
                day
            );

            // we need to switch to getting an hourly report for this date
            return get_hourly_report(
                expires,
                project,
                day,
                account,
                slurm_nodes,
                cluster,
                partition_command,
            )
            .await;
        }
        Err(e) => {
            tracing::warn!(
                "Could not get usage for project {} on {}: {}",
                project.project(),
                day,
                e
            );

            // we will return an empty report - this will not be complete
            // and will not be cached
            Ok(DailyProjectUsageReport::default())
        }
    }
}

pub async fn get_usage_report(
    project: &ProjectMapping,
    dates: &DateRange,
    expires: &chrono::DateTime<Utc>,
) -> Result<ProjectUsageReport, Error> {
    assert_not_expired(expires)?;

    let account = SlurmAccount::from_mapping(project)?;

    let account = match get_account(account.name(), expires).await {
        Ok(Some(account)) => account,
        Ok(None) => {
            tracing::warn!("Could not get account {}", account.name());
            return Ok(ProjectUsageReport::new(project.project()));
        }
        Err(e) => {
            tracing::warn!("Could not get account {}: {}", account.name(), e);
            return Ok(ProjectUsageReport::new(project.project()));
        }
    };

    let mut report = ProjectUsageReport::new(project.project());
    let slurm_nodes = cache::get_nodes().await?;
    let now = chrono::Utc::now();
    let cluster = cache::get_cluster().await?;
    let partition = cache::get_partition().await?;

    let partition_command = match partition {
        Some(partition) => format!("--partition={}", partition),
        None => "".to_string(),
    };

    // we now request the data day by day - do this in parallel
    let mut tasks = Vec::new();

    for day in dates.days() {
        if day.day().start_time().and_utc() > now {
            // we can't get the usage for this day yet as it is in the future
            continue;
        }

        let expires = *expires;
        let project = project.clone();
        let account = account.clone();
        let slurm_nodes = slurm_nodes.clone();
        let cluster = cluster.clone();
        let partition_command = partition_command.clone();
        let day = day.clone();
        let day2 = day.clone();

        tasks.push((
            tokio::spawn(async move {
                get_daily_report(
                    &expires,
                    &project,
                    &day,
                    &account,
                    &slurm_nodes,
                    &cluster,
                    &partition_command,
                )
                .await
            }),
            day2,
        ));
    }

    for (task, day) in tasks {
        let daily_report = match task.await {
            Ok(report) => match report {
                Ok(report) => report,
                Err(e) => {
                    tracing::warn!("Could not get daily report: {}", e);
                    // we will return an empty report for this day
                    DailyProjectUsageReport::default()
                }
            },
            Err(e) => {
                tracing::warn!("Could not get daily report: {}", e);
                // we will return an empty report for this day
                DailyProjectUsageReport::default()
            }
        };

        // now save this to the overall report
        report.set_report(&day, &daily_report);
    }

    Ok(report)
}

pub async fn get_limit(
    project: &ProjectMapping,
    expires: &chrono::DateTime<Utc>,
) -> Result<Usage, Error> {
    assert_not_expired(expires)?;

    let account = SlurmAccount::from_mapping(project)?;

    let account = match get_account(account.name(), expires).await? {
        Some(account) => account,
        None => {
            tracing::warn!("Could not get account {}", account.name());
            return Err(Error::NotFound(account.name().to_string()));
        }
    };

    // check that the limits in slurm match up...
    let cmd = priority_runner(expires).await?.build_command(
        "SACCTMGR",
        vec![
            "--json".to_string(),
            "show".to_string(),
            "association".to_string(),
            "where".to_string(),
            format!("account={}", account.name()),
            format!("cluster={}", cache::get_cluster().await?),
        ],
    )?;

    let response = priority_runner(expires)
        .await?
        .run_json(&cmd, DEFAULT_TIMEOUT)
        .await?;

    let limits = match response.get("associations") {
        Some(limits) => match limits.as_array() {
            Some(limits) => {
                let mut slurm_limits: Vec<SlurmLimit> = Vec::new();

                for limit in limits {
                    slurm_limits.push(SlurmLimit::construct(limit)?);
                }

                slurm_limits
            }
            None => {
                tracing::warn!("Limits is not an array: {:?}", limits);
                return Err(Error::Call("Limits is not an array".to_string()));
            }
        },
        None => Vec::new(),
    };

    let cluster = cache::get_cluster().await?;

    let project_limit = account.limit();

    let slurm_limit = match limits
        .iter()
        .find(|l| l.account() == account.name() && l.cluster() == cluster)
    {
        Some(slurm_limit) => slurm_limit,
        None => {
            tracing::warn!("Could not find limit for account {}", account.name());
            return Err(Error::NotFound(account.name().to_string()));
        }
    };

    tracing::debug!(
        "Found limit for account {}: {}",
        account.name(),
        slurm_limit
    );

    let node = cache::get_default_node().await?;

    let mut actual_slurm_limit: Option<Usage> = None;

    if node.has_cpus() && node.cpus() > 0 {
        if let Some(cpu_limit) = slurm_limit.cpu_limit() {
            let check = node.cpus() * project_limit.seconds();
            if check != cpu_limit.seconds() {
                if check != 0 {
                    tracing::warn!(
                        "CPU limit for account {} does not match: {} != {}",
                        account.name(),
                        check,
                        cpu_limit.seconds()
                    );
                }

                actual_slurm_limit = Some(Usage::new(cpu_limit.seconds() / node.cpus()));
            }
        }
    }

    if node.has_gpus() && node.gpus() > 0 {
        if let Some(gpu_limit) = slurm_limit.gpu_limit() {
            let check = node.gpus() * project_limit.seconds();
            if check != gpu_limit.seconds() {
                if check != 0 {
                    tracing::warn!(
                        "GPU limit for account {} does not match: {} != {}",
                        account.name(),
                        check,
                        gpu_limit.seconds()
                    );
                }

                if actual_slurm_limit.is_none() {
                    actual_slurm_limit = Some(Usage::new(gpu_limit.seconds() / node.gpus()));
                }
            }
        }
    }

    if node.has_mem() && node.mem() > 0 {
        if let Some(mem_limit) = slurm_limit.mem_limit() {
            let check = node.mem() * project_limit.seconds();
            if check != mem_limit.seconds() {
                if check != 0 {
                    tracing::warn!(
                        "Memory limit for account {} does not match: {} != {}",
                        account.name(),
                        check,
                        mem_limit.seconds()
                    );
                }

                if actual_slurm_limit.is_none() {
                    actual_slurm_limit = Some(Usage::new(mem_limit.seconds() / node.mem()));
                }
            }
        }
    }

    if node.has_billing() && node.billing() > 0 {
        if let Some(billing_limit) = slurm_limit.billing_limit() {
            let check = node.billing() * project_limit.seconds();
            if check != billing_limit.seconds() {
                if check != 0 {
                    tracing::warn!(
                        "Billing limit for account {} does not match: {} != {}",
                        account.name(),
                        check,
                        billing_limit.seconds()
                    );
                }

                if actual_slurm_limit.is_none() {
                    actual_slurm_limit = Some(Usage::new(billing_limit.seconds() / node.billing()));
                }
            }
        }
    }

    if let Some(actual_slurm_limit) = actual_slurm_limit {
        // we need to set this to the actual slurm limit
        let mut account = account.clone();
        account.set_limit(&actual_slurm_limit);

        // now save the account to the cache
        cache::add_account(&account).await?;

        tracing::info!("Updated account limit to {}", actual_slurm_limit);
        return Ok(actual_slurm_limit);
    }

    Ok(*account.limit())
}

pub async fn set_limit(
    project: &ProjectMapping,
    limit: &Usage,
    expires: &chrono::DateTime<Utc>,
) -> Result<Usage, Error> {
    assert_not_expired(expires)?;

    let account = SlurmAccount::from_mapping(project)?;

    match get_account(account.name(), expires).await? {
        Some(account) => {
            // Refuse to modify an account this agent does not manage.
            //
            // `SlurmAccount::from_mapping` hard-wires `organization` to the
            // managed org, so the create path's existing check can never fail -
            // it validates a locally-constructed object, not the one that
            // actually exists in Slurm. Nothing checked the *fetched* account,
            // so a peer-chosen `local_group` naming any real account on the
            // cluster had its `GrpTRESMins` rewritten. See
            // `docs/specifications/security-review-2.md` (finding R5).
            if !account.is_managed() {
                tracing::warn!(
                    "Refusing to set a limit on Slurm account '{}': it is in \
                     organization '{}', not the OpenPortal-managed '{}'.",
                    account.name(),
                    account.organization(),
                    get_managed_organization()
                );
                return Err(Error::UnmanagedGroup(format!(
                    "Cannot set a limit on Slurm account '{}' - it is not managed by OpenPortal",
                    account.name()
                )));
            }

            let mut account = account.clone();

            account.set_limit(limit);

            let cluster = cache::get_cluster().await?;

            // calculate the GRES limits in terms of CPU, GPU and Memory
            let node = cache::get_default_node().await?;

            let mut tres: Vec<String> = Vec::new();

            if node.has_cpus() {
                tres.push(format!(
                    "cpu={}",
                    (node.cpus() as f64 * limit.minutes()) as u64
                ));
            }

            if node.has_gpus() {
                tres.push(format!(
                    "gres/gpu={}",
                    (node.gpus() as f64 * limit.minutes()) as u64
                ));
            }

            if node.has_mem() {
                tres.push(format!(
                    "mem={}",
                    (node.mem() as f64 * limit.minutes()) as u64
                ));
            }

            if node.has_billing() {
                tres.push(format!(
                    "billing={}",
                    (node.billing() as f64 * limit.minutes()) as u64
                ));
            }

            if !tres.is_empty() {
                let cmd = priority_runner(expires).await?.build_command(
                    "SACCTMGR",
                    vec![
                        "--immediate".to_string(),
                        "modify".to_string(),
                        "account".to_string(),
                        account.name().to_string(),
                        "set".to_string(),
                        format!("GrpTRESMins={}", tres.join(",")),
                        "where".to_string(),
                        format!("cluster={}", cluster),
                    ],
                )?;

                priority_runner(expires)
                    .await?
                    .run(&cmd, DEFAULT_TIMEOUT)
                    .await?;
            }

            // now we've made the change, save the account to the cache
            cache::add_account(&account).await?;

            Ok(*account.limit())
        }
        None => {
            tracing::warn!("Could not get account {}", account.name());
            Err(Error::NotFound(account.name().to_string()))
        }
    }
}

///
/// How far back `has_active_jobs` looks for a job that is still queued or
/// running.
///
/// `sacct` needs an explicit window whenever a state filter is given, and the
/// controller's queue carries no such bound. A year is far longer than any real
/// scheduler will hold a job pending or running, and this is an occasional
/// verification query rather than something on the add/remove path.
///
const ACTIVE_JOB_WINDOW_DAYS: i64 = 365;

///
/// Return whether Slurm still holds a queued (PENDING) or running (RUNNING) job
/// matching `filter`, which is a single `sacct` selector such as `--user=bob`
/// or `--account=proj`.
///
/// The Slurm user and account records are deliberately kept so that the
/// accounting history stays intact, so the jobs are the only thing a removal
/// changes here - which makes them what "has the removal finished?" has to
/// mean.
///
/// RUNNING is counted even though `remove_local_user` / `remove_local_project`
/// only cancel what is PENDING. That asymmetry is deliberate, and is the one
/// place in these checks where a `false` is not something re-running the
/// removal can change: OpenPortal never destroys anything - it disables,
/// recycles and cancels only what has not started - so a job already running is
/// left to finish, and until it does the user or project genuinely has not
/// finished leaving the cluster. Do not close the gap by having removal cancel
/// running jobs. It is barely reachable in practice: a removed user has already
/// lost the ability to submit, and jobs run for a day or two at most, so this
/// resolves itself.
///
async fn has_active_jobs(filter: &str, expires: &chrono::DateTime<Utc>) -> Result<bool, Error> {
    let cluster = cache::get_cluster().await?;

    let start_time = chrono::Utc::now() - chrono::Duration::days(ACTIVE_JOB_WINDOW_DAYS);

    let cmd = priority_runner(expires).await?.build_command(
        "SACCT",
        vec![
            "--noheader".to_string(),
            "--parsable2".to_string(),
            "--allocations".to_string(),
            "--allusers".to_string(),
            "--state=PENDING,RUNNING".to_string(),
            "--format=JobID".to_string(),
            format!("--starttime={}", start_time.format("%Y-%m-%dT%H:%M:%S")),
            format!("--cluster={}", cluster),
            filter.to_string(),
        ],
    )?;

    let output = priority_runner(expires)
        .await?
        .run(&cmd, DEFAULT_TIMEOUT)
        .await?;

    Ok(output.lines().any(|line| !line.trim().is_empty()))
}

///
/// Return whether everything `add_project` does for this mapping has been done:
/// the Slurm account exists, is managed by OpenPortal, is attached to this
/// cluster, and carries the name the mapping says it should.
///
/// Read straight from Slurm rather than through this agent's account cache -
/// the question is whether Slurm really is in the state an earlier
/// `add_local_project` claimed to leave it in, and the cache would only replay
/// that claim back.
///
pub async fn is_local_project_added(
    mapping: &ProjectMapping,
    expires: &chrono::DateTime<Utc>,
) -> Result<bool, Error> {
    assert_not_expired(expires)?;

    let expected = SlurmAccount::from_mapping(mapping)?;

    let account = match get_account_from_slurm(expected.name(), expires).await? {
        Some(account) => account,
        None => {
            tracing::info!("Slurm account {} does not exist", expected.name());
            return Ok(false);
        }
    };

    if !account.is_managed() {
        tracing::info!(
            "Slurm account {} is not managed by OpenPortal - nothing for add_local_project to do",
            account.name()
        );
        return Ok(true);
    }

    let cluster = cache::get_cluster().await?;

    if !account.in_cluster(&cluster) {
        tracing::info!(
            "Slurm account {} is not in cluster {}, so has not been added",
            account.name(),
            cluster
        );
        return Ok(false);
    }

    Ok(true)
}

///
/// Return whether everything `remove_local_project` does for this mapping has
/// been done - that is, that no job of the project's is queued or running. The
/// account itself is kept on purpose, so that its usage history survives the
/// project being removed and its associations stay stable if it is ever
/// re-added, which leaves the jobs as the only thing removal changes here.
///
/// See `has_active_jobs` for why a running job counts even though removal
/// deliberately does not cancel one.
///
pub async fn is_local_project_removed(
    mapping: &ProjectMapping,
    expires: &chrono::DateTime<Utc>,
) -> Result<bool, Error> {
    assert_not_expired(expires)?;

    let account = clean_account_name(mapping.local_group())?;

    if has_active_jobs(&format!("--account={}", account), expires).await? {
        tracing::info!(
            "Slurm account {} still has queued or running jobs, so has not been removed",
            account
        );
        return Ok(false);
    }

    Ok(true)
}

///
/// Return whether everything `add_user` does for this mapping has been done:
/// the Slurm user exists, has the project's account as their default, and is
/// associated with it on this cluster. Read straight from Slurm, not the cache.
///
pub async fn is_local_user_added(
    mapping: &UserMapping,
    expires: &chrono::DateTime<Utc>,
) -> Result<bool, Error> {
    assert_not_expired(expires)?;

    // The user cannot be fully added while the account they are meant to
    // default to is not - `get_user_create_if_not_exists` creates it first.
    if !is_local_project_added(&mapping.clone().into(), expires).await? {
        return Ok(false);
    }

    let expected = SlurmUser::from_mapping(mapping)?;

    let user = match get_user_from_slurm(expected.name(), expires).await? {
        Some(user) => user,
        None => {
            tracing::info!("Slurm user {} does not exist", expected.name());
            return Ok(false);
        }
    };

    let account = SlurmAccount::from_mapping(&mapping.clone().into())?;
    let cluster = cache::get_cluster().await?;

    if *user.default_account() != Some(account.name().to_string()) {
        tracing::info!(
            "Slurm user {} does not default to account {}, so has not been added",
            user.name(),
            account.name()
        );
        return Ok(false);
    }

    if !user
        .associations()
        .iter()
        .any(|a| a.account() == account.name() && a.cluster() == cluster)
    {
        tracing::info!(
            "Slurm user {} is not associated with account {} on cluster {}, so has not \
             been added",
            user.name(),
            account.name(),
            cluster
        );
        return Ok(false);
    }

    Ok(true)
}

///
/// Return whether everything `remove_local_user` does for this mapping has been
/// done - that is, that no job of theirs is queued or running. As with a
/// project, the Slurm user and their associations are kept so that their usage
/// history survives, and it is the account agent that stops them logging in.
///
/// See `has_active_jobs` for why a running job counts even though removal
/// deliberately does not cancel one.
///
pub async fn is_local_user_removed(
    mapping: &UserMapping,
    expires: &chrono::DateTime<Utc>,
) -> Result<bool, Error> {
    assert_not_expired(expires)?;

    let user = clean_user_name(mapping.local_user().unix()?)?;

    if has_active_jobs(&format!("--user={}", user), expires).await? {
        tracing::info!(
            "Slurm user {} still has queued or running jobs, so has not been removed",
            user
        );
        return Ok(false);
    }

    Ok(true)
}

pub async fn cancel_pending_user_jobs(
    user: &str,
    expires: &chrono::DateTime<Utc>,
) -> Result<(), Error> {
    assert_not_expired(expires)?;

    let user = clean_user_name(user)?;

    // As for `cancel_pending_project_jobs`: resolve the user and refuse unless
    // they are associated with at least one OpenPortal-managed account.
    // `SlurmUser` carries no organization of its own, so "managed" is defined
    // by association. See
    // `docs/specifications/security-review-2.md` (finding R5).
    match get_user(&user, expires).await? {
        Some(existing) => {
            let mut manages_any = false;

            for association in existing.associations() {
                if let Some(account) = get_account(association.account(), expires).await? {
                    if account.is_managed() {
                        manages_any = true;
                        break;
                    }
                }
            }

            if !manages_any {
                tracing::warn!(
                    "Refusing to cancel jobs for Slurm user '{}': they are not \
                     associated with any OpenPortal-managed account.",
                    user
                );
                return Err(Error::UnmanagedGroup(format!(
                    "Cannot cancel jobs for Slurm user '{}' - they are not managed by OpenPortal",
                    user
                )));
            }
        }
        None => {
            tracing::warn!(
                "Not cancelling jobs for Slurm user '{}' - they do not exist",
                user
            );
            return Ok(());
        }
    }

    let cluster = cache::get_cluster().await?;

    tracing::info!(
        "Cancelling all pending jobs for user {} in cluster {}",
        user,
        cluster
    );

    let cmd = priority_runner(expires).await?.build_command(
        "SCANCEL",
        vec![
            "--verbose".to_string(),
            format!("--user={}", user),
            "--state=PENDING".to_string(),
            format!("--cluster={}", cluster),
        ],
    )?;

    match priority_runner(expires)
        .await?
        .run(&cmd, DEFAULT_TIMEOUT)
        .await
    {
        Ok(output) => {
            if !output.is_empty() {
                tracing::info!("scancel output: {}", output);
            }
            Ok(())
        }
        Err(e) => {
            tracing::warn!("Could not cancel pending jobs for user {}: {}", user, e);
            // Don't fail the whole operation if scancel fails - log the error and continue
            Ok(())
        }
    }
}

pub async fn cancel_pending_project_jobs(
    account: &str,
    expires: &chrono::DateTime<Utc>,
) -> Result<(), Error> {
    assert_not_expired(expires)?;

    let account = clean_account_name(account)?;

    // Resolve the account and refuse unless OpenPortal manages it. This took a
    // bare string and cancelled against it with no lookup at all, so a
    // peer-chosen `local_group` could `scancel` every pending job of any
    // account on the cluster. See
    // `docs/specifications/security-review-2.md` (finding R5).
    match get_account(&account, expires).await? {
        Some(existing) if existing.is_managed() => {}
        Some(existing) => {
            tracing::warn!(
                "Refusing to cancel jobs for Slurm account '{}': it is in \
                 organization '{}', not the OpenPortal-managed '{}'.",
                account,
                existing.organization(),
                get_managed_organization()
            );
            return Err(Error::UnmanagedGroup(format!(
                "Cannot cancel jobs for Slurm account '{}' - it is not managed by OpenPortal",
                account
            )));
        }
        None => {
            // Nothing to cancel for an account that does not exist - and, as
            // for removal generally, this stays idempotent rather than erroring.
            tracing::warn!(
                "Not cancelling jobs for Slurm account '{}' - it does not exist",
                account
            );
            return Ok(());
        }
    }

    let cluster = cache::get_cluster().await?;

    tracing::info!(
        "Cancelling all pending jobs for account {} in cluster {}",
        account,
        cluster
    );

    let cmd = priority_runner(expires).await?.build_command(
        "SCANCEL",
        vec![
            "--verbose".to_string(),
            format!("--account={}", account),
            "--state=PENDING".to_string(),
            format!("--cluster={}", cluster),
        ],
    )?;

    match priority_runner(expires)
        .await?
        .run(&cmd, DEFAULT_TIMEOUT)
        .await
    {
        Ok(output) => {
            if !output.is_empty() {
                tracing::info!("scancel output: {}", output);
            }
            Ok(())
        }
        Err(e) => {
            tracing::warn!(
                "Could not cancel pending jobs for account {}: {}",
                account,
                e
            );
            // Don't fail the whole operation if scancel fails - log the error and continue
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slurm::test_fixture::*;
    use crate::slurm::Attempt;

    /// Build a daily report the way `get_hourly_report` and `get_daily_report`
    /// do, over the records `sacct` would return for one window.
    fn report_for(
        window: (chrono::DateTime<Utc>, chrono::DateTime<Utc>),
    ) -> (DailyProjectUsageReport, ReportTotals) {
        let (start, _) = window;
        let mut report = DailyProjectUsageReport::default();
        let mut totals = ReportTotals::default();

        for job in consumers_for(window) {
            record_job(&mut report, &job, &start, &mut totals);
        }

        (report, totals)
    }

    ///
    /// The fixture's record for one job, on its own, optionally still running.
    ///
    /// `sacct` reports a running record with no end time at all and an
    /// `elapsed` that is the runtime *so far* - a figure that grows every time
    /// the record is read. The fixture has no such record, because a fixture
    /// of finished jobs cannot have one: whether a record has ended is a
    /// question about the moment it was read, not about its contents.
    ///
    fn one_job(id: u64, running_for: Option<i64>) -> serde_json::Value {
        // Through the window filter first, so this is what the real query would
        // have returned - a job's other attempts being visible or not is what
        // decides whether an attempt is classified as superseded. Only then is
        // the record made to look like one still running, because a record with
        // no end time would not survive a filter written for finished ones.
        let (start, end) = day_one();
        let mut fixture = records_in_window(&start, &end);

        let Some(records) = fixture.get_mut("jobs").and_then(|jobs| jobs.as_array_mut()) else {
            unreachable!("the fixture has a jobs array");
        };

        records.retain(|record| record.get("job_id").and_then(|i| i.as_u64()) == Some(id));

        if let Some(elapsed) = running_for {
            let Some(record) = records.first_mut() else {
                unreachable!("the fixture has a record for this job");
            };

            record["time"]["end"] = serde_json::json!(0);
            record["time"]["elapsed"] = serde_json::json!(elapsed);
            record["state"]["current"] = serde_json::json!(["RUNNING"]);
        }

        fixture
    }

    /// Build a day-one report over exactly the records given.
    fn report_over(records: &serde_json::Value) -> (DailyProjectUsageReport, ReportTotals) {
        let (start, end) = day_one();
        let mut report = DailyProjectUsageReport::default();
        let mut totals = ReportTotals::default();

        let Ok(jobs) = SlurmJob::get_consumers(records, &start, &end, &test_nodes()) else {
            unreachable!("the fixture parses");
        };

        for job in &jobs {
            record_job(&mut report, job, &start, &mut totals);
        }

        (report, totals)
    }

    #[test]
    fn test_a_job_still_running_when_the_window_closes_contributes_no_runtime() {
        // The runtime and the expansion factor are recorded once, in the window
        // the job started in, and never revisited - so recording them from a
        // record that has not finished freezes whatever `elapsed` had reached
        // at that moment. Job 100 ran for an hour; caught half an hour in, it
        // used to be written down as a half-hour job that had waited an hour,
        // giving an expansion factor of 3.00 against a true 2.00. The longer
        // the job, the worse the error: a job caught an hour into a thirty-hour
        // run is out by more than an order of magnitude.
        let (report, totals) = report_over(&one_job(100, Some(1800)));

        // it is still a job, and its wait is already final
        assert_eq!(report.num_jobs(), 1);
        assert_eq!(report.total_wait_seconds(), 3600);

        // but nothing is claimed about how long it ran
        assert_eq!(report.expansion_jobs(), 0);
        assert_eq!(report.total_runtime_seconds(), 0);
        assert_eq!(report.average_expansion_factor(), 0.0);
        assert_eq!(report.average_runtime_seconds(), 0);

        // and the window is marked as one that must not be frozen
        assert!(totals.saw_unfinished_job);

        // the usage is recorded as it always was: the job did hold those nodes
        // from its start to the end of the window, whatever it does next
        assert_eq!(report.total_usage(), Usage::new(79200));
        assert!(report.is_consistent());
    }

    #[test]
    fn test_the_same_job_records_its_real_runtime_once_it_has_finished() {
        // The other side of it: read again after the job ends - which is what
        // declining to cache the window buys - and the true figures are the
        // ones that get written down.
        let (report, totals) = report_over(&one_job(100, None));

        assert_eq!(report.num_jobs(), 1);
        assert_eq!(report.expansion_jobs(), 1);
        assert_eq!(report.total_runtime_seconds(), 3600);
        assert_eq!(report.total_wait_seconds(), 3600);
        assert_eq!(report.average_expansion_factor(), 2.0);
        assert_eq!(report.aggregate_expansion_factor(), 2.0);

        assert!(!totals.saw_unfinished_job);
        assert!(report.is_consistent());
    }

    #[test]
    fn test_an_attempt_that_ends_after_the_window_has_still_ended() {
        // "Finished" is a question about the moment we read the record, not
        // about whether it finished inside the window being reported on. Job
        // 900's attempt starts on day one and ends on day two, and its elapsed
        // time is final either way - so day one records its full runtime and
        // stays completable. Asking whether it ended *inside* the window would
        // call every attempt spanning midnight unfinished and stop the day it
        // began in from ever being cached.
        let (report, totals) = report_over(&one_job(900, None));

        assert_eq!(report.num_jobs(), 1);
        assert_eq!(report.expansion_jobs(), 1);
        assert_eq!(report.total_runtime_seconds(), 43200);
        assert!(!totals.saw_unfinished_job);

        // day one still only bills the part of it that fell inside day one
        assert_eq!(report.total_usage(), Usage::new(14400));
    }

    #[test]
    fn test_a_requeue_is_counted_on_the_day_it_happened_not_the_day_it_started() {
        // The regression this test exists for. A requeue event was only counted
        // if the superseded attempt had also *started* inside the window, a
        // guard copied from the job count without noticing that the two need
        // opposite treatment. The attempts that get requeued are the long ones,
        // so the requeue almost always falls on the day after the attempt
        // began, and the guard could almost never be satisfied: on real data
        // the event count came out as 1 where there were several, while the
        // usage those events accounted for was correct throughout.
        //
        // Job 900 is that shape - an attempt running past midnight, requeued on
        // day two, replaced by an attempt that never ran.
        let (day_one_report, _) = report_for(day_one());
        let (day_two_report, _) = report_for(day_two());

        // day one cannot see the requeue: the successor does not exist yet, so
        // the attempt is still the job's last one, and its usage is reported as
        // ordinary usage - exactly as default sacct reported it
        assert_eq!(day_one_report.requeue_events_for_user("user_six"), 0);
        assert_eq!(day_one_report.requeue_usage("user_six"), Usage::default());
        assert_eq!(day_one_report.usage("user_six"), Usage::new(14400));

        // day two sees it, and counts it, even though the attempt started the
        // day before
        assert_eq!(day_two_report.requeue_events_for_user("user_six"), 1);
        assert_eq!(day_two_report.requeue_usage("user_six"), Usage::new(28800));

        // counted once across the two days, not twice and not never
        assert_eq!(
            day_one_report.requeue_events_for_user("user_six")
                + day_two_report.requeue_events_for_user("user_six"),
            1
        );
    }

    #[test]
    fn test_a_job_is_counted_once_however_many_windows_it_spans() {
        // The other half of the asymmetry: a job *does* need the guard the
        // requeue count must not have. Job 900's attempt is the job's last one
        // on day one and a superseded one on day two, and it must be counted as
        // a job exactly once - on the day it started.
        let (day_one_report, _) = report_for(day_one());
        let (day_two_report, _) = report_for(day_two());

        assert_eq!(day_one_report.num_jobs_for_user("user_six"), 1);
        assert_eq!(day_two_report.num_jobs_for_user("user_six"), 0);
    }

    #[test]
    fn test_the_days_report_splits_usage_without_losing_or_repeating_any() {
        let (report, totals) = report_for(day_one());

        // what we have always reported, unchanged by requeue accounting
        assert_eq!(report.total_usage(), Usage::new(28800));
        assert_eq!(report.num_jobs(), 9);

        // and what was invisible before
        assert_eq!(report.total_requeue_usage(), Usage::new(20700));
        assert_eq!(report.num_requeue_events(), 7);
        assert_eq!(
            report.total_usage_including_requeues(),
            Usage::new(28800 + 20700)
        );

        // the shadow counters agree with the report's own totals, and the
        // report agrees with itself
        assert_eq!(totals.usage, report.total_usage().seconds());
        assert_eq!(totals.requeue_usage, report.total_requeue_usage().seconds());
        assert_eq!(totals.requeue_events, report.num_requeue_events());
        assert!(report.is_consistent());
    }

    #[test]
    fn test_requeue_events_are_attributed_to_the_state_that_interrupted_them() {
        // Which state did the interrupting is the difference between "the
        // project spent this" and "the site lost this", so it has to survive
        // into the report rather than being flattened into one requeue total.
        let (report, _) = report_for(day_one());

        assert_eq!(
            report.requeue_states(),
            vec![
                ("NODE_FAIL".to_string(), 2),
                ("OTHER".to_string(), 1),
                ("PREEMPTED".to_string(), 1),
                ("REQUEUED".to_string(), 3),
            ]
        );

        // the per-state maps account for every event and every second
        assert_eq!(
            report
                .requeue_states()
                .iter()
                .map(|(_, count)| count)
                .sum::<u64>(),
            report.num_requeue_events()
        );
        assert_eq!(
            report
                .requeue_states()
                .iter()
                .map(|(state, _)| report.requeue_usage_in_state(state))
                .sum::<Usage>(),
            report.total_requeue_usage()
        );
    }

    #[test]
    fn test_the_three_wait_figures_are_each_exact() {
        // A client can ask for the wait excluding requeues (what it always
        // got), the wait per requeue, or the total wait per job including every
        // attempt - and none of the three double counts, because a record is
        // either a job's last attempt in a window or a superseded one.
        let (report, _) = report_for(day_one());

        assert_eq!(report.total_wait_seconds(), 66480);
        assert_eq!(report.requeue_wait_seconds(), 21000);

        assert_eq!(report.average_wait_seconds(), 66480 / 9);
        assert_eq!(report.average_requeue_wait_seconds(), 21000 / 7);
        assert_eq!(
            report.average_wait_seconds_including_requeues(),
            (66480 + 21000) / 9
        );
    }

    #[test]
    fn test_the_mean_job_size_comes_from_what_slurm_allocated() {
        // The fixture's jobs each hold a whole 128-core node with no GPUs, so
        // the mean job size is 128 cores - and it is recorded once per job,
        // regardless of how many attempts that job took.
        let (report, _) = report_for(day_one());

        assert_eq!(report.num_jobs(), 9);
        assert_eq!(report.total_allocated_cpus(), 9 * 128);
        assert_eq!(report.average_cpus_per_job(), 128.0);
        assert_eq!(report.average_gpus_per_job(), 0.0);

        // job 300 took three attempts and is still one 128-core job
        assert_eq!(report.average_cpus_per_job_for_user("user_two"), 128.0);
        assert!(report.is_consistent());
    }

    #[test]
    fn test_the_expansion_factor_uses_the_whole_job_not_the_windowed_part() {
        // Both halves of the ratio are properties of the job rather than of the
        // window it is reported in, so a job running past midnight must not be
        // recorded as having a runtime of "until the window closed" - that would
        // inflate the factor for exactly the long jobs it should reassure about.
        //
        // Job 900's attempt ran for twelve hours from 20:00 on day one, so four
        // of them fall inside day one. It waited twelve hours to start.
        let (report, totals) = report_for(day_one());

        // the runtime counted is the whole twelve hours, not the four
        assert_eq!(report.runtime_seconds_for_user("user_six"), 43200);
        assert_eq!(totals.runtime_seconds, report.total_runtime_seconds());

        // it waited as long as it ran, so 86400 of turnaround over 43200 of
        // runtime - had the runtime been clipped to the four hours inside day
        // one, the same job would have scored 4.0
        assert!((report.expansion_factor_for_user("user_six") - 2.0).abs() < 1e-9);
    }

    #[test]
    fn test_only_the_jobs_counted_as_jobs_contribute_an_expansion_factor() {
        // The mean needs a denominator it agrees with, so the population is
        // exactly `num_jobs` - one job, once, in the window it started in. A
        // superseded attempt has its own wait recorded as requeue wait instead.
        let (day_one_report, _) = report_for(day_one());
        let (day_two_report, _) = report_for(day_two());

        // job 900 is counted on day one, where it started
        assert!(day_one_report.runtime_seconds_for_user("user_six") > 0);

        // on day two the same attempt is a superseded one, so it contributes no
        // expansion factor there - it is not a job that started that day
        assert_eq!(day_two_report.runtime_seconds_for_user("user_six"), 0);
        assert_eq!(day_two_report.average_expansion_factor(), 0.0);

        // and the report agrees with itself
        assert!(day_one_report.is_consistent());
        assert!(day_two_report.is_consistent());
    }

    #[test]
    fn test_the_days_expansion_factors_are_reported_both_ways() {
        let (report, _) = report_for(day_one());

        // nine jobs, each contributing its own ratio
        assert_eq!(report.num_jobs(), 9);
        assert!(report.average_expansion_factor() > 0.0);
        assert!(report.aggregate_expansion_factor() > 0.0);

        // the two differ, which is the reason for carrying both: the mean is
        // moved by the short jobs and the aggregate by the long ones
        assert!(
            (report.average_expansion_factor() - report.aggregate_expansion_factor()).abs() > 1e-9
        );
    }

    #[test]
    fn test_reservation_usage_counts_every_attempt_that_held_the_nodes() {
        // A reservation's occupancy is physical: a superseded attempt held its
        // nodes exactly as the replacement did, so both count towards what went
        // into the reservation. The discarded share is carried alongside so the
        // two can still be separated.
        let (report, _) = report_for(day_one());

        // job 300 ran in gpu_bench: 300s completed, plus 1800s and 900s of node
        // failures that occupied the reservation before it
        assert_eq!(report.reservation_usage("gpu_bench"), Usage::new(3000));
        assert_eq!(
            report.reservation_requeue_usage("gpu_bench"),
            Usage::new(2700)
        );

        // jobs 100 and 200 ran in maintenance_test: 3600 + 1800 completed, plus
        // 3600 preempted
        assert_eq!(
            report.reservation_usage("maintenance_test"),
            Usage::new(9000)
        );
        assert_eq!(
            report.reservation_requeue_usage("maintenance_test"),
            Usage::new(3600)
        );

        // job 400's two attempts ran under two *instances* of `interactive`,
        // which is one reservation as far as a report is concerned: 1800s
        // discarded plus the 600s that finished
        assert_eq!(report.reservation_usage("interactive"), Usage::new(2400));
        assert_eq!(
            report.reservation_requeue_usage("interactive"),
            Usage::new(1800)
        );

        assert_eq!(
            report.reservations(),
            vec!["gpu_bench", "interactive", "maintenance_test"]
        );
        assert!(report.has_reservations());
    }

    #[test]
    fn test_reservation_jobs_are_counted_like_jobs_not_like_records() {
        let (report, _) = report_for(day_one());

        // job 300's three records are one job, in the window it started in
        assert_eq!(report.reservation_jobs("gpu_bench"), 1);
        // jobs 100 and 200
        assert_eq!(report.reservation_jobs("maintenance_test"), 2);

        // job 400 is one job however many reservation instances its attempts
        // ran under
        assert_eq!(report.reservation_jobs("interactive"), 1);
    }

    #[test]
    fn test_reserved_and_unreserved_usage_partition_the_days_consumption() {
        // Reservation usage is a subset of everything consumed, so the two
        // complement each other within the true total rather than within the
        // reported one - the reservation figures count superseded attempts.
        let (report, _) = report_for(day_one());

        assert_eq!(report.total_reservation_usage(), Usage::new(14400));
        assert_eq!(
            report.total_reservation_usage() + report.usage_outside_reservations(),
            report.total_usage_including_requeues()
        );

        // a reservation cannot hold more than the day consumed
        assert!(report.is_consistent());
    }

    #[test]
    fn test_a_day_with_no_reservations_records_none() {
        // Day two holds only job 900, which ran outside any reservation - the
        // overwhelmingly common case.
        let (report, _) = report_for(day_two());

        assert!(!report.has_reservations());
        assert!(report.reservations().is_empty());
        assert_eq!(report.total_reservation_usage(), Usage::default());
        assert_eq!(
            report.usage_outside_reservations(),
            report.total_usage_including_requeues()
        );
    }

    #[test]
    fn test_a_zero_duration_final_attempt_leaves_the_base_figure_alone() {
        // Job 500 ran for two hours, was requeued, and its replacement was
        // cancelled before it ran. Default sacct returned only that
        // zero-elapsed replacement, so the job was reported as having consumed
        // nothing - and it still is, in the figure that has to stay unchanged.
        // All of it is in the requeue figure instead.
        let jobs = consumers_for(day_one());

        assert_eq!(usage_of(&jobs, 500, Attempt::Base), 0);
        assert_eq!(usage_of(&jobs, 500, Attempt::Requeued), 7200);
    }
}
