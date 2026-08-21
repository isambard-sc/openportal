// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;
use greatwestern::grammar::{ProjectIdentifier, UserIdentifier};
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use templemeads::agent::Peer;
use templemeads::Error;
use tokio::sync::{Mutex, RwLock};

use std::sync::Arc;

use crate::freeipa::{get_op_instance_group, IPAGroup, IPAUser};

/// This file manages the directory of all users added to the system

#[derive(Debug, Clone, Default)]
struct Database {
    users: HashMap<UserIdentifier, IPAUser>,
    groups: HashMap<ProjectIdentifier, IPAGroup>,
    system_groups: Vec<IPAGroup>,
    instance_groups: HashMap<Peer, Vec<IPAGroup>>,
    users_in_group: HashMap<ProjectIdentifier, HashSet<UserIdentifier>>,
    user_mutexes: HashMap<UserIdentifier, Arc<Mutex<()>>>,
    group_mutexes: HashMap<ProjectIdentifier, Arc<Mutex<()>>>,
}

static CACHE: Lazy<RwLock<Database>> = Lazy::new(|| RwLock::new(Database::default()));

/// Upper bounds on the cache maps - keyed on peer-supplied identifiers and previously
/// never pruned or capped. See `docs/specifications/security-review-2.md` (finding R33).
const MAX_CACHED_USERS: usize = 10_000;
const MAX_CACHED_GROUPS: usize = 10_000;
const MAX_CACHED_INSTANCE_GROUPS: usize = 1_000;
const MAX_CACHED_MUTEXES: usize = 10_000;

///
/// Keep the cache maps bounded. Called on every write, which is O(1) per map.
///
/// The data maps are pure caches, so they are flushed wholesale when they exceed their
/// cap - a miss just re-queries FreeIPA. The caps are far above any real directory.
///
/// The **mutex** map is handled differently: it is the identity of a lock, not a
/// cache, so clearing it while a task holds its `Arc` would give the next caller a
/// different mutex for the same user and silently lose mutual exclusion. Only entries
/// nobody holds are dropped (`strong_count == 1` means the map is the sole owner).
///
fn enforce_cache_bounds(cache: &mut Database) {
    if cache.users.len() > MAX_CACHED_USERS {
        tracing::warn!(
            "FreeIPA user cache exceeded {} entries - flushing it.",
            MAX_CACHED_USERS
        );
        cache.users.clear();
    }

    if cache.groups.len() > MAX_CACHED_GROUPS {
        tracing::warn!(
            "FreeIPA group cache exceeded {} entries - flushing it.",
            MAX_CACHED_GROUPS
        );
        cache.groups.clear();
    }

    if cache.users_in_group.len() > MAX_CACHED_GROUPS {
        tracing::warn!(
            "FreeIPA group-membership cache exceeded {} entries - flushing it.",
            MAX_CACHED_GROUPS
        );
        cache.users_in_group.clear();
    }

    if cache.instance_groups.len() > MAX_CACHED_INSTANCE_GROUPS {
        tracing::warn!(
            "FreeIPA instance-group cache exceeded {} peers - flushing it.",
            MAX_CACHED_INSTANCE_GROUPS
        );
        cache.instance_groups.clear();
    }

    if cache.user_mutexes.len() > MAX_CACHED_MUTEXES {
        let before = cache.user_mutexes.len();
        cache
            .user_mutexes
            .retain(|_, mutex| std::sync::Arc::strong_count(mutex) > 1);
        tracing::warn!(
            "User mutex map exceeded {} entries - dropped {} that nobody was holding.",
            MAX_CACHED_MUTEXES,
            before - cache.user_mutexes.len()
        );
    }

    if cache.group_mutexes.len() > MAX_CACHED_MUTEXES {
        let before = cache.group_mutexes.len();
        cache
            .group_mutexes
            .retain(|_, mutex| std::sync::Arc::strong_count(mutex) > 1);
        tracing::warn!(
            "Group mutex map exceeded {} entries - dropped {} that nobody was holding.",
            MAX_CACHED_MUTEXES,
            before - cache.group_mutexes.len()
        );
    }
}

