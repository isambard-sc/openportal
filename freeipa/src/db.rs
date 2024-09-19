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
    system_groups: Vec<IPAGroup>,
}

static DB: Lazy<RwLock<Database>> = Lazy::new(|| RwLock::new(Database::default()));

///
/// Return the IPAUser for the passed UserIdentifier, if this
/// user exists in the system. Returns None if the user does not
///
pub async fn get_user(identifier: &UserIdentifier) -> Result<Option<IPAUser>, Error> {
    let db = DB.read().await;
    Ok(db.users.get(identifier).cloned())
}

///
/// Return all of the default system groups that should be used
/// for all users managed by OpenPortal on this system
///
pub async fn get_system_groups() -> Result<Vec<IPAGroup>, Error> {
    let db = DB.read().await;
    Ok(db.system_groups.clone())
}

///
/// Set the list of all system groups that should be used for all users
/// managed by OpenPortal on this system
///
pub async fn set_system_groups(groups: Vec<IPAGroup>) -> Result<(), Error> {
    let mut db = DB.write().await;
    db.system_groups = groups;
    Ok(())
}

///
/// Add a new user to the database
///
pub async fn add_existing_user(user: &IPAUser) -> Result<(), Error> {
    let mut db = DB.write().await;
    db.users.insert(user.identifier().clone(), user.clone());
    Ok(())
}

///
/// Add the existing users that we are managing that already
/// exist in FreeIPA
///
pub async fn add_existing_users(users: Vec<IPAUser>) -> Result<(), Error> {
    let mut db = DB.write().await;

    for user in users {
        let identifier = user.identifier().clone();

        match identifier.is_valid() {
            true => {
                db.users.insert(identifier, user);
            }
            false => {
                tracing::error!(
                    "Unable to create a valid UserIdentifier for user: {:?}",
                    user
                );
                continue;
            }
        }
    }

    Ok(())
}
