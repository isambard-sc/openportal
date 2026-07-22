// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! Assignment state — which projects and users have been added to this
//! cloud account. There is no cloud-side API to query this yet, so this
//! agent is the only source of truth for it, and it has to survive a
//! restart.
//!
//! State is persisted as one plain JSON file per project in a configured
//! `state_dir` (see `docs/plans/op-cloudaccount-design.md` §4.1 for the
//! reasoning). An in-memory cache mirrors `slurm/src/cache.rs`'s pattern
//! and is the only thing normal operation reads from; every mutation
//! updates the cache and then writes the corresponding file straight
//! through, atomically (write to a `.tmp` file, then rename).

use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tokio::sync::RwLock;

use serde::{Deserialize, Serialize};
use greatwestern::grammar::{ProjectIdentifier, ProjectMapping, UserIdentifier, UserMapping};
use templemeads::portal_identifier::PortalIdentifier;
use templemeads::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProjectState {
    mapping: ProjectMapping,
    #[serde(default)]
    users: HashMap<UserIdentifier, UserMapping>,
    #[serde(default)]
    blocked: bool,
    #[serde(default)]
    blocked_users: HashSet<UserIdentifier>,
}

impl ProjectState {
    fn new(project: &ProjectIdentifier) -> Result<Self, Error> {
        Ok(Self {
            mapping: ProjectMapping::new(project, &project.to_string())?,
            users: HashMap::new(),
            blocked: false,
            blocked_users: HashSet::new(),
        })
    }
}

struct Database {
    state_dir: Option<PathBuf>,
    projects: HashMap<ProjectIdentifier, ProjectState>,
}

static CACHE: Lazy<RwLock<Database>> = Lazy::new(|| {
    RwLock::new(Database {
        state_dir: None,
        projects: HashMap::new(),
    })
});

fn state_path(state_dir: &Path, project: &ProjectIdentifier) -> PathBuf {
    state_dir.join(format!("{}.json", project))
}

/// Point the state store at `state_dir` and load any project state files
/// already there (e.g. from before an agent restart).
pub async fn initialise(state_dir: &Path) -> Result<(), Error> {
    tokio::fs::create_dir_all(state_dir).await.map_err(|e| {
        Error::Failed(format!(
            "Cannot create cloudaccount state-dir '{}': {}",
            state_dir.display(),
            e
        ))
    })?;

    let mut projects = HashMap::new();

    let mut entries = tokio::fs::read_dir(state_dir).await.map_err(|e| {
        Error::Failed(format!(
            "Cannot read cloudaccount state-dir '{}': {}",
            state_dir.display(),
            e
        ))
    })?;

    while let Some(entry) = entries.next_entry().await.map_err(Error::IO)? {
        let path = entry.path();

        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }

        match tokio::fs::read_to_string(&path).await {
            Ok(contents) => match serde_json::from_str::<ProjectState>(&contents) {
                Ok(state) => {
                    projects.insert(state.mapping.project().clone(), state);
                }
                Err(e) => {
                    tracing::warn!(
                        "Could not parse cloudaccount state file '{}': {}. Skipping.",
                        path.display(),
                        e
                    );
                }
            },
            Err(e) => {
                tracing::warn!(
                    "Could not read cloudaccount state file '{}': {}. Skipping.",
                    path.display(),
                    e
                );
            }
        }
    }

    let mut cache = CACHE.write().await;
    cache.state_dir = Some(state_dir.to_path_buf());
    cache.projects = projects;

    Ok(())
}

async fn write_state(state_dir: &Path, state: &ProjectState) -> Result<(), Error> {
    let path = state_path(state_dir, state.mapping.project());
    let tmp_path = path.with_extension("json.tmp");

    let contents = serde_json::to_string_pretty(state)?;

    tokio::fs::write(&tmp_path, contents).await.map_err(|e| {
        Error::Failed(format!(
            "Cannot write cloudaccount state file '{}': {}",
            tmp_path.display(),
            e
        ))
    })?;

    tokio::fs::rename(&tmp_path, &path).await.map_err(|e| {
        Error::Failed(format!(
            "Cannot finalise cloudaccount state file '{}': {}",
            path.display(),
            e
        ))
    })?;

    Ok(())
}

async fn state_dir(cache: &Database) -> Result<PathBuf, Error> {
    cache.state_dir.clone().ok_or_else(|| {
        Error::Misconfigured("cloudaccount state store has not been initialised".to_string())
    })
}