///
/// Return the IPAUser for the passed UserIdentifier, if this
/// user exists in the system. Returns None if the user does not
///
pub async fn get_user(identifier: &UserIdentifier) -> Result<Option<IPAUser>, Error> {
    let cache = CACHE.read().await;
    Ok(cache.users.get(identifier).cloned())
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
/// Return a mutex that can be used to protect this group
///
/// This is the counterpart of `get_user_mutex`, and exists for the same
/// reason: only one task at a time may decide that a group needs creating.
/// Unlike users, concurrent group creations are not collapsed by the job
/// Board - two AddUser jobs for different users in one project both need that
/// project's group - so without this lock they race, and two `group_add`
/// calls for one cn on two masters leave an unreconcilable LDAP replication
/// conflict behind.
///
pub async fn get_group_mutex(identifier: &ProjectIdentifier) -> Result<Arc<Mutex<()>>, Error> {
    let mut cache = CACHE.write().await;
    enforce_cache_bounds(&mut cache);
    Ok(cache
        .group_mutexes
        .entry(identifier.clone())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone())
}

///
/// Remember that the passed user is associated with the passed group
/// Currently unused, but want to keep it around for future use
///
#[allow(dead_code)]
pub async fn add_user_to_group(user: &IPAUser, group: &IPAGroup) -> Result<(), Error> {
    let mut cache = CACHE.write().await;
    enforce_cache_bounds(&mut cache);

    let user = match cache.users.get(user.identifier()) {
        Some(user) => user.clone(),
        None => {
            cache.users.insert(user.identifier().clone(), user.clone());
            user.clone()
        }
    };

    let group = match cache.groups.get(group.identifier()) {
        Some(group) => group.clone(),
        None => {
            cache
                .groups
                .insert(group.identifier().clone(), group.clone());
            group.clone()
        }
    };

    cache
        .users_in_group
        .entry(group.identifier().clone())
        .or_insert_with(HashSet::new)
        .insert(user.identifier().clone());
    Ok(())
}

///
/// Remember that the passed user is associated with the passed groups
///
pub async fn add_user_to_groups(user: &IPAUser, groups: &[IPAGroup]) -> Result<(), Error> {
    if groups.is_empty() {
        return Ok(());
    }

    let mut cache = CACHE.write().await;
    enforce_cache_bounds(&mut cache);

    let user = match cache.users.get(user.identifier()) {
        Some(user) => user.clone(),
        None => {
            cache.users.insert(user.identifier().clone(), user.clone());
            user.clone()
        }
    };

    groups.iter().for_each(|group| {
        let group = match cache.groups.get(group.identifier()) {
            Some(group) => group.clone(),
            None => {
                cache
                    .groups
                    .insert(group.identifier().clone(), group.clone());
                group.clone()
            }
        };

        cache
            .users_in_group
            .entry(group.identifier().clone())
            .or_insert_with(HashSet::new)
            .insert(user.identifier().clone());
    });

    Ok(())
}

///
/// Set that the passed project has the passed users associated with it
///
pub async fn set_users_in_group(group: &IPAGroup, users: &[IPAUser]) -> Result<(), Error> {
    if users.is_empty() {
        return Ok(());
    }

    let mut cache = CACHE.write().await;
    enforce_cache_bounds(&mut cache);

    // make sure we have cached the group and users
    let group = match cache.groups.get(group.identifier()) {
        Some(group) => group.clone(),
        None => {
            cache
                .groups
                .insert(group.identifier().clone(), group.clone());
            group.clone()
        }
    };

    let users: Vec<IPAUser> = users
        .iter()
        .map(|u| {
            cache
                .users
                .entry(u.identifier().clone())
                .or_insert_with(|| u.clone())
                .clone()
        })
        .collect();

    cache.users_in_group.insert(
        group.identifier().clone(),
        users.iter().map(|u| u.identifier().clone()).collect(),
    );

    Ok(())
}

///
/// Remove the passed user from the passed group
/// Currently unused, but want to keep it around for future use
///
#[allow(dead_code)]
pub async fn remove_user_from_group(group: &IPAGroup, user: &IPAUser) -> Result<(), Error> {
    let mut cache = CACHE.write().await;
    enforce_cache_bounds(&mut cache);

    let group = match cache.groups.get(group.identifier()) {
        Some(group) => group.clone(),
        None => {
            cache
                .groups
                .insert(group.identifier().clone(), group.clone());
            group.clone()
        }
    };

    let user = match cache.users.get(user.identifier()) {
        Some(user) => user.clone(),
        None => {
            cache.users.insert(user.identifier().clone(), user.clone());
            user.clone()
        }
    };

    if let Some(users) = cache.users_in_group.get_mut(group.identifier()) {
        users.retain(|u| u != user.identifier());
    }

    Ok(())
}

///
/// Return all users we know are associated with the passed group
///
pub async fn get_users_in_group(group: &IPAGroup) -> Result<Vec<IPAUser>, Error> {
    let cache = CACHE.read().await;
    Ok(cache
        .users_in_group
        .get(group.identifier())
        .map(|users| {
            users
                .iter()
                .filter_map(|u| cache.users.get(u))
                .cloned()
                .collect()
        })
        .unwrap_or_default())
}

///
/// Return the names and identifiers for all of the internal groups
/// (including for all peers)
///
pub async fn get_internal_group_ids() -> Result<HashMap<String, ProjectIdentifier>, Error> {
    let cache = CACHE.read().await;
    let mut internal_groups = HashMap::new();

    for group in cache.system_groups.clone() {
        internal_groups.insert(group.groupid().to_string(), group.identifier().clone());
    }

    for groups in cache.instance_groups.values() {
        for group in groups {
            internal_groups.insert(group.groupid().to_string(), group.identifier().clone());
        }
    }

    Ok(internal_groups)
}

///
/// Return all of the default system groups that should be used
/// for all users managed by OpenPortal on this system
///
pub async fn get_system_groups() -> Result<Vec<IPAGroup>, Error> {
    let cache = CACHE.read().await;
    Ok(cache.system_groups.clone())
}

///
/// Set the list of all system groups that should be used for all users
/// managed by OpenPortal on this system
///
pub async fn set_system_groups(groups: &[IPAGroup]) -> Result<(), Error> {
    let mut cache = CACHE.write().await;
    enforce_cache_bounds(&mut cache);
    cache.system_groups = groups.to_vec();
    tracing::info!("Setting system groups to {:?}", cache.system_groups);
    Ok(())
}

///
/// Set the list of all instance groups that should be used for each
/// instance that connects to this agent. These groups should be added
/// for all users managed by OpenPortal who are added to this instance
///
pub async fn set_instance_groups(groups: &HashMap<Peer, Vec<IPAGroup>>) -> Result<(), Error> {
    // make sure to add the instance group for each peer to the list,
    // if it doesn't already exist
    let mut instance_groups = groups.clone();

    for (peer, groups) in groups {
        let op_instance_group = get_op_instance_group(peer)?;

        if !groups
            .iter()
            .any(|g| g.groupid() == op_instance_group.groupid())
        {
            let mut groups = groups.clone();
            groups.push(op_instance_group);
            instance_groups.insert(peer.clone(), groups);
        }
    }

    let mut cache = CACHE.write().await;
    enforce_cache_bounds(&mut cache);
    cache.instance_groups = groups.clone();

    tracing::info!("Setting instance groups to {:?}", cache.instance_groups);
    Ok(())
}

///
/// Return all of the instance groups that should be used for users
/// added via the specified instance. Returns an empty list if there
/// are on instance groups for this instance
///
pub async fn get_instance_groups(instance: &Peer) -> Result<Vec<IPAGroup>, Error> {
    let mut groups = CACHE
        .read()
        .await
        .instance_groups
        .get(instance)
        .cloned()
        .unwrap_or_default();

    let op_instance_group = get_op_instance_group(instance)?;

    // does groups contains a group with the same groupid as op_instance_group?
    // This would be the case if groups is empty (no user supplied instance groups)
    if !groups
        .iter()
        .any(|g| g.groupid() == op_instance_group.groupid())
    {
        groups.push(op_instance_group);

        let mut cache = CACHE.write().await;
        enforce_cache_bounds(&mut cache);
        cache
            .instance_groups
            .insert(instance.clone(), groups.clone());
    }

    Ok(groups)
}

///
/// Add a user that exits in FreeIPA that we are managing to the database
///
pub async fn add_existing_user(user: &IPAUser) -> Result<(), Error> {
    let mut cache = CACHE.write().await;
    enforce_cache_bounds(&mut cache);
    cache.users.insert(user.identifier().clone(), user.clone());
    Ok(())
}

///
/// Add a number of existing users to the database.
/// Currently unused, but want to keep it around for future use
///
#[allow(dead_code)]
pub async fn add_existing_users(users: &[IPAUser]) -> Result<(), Error> {
    let mut cache = CACHE.write().await;
    enforce_cache_bounds(&mut cache);
    users.iter().for_each(|u| {
        // only insert if they don't already exist
        cache
            .users
            .entry(u.identifier().clone())
            .or_insert_with(|| u.clone());
    });
    Ok(())
}

///
/// Remove a user from the database
///
pub async fn remove_existing_user(user: &IPAUser) -> Result<(), Error> {
    let mut cache = CACHE.write().await;
    enforce_cache_bounds(&mut cache);
    cache.users.remove(user.identifier());

    cache.users_in_group.values_mut().for_each(|users| {
        users.retain(|u| u != user.identifier());
    });

    Ok(())
}

///
/// Return the IPAGroup for the named group (or None)
/// if it doesn't exist
///
pub async fn get_group(group: &ProjectIdentifier) -> Result<Option<IPAGroup>, Error> {
    tracing::debug!("Getting group {} from cache - awaiting lock...", group);
    let cache = CACHE.read().await;
    tracing::debug!(
        "Lock obtained. Cache contains {} groups",
        cache.groups.len()
    );
    Ok(cache.groups.get(group).cloned())
}

///
/// Add an existing group to the database
///
pub async fn add_existing_group(group: &IPAGroup) -> Result<(), Error> {
    let mut cache = CACHE.write().await;
    enforce_cache_bounds(&mut cache);
    cache
        .groups
        .insert(group.identifier().clone(), group.clone());

    Ok(())
}

///
/// Remove a group from the database
///
#[allow(dead_code)]
pub async fn remove_existing_group(group: &IPAGroup) -> Result<(), Error> {
    let mut cache = CACHE.write().await;
    enforce_cache_bounds(&mut cache);
    cache.groups.remove(group.identifier());
    cache.users_in_group.remove(group.identifier());

    Ok(())
}

///
/// Clear the cache - we need to do this is FreeIPA is changed behine
/// our back
///
pub async fn clear() -> Result<(), Error> {
    let mut cache = CACHE.write().await;
    enforce_cache_bounds(&mut cache);
    cache.users.clear();
    cache.groups.clear();
    cache.users_in_group.clear();
    Ok(())
}
