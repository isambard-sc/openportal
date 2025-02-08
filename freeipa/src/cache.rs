// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use templemeads::agent::Peer;
use templemeads::grammar::{ProjectIdentifier, UserIdentifier};
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
}

static CACHE: Lazy<RwLock<Database>> = Lazy::new(|| RwLock::new(Database::default()));

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
    Ok(cache
        .user_mutexes
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
    let cache = CACHE.read().await;
    Ok(cache.groups.get(group).cloned())
}

///
/// Add an existing group to the database
///
pub async fn add_existing_group(group: &IPAGroup) -> Result<(), Error> {
    let mut cache = CACHE.write().await;
    cache
        .groups
        .insert(group.identifier().clone(), group.clone());

    Ok(())
}

///
/// Add a number of existing groups to the database
///
pub async fn add_existing_groups(groups: &[IPAGroup]) -> Result<(), Error> {
    let mut cache = CACHE.write().await;
    groups.iter().for_each(|g| {
        cache
            .groups
            .entry(g.identifier().clone())
            .or_insert_with(|| g.clone());
    });
    Ok(())
}

///
/// Remove a group from the database
///
#[allow(dead_code)]
pub async fn remove_existing_group(group: &IPAGroup) -> Result<(), Error> {
    let mut cache = CACHE.write().await;
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
    cache.users.clear();
    cache.groups.clear();
    cache.users_in_group.clear();
    Ok(())
}
