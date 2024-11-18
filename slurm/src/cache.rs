// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use templemeads::Error;
use tokio::sync::RwLock;

use crate::slurm::{SlurmAccount, SlurmUser};

/// This file manages the directory of all users added to the system

#[derive(Debug, Clone, Default)]
struct Database {
    accounts: HashMap<String, SlurmAccount>,
    associations: HashMap<String, Vec<SlurmUser>>,
}

static CACHE: Lazy<RwLock<Database>> = Lazy::new(|| RwLock::new(Database::default()));

pub async fn get_account(name: &str) -> Result<Option<SlurmAccount>, Error> {
    let cache = CACHE.read().await;
    Ok(cache.accounts.get(name).cloned())
}

pub async fn get_associations(account: &SlurmAccount) -> Vec<SlurmUser> {
    let cache = CACHE.read().await;
    cache
        .associations
        .get(account.name())
        .cloned()
        .unwrap_or_default()
}

pub async fn add_account(account: &SlurmAccount) -> Result<(), Error> {
    let mut cache = CACHE.write().await;
    cache
        .accounts
        .insert(account.name().to_string(), account.clone());
    Ok(())
}

pub async fn add_association(user: &SlurmUser, account: &SlurmAccount) -> Result<(), Error> {
    let mut cache = CACHE.write().await;
    let users = cache
        .associations
        .entry(account.name().to_string())
        .or_insert_with(Vec::new);

    if users.iter().any(|u| u == user) {
        return Ok(());
    }

    users.push(user.clone());
    Ok(())
}

pub async fn user_is_associated(user: &SlurmUser, account: &SlurmAccount) -> bool {
    let cache = CACHE.read().await;
    cache
        .associations
        .get(account.name())
        .map(|users| users.iter().any(|u| u == user))
        .unwrap_or(false)
}

///
/// Clear the cache - we need to do this if Slurm is changed behine
/// our back
///
pub async fn clear() -> Result<(), Error> {
    let mut cache = CACHE.write().await;
    cache.accounts.clear();
    cache.associations.clear();
    Ok(())
}
