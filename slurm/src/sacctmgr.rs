// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Context;
use anyhow::Result;
use chrono::Utc;
use once_cell::sync::Lazy;
use templemeads::grammar::{DateRange, ProjectMapping, UserMapping};
use templemeads::job::assert_not_expired;
use templemeads::usagereport::{DailyProjectUsageReport, ProjectUsageReport, Usage};
use templemeads::Error;
use tokio::sync::{Mutex, MutexGuard, Semaphore};

use crate::cache;
use crate::slurm::{
    clean_account_name, clean_user_name, get_managed_organization, SlurmAccount, SlurmLimit,
    SlurmUser,
};
use crate::slurm::{SlurmJob, SlurmNodes};

#[derive(Debug, Clone)]
pub struct SlurmRunner {
    sacct: String,
    sacctmgr: String,
    scontrol: String,
}

impl Default for SlurmRunner {
    fn default() -> Self {
        SlurmRunner {
            sacct: "sacct".to_string(),
            sacctmgr: "sacctmgr".to_string(),
            scontrol: "scontrol".to_string(),
        }
    }
}

impl SlurmRunner {
    pub fn sacct(&self) -> &str {
        &self.sacct
    }

    pub fn sacctmgr(&self) -> &str {
        &self.sacctmgr
    }

    pub fn scontrol(&self) -> &str {
        &self.scontrol
    }

    pub fn process(&self, cmd: &str) -> Result<Vec<String>, Error> {
        // replace all instances of SACCTMGR with the value of sacctmgr
        // and all instances of SCONTROL with the value of scontrol,
        // and then split into a vector using shlex

        // the command should start with SACCTMGR or SCONTROL
        if !cmd.starts_with("SACCTMGR") && !cmd.starts_with("SCONTROL") && !cmd.starts_with("SACCT")
        {
            tracing::error!(
                "Slurm command '{}' does not start with SACCT, SACCTMGR or SCONTROL",
                cmd
            );
            return Err(Error::Call(format!(
                "Command does not start with SACCT, SACCTMGR or SCONTROL: {}",
                cmd
            )));
        }

        match shlex::split(
            &cmd.replace("SACCTMGR", self.sacctmgr())
                .replace("SCONTROL", self.scontrol())
                .replace("SACCT", self.sacct()),
        ) {
            Some(cmd) => Ok(cmd),
            None => {
                tracing::error!("Unable to parse slurm command '{}'", cmd);
                Err(Error::Call(format!("Could not parse command: {}", cmd)))
            }
        }
    }

    pub async fn run(&self, cmd: &str, timeout: std::time::Duration) -> Result<String, Error> {
        let processed_cmd = self.process(cmd)?;

        tracing::debug!("Running command: {:?}", processed_cmd);

        let start_time = chrono::Utc::now();
        let output = tokio::process::Command::new(&processed_cmd[0])
            .args(&processed_cmd[1..])
            .kill_on_drop(true)
            .output();

        // use a tokio timeout to ensure we won't block indefinitely - no job should take more than 60 seconds
        let output = match tokio::time::timeout(timeout, output).await {
            Ok(output) => output,
            Err(_) => {
                tracing::error!(
                    "Command '{}' timed out after {:?} seconds",
                    cmd,
                    timeout.as_secs()
                );
                return Err(Error::Timeout("Command timed out".to_string()));
            }
        };

        let end_time = chrono::Utc::now();

        let duration_ms = (end_time - start_time).num_milliseconds();

        if duration_ms > 2500 {
            tracing::warn!(
                "Running command '{}' took {} seconds",
                cmd,
                duration_ms as f64 / 1000.0
            );
        }

        let output = match output {
            Ok(output) => output,
            Err(e) => {
                tracing::error!("Could not run command '{}': {}", cmd, e);
                tracing::error!("Processed command: {:?}", processed_cmd);
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
                "Command '{}' failed: {}",
                cmd,
                String::from_utf8(output.stderr.clone()).context("Could not parse error")?
            );
            Err(Error::Call(format!(
                "Command '{}' failed: {}",
                cmd,
                String::from_utf8(output.stderr).context("Could not parse error")?
            )))
        }
    }