/// Add (or re-add) a project to this cloud account. Idempotent - adding
/// an already-known project just returns its existing mapping, unless it
/// is blocked, in which case it stays blocked (use `unblock_project` to
/// re-enable it).
pub async fn add_project(project: &ProjectIdentifier) -> Result<ProjectMapping, Error> {
    let mut cache = CACHE.write().await;
    let dir = state_dir(&cache).await?;

    let state = match cache.projects.get(project) {
        Some(state) => state.clone(),
        None => ProjectState::new(project)?,
    };

    write_state(&dir, &state).await?;
    let mapping = state.mapping.clone();
    cache.projects.insert(project.clone(), state);

    Ok(mapping)
}

/// Remove a project from this cloud account. The cost-report history in
/// the accounting directory is untouched by this - it belongs to the
/// cloud operators, not to our assignment state.
pub async fn remove_project(project: &ProjectIdentifier) -> Result<ProjectMapping, Error> {
    let mut cache = CACHE.write().await;
    let dir = state_dir(&cache).await?;

    let mapping = match cache.projects.remove(project) {
        Some(state) => state.mapping,
        None => ProjectMapping::new(project, &project.to_string())?,
    };

    let path = state_path(&dir, project);
    match tokio::fs::remove_file(&path).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(Error::Failed(format!(
                "Cannot remove cloudaccount state file '{}': {}",
                path.display(),
                e
            )))
        }
    }

    Ok(mapping)
}

async fn set_project_blocked(
    project: &ProjectIdentifier,
    blocked: bool,
) -> Result<ProjectMapping, Error> {
    let mut cache = CACHE.write().await;
    let dir = state_dir(&cache).await?;

    let mut state = match cache.projects.get(project) {
        Some(state) => state.clone(),
        None => ProjectState::new(project)?,
    };

    state.blocked = blocked;

    write_state(&dir, &state).await?;
    let mapping = state.mapping.clone();
    cache.projects.insert(project.clone(), state);

    Ok(mapping)
}

pub async fn block_project(project: &ProjectIdentifier) -> Result<ProjectMapping, Error> {
    set_project_blocked(project, true).await
}

pub async fn unblock_project(project: &ProjectIdentifier) -> Result<ProjectMapping, Error> {
    set_project_blocked(project, false).await
}

pub async fn is_blocked_project(project: &ProjectIdentifier) -> Result<bool, Error> {
    let cache = CACHE.read().await;
    Ok(cache
        .projects
        .get(project)
        .map(|s| s.blocked)
        .unwrap_or(false))
}

/// Add (or re-add) a user to their project. The project is created if it
/// doesn't already exist - there is no strict ordering requirement between
/// `AddProject` and `AddUser` for this prototype.
pub async fn add_user(user: &UserIdentifier) -> Result<UserMapping, Error> {
    let project = user.project_identifier();
    let mut cache = CACHE.write().await;
    let dir = state_dir(&cache).await?;

    let mut state = match cache.projects.get(&project) {
        Some(state) => state.clone(),
        None => ProjectState::new(&project)?,
    };

    let mapping = UserMapping::new(user, &user.username(), state.mapping.local_group())?;
    state.users.insert(user.clone(), mapping.clone());

    write_state(&dir, &state).await?;
    cache.projects.insert(project, state);

    Ok(mapping)
}

pub async fn remove_user(user: &UserIdentifier) -> Result<UserMapping, Error> {
    let project = user.project_identifier();
    let mut cache = CACHE.write().await;
    let dir = state_dir(&cache).await?;

    let Some(mut state) = cache.projects.get(&project).cloned() else {
        return UserMapping::new(user, &user.username(), &project.to_string());
    };

    let mapping = match state.users.remove(user) {
        Some(mapping) => mapping,
        None => UserMapping::new(user, &user.username(), state.mapping.local_group())?,
    };

    state.blocked_users.remove(user);
    write_state(&dir, &state).await?;
    cache.projects.insert(project, state);

    Ok(mapping)
}

