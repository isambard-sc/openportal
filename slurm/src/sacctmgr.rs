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
    clean_account_name, clean_user_name, get_managed_organization, RequeuePolicy, SlurmAccount,
    SlurmLimit, SlurmUser,
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
    charged_requeue_usage: u64,
    charged_requeue_events: u64,
    charged_requeue_wait_seconds: u64,
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
/// `policy` then decides which of the superseded attempts are charged anyway.
/// A charged attempt is accumulated into the main figures with every other job,
/// exactly as though it had never been requeued, *and* recorded again in the
/// charged-requeue maps so that it can still be seen - those maps describe
/// usage that is already in the total, and are never added to it. See
/// `docs/plans/slurm-requeue-charging-design.md`.
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
    policy: RequeuePolicy,
    totals: &mut ReportTotals,
) {
    let usage = job.billed_node_seconds();
    let wait_seconds = job.wait_time().num_seconds().max(0) as u64;

    // A record that is still running when the window ends reappears in the next
    // window, so counting a *job* needs this guard or a long job is counted
    // once per window it touches.
    let started_in_window = job.original_start_time() >= window_start;

    if job.is_requeued_attempt() && policy.charges(job.terminal_state()) {
        // Charged: the attempt counts as usage like any other, and is recorded
        // in the charged maps alongside rather than instead - those describe a
        // subset of what has just been added, never an addition to it.
        let state = job.terminal_state();

        report.add_charged_requeue_usage(job.user(), Usage::new(usage));
        report.add_charged_requeue_state_usage(state, Usage::new(usage));
        totals.charged_requeue_usage = totals.charged_requeue_usage.saturating_add(usage);

        report.add_charged_requeue_component_usage(
            "cpu",
            job.user(),
            Usage::new(job.cpu_seconds()),
        );
        report.add_charged_requeue_component_usage(
            "memory",
            job.user(),
            Usage::new(job.memory_seconds()),
        );
        report.add_charged_requeue_component_usage(
            "gpu",
            job.user(),
            Usage::new(job.gpu_seconds()),
        );
        report.add_charged_requeue_component_usage(
            "billing",
            job.user(),
            Usage::new(job.billing_seconds()),
        );

        // Counted with no window guard, for the reason set out at length on the
        // absorbed events below: a superseded attempt is classified `Requeued`
        // in exactly one window, the one holding the requeue itself.
        report.add_charged_requeue_events(job.user(), state, 1);
        report.add_charged_requeue_wait_seconds(job.user(), wait_seconds);
        totals.charged_requeue_events = totals.charged_requeue_events.saturating_add(1);
        totals.charged_requeue_wait_seconds = totals
            .charged_requeue_wait_seconds
            .saturating_add(wait_seconds);

        // A charged attempt held the reservation's nodes exactly as an absorbed
        // one did, and `reservation_requeue_usage` is the record of what a
        // reservation's occupancy owed to requeues. It is not a charging
        // figure, so both kinds belong in it.
        if job.is_reserved() {
            report.add_reservation_requeue_usage(job.reservation(), job.user(), Usage::new(usage));
        }

        // ...and then falls through to the usage accumulation below, which is
        // what "charged like any other job" means. What it must *not* pick up
        // is the job count, the expansion factor or the job size: those are
        // properties of a job, counted once in the window its base attempt
        // started in, and a superseded attempt is not a second job.
        report.add_usage(job.user(), Usage::new(usage));
        totals.usage = totals.usage.saturating_add(usage);

        report.add_component_usage("cpu", job.user(), Usage::new(job.cpu_seconds()));
        report.add_component_usage("memory", job.user(), Usage::new(job.memory_seconds()));
        report.add_component_usage("gpu", job.user(), Usage::new(job.gpu_seconds()));
        report.add_component_usage("billing", job.user(), Usage::new(job.billing_seconds()));

        if job.is_reserved() {
            report.add_reservation_usage(job.reservation(), job.user(), Usage::new(usage));
        }

        return;
    }

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
    policy: RequeuePolicy,
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

    if report.num_charged_requeue_events() != totals.charged_requeue_events
        || report.charged_requeue_wait_seconds() != totals.charged_requeue_wait_seconds
        || report.total_charged_requeue_usage().seconds() != totals.charged_requeue_usage
    {
        consistent = false;
        tracing::warn!(
            "Charged requeue inconsistency for project {} on {}: \
             local counters ({} events, {}s wait, {}s usage) differ from report totals \
             ({} events, {}s wait, {}s usage). This may indicate a bug.",
            project.project(),
            day,
            totals.charged_requeue_events,
            totals.charged_requeue_wait_seconds,
            totals.charged_requeue_usage,
            report.num_charged_requeue_events(),
            report.charged_requeue_wait_seconds(),
            report.total_charged_requeue_usage().seconds()
        );
    }

    // A state the report charged for that the policy does not charge for means
    // the two have come apart - a report built under one policy merged with one
    // built under another, or the split applied somewhere that did not ask.
    // Checked here rather than in `is_consistent` because the policy is the
    // agent's, not the report's: a report travels between agents, and the one
    // reading it may be configured differently from the one that wrote it.
    for state in report.charged_requeue_states() {
        if !policy.charges(&state) {
            consistent = false;
            tracing::warn!(
                "Report for project {} on {} charges requeues in state '{}', which the \
                 policy '{}' in force here does not charge. This may indicate a bug, or \
                 a report built under a different policy.",
                project.project(),
                day,
                state,
                policy
            );
        }
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
    is_partial: bool,
    project: &ProjectMapping,
    day: &greatwestern::grammar::Date,
    now: &chrono::DateTime<Utc>,
) {
    if is_partial {
        // some part of the day could not be read at all - see `get_hourly_report`.
        // What we have is the truth about the hours we did read, and nothing at
        // all about the rest, so it must not be completed or cached: a cached
        // day is never re-read, and this one has to be.
        tracing::warn!(
            "Not caching the report for project {} on {}: part of the day could not be \
             read from Slurm, so the report is incomplete.",
            project.project(),
            day
        );
        return;
    }

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

    // Read once for the whole day rather than per record: a day built under two
    // different policies would be internally inconsistent, and nothing else
    // here would notice.
    let policy = cache::get_requeue_policy().await;

    // Hours that Slurm would not answer for. An hour is a small enough piece
    // that losing one is better than losing the whole run, so a failure here
    // is skipped rather than propagated - but the day it belongs to is then
    // only partly known, and must not be completed or cached as if it were
    // whole.
    let mut skipped_hours: usize = 0;

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
                record_job(
                    &mut daily_report,
                    job,
                    &hour_start_time,
                    policy,
                    &mut totals,
                );
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
            .await;

        // An hour is already the smallest piece we know how to ask for, so
        // there is nothing left to fall back to: record that this one is
        // missing and carry on with the rest of the day. This matters most at
        // the end of a long reporting run, where one hour that `sacct` cannot
        // answer would otherwise throw away every hour read before it.
        let jobs = match response {
            Ok(response) => {
                match SlurmJob::get_consumers(&response, &start_time, &end_time, slurm_nodes) {
                    Ok(jobs) => jobs,
                    Err(e) => {
                        tracing::warn!(
                            "Could not read the Slurm records for project {} for {}: {}. \
                             This hour is missing from the report for {}.",
                            project.project(),
                            hour,
                            e,
                            day
                        );
                        skipped_hours = skipped_hours.saturating_add(1);
                        continue;
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    "Could not get usage for project {} for {}: {}. \
                     This hour is missing from the report for {}.",
                    project.project(),
                    hour,
                    e,
                    day
                );
                skipped_hours = skipped_hours.saturating_add(1);
                continue;
            }
        };

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
            record_job(&mut daily_report, job, &start_time, policy, &mut totals);
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

    if skipped_hours > 0 {
        tracing::warn!(
            "The report for project {} on {} is missing {} of the day's hours, which \
             Slurm could not be asked for. Its usage is therefore a lower bound, and \
             the day will be read again rather than cached.",
            project.project(),
            day,
            skipped_hours
        );
    }

    // runtime consistency check: local shadow counters must match the report's scalar totals
    let counters_agree = check_counter_consistency(&daily_report, &totals, policy, project, day);

    complete_and_cache_if_final(
        &mut daily_report,
        &totals,
        counters_agree,
        skipped_hours > 0,
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
    let policy = cache::get_requeue_policy().await;
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
                record_job(&mut daily_report, job, &start_time, policy, &mut totals);
            }

            // runtime consistency check
            let counters_agree =
                check_counter_consistency(&daily_report, &totals, policy, project, day);

            complete_and_cache_if_final(
                &mut daily_report,
                &totals,
                counters_agree,
                false,
                project,
                day,
                &now,
            )
            .await;

            Ok(daily_report)
        }
        Err(e) => {
            // Any failure here means the same thing: a day was more than this
            // `sacct` could answer in one go. A wall-clock timeout is only the
            // politest of the ways that happens - an out-of-memory kill, or a
            // limit enforced on the scheduler's side, exits non-zero instead,
            // and output truncated part-way through comes back as JSON that
            // will not parse. All three used to fall through to an empty
            // report, which reads exactly like a project that ran nothing.
            // Asking for the day an hour at a time is the answer to all of
            // them, and if `sacct` is genuinely broken rather than merely
            // overloaded, the hourly queries say so just as loudly.
            tracing::warn!(
                "Could not get usage for project {} on {}: {}. Switching to hourly reporting.",
                project.project(),
                day,
                e
            );

            get_hourly_report(
                expires,
                project,
                day,
                account,
                slurm_nodes,
                cluster,
                partition_command,
            )
            .await
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

    record_limit_correction(project, &report).await;

    Ok(report)
}

///
/// Update the cached requeue correction from a report we have just built.
///
/// A report must not write to the cluster: this records what the correction
/// should be and stops. The hourly applier in `spawn_limit_applier` does the
/// writing, which keeps a reporting call from turning into a burst of
/// `sacctmgr modify` and keeps a job's deadline from being spent on one.
///
/// Only the current month counts. The caller resets each month's limit and
/// Slurm's counters together, so a correction that reached back into last month
/// would inflate a limit against a counter that no longer holds the usage it is
/// correcting for. A report covering part of the month contributes what it
/// saw - the cache ratchets, so a partial view can only ever be caught up by a
/// fuller one.
///
async fn record_limit_correction(project: &ProjectMapping, report: &ProjectUsageReport) {
    let month = current_correction_month();

    let absorbed: Usage = report
        .dates()
        .into_iter()
        .filter(|day| correction_month(day) == month)
        .map(|day| report.get_report(&day).total_requeue_usage())
        .sum();

    let Ok(account) = SlurmAccount::from_mapping(project) else {
        return;
    };

    let held =
        cache::record_limit_correction(project.project(), account.name(), &month, absorbed).await;

    tracing::debug!(
        "Requeue correction for project {} in {}: {} seconds absorbed this report, {} held.",
        project.project(),
        month,
        absorbed.seconds(),
        held.seconds()
    );
}

///
/// The limit Slurm holds for an account, or `None` if it holds none.
///
/// `None` means the account is *unlimited*, which is a different thing from a
/// limit of zero and must never be corrected into one.
///
async fn slurm_limit_for(
    account: &SlurmAccount,
    expires: &chrono::DateTime<Utc>,
) -> Result<Option<SlurmLimit>, Error> {
    let cluster = cache::get_cluster().await?;

    let cmd = priority_runner(expires).await?.build_command(
        "SACCTMGR",
        vec![
            "--json".to_string(),
            "show".to_string(),
            "association".to_string(),
            "where".to_string(),
            format!("account={}", account.name()),
            format!("cluster={}", cluster),
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

    Ok(limits
        .into_iter()
        .find(|l| l.account() == account.name() && l.cluster() == cluster))
}

///
/// How often the applier looks for a correction to write.
///
/// An hour is ample. The site's Slurm policy *holds* over-spending jobs rather
/// than killing them, so the worst a late correction costs is a job held a
/// little longer, or one that starts with insufficient credit and overspends a
/// little. Neither is worth a tighter loop against `slurmctld`.
const LIMIT_APPLIER_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3600);

/// How long the applier gives itself for one pass.
const LIMIT_APPLIER_EXPIRY_MINUTES: i64 = 30;

///
/// Start the background task that writes requeue corrections into Slurm.
///
/// Reports compute the correction and stop; this writes it. Separating them
/// keeps a reporting call from mutating cluster state, and keeps a burst of
/// `sacctmgr modify` from riding on a nightly sweep of every project.
///
/// Call once at startup. The task runs indefinitely.
///
pub fn spawn_limit_applier() {
    tokio::spawn(async {
        loop {
            tokio::time::sleep(LIMIT_APPLIER_INTERVAL).await;

            if let Err(e) = apply_limit_corrections().await {
                tracing::error!("Could not apply requeue limit corrections: {}", e);
            }
        }
    });
}

///
/// One pass of the applier: bring every project's `GrpTRESMins` up to its
/// requested limit plus this month's correction.
///
/// Four things it will not do, each of which is a way to lose a project's
/// allocation:
///
/// - **It never lowers a limit.** The base/requeue split is window-local, so a
///   recomputed month can come out lower; acting on that would hold a project's
///   jobs for a reclassification. The cache ratchets and so does this.
/// - **It never creates a limit.** An account with no `GrpTRESMins` is
///   unlimited, and correcting an unlimited account would cap it.
/// - **It never corrects a zero.** A requested limit of zero is the caller
///   stopping the project.
/// - **It never touches an account OpenPortal does not manage.**
///
async fn apply_limit_corrections() -> Result<(), Error> {
    let month = current_correction_month();
    let pending = cache::limit_corrections_to_apply(&month).await;

    if pending.is_empty() {
        return Ok(());
    }

    tracing::info!(
        "Applying requeue limit corrections for {} project(s) in {}.",
        pending.len(),
        month
    );

    for (project, correction) in pending {
        let expires = chrono::Utc::now() + chrono::Duration::minutes(LIMIT_APPLIER_EXPIRY_MINUTES);

        // Serialised against everything else that touches this project, so a
        // correction cannot interleave with a `set_limit` and leave Slurm
        // holding one figure while the cache records another.
        let mutex = cache::get_project_mutex(&project).await?;
        let _guard = mutex.lock().await;

        if let Err(e) = apply_limit_correction(&project, &correction, &expires).await {
            tracing::error!(
                "Could not apply the requeue correction for project {}: {}. It will be \
                 retried on the next pass.",
                project,
                e
            );
        }
    }

    Ok(())
}

async fn apply_limit_correction(
    project: &greatwestern::grammar::ProjectIdentifier,
    correction: &cache::LimitCorrection,
    expires: &chrono::DateTime<Utc>,
) -> Result<(), Error> {
    let Some(account) = get_account(correction.account(), expires).await? else {
        tracing::warn!(
            "Not correcting the limit for project {}: Slurm has no account {}.",
            project,
            correction.account()
        );
        return Ok(());
    };

    if !account.is_managed() {
        tracing::warn!(
            "Refusing to correct the limit on Slurm account '{}': it is in organization \
             '{}', not the OpenPortal-managed '{}'.",
            account.name(),
            account.organization(),
            get_managed_organization()
        );
        return Ok(());
    }

    if account.limit().is_zero() {
        // Either the caller has stopped this project, or we do not yet know
        // what it asked for. Neither is a limit to add a correction to, and
        // both would be made worse by inventing one.
        tracing::debug!(
            "Not correcting the limit for account {}: no requested limit is recorded.",
            account.name()
        );
        return Ok(());
    }

    // An account with no `GrpTRESMins` at all is unlimited. Correcting it would
    // cap it, which is the opposite of what a correction is for.
    let holds_a_limit = match slurm_limit_for(&account, expires).await? {
        Some(slurm_limit) => slurm_limit.has_any_limit(),
        None => false,
    };

    if !holds_a_limit {
        tracing::debug!(
            "Not correcting the limit for account {}: Slurm holds no limit for it.",
            account.name()
        );
        return Ok(());
    }

    let applied = corrected_limit(account.limit(), &correction.computed());
    let already = corrected_limit(account.limit(), &correction.applied());

    if applied.seconds() <= already.seconds() {
        tracing::debug!(
            "Requeue correction for account {} would not raise its limit ({} vs {}). \
             Leaving it alone.",
            account.name(),
            applied,
            already
        );
        return Ok(());
    }

    write_slurm_limit(&account, &applied, expires).await?;

    cache::set_applied_limit_correction(
        project,
        account.name(),
        &current_correction_month(),
        correction.computed(),
    )
    .await;

    tracing::info!(
        "Raised the Slurm limit for account {} to {} - {} requested plus {} of requeue \
         usage this month that was absorbed rather than charged.",
        account.name(),
        applied,
        account.limit(),
        correction.computed()
    );

    Ok(())
}

///
/// The month a correction belongs to, as `YYYY-MM`.
///
/// op-slurm does not know when the accounting month turns over - the caller
/// calculates each month's limit after the previous month has been invoiced,
/// and resets Slurm's counters with it. All this has to guarantee is that a
/// correction covers only the current calendar month's jobs; the caller's reset
/// then lands on a correction that is starting from zero again, and the two
/// stay in step without either knowing the other's schedule.
///
fn correction_month(day: &greatwestern::grammar::Date) -> String {
    day.to_chrono().format("%Y-%m").to_string()
}

fn current_correction_month() -> String {
    correction_month(&greatwestern::grammar::Date::today())
}

///
/// The limit to write into Slurm for a project asking for `requested`, given a
/// correction of `correction`.
///
/// Three rules live here, and each of them is a way to lose a project's
/// allocation if it is got wrong:
///
/// - **Zero stays zero.** A requested limit of zero is the caller stopping the
///   project, and adding a correction to it would hand back an allowance at
///   precisely the moment the intent was to withdraw one.
/// - **The correction is rounded up to whole minutes, once.** `GrpTRESMins` is
///   in minutes and `set_limit` truncates on the way in, so a figure recomputed
///   and re-truncated on every pass drifts downward a minute per TRES at a
///   time. Rounding up here means the drift cannot happen and what little is
///   lost is lost in the project's favour.
/// - **The addition saturates.** Release builds are `panic = "abort"` with
///   `overflow-checks = true`, so a bare `+` on figures this size is a process
///   kill rather than a wrong answer.
///
fn corrected_limit(requested: &Usage, correction: &Usage) -> Usage {
    if requested.is_zero() {
        return Usage::default();
    }

    let whole_minutes = correction.seconds().div_ceil(60).saturating_mul(60);

    Usage::new(requested.seconds().saturating_add(whole_minutes))
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

    let project_limit = account.limit();

    let slurm_limit = match slurm_limit_for(&account, expires).await? {
        Some(slurm_limit) => slurm_limit,
        None => {
            tracing::warn!("Could not find limit for account {}", account.name());
            return Err(Error::NotFound(account.name().to_string()));
        }
    };

    let slurm_limit = &slurm_limit;

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

    // What Slurm holds is `requested + correction`, not `requested`, so the
    // account's own limit is the wrong thing to have compared it against and
    // the wrong thing to overwrite with it. Three values, never two - see
    // `docs/plans/slurm-requeue-charging-design.md` §4.1.
    let month = current_correction_month();
    let correction = cache::get_limit_correction(project.project(), &month).await;

    let Some(observed) = actual_slurm_limit else {
        // Slurm holds exactly what we last applied, so the requested limit is
        // whatever it was.
        return Ok(*account.limit());
    };

    let applied = corrected_limit(account.limit(), &correction_or_zero(&correction));

    if observed.seconds() == applied.seconds() {
        return Ok(*account.limit());
    }

    // A cold cache knows no requested limit - `SlurmAccount::construct` leaves
    // it at zero, because the limit is never read from the account itself. What
    // Slurm holds is then the only evidence there is, and it can be decomposed
    // only once the correction is known.
    if account.limit().is_zero() {
        let Some(correction) = correction else {
            tracing::warn!(
                "Do not yet know the requeue correction for account {} in {}, so cannot say \
                 how much of its Slurm limit of {} was asked for. Reporting the Slurm \
                 figure; a usage report for this month will settle it.",
                account.name(),
                month,
                observed
            );

            return Ok(observed);
        };

        let requested = Usage::new(
            observed
                .seconds()
                .saturating_sub(correction.applied().seconds()),
        );

        let mut account = account.clone();
        account.set_limit(&requested);
        cache::add_account(&account).await?;

        tracing::info!(
            "Recovered the requested limit for account {}: {} of the {} Slurm holds, the \
             rest being this month's requeue correction.",
            account.name(),
            requested,
            observed
        );

        return Ok(requested);
    }

    // op-slurm has sole authority over these accounts, so a `GrpTRESMins` that
    // disagrees with what we applied is something that changed behind our back.
    // It is put back, not believed.
    tracing::error!(
        "Slurm limit for account {} is {}, not the {} this agent applied ({} requested plus \
         a {} requeue correction). Something changed it outside OpenPortal. Setting it back.",
        account.name(),
        observed,
        applied,
        account.limit(),
        correction_or_zero(&correction)
    );

    write_slurm_limit(&account, &applied, expires).await?;

    Ok(*account.limit())
}

/// The applied part of a correction, or zero when none is known.
///
/// Only ever used where zero is the *safe* reading - deciding what Slurm ought
/// to be holding right now. Nothing that could lower a limit may use it.
fn correction_or_zero(correction: &Option<cache::LimitCorrection>) -> Usage {
    match correction {
        Some(correction) => correction.applied(),
        None => Usage::default(),
    }
}

///
/// Write `limit` into the account's `GrpTRESMins`, in every TRES the default
/// node describes.
///
/// This is the one place that talks to `sacctmgr` about limits, so that the
/// figure Slurm holds and the figure the cache thinks it holds can only ever be
/// set together. `limit` is the *applied* figure - the requested limit plus the
/// requeue correction - never the requested one.
///
async fn write_slurm_limit(
    account: &SlurmAccount,
    limit: &Usage,
    expires: &chrono::DateTime<Utc>,
) -> Result<(), Error> {
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

    if tres.is_empty() {
        return Ok(());
    }

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

    Ok(())
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

            // The account carries the *requested* limit - what the caller asked
            // for and what `get_limit` gives back. What Slurm is told is that
            // plus this month's requeue correction.
            account.set_limit(limit);

            let month = current_correction_month();
            let correction = cache::get_limit_correction(project.project(), &month).await;

            // Unknown is not zero, but a caller setting a limit is an
            // instruction, not a guess: it is honoured with whatever correction
            // is known, and the hourly applier adds the rest as soon as a usage
            // report has computed it. Writing nothing instead would leave a
            // project running on an old limit the caller has just withdrawn -
            // which is exactly the case where the caller has zeroed it because
            // the project overspent.
            if correction.is_none() && !limit.is_zero() {
                tracing::warn!(
                    "Setting the limit for account {} without knowing this month's requeue \
                     correction - it will be applied once a usage report has computed it.",
                    account.name()
                );
            }

            let applied = corrected_limit(limit, &correction_or_zero(&correction));

            write_slurm_limit(&account, &applied, expires).await?;

            // now we've made the change, save the account to the cache
            cache::add_account(&account).await?;

            cache::set_applied_limit_correction(
                project.project(),
                account.name(),
                &month,
                Usage::new(applied.seconds().saturating_sub(limit.seconds())),
            )
            .await;

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

    /// A project identifier unique to the calling test.
    ///
    /// The cache is a process-wide `static`, so tests that write to it would
    /// otherwise see each other's entries - and `cargo test` runs them in
    /// parallel, which would make the interference intermittent.
    fn test_project() -> greatwestern::grammar::ProjectIdentifier {
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT: AtomicU64 = AtomicU64::new(0);

        let id = NEXT.fetch_add(1, Ordering::Relaxed);

        let Ok(project) =
            greatwestern::grammar::ProjectIdentifier::parse(&format!("proj{}.testportal", id))
        else {
            unreachable!("this is a well-formed project identifier");
        };

        project
    }

    /// Build a daily report the way `get_hourly_report` and `get_daily_report`
    /// do, over the records `sacct` would return for one window.
    ///
    /// Under `NoCharge`, which is the behaviour that existed before there was a
    /// charging policy: these cases are about the classification machinery, and
    /// asserting it against the policy that moves none of it keeps them testing
    /// one thing. `report_for_policy` is for the cases that are about the
    /// policy itself.
    fn report_for(
        window: (chrono::DateTime<Utc>, chrono::DateTime<Utc>),
    ) -> (DailyProjectUsageReport, ReportTotals) {
        report_for_policy(window, RequeuePolicy::NoCharge)
    }

    fn report_for_policy(
        window: (chrono::DateTime<Utc>, chrono::DateTime<Utc>),
        policy: RequeuePolicy,
    ) -> (DailyProjectUsageReport, ReportTotals) {
        let (start, _) = window;
        let mut report = DailyProjectUsageReport::default();
        let mut totals = ReportTotals::default();

        for job in consumers_for(window) {
            record_job(&mut report, &job, &start, policy, &mut totals);
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
            record_job(
                &mut report,
                job,
                &start,
                RequeuePolicy::NoCharge,
                &mut totals,
            );
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
    fn test_a_correction_raises_the_limit_by_whole_minutes() {
        // `GrpTRESMins` is in minutes and `set_limit` truncates on the way in,
        // so a correction recomputed on every pass would drift downward a
        // minute per TRES at a time. Rounded up, once, and what little is lost
        // is lost in the project's favour.
        let requested = Usage::new(3600);

        assert_eq!(
            corrected_limit(&requested, &Usage::new(60)),
            Usage::new(3660)
        );

        // 90 seconds is a minute and a half, and becomes two minutes
        assert_eq!(
            corrected_limit(&requested, &Usage::new(90)),
            Usage::new(3720)
        );

        // a single second still costs a whole minute, upward
        assert_eq!(
            corrected_limit(&requested, &Usage::new(1)),
            Usage::new(3660)
        );

        assert_eq!(corrected_limit(&requested, &Usage::default()), requested);
    }

    #[test]
    fn test_a_correction_is_never_added_to_a_limit_of_zero() {
        // A requested limit of zero is the caller stopping an overspent
        // project. Handing back an allowance at that moment is the one thing
        // this must never do.
        assert_eq!(
            corrected_limit(&Usage::default(), &Usage::new(86400)),
            Usage::default()
        );
    }

    #[test]
    fn test_the_corrected_limit_saturates_rather_than_panicking() {
        // Release builds are `panic = "abort"` with `overflow-checks = true`,
        // so an overflow here is a process kill rather than a wrong answer.
        let huge = Usage::new(u64::MAX);

        assert_eq!(corrected_limit(&huge, &huge), huge);
        assert_eq!(corrected_limit(&huge, &Usage::new(60)), huge);
        assert_eq!(corrected_limit(&Usage::new(60), &huge), huge);
    }

    #[test]
    fn test_a_correction_never_compounds_over_repeated_cycles() {
        // The regression test this whole design turns on. `get_limit` used to
        // adopt whatever Slurm held into the account's own limit - and what
        // Slurm holds is now deliberately `requested + correction`, so adopting
        // it would make the next correction add to an already-corrected base
        // and the limit would ratchet upward on every cycle.
        //
        // Modelled here rather than driven through `sacctmgr`: the arithmetic
        // is the part that compounds.
        let requested = Usage::new(36000);
        let correction = Usage::new(1800);

        let applied = corrected_limit(&requested, &correction);
        assert_eq!(applied, Usage::new(37800));

        // ten more passes over the same month, each recomputing from the
        // requested limit rather than from what Slurm holds
        let mut latest = applied;

        for _ in 0..10 {
            latest = corrected_limit(&requested, &correction);
            assert_eq!(latest, applied);
        }

        // and a correction that has grown raises it exactly once, to the new
        // figure rather than by it
        let grown = corrected_limit(&requested, &Usage::new(3600));
        assert_eq!(grown, Usage::new(39600));
        assert_eq!(latest, applied);
    }

    #[tokio::test]
    async fn test_a_correction_is_unknown_until_it_is_computed() {
        // Unknown is not zero. An agent that has just restarted knows nothing,
        // and reading that as "no correction needed" would push a limit short
        // by however much the site has absorbed this month.
        let project = test_project();
        let month = "2026-09";

        assert!(cache::get_limit_correction(&project, month).await.is_none());

        cache::record_limit_correction(&project, "testaccount", month, Usage::new(600)).await;

        let correction = cache::get_limit_correction(&project, month)
            .await
            .expect("just recorded");

        assert_eq!(correction.computed(), Usage::new(600));

        // nothing has been written into Slurm yet
        assert_eq!(correction.applied(), Usage::default());

        // and a correction for one month says nothing about another
        assert!(cache::get_limit_correction(&project, "2026-10")
            .await
            .is_none());
    }

    #[tokio::test]
    async fn test_a_correction_only_ever_increases_within_a_month() {
        // The base/requeue split is window-local, so a recomputed month can
        // come out lower. Acting on that would lower the Slurm limit and hold a
        // project's jobs for a reclassification rather than for anything it did.
        let project = test_project();
        let month = "2026-11";

        cache::record_limit_correction(&project, "testaccount", month, Usage::new(3600)).await;

        let held =
            cache::record_limit_correction(&project, "testaccount", month, Usage::new(1800)).await;

        assert_eq!(held, Usage::new(3600));

        let held =
            cache::record_limit_correction(&project, "testaccount", month, Usage::new(7200)).await;

        assert_eq!(held, Usage::new(7200));
    }

    #[tokio::test]
    async fn test_a_new_month_replaces_the_correction_rather_than_growing_it() {
        // The caller resets the limit and Slurm's counters together at the turn
        // of the month, so last month's correction is not a smaller correction -
        // it is the wrong one, and carrying it forward would inflate a limit
        // against a counter that no longer holds the usage it corrects for.
        let project = test_project();

        cache::record_limit_correction(&project, "testaccount", "2026-11", Usage::new(7200)).await;

        let held =
            cache::record_limit_correction(&project, "testaccount", "2026-12", Usage::new(60))
                .await;

        assert_eq!(held, Usage::new(60));
        assert!(cache::get_limit_correction(&project, "2026-11")
            .await
            .is_none());

        // and the applied figure starts again from nothing
        let correction = cache::get_limit_correction(&project, "2026-12")
            .await
            .expect("just recorded");
        assert_eq!(correction.applied(), Usage::default());
    }

    #[tokio::test]
    async fn test_only_a_correction_ahead_of_what_was_applied_is_pending() {
        let project = test_project();
        let month = "2027-01";

        cache::record_limit_correction(&project, "testaccount", month, Usage::new(3600)).await;

        let pending = cache::limit_corrections_to_apply(month).await;
        assert!(pending.iter().any(|(p, _)| p == &project));

        cache::set_applied_limit_correction(&project, "testaccount", month, Usage::new(3600)).await;

        let pending = cache::limit_corrections_to_apply(month).await;
        assert!(!pending.iter().any(|(p, _)| p == &project));

        // a month that is not this one is never pending
        cache::record_limit_correction(&project, "testaccount", month, Usage::new(7200)).await;
        let pending = cache::limit_corrections_to_apply("2027-02").await;
        assert!(!pending.iter().any(|(p, _)| p == &project));
    }

    #[test]
    fn test_charging_moves_the_user_s_own_requeues_into_the_usage() {
        // The policy in one assertion: what the site caused stays discarded,
        // what the user asked for is charged, and the true total is the same
        // either way - charging moves usage between buckets, it never invents
        // or loses any.
        let (absorbed, _) = report_for_policy(day_one(), RequeuePolicy::NoCharge);
        let (charged, _) = report_for_policy(day_one(), RequeuePolicy::ChargeRequeueStateOnly);

        let requeued_by_user = absorbed.requeue_usage_in_state("REQUEUED");
        assert!(!requeued_by_user.is_zero(), "the fixture has user requeues");

        assert_eq!(
            charged.total_usage(),
            absorbed.total_usage() + requeued_by_user
        );
        assert_eq!(
            charged.total_requeue_usage(),
            absorbed.total_requeue_usage() - requeued_by_user
        );
        assert_eq!(charged.total_charged_requeue_usage(), requeued_by_user);

        assert_eq!(
            charged.total_usage_including_requeues(),
            absorbed.total_usage_including_requeues()
        );
    }

    #[test]
    fn test_only_the_bare_requeue_state_is_charged() {
        // A record that reports both a node failure and a requeue is a node
        // failure: `terminal_state`'s precedence puts NODE_FAIL first, which is
        // what makes charging the REQUEUED bucket safe. OTHER - a state a
        // future Slurm might report - is never charged either.
        let (report, _) = report_for_policy(day_one(), RequeuePolicy::ChargeRequeueStateOnly);

        assert_eq!(
            report.charged_requeue_states(),
            vec!["REQUEUED".to_string()]
        );

        for state in ["NODE_FAIL", "PREEMPTED", "OTHER"] {
            assert_eq!(report.charged_requeue_events_in_state(state), 0);
            assert!(report.charged_requeue_usage_in_state(state).is_zero());
        }

        // and what was charged is exactly what stopped being absorbed
        let (absorbed, _) = report_for_policy(day_one(), RequeuePolicy::NoCharge);
        assert_eq!(
            report.num_charged_requeue_events(),
            absorbed.requeue_events_in_state("REQUEUED")
        );
        assert_eq!(report.requeue_events_in_state("REQUEUED"), 0);
    }

    #[test]
    fn test_a_charged_requeue_is_usage_but_is_not_a_second_job() {
        // The attempt is charged like any other job, which is about *usage*. A
        // superseded attempt is not an extra job, did not queue a second time
        // for the purposes of the mean wait, and must not move the expansion
        // factor or the mean job size.
        let (absorbed, _) = report_for_policy(day_one(), RequeuePolicy::NoCharge);
        let (charged, _) = report_for_policy(day_one(), RequeuePolicy::ChargeRequeueStateOnly);

        assert_eq!(charged.num_jobs(), absorbed.num_jobs());
        assert_eq!(charged.total_wait_seconds(), absorbed.total_wait_seconds());
        assert_eq!(
            charged.total_runtime_seconds(),
            absorbed.total_runtime_seconds()
        );
        assert_eq!(charged.expansion_jobs(), absorbed.expansion_jobs());
        assert_eq!(
            charged.total_allocated_cpus(),
            absorbed.total_allocated_cpus()
        );
    }

    #[test]
    fn test_no_charge_leaves_every_figure_where_it_was() {
        // The escape hatch has to be exactly the old behaviour, or a site that
        // sets it is not opting out of anything.
        //
        // The figures are the ones this fixture reported before charging
        // existed, written out rather than derived, so that a change to the
        // split has to come here and say so. Other tests in this module pin the
        // same two numbers through `report_for`, but only because that helper
        // happens to default to this policy - which is not a guarantee, it is a
        // default someone could change.
        let (report, totals) = report_for_policy(day_one(), RequeuePolicy::NoCharge);

        assert_eq!(report.total_usage(), Usage::new(28800));
        assert_eq!(report.total_requeue_usage(), Usage::new(20700));
        assert_eq!(report.num_jobs(), 9);
        assert_eq!(report.num_requeue_events(), 7);

        // And the same figures derived from the records rather than recited:
        // with nothing charged, what is reported is exactly the base attempts
        // and what is discarded is exactly the superseded ones. This is the
        // property; the constants above are the witness that it has not moved.
        let jobs = consumers_for(day_one());

        let summed = |requeued: bool| {
            Usage::new(
                jobs.iter()
                    .filter(|job| job.is_requeued_attempt() == requeued)
                    .fold(0u64, |total, job| {
                        total.saturating_add(job.billed_node_seconds())
                    }),
            )
        };

        assert_eq!(report.total_usage(), summed(false));
        assert_eq!(report.total_requeue_usage(), summed(true));

        assert!(!report.has_charged_requeues());
        assert!(report.charged_requeue_states().is_empty());
        assert_eq!(totals.charged_requeue_events, 0);
        assert_eq!(totals.charged_requeue_usage, 0);
    }

    #[test]
    fn test_charging_leaves_the_true_total_and_the_base_attempts_alone() {
        // The other half of the same guarantee: turning charging *on* must not
        // move the base attempts either. Only the superseded ones change
        // bucket, so what a project consumed in total is the same figure under
        // both policies - which is what makes the switch a charging decision
        // rather than a change of measurement.
        let jobs = consumers_for(day_one());

        let base = Usage::new(
            jobs.iter()
                .filter(|job| !job.is_requeued_attempt())
                .fold(0u64, |total, job| {
                    total.saturating_add(job.billed_node_seconds())
                }),
        );

        let (charged, _) = report_for_policy(day_one(), RequeuePolicy::ChargeRequeueStateOnly);

        // the base attempts are untouched: what charging added is exactly the
        // charged requeues, no more
        assert_eq!(
            charged.total_usage(),
            base + charged.total_charged_requeue_usage()
        );

        // and the true total is the same 49500 seconds either way
        assert_eq!(
            charged.total_usage_including_requeues(),
            Usage::new(28800 + 20700)
        );
    }

    #[test]
    fn test_a_charged_day_agrees_with_its_own_counters() {
        // The shadow counters are what the agent uses to decide whether a day
        // may be cached, so the charged ones have to track the report exactly
        // as the absorbed ones do.
        let (report, totals) = report_for_policy(day_one(), RequeuePolicy::ChargeRequeueStateOnly);

        assert_eq!(report.total_usage().seconds(), totals.usage);
        assert_eq!(
            report.total_charged_requeue_usage().seconds(),
            totals.charged_requeue_usage
        );
        assert_eq!(
            report.num_charged_requeue_events(),
            totals.charged_requeue_events
        );
        assert_eq!(
            report.charged_requeue_wait_seconds(),
            totals.charged_requeue_wait_seconds
        );
        assert!(report.is_consistent());
    }

    #[test]
    fn test_the_charged_share_is_of_everything_requeueing_cost() {
        let (report, _) = report_for_policy(day_one(), RequeuePolicy::ChargeRequeueStateOnly);

        let charged = report.total_charged_requeue_usage().seconds();
        let all = report.total_requeue_usage_including_charged().seconds();

        assert!(charged > 0 && charged < all, "the fixture has both kinds");
        assert_eq!(
            report.charged_requeue_share_per_mille(),
            Some(charged * 1000 / all)
        );

        // a day with no requeues at all has no share, which is not zero
        assert_eq!(
            DailyProjectUsageReport::default().charged_requeue_share_per_mille(),
            None
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
