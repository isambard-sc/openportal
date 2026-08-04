// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;
use greatwestern::grammar::{Date, Hour, ProjectIdentifier, UserIdentifier};
use greatwestern::usagereport::DailyProjectUsageReport;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Arc;
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

/// Upper bounds on the cache maps.
///
/// These are keyed on peer-supplied identifiers and were never pruned or capped, so a
/// flood of distinct project or user names grew them without limit. See
/// `docs/specifications/security-review-2.md` (finding R33).
///
/// Sized for a large national facility rather than a small one, because fetching usage
/// data taxes `slurmctld` - dropping a cache entry is expensive here in a way it is not
/// for most caches, so the caps are set high enough that they should never be reached
/// in normal operation and memory is spent instead.
const MAX_CACHED_ACCOUNTS: usize = 10_000;
const MAX_CACHED_USERS: usize = 100_000;
/// Projects for which usage data is held (each entry holds up to
/// `MAX_CACHED_DATES_PER_PROJECT` days).
const MAX_CACHED_REPORTS: usize = 10_000;
/// Days of usage held per project - roughly three months, comfortably covering the two
/// months an operator expects to keep. With `MAX_CACHED_REPORTS` this bounds the total
/// at ~1,000,000 daily reports.
const MAX_CACHED_DATES_PER_PROJECT: usize = 100;
const MAX_CACHED_MUTEXES: usize = 100_000;

///
/// Keep the cache maps bounded. Called on every write; O(1) per map in the normal case.
///
/// **Policy: evict, never flush.** Re-fetching usage data taxes `slurmctld`, so a cap
/// breach costs a single entry rather than the whole map. For the date-keyed maps the
/// *oldest* date is dropped, which is the natural policy for a time series and keeps
/// the recent data that queries actually want. For the identifier-keyed maps there is
/// no access order to use, so an arbitrary entry goes - still far better than flushing,
/// since a miss on one project or user re-fetches only that one.
///
/// The **mutex** maps are deliberately different: they are the identity of a lock, not
/// a cache, so dropping one while a task holds its `Arc` would hand the next caller a
/// *different* mutex for the same user and silently lose mutual exclusion. Only entries
/// nobody is holding are dropped - `strong_count == 1` means the map is the sole owner.
///
fn enforce_cache_bounds(cache: &mut Database) {
    evict_arbitrary_until(&mut cache.accounts, MAX_CACHED_ACCOUNTS, "Slurm account");
    evict_arbitrary_until(&mut cache.users, MAX_CACHED_USERS, "Slurm user");
    evict_arbitrary_until(
        &mut cache.reports,
        MAX_CACHED_REPORTS,
        "Slurm usage-report project",
    );

    // Bound each project's own history, oldest first.
    for (project, usage) in cache.reports.iter_mut() {
        evict_oldest_until(
            &mut usage.reports,
            MAX_CACHED_DATES_PER_PROJECT,
            &format!("daily usage for {}", project),
        );
        evict_oldest_until(
            &mut usage.hourly_reports,
            MAX_CACHED_DATES_PER_PROJECT,
            &format!("hourly usage for {}", project),
        );
    }

    retain_held_mutexes(&mut cache.user_mutexes, MAX_CACHED_MUTEXES, "user");
    retain_held_mutexes(&mut cache.project_mutexes, MAX_CACHED_MUTEXES, "project");
}

/// Drop arbitrary entries until `map` is within `max`. Used where there is no access
/// order to exploit - losing one entry costs one re-fetch.
fn evict_arbitrary_until<K, V>(map: &mut HashMap<K, V>, max: usize, what: &str)
where
    K: std::hash::Hash + Eq + Clone,
{
    while map.len() > max {
        let Some(victim) = map.keys().next().cloned() else {
            break;
        };

        tracing::warn!(
            "{} cache is at its {} entry limit - evicting one entry to make room.",
            what,
            max
        );
        map.remove(&victim);
    }
}

