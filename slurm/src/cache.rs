// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Arc;
use templemeads::grammar::{Date, Hour, ProjectIdentifier, UserIdentifier};
use templemeads::usagereport::DailyProjectUsageReport;
use templemeads::Error;
use tokio::sync::{Mutex, RwLock};

use crate::slurm::{SlurmAccount, SlurmJob, SlurmNode, SlurmNodes, SlurmUser};

#[derive(Debug, Clone, Default)]
struct UsageDatabase {
    reports: HashMap<Date, DailyProjectUsageReport>,
    hourly_reports: HashMap<Date, HashMap<Hour, Vec<SlurmJob>>>,
}

#[derive(Debug, Clone, Default)]
struct Database {
    cluster: Option<String>,
    partition: Option<String>,
    parent_account: String,
    accounts: HashMap<String, SlurmAccount>,
    users: HashMap<String, SlurmUser>,
    nodes: Option<SlurmNodes>,
    reports: HashMap<ProjectIdentifier, UsageDatabase>,
    user_mutexes: HashMap<UserIdentifier, Arc<Mutex<()>>>,
    project_mutexes: HashMap<ProjectIdentifier, Arc<Mutex<()>>>,
}

static CACHE: Lazy<RwLock<Database>> = Lazy::new(|| RwLock::new(Database::default()));

///
/// Return a mutex that can be used to protect this user
///
pub async fn get_user_mutex(identifier: &UserIdentifier) -> Result<Arc<Mutex<()>>, Error> {
    let mut cache = CACHE.write().await;
    Ok(cache
        .user_mutexes
        .entry(identifier.clone())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone())
}

///
/// Return a mutex that can be used to protect this project
///
pub async fn get_project_mutex(identifier: &ProjectIdentifier) -> Result<Arc<Mutex<()>>, Error> {
    let mut cache = CACHE.write().await;
    Ok(cache
        .project_mutexes
        .entry(identifier.clone())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone())
}

pub async fn get_option_cluster() -> Result<Option<String>, Error> {
    let cache = CACHE.read().await;
    Ok(cache.cluster.clone())
}

pub async fn get_cluster() -> Result<String, Error> {
    let cache = CACHE.read().await;

    match cache.cluster {
        Some(ref cluster) => Ok(cluster.clone()),
        None => Ok("linux".to_string()),
    }
}

pub async fn set_cluster(cluster: &str) -> Result<(), Error> {
    let mut cache = CACHE.write().await;

    if cache.cluster != Some(cluster.to_string()) {
        cache.accounts.clear();
        cache.users.clear();
        cache.reports.clear();
    }

    cache.cluster = Some(cluster.to_string());
    Ok(())
}

pub async fn get_partition() -> Result<Option<String>, Error> {
    let cache = CACHE.read().await;

    match cache.partition {
        Some(ref partition) => Ok(Some(partition.clone())),
        None => Ok(None),
    }
}

pub async fn set_partition(partition: &str) -> Result<(), Error> {
    let mut cache = CACHE.write().await;

    let partition = partition.trim();

    if partition.is_empty() {
        cache.partition = None;
    } else {
        cache.partition = Some(partition.to_string());
    }

    Ok(())
}

pub async fn set_parent_account(parent_account: &str) -> Result<(), Error> {
    let parent_account = parent_account.trim();

    if parent_account.is_empty() {
        return Err(Error::Bug("Parent account cannot be empty".to_string()));
    }

    let mut cache = CACHE.write().await;

    cache.parent_account = parent_account.to_string();

    Ok(())
}

///
/// Return the name of the parent account
///
pub async fn get_parent_account() -> Result<String, Error> {
    let cache = CACHE.read().await;

    if cache.parent_account.is_empty() {
        return Err(Error::Bug("Parent account has not been set".to_string()));
    }

    Ok(cache.parent_account.clone())
}

///
/// Return the account from the cache - this is guaranteed to
/// be an account that is associated with the cluster being managed
///
pub async fn get_account(name: &str) -> Result<Option<SlurmAccount>, Error> {
    let cache = CACHE.read().await;
    Ok(cache.accounts.get(name).cloned())
}

///
/// Add an account to the cache - note that this will silently
/// ignore accounts that are not associated with the cluster
///
pub async fn add_account(account: &SlurmAccount) -> Result<(), Error> {
    let mut cache = CACHE.write().await;

    // we only cache accounts that match the cluster
    if let Some(ref cluster) = cache.cluster {
        if !account.in_cluster(cluster) {
            tracing::warn!(
                "Ignoring account '{}' as it is not associated with cluster '{}'",
                account.name(),
                cluster
            );
            return Ok(());
        }
    }

    cache
        .accounts
        .insert(account.name().to_string(), account.clone());
    Ok(())
}

pub async fn add_user(user: &SlurmUser) -> Result<(), Error> {
    let mut cache = CACHE.write().await;
    cache.users.insert(user.name().to_string(), user.clone());
    Ok(())
}

pub async fn get_user(name: &str) -> Result<Option<SlurmUser>, Error> {
    let cache = CACHE.read().await;
    Ok(cache.users.get(name).cloned())
}

pub async fn set_default_node(node: &SlurmNode) -> Result<(), Error> {
    let mut cache = CACHE.write().await;

    match cache.nodes {
        Some(ref mut nodes) => nodes.set_default(node),
        None => cache.nodes = Some(SlurmNodes::new(node)),
    }

    Ok(())
}

