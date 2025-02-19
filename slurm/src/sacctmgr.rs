// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Context;
use anyhow::Result;
use once_cell::sync::Lazy;
use templemeads::grammar::{DateRange, ProjectMapping, UserMapping};
use templemeads::usagereport::{DailyProjectUsageReport, ProjectUsageReport, Usage};
use templemeads::Error;
use tokio::sync::{Mutex, MutexGuard};

use crate::cache;
use crate::slurm::SlurmJob;
use crate::slurm::{
    clean_account_name, clean_user_name, get_managed_organization, SlurmAccount, SlurmUser,
};

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

    pub async fn run(&self, cmd: &str) -> Result<String, Error> {
        let processed_cmd = self.process(cmd)?;

        tracing::info!("Running command: {:?}", processed_cmd);

        let output = match tokio::process::Command::new(&processed_cmd[0])
            .args(&processed_cmd[1..])
            .output()
            .await
        {
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

    pub async fn run_json(&self, cmd: &str) -> Result<serde_json::Value, Error> {
        let output = self.run(cmd).await?;

        match serde_json::from_str(&output) {
            Ok(output) => Ok(output),
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

    runner()
        .await?
        .run(&format!(
            "SACCTMGR --immediate add account name=\"{}\" cluster=\"{}\" organization=\"{}\" description=\"{}\"",
            account.name(),
            cluster,
            account.organization(),
            account.description()
        ))
        .await?;

    Ok(account.clone())
}

async fn get_account_from_slurm(account: &str) -> Result<Option<SlurmAccount>, Error> {
    let account = clean_account_name(account)?;

    let cluster = cache::get_cluster().await?;

    let response = match runner()
        .await?
        .run_json(&format!(
            "SACCTMGR --json list accounts withassoc name={} cluster={}",
            account, cluster
        ))
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

            tracing::info!("Using existing slurm account {}", existing_account);
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
        .run_json(&format!(
            "SACCTMGR --json list users name={} cluster={} WithAssoc",
            user, cluster
        ))
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

    runner().await?.run(
        &format!(
            "SACCTMGR --immediate add account name=\"{}\" Clusters=\"{}\" Associations=\"{}\" Comment=\"Created by OpenPortal\"",
            account.name(),
            cluster,
            account.name()
        )
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
        tracing::info!(
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
                )
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

        tracing::info!("Updated user: {}", user);
    }

    if make_default && *user.default_account() != Some(account.name().to_string()) {
        tracing::info!("Will set user default account here");

        runner()
            .await?
            .run(&format!(
                "SACCTMGR --immediate add user name=\"{}\" Clusters=\"{}\" Accounts=\"{}\" DefaultAccount=\"{}\"",
                user.name(),
                cluster,
                account.name(),
                account.name()
            ))
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
        tracing::info!("Using existing user: {}", user);
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
            tracing::info!("Using existing user {}", slurm_user);
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
                    username, cluster, account, account)
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
    tracing::info!(
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
        .run("SACCTMGR --noheader --parsable2 list clusters")
        .await?;

    // the output is the list of clusters, one per line, separated by '|', where
    // the cluster name is the first column
    let clusters: Vec<String> = clusters
        .lines()
        .map(|line| line.split('|').next().unwrap_or_default().to_string())
        .collect();

    tracing::info!("Clusters: {:?}", clusters);

    if let Some(requested_cluster) = requested_cluster {
        if clusters.contains(&requested_cluster) {
            tracing::info!("Using requested cluster: {}", requested_cluster);
        } else {
            tracing::warn!(
                "Requested cluster {} not found in list of clusters: {:?}",
                requested_cluster,
                clusters
            );
            return Err(Error::Login("Requested cluster not found".to_string()));
        }
    } else {
        tracing::info!(
            "Using the first cluster available by default: {}",
            clusters[0]
        );
        cache::set_cluster(&clusters[0]).await?;
    }

    Ok(())
}

pub async fn add_project(project: &ProjectMapping) -> Result<(), Error> {
    let account = SlurmAccount::from_mapping(project)?;

    let account = get_account_create_if_not_exists(&account).await?;

    tracing::info!("Added account: {}", account);

    Ok(())
}

pub async fn add_user(user: &UserMapping) -> Result<(), Error> {
    let user: SlurmUser = get_user_create_if_not_exists(user).await?;

    tracing::info!("Added user: {}", user);

    Ok(())
}

pub async fn get_usage_report(
    project: &ProjectMapping,
    dates: &DateRange,
) -> Result<ProjectUsageReport, Error> {
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

    // we now request the data day by day
    for day in dates.days() {
        let start_time = day.day().start_time().and_utc();
        let end_time = day.day().end_time().and_utc();

        if start_time > now {
            // we can't get the usage for this day yet as it is in the future
            continue;
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

        // have we got this report in the cache?
        if let Some(daily_report) = cache::get_report(project.project(), &day).await? {
            report.set_report(&day, &daily_report);
            continue;
        }

        // it is not cached, so get from slurm
        let response = runner()
            .await?
            .run_json(&format!(
                "SACCT --noconvert --allocations --allusers --starttime={} --endtime={} --account={} --cluster={} --json",
                day,
                day.next(),
                account.name(),
                cluster
            ))
            .await?;

        let jobs = SlurmJob::get_consumers(&response, &start_time, &end_time, &slurm_nodes)?;

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

            match cache::set_report(project.project(), &day, &daily_report).await {
                Ok(_) => (),
                Err(e) => {
                    tracing::error!("Could not cache report for {}: {}", day, e);
                }
            }
        }

        // now save this to the overall report
        report.set_report(&day, &daily_report);
    }

    Ok(report)
}

pub async fn get_limit(project: &ProjectMapping) -> Result<Usage, Error> {
    // this is a null function for now... just return the cached value
    let account = SlurmAccount::from_mapping(project)?;

    match get_account(account.name()).await? {
        Some(account) => Ok(*account.limit()),
        None => {
            tracing::warn!("Could not get account {}", account.name());
            Err(Error::NotFound(account.name().to_string()))
        }
    }
}

pub async fn set_limit(project: &ProjectMapping, limit: &Usage) -> Result<Usage, Error> {
    // this is a null function for now... it just sets and returns a cached value
    let account = SlurmAccount::from_mapping(project)?;

    match get_account(account.name()).await? {
        Some(account) => {
            let mut account = account.clone();
            account.set_limit(limit);

            cache::add_account(&account).await?;

            Ok(*account.limit())
        }
        None => {
            tracing::warn!("Could not get account {}", account.name());
            Err(Error::NotFound(account.name().to_string()))
        }
    }
}