/// Drop the oldest-keyed entries until `map` is within `max`. For the date-keyed usage
/// maps, where the recent end is what queries want.
fn evict_oldest_until<K, V>(map: &mut HashMap<K, V>, max: usize, what: &str)
where
    K: std::hash::Hash + Eq + Clone + Ord,
{
    while map.len() > max {
        let Some(oldest) = map.keys().min().cloned() else {
            break;
        };

        tracing::debug!(
            "Cache of {} is at its {} entry limit - dropping the oldest entry.",
            what,
            max
        );
        map.remove(&oldest);
    }
}

/// Drop only those mutexes nobody is currently holding - see `enforce_cache_bounds`.
fn retain_held_mutexes<K>(map: &mut HashMap<K, Arc<Mutex<()>>>, max: usize, what: &str)
where
    K: std::hash::Hash + Eq,
{
    if map.len() <= max {
        return;
    }

    let before = map.len();
    map.retain(|_, mutex| Arc::strong_count(mutex) > 1);

    tracing::warn!(
        "{} mutex map exceeded {} entries - dropped {} that nobody was holding.",
        what,
        max,
        before - map.len()
    );
}

///
/// Return a mutex that can be used to protect this user
///
pub async fn get_user_mutex(identifier: &UserIdentifier) -> Result<Arc<Mutex<()>>, Error> {
    let mut cache = CACHE.write().await;
    enforce_cache_bounds(&mut cache);
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
    enforce_cache_bounds(&mut cache);
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
    enforce_cache_bounds(&mut cache);

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
    enforce_cache_bounds(&mut cache);

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
    enforce_cache_bounds(&mut cache);

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
    enforce_cache_bounds(&mut cache);

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
    enforce_cache_bounds(&mut cache);
    cache.users.insert(user.name().to_string(), user.clone());
    Ok(())
}

pub async fn get_user(name: &str) -> Result<Option<SlurmUser>, Error> {
    let cache = CACHE.read().await;
    Ok(cache.users.get(name).cloned())
}

pub async fn set_default_node(node: &SlurmNode) -> Result<(), Error> {
    let mut cache = CACHE.write().await;
    enforce_cache_bounds(&mut cache);

    match cache.nodes {
        Some(ref mut nodes) => nodes.set_default(node),
        None => cache.nodes = Some(SlurmNodes::new(node)),
    }

    Ok(())
}

#[allow(dead_code)]
pub async fn set_node(name: &str, node: &SlurmNode) -> Result<(), Error> {
    let mut cache = CACHE.write().await;
    enforce_cache_bounds(&mut cache);

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
    enforce_cache_bounds(&mut cache);

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
    enforce_cache_bounds(&mut cache);

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
///
/// Drop everything cached about one Slurm account.
///
/// Used when a cached account is found to disagree with Slurm, or to have been removed
/// behind our back. Only that account's entry is invalidated: the discrepancy says
/// nothing about any *other* account, and flushing the whole cache means every project
/// re-queries `slurmctld` at once - which is expensive enough that dropping data should
/// be as targeted as possible. See
/// `docs/specifications/security-review-2.md` (finding R33).
///
pub async fn remove_account(name: &str) -> Result<(), Error> {
    let mut cache = CACHE.write().await;
    enforce_cache_bounds(&mut cache);
    cache.accounts.remove(name);
    Ok(())
}

///
/// Drop everything cached about one Slurm user. As [`remove_account`].
///
pub async fn remove_user(name: &str) -> Result<(), Error> {
    let mut cache = CACHE.write().await;
    enforce_cache_bounds(&mut cache);
    cache.users.remove(name);
    Ok(())
}

///
/// Drop **all** cached accounts and users.
///
/// Prefer [`remove_account`]/[`remove_user`] where the invalidation concerns a single
/// named object - this forces every project to re-query `slurmctld`.
///
#[allow(dead_code)]
pub async fn clear() -> Result<(), Error> {
    let mut cache = CACHE.write().await;
    enforce_cache_bounds(&mut cache);
    tracing::warn!("Clearing the entire Slurm account and user cache");
    cache.accounts.clear();
    cache.users.clear();
    Ok(())
}
