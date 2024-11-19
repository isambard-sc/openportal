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
    users: HashMap<String, SlurmUser>,
}

static CACHE: Lazy<RwLock<Database>> = Lazy::new(|| RwLock::new(Database::default()));

pub async fn get_account(name: &str) -> Result<Option<SlurmAccount>, Error> {
    let cache = CACHE.read().await;
    Ok(cache.accounts.get(name).cloned())
}

pub async fn add_account(account: &SlurmAccount) -> Result<(), Error> {
    let mut cache = CACHE.write().await;
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