async fn set_user_blocked(user: &UserIdentifier, blocked: bool) -> Result<UserMapping, Error> {
    let project = user.project_identifier();
    let mut cache = CACHE.write().await;
    let dir = state_dir(&cache).await?;

    let mut state = match cache.projects.get(&project) {
        Some(state) => state.clone(),
        None => ProjectState::new(&project)?,
    };

    let mapping = state.users.get(user).cloned().unwrap_or(UserMapping::new(
        user,
        &user.username(),
        state.mapping.local_group(),
    )?);

    state.users.insert(user.clone(), mapping.clone());

    if blocked {
        state.blocked_users.insert(user.clone());
    } else {
        state.blocked_users.remove(user);
    }

    write_state(&dir, &state).await?;
    cache.projects.insert(project, state);

    Ok(mapping)
}

pub async fn block_user(user: &UserIdentifier) -> Result<UserMapping, Error> {
    set_user_blocked(user, true).await
}

pub async fn unblock_user(user: &UserIdentifier) -> Result<UserMapping, Error> {
    set_user_blocked(user, false).await
}

pub async fn is_blocked_user(user: &UserIdentifier) -> Result<bool, Error> {
    let project = user.project_identifier();
    let cache = CACHE.read().await;
    Ok(cache
        .projects
        .get(&project)
        .map(|s| s.blocked_users.contains(user))
        .unwrap_or(false))
}

pub async fn get_projects(portal: &PortalIdentifier) -> Result<Vec<ProjectMapping>, Error> {
    let cache = CACHE.read().await;
    Ok(cache
        .projects
        .values()
        .filter(|s| s.mapping.project().portal() == portal.portal())
        .map(|s| s.mapping.clone())
        .collect())
}

pub async fn get_users(project: &ProjectIdentifier) -> Result<Vec<UserMapping>, Error> {
    let cache = CACHE.read().await;
    Ok(cache
        .projects
        .get(project)
        .map(|s| s.users.values().cloned().collect())
        .unwrap_or_default())
}

pub async fn get_project_mapping(project: &ProjectIdentifier) -> Result<ProjectMapping, Error> {
    let cache = CACHE.read().await;
    match cache.projects.get(project) {
        Some(state) => Ok(state.mapping.clone()),
        None => Err(Error::NotFound(format!(
            "No project '{}' has been added to this cloud account",
            project
        ))),
    }
}

pub async fn get_user_mapping(user: &UserIdentifier) -> Result<UserMapping, Error> {
    let project = user.project_identifier();
    let cache = CACHE.read().await;
    match cache.projects.get(&project).and_then(|s| s.users.get(user)) {
        Some(mapping) => Ok(mapping.clone()),
        None => Err(Error::NotFound(format!(
            "No user '{}' has been added to this cloud account",
            user
        ))),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    // Exercises the whole assignment-state lifecycle against a real
    // temp directory, including a simulated agent restart (re-running
    // `initialise` reloads everything from disk into a fresh in-memory
    // cache) - this is the scenario design doc §13 calls out as the one
    // that actually matters: state must survive a restart.
    #[tokio::test]
    async fn test_state_round_trip_survives_restart() {
        let dir = std::env::temp_dir().join("op-cloudaccount-test-state-round-trip");
        let _ = tokio::fs::remove_dir_all(&dir).await;

        initialise(&dir).await.unwrap();

        let project = ProjectIdentifier::parse("roundtrip.testportal").unwrap();
        let user = UserIdentifier::parse("alice.roundtrip.testportal").unwrap();
        let portal = PortalIdentifier::parse("testportal").unwrap();

        add_project(&project).await.unwrap();
        add_user(&user).await.unwrap();
        block_user(&user).await.unwrap();

        assert!(is_blocked_user(&user).await.unwrap());
        assert_eq!(get_projects(&portal).await.unwrap().len(), 1);
        assert_eq!(get_users(&project).await.unwrap().len(), 1);

        // simulate a restart: re-initialise from the same directory
        initialise(&dir).await.unwrap();

        assert!(is_blocked_user(&user).await.unwrap());
        let projects_after_restart = get_projects(&portal).await.unwrap();
        assert_eq!(projects_after_restart.len(), 1);

        let users_after_restart = get_users(&project).await.unwrap();
        assert_eq!(users_after_restart.len(), 1);
        assert_eq!(users_after_restart[0].local_user(), "alice");

        remove_user(&user).await.unwrap();
        assert_eq!(get_users(&project).await.unwrap().len(), 0);
        assert!(get_user_mapping(&user).await.is_err());

        remove_project(&project).await.unwrap();
        assert_eq!(get_projects(&portal).await.unwrap().len(), 0);
        assert!(get_project_mapping(&project).await.is_err());

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