#[allow(dead_code)]
pub async fn set_node(name: &str, node: &SlurmNode) -> Result<(), Error> {
    let mut cache = CACHE.write().await;

    match cache.nodes {
        Some(ref mut nodes) => nodes.set(name, node),
        None => {
            let mut nodes = SlurmNodes::new(node);
            nodes.set(name, node);
            cache.nodes = Some(nodes);
        }
    }

    Ok(())
}

pub async fn get_default_node() -> Result<SlurmNode, Error> {
    let cache = CACHE.read().await;

    match cache.nodes {
        Some(ref nodes) => Ok(nodes.get_default().clone()),
        None => Err(Error::Bug(
            "No nodes have been set in the cache".to_string(),
        )),
    }
}

pub async fn get_nodes() -> Result<SlurmNodes, Error> {
    let cache = CACHE.read().await;

    match cache.nodes {
        Some(ref nodes) => Ok(nodes.clone()),
        None => Err(Error::Bug(
            "No nodes have been set in the cache".to_string(),
        )),
    }
}

pub async fn get_report(
    project: &ProjectIdentifier,
    date: &Date,
) -> Result<Option<DailyProjectUsageReport>, Error> {
    let cache = CACHE.read().await;

    match cache.reports.get(project) {
        Some(usage) => Ok(usage.reports.get(date).cloned()),
        None => Ok(None),
    }
}

pub async fn set_report(
    project: &ProjectIdentifier,
    date: &Date,
    report: &DailyProjectUsageReport,
) -> Result<(), Error> {
    let today = Date::today();

    if date > &today {
        return Err(Error::Bug(format!(
            "Cannot cache a report for project '{}' for future date: {} - {}",
            project, date, report
        )));
    }

    if !report.is_complete() {
        return Err(Error::Bug(format!(
            "Cannot cache an incomplete report for project '{}' for date: {} - {}",
            project, date, report
        )));
    }

    let mut cache = CACHE.write().await;

    match cache.reports.get_mut(project) {
        Some(usage) => {
            // delete the oldest reports while there are >= 80 reports cached
            // This ensures we only cache a maximum of 80 days of reports
            // per project
            while usage.reports.len() >= 80 {
                let mut oldest = today.clone();

                for date in usage.reports.keys() {
                    if date < &oldest {
                        oldest = date.clone();
                    }
                }

                usage.reports.remove(&oldest);
            }

            usage.reports.insert(date.clone(), report.clone());

            // also remove any hourly report for this date
            usage.hourly_reports.remove(date);
        }
        None => {
            let mut usage = UsageDatabase::default();
            usage.reports.insert(date.clone(), report.clone());
            cache.reports.insert(project.clone(), usage);
        }
    }

    Ok(())
}

///
/// Return whether or not we need to get the report hourly for this
/// project and date
///
pub async fn compute_via_hourly_reports(
    project: &ProjectIdentifier,
    date: &Date,
) -> Result<bool, Error> {
    let cache = CACHE.read().await;

    match cache.reports.get(project) {
        Some(usage) => Ok(usage.hourly_reports.contains_key(date)),
        None => Ok(false),
    }
}

///
/// Set the hourly reports collected so far for this project and date
/// (they should be in hour order)
///
pub async fn set_hourly_report(
    project: &ProjectIdentifier,
    hour: &Hour,
    reports: &[SlurmJob],
) -> Result<(), Error> {
    let date = hour.day();
    let today = Date::today();

    if date > today {
        return Err(Error::Bug(format!(
            "Cannot cache hourly reports for project '{}' for future date: {} - {} reports",
            project,
            date,
            reports.len()
        )));
    }

    let mut cache = CACHE.write().await;

    match cache.reports.get_mut(project) {
        Some(usage) => match usage.hourly_reports.get_mut(&date) {
            Some(date_reports) => {
                date_reports.insert(hour.clone(), reports.to_vec());
            }
            None => {
                let mut date_reports = HashMap::new();
                date_reports.insert(hour.clone(), reports.to_vec());
                usage.hourly_reports.insert(date.clone(), date_reports);
            }
        },
        None => {
            let mut usage = UsageDatabase::default();
            let mut date_reports = HashMap::new();
            date_reports.insert(hour.clone(), reports.to_vec());
            usage.hourly_reports.insert(date.clone(), date_reports);
            cache.reports.insert(project.clone(), usage);
        }
    }

    Ok(())
}

///
/// Get the hourly reports collected so far for this project and date.
/// They are returned in hour order
///
pub async fn get_hourly_report(
    project: &ProjectIdentifier,
    hour: &Hour,
) -> Result<Option<Vec<SlurmJob>>, Error> {
    let date = hour.day();
    let cache = CACHE.read().await;

    match cache.reports.get(project) {
        Some(usage) => match usage.hourly_reports.get(&date) {
            Some(date_reports) => match date_reports.get(hour) {
                Some(reports) => Ok(Some(reports.clone())),
                None => Ok(None),
            },
            None => Ok(None),
        },
        None => Ok(None),
    }
}

///
/// Clear the cache - we need to do this if Slurm is changed behine
/// our back
///
pub async fn clear() -> Result<(), Error> {
    let mut cache = CACHE.write().await;
    cache.accounts.clear();
    cache.users.clear();
    Ok(())
}
