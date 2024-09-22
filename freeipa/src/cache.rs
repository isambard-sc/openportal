// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use templemeads::grammar::UserIdentifier;
use templemeads::Error;
use tokio::sync::RwLock;

use crate::freeipa::{IPAGroup, IPAUser};

/// This file manages the directory of all users added to the system

#[derive(Debug, Clone, Default)]
struct Database {
    users: HashMap<UserIdentifier, IPAUser>,
    groups: HashMap<String, IPAGroup>,
    system_groups: Vec<IPAGroup>,
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
/// Add a user that exits in FreeIPA that we are managing to the database
///
pub async fn add_existing_user(user: &IPAUser) -> Result<(), Error> {
    match user.identifier().is_valid() {
        true => {
            let mut cache = CACHE.write().await;
            cache.users.insert(user.identifier().clone(), user.clone());
            Ok(())
        }
        false => {
            tracing::error!(
                "Unable to register {:?} as their UserIdentifier is not valid",
                user
            );
            Ok(())
        }
    }
}

///
/// Return the IPAGroup for the named group (or None)
/// if it doesn't exist
///
pub async fn get_group(group: &str) -> Result<Option<IPAGroup>, Error> {
    let cache = CACHE.read().await;
    Ok(cache.groups.get(group).cloned())
}

///
/// Add an existing group to the database
///
pub async fn add_existing_group(group: &IPAGroup) -> Result<(), Error> {
    match group.identifier().is_empty() {
        true => {
            tracing::error!(
                "Unable to register {:?} as the group identifier is not valid",
                group
            );
        }
        false => {
            let mut cache = CACHE.write().await;
            cache
                .groups
                .insert(group.identifier().to_owned(), group.clone());
        }
    }

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
    Ok(())
}