    pub async fn run_json(
        &self,
        cmd: &str,
        timeout: std::time::Duration,
    ) -> Result<serde_json::Value, Error> {
        let output = self.run(cmd, timeout).await?;

        let start_time = chrono::Utc::now();
        match serde_json::from_str(&output) {
            Ok(output) => {
                let end_time = chrono::Utc::now();
                let duration_ms = (end_time - start_time).num_milliseconds();

                if duration_ms > 2500 {
                    tracing::warn!(
                        "Parsing JSON output of command '{}' took {} seconds",
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

/// A mutex to ensure that only one command is run at a time
static SLURM_RUNNER: Lazy<Mutex<SlurmRunner>> = Lazy::new(|| Mutex::new(SlurmRunner::default()));

/// The default timeout (30 seconds)
pub const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

// function to return the runner protected by a MutexGuard - this ensures
// that we can only run a single slurm command at a time, thereby not
// overloading the server
pub async fn runner<'mg>() -> Result<MutexGuard<'mg, SlurmRunner>, Error> {
    Ok(SLURM_RUNNER.lock().await)
}

async fn force_add_slurm_account(account: &SlurmAccount) -> Result<SlurmAccount, Error> {
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

    runner()
        .await?
        .run(&format!(
            "SACCTMGR --immediate add account name=\"{}\" cluster=\"{}\" parent=\"{}\" organization=\"{}\" description=\"{}\"",
            account.name(),
            cluster,
            parent_account,
            account.organization(),
            account.description()
        ), DEFAULT_TIMEOUT
        )
        .await?;

    Ok(account.clone())
}

async fn get_account_from_slurm(account: &str) -> Result<Option<SlurmAccount>, Error> {
    let account = clean_account_name(account)?;

    let cluster = cache::get_cluster().await?;

    let response = match runner()
        .await?
        .run_json(
            &format!(
                "SACCTMGR --json list accounts withassoc name={} cluster={}",
                account, cluster
            ),
            DEFAULT_TIMEOUT,
        )
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

async fn get_account(account: &str) -> Result<Option<SlurmAccount>, Error> {
    // need to GET /slurm/vX.Y.Z/accounts/{account.name}
    // and return the account if it exists
    let cached_account = cache::get_account(account).await?;

    if let Some(cached_account) = cached_account {
        // double-check that the account actually exists...
        let existing_account = match get_account_from_slurm(cached_account.name()).await {
            Ok(account) => account,
            Err(e) => {
                tracing::warn!("Could not get account {}: {}", cached_account.name(), e);
                cache::clear().await?;
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

                // clear the cache as something has changed behind our back
                cache::clear().await?;

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
            cache::clear().await?;
            return Ok(None);
        }
    }

    // see if we can read the account from slurm
    let account = match get_account_from_slurm(account).await {
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

async fn get_account_create_if_not_exists(account: &SlurmAccount) -> Result<SlurmAccount, Error> {
    let existing_account = get_account(account.name()).await?;

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
    let account = force_add_slurm_account(account).await?;

    // get the account as created
    match get_account(account.name()).await {
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

async fn get_user_from_slurm(user: &str) -> Result<Option<SlurmUser>, Error> {
    let user = clean_user_name(user)?;
    let cluster = cache::get_cluster().await?;

    let response = runner()
        .await?
        .run_json(
            &format!(
                "SACCTMGR --json list users name={} cluster={} WithAssoc",
                user, cluster
            ),
            DEFAULT_TIMEOUT,
        )
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

async fn get_user(user: &str) -> Result<Option<SlurmUser>, Error> {
    let cached_user = cache::get_user(user).await?;

    if let Some(cached_user) = cached_user {
        // double-check that the user actually exists...
        let existing_user = match get_user_from_slurm(cached_user.name()).await {
            Ok(user) => user,
            Err(e) => {
                tracing::warn!("Could not get user {}: {}", cached_user.name(), e);
                cache::clear().await?;
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

                // clear the cache as something has changed behind our back
                cache::clear().await?;

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
            cache::clear().await?;
            return Ok(None);
        }
    }

    // see if we can read the user from slurm
    let user = match get_user_from_slurm(user).await {
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

async fn add_account_association(account: &SlurmAccount) -> Result<(), Error> {
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

    runner().await?.run(
        &format!(
            "SACCTMGR --immediate add account name=\"{}\" Clusters=\"{}\" parent=\"{}\" Associations=\"{}\" Comment=\"Created by OpenPortal\"",
            account.name(),
            cluster,
            parent_account,
            account.name()
        ), DEFAULT_TIMEOUT
    ).await?;

    Ok(())
}

async fn add_user_association(
    user: &SlurmUser,
    account: &SlurmAccount,
    make_default: bool,
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
        add_account_association(account).await?;

        // add the association
        runner()
            .await?
            .run(
                &format!(
                    "SACCTMGR --immediate add user name=\"{}\" Clusters=\"{}\" Accounts=\"{}\" Comment=\"Created by OpenPortal\"",
                    user.name(),
                    cluster,
                    account.name()
                ), DEFAULT_TIMEOUT
            )
            .await?;

        // update the user
        user = match get_user_from_slurm(user.name()).await? {
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

        runner()
            .await?
            .run(&format!(
                "SACCTMGR --immediate add user name=\"{}\" Clusters=\"{}\" Accounts=\"{}\" DefaultAccount=\"{}\"",
                user.name(),
                cluster,
                account.name(),
                account.name()
            ), DEFAULT_TIMEOUT
            )
            .await?;

        // update the user
        user = match get_user_from_slurm(user.name()).await? {
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

async fn get_user_create_if_not_exists(user: &UserMapping) -> Result<SlurmUser, Error> {
    // first, make sure that the account exists
    let slurm_account =
        get_account_create_if_not_exists(&SlurmAccount::from_mapping(&user.clone().into())?)
            .await?;

    let cluster = cache::get_cluster().await?;

    // now get the user from slurm
    let slurm_user = get_user(user.local_user()).await?;

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
    let username = clean_user_name(user.local_user())?;
    let account = clean_account_name(slurm_account.name())?;

    let cluster = cache::get_cluster().await?;

    runner().await?.run(
        &format!("SACCTMGR --immediate add user name=\"{}\" Clusters=\"{}\" Accounts=\"{}\" DefaultAccount=\"{}\" Comment=\"Created by OpenPortal\"",
                    username, cluster, account, account), DEFAULT_TIMEOUT
    ).await?;

    // now load the user from slurm to make sure it exists
    let slurm_user = match get_user(user.local_user()).await? {
        Some(user) => user,
        None => {
            return Err(Error::Call(format!(
                "Could not get user that was just created! '{}'",
                user.local_user()
            )))
        }
    };

    // now add the association to the account, making it the default
    let slurm_user = add_user_association(&slurm_user, &slurm_account, true).await?;

    let user = SlurmUser::from_mapping(user)?;

    // check we have the user we expected
    if slurm_user != user {
        tracing::warn!("User {} exists, but with different details.", user.name());
        tracing::warn!("Existing: {:?}, new: {:?}", slurm_user, user);
    }

    Ok(slurm_user)
}

pub async fn set_commands(sacct: &str, sacctmgr: &str, scontrol: &str) {
    tracing::debug!(
        "Using command line slurmd commands: sacctmgr: {}, scontrol: {}",
        sacctmgr,
        scontrol
    );

    let mut runner = SLURM_RUNNER.lock().await;
    runner.sacct = sacct.to_string();
    runner.sacctmgr = sacctmgr.to_string();
    runner.scontrol = scontrol.to_string();
}

pub async fn find_cluster() -> Result<(), Error> {
    // now get the requested cluster from the cache
    let requested_cluster = cache::get_option_cluster().await?;

    // ask slurm for all of the clusters
    let clusters = runner()
        .await?
        .run(
            "SACCTMGR --noheader --parsable2 list clusters",
            DEFAULT_TIMEOUT,
        )
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
        tracing::debug!(
            "Using the first cluster available by default: {}",
            clusters[0]
        );
        cache::set_cluster(&clusters[0]).await?;
    }

    Ok(())
}

static MAX_CONCURRENT_REQUESTS: Lazy<Semaphore> = Lazy::new(|| Semaphore::new(10));

pub async fn add_project(
    project: &ProjectMapping,
    expires: &chrono::DateTime<Utc>,
) -> Result<(), Error> {
    // ensure that we don't have too many concurrent requests
    let _permit = MAX_CONCURRENT_REQUESTS
        .acquire()
        .await
        .map_err(|_| Error::Call("Failed to acquire semaphore for adding project".to_string()))?;

    assert_not_expired(expires)?;

    let account = SlurmAccount::from_mapping(project)?;

    let account = get_account_create_if_not_exists(&account).await?;

    tracing::info!("Added account: {}", account);

    Ok(())
}

pub async fn add_user(user: &UserMapping, expires: &chrono::DateTime<Utc>) -> Result<(), Error> {
    // ensure that we don't have too many concurrent requests
    let _permit = MAX_CONCURRENT_REQUESTS
        .acquire()
        .await
        .map_err(|_| Error::Call("Failed to acquire semaphore for adding user".to_string()))?;

    assert_not_expired(expires)?;

    let user: SlurmUser = get_user_create_if_not_exists(user).await?;

    tracing::info!("Added user: {}", user);

    Ok(())
}

async fn get_hourly_report(
    expires: &chrono::DateTime<Utc>,
    project: &ProjectMapping,
    day: &templemeads::grammar::Date,
    account: &SlurmAccount,
    slurm_nodes: &SlurmNodes,
    cluster: &str,
    partition_command: &str,
) -> Result<DailyProjectUsageReport, Error> {
    let now = chrono::Utc::now();
    let mut daily_report = DailyProjectUsageReport::default();
    let mut total_usage: u64 = 0;
    let mut num_jobs: u64 = 0;

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

            num_jobs += hourly_report.len() as u64;

            for job in hourly_report {
                total_usage += job.billed_node_seconds();
                daily_report.add_usage(job.user(), Usage::new(job.billed_node_seconds()));
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
        let response = runner()
        .await?
        .run_json(&format!(
            "SACCT --noconvert --allocations --allusers --starttime={} --endtime={} --account={} --cluster={} {} --json",
            start_time.format("%Y-%m-%dT%H:%M:%S"),
            end_time.format("%Y-%m-%dT%H:%M:%S"),
            account.name(),
            cluster,
            partition_command
        ), std::time::Duration::from_secs(120))
        .await?;

        let jobs = SlurmJob::get_consumers(&response, &start_time, &end_time, slurm_nodes)?;

        tracing::debug!(
            "Got {} jobs for project {} on {}",
            jobs.len(),
            project.project(),
            hour
        );

        // cache this hourly report if it is in the past
        if hour.end_time().and_utc() < now {
            match cache::set_hourly_report(project.project(), &hour, &jobs).await {
                Ok(_) => (),
                Err(e) => {
                    tracing::error!("Could not cache hourly report for {}: {}", hour, e);
                }
            }
        }

        num_jobs += jobs.len() as u64;

        for job in jobs {
            total_usage += job.billed_node_seconds();
            daily_report.add_usage(job.user(), Usage::new(job.billed_node_seconds()));
        }
    }

    tracing::debug!(
        "Got {} jobs consuming {} seconds for project {} on {}",
        num_jobs,
        total_usage,
        project.project(),
        day
    );

    // check that the total usage in the daily report matches the total usage calculated manually
    if daily_report.total_usage().seconds() != total_usage {
        // it doesn't - we don't want to mark this as complete or cache it, because
        // this points to some error when generating the values...
        tracing::error!(
            "Total usage in daily report does not match total usage calculated manually: {} != {}",
            daily_report.total_usage().seconds(),
            total_usage
        );
    } else if day.day().end_time().and_utc() < now {
        // we can set this day as completed if it is in the past
        daily_report.set_complete();

        match cache::set_report(project.project(), day, &daily_report).await {
            Ok(_) => (),
            Err(e) => {
                tracing::error!("Could not cache report for {}: {}", day, e);
            }
        }
    }

    Ok(daily_report)
}

async fn get_daily_report(
    expires: &chrono::DateTime<Utc>,
    project: &ProjectMapping,
    day: &templemeads::grammar::Date,
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
    let response = runner()
            .await?
            .run_json(&format!(
                "SACCT --noconvert --allocations --allusers --starttime={} --endtime={} --account={} --cluster={} {} --json",
                start_time.format("%Y-%m-%dT%H:%M:%S"),
                end_time.format("%Y-%m-%dT%H:%M:%S"),
                account.name(),
                cluster,
                partition_command
            ), std::time::Duration::from_secs(20))
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
            let mut total_usage: u64 = 0;

            for job in jobs {
                total_usage += job.billed_node_seconds();
                daily_report.add_usage(job.user(), Usage::new(job.billed_node_seconds()));
            }

            // check that the total usage in the daily report matches the total usage calculated manually
            if daily_report.total_usage().seconds() != total_usage {
                // it doesn't - we don't want to mark this as complete or cache it, because
                // this points to some error when generating the values...
                tracing::error!(
                    "Total usage in daily report does not match total usage calculated manually: {} != {}",
                    daily_report.total_usage().seconds(),
                    total_usage
                );
            } else if day.day().end_time().and_utc() < now {
                // we can set this day as completed if it is in the past
                daily_report.set_complete();

                match cache::set_report(project.project(), day, &daily_report).await {
                    Ok(_) => (),
                    Err(e) => {
                        tracing::error!("Could not cache report for {}: {}", day, e);
                    }
                }
            }
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
    // ensure that we don't have too many concurrent requests
    let _permit = MAX_CONCURRENT_REQUESTS
        .acquire()
        .await
        .map_err(|_| Error::Call("Failed to acquire semaphore for getting usage".to_string()))?;

    assert_not_expired(expires)?;

    let account = SlurmAccount::from_mapping(project)?;

    let account = match get_account(account.name()).await {
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

    // we now request the data day by day
    for day in dates.days() {
        if day.day().start_time().and_utc() > now {
            // we can't get the usage for this day yet as it is in the future
            continue;
        }

        let daily_report = match get_daily_report(
            expires,
            project,
            &day,
            &account,
            &slurm_nodes,
            &cluster,
            &partition_command,
        )
        .await
        {
            Ok(report) => report,
            Err(e) => {
                tracing::warn!(
                    "Could not get usage for project {} on {}: {}",
                    project.project(),
                    day,
                    e
                );
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
    // ensure that we don't have too many concurrent requests
    let _permit = MAX_CONCURRENT_REQUESTS
        .acquire()
        .await
        .map_err(|_| Error::Call("Failed to acquire semaphore for getting limit".to_string()))?;

    assert_not_expired(expires)?;

    let account = SlurmAccount::from_mapping(project)?;

    let account = match get_account(account.name()).await? {
        Some(account) => account,
        None => {
            tracing::warn!("Could not get account {}", account.name());
            return Err(Error::NotFound(account.name().to_string()));
        }
    };

    // check that the limits in slurm match up...
    let response = runner()
        .await?
        .run_json(
            &format!(
                "SACCTMGR --json show association where account={} cluster={}",
                account.name(),
                cache::get_cluster().await?
            ),
            DEFAULT_TIMEOUT,
        )
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
    // ensure that we don't have too many concurrent requests
    let _permit = MAX_CONCURRENT_REQUESTS
        .acquire()
        .await
        .map_err(|_| Error::Call("Failed to acquire semaphore for setting limit".to_string()))?;

    assert_not_expired(expires)?;

    let account = SlurmAccount::from_mapping(project)?;

    match get_account(account.name()).await? {
        Some(account) => {
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

            if !tres.is_empty() {
                runner()
                    .await?
                    .run(&format!(
                        "SACCTMGR --immediate modify account {} set GrpTRESMins={} where cluster={}",
                        account.name(),
                        tres.join(","),
                        cluster,
                    ), DEFAULT_TIMEOUT
                    )
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
