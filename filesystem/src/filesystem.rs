// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use templemeads::Error;

use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;

use nix::unistd::{Gid, Group, Uid, User};

use tokio::sync::Mutex;

static FS_LOCK: Lazy<Arc<Mutex<()>>> = Lazy::new(|| Arc::new(Mutex::new(())));

///
/// Clean and check the passed file / directory permissions. This function
/// will check that the permissions are valid, and return them as a u32.
/// If the permissions are invalid, then an error will be returned.
/// The permissions should be passed as a string, e.g. "0755".
///
pub async fn clean_and_check_permissions(permissions: &str) -> Result<u32, Error> {
    let permissions = permissions.trim();

    // make sure that the permissions have four characters - if not, prepend a 0
    let permissions = if permissions.len() == 3 {
        format!("0{}", permissions)
    } else {
        permissions.to_string()
    };

    // convert the permissions into a u32
    let permissions = u32::from_str_radix(&permissions, 8)
        .with_context(|| format!("Could not convert permissions '{}' into a u32", permissions))?;

    // check the permissions are valid
    if permissions > 0o7777 {
        return Err(Error::State(format!(
            "Permissions '{}' are invalid. Must be between 0000 and 7777",
            permissions
        )));
    }

    Ok(permissions)
}

///
/// Clean and check the path 'path'. This function will canonicalize
/// the path, and check that it exists, if 'check_exists' is true.
///
/// The function will return the cleaned path as a string, or an error
/// if the path is invalid.
///
/// This will also check that the path is not in a sensitive location,
/// such as /etc, /var, /usr, /bin, /sbin, /lib, /lib64, /boot, /root,
/// /dev, /proc, /sys, /run, /tmp, or /.
///
pub async fn clean_and_check_path(path: &str, check_exists: bool) -> Result<String, Error> {
    let mut path = path.trim();

    while path.ends_with("/") {
        path = path.trim_end_matches('/');
    }

    let mut path = Path::new(path).to_owned();

    // convert into a path
    if check_exists {
        path = path
            .canonicalize()
            .with_context(|| format!("Could not canonicalize path '{}'", path.to_string_lossy()))?;
    }

    if check_exists && !path.exists() {
        return Err(Error::State(format!(
            "The path '{}' does not exist.",
            path.to_string_lossy()
        )));
    }

    // make sure the path is not somewhere sensitive
    if path.starts_with("/etc")
        || path.starts_with("/var")
        || path.starts_with("/usr")
        || path.starts_with("/bin")
        || path.starts_with("/sbin")
        || path.starts_with("/lib")
        || path.starts_with("/lib64")
        || path.starts_with("/boot")
        || path.starts_with("/root")
        || path.starts_with("/dev")
        || path.starts_with("/proc")
        || path.starts_with("/sys")
        || path.starts_with("/run")
        || path.starts_with("/tmp")
        || path == Path::new("/")
    {
        return Err(Error::State(format!(
            "The path '{}' is in a sensitive location.",
            path.to_string_lossy()
        )));
    }

    Ok(path.to_string_lossy().to_string())
}

async fn create_dir(
    dir: &str,
    username: &str,
    groupname: &str,
    permissions: &str,
) -> Result<(), Error> {
    let dir = clean_and_check_path(dir, false).await?;

    // convert the permissions into a u32
    let permissions = clean_and_check_permissions(permissions).await?;

    tracing::info!(
        "Creating directory '{}' for user '{}' and group '{}' with permissions '{}'",
        dir,
        username,
        groupname,
        unix_mode::to_string(permissions)
    );

    // convert the username into a uid
    let uid = match User::from_name(username) {
        Ok(user) => match user {
            Some(user) => user.uid,
            None => {
                return Err(Error::State(format!(
                    "Could not find a user called {}",
                    username
                )))
            }
        },
        Err(e) => {
            return Err(Error::State(format!(
                "Could not search for user {}: {}",
                username, e
            )))
        }
    };

    // conver the groupname into a gid
    let gid = match Group::from_name(groupname) {
        Ok(group) => match group {
            Some(group) => group.gid,
            None => {
                return Err(Error::State(format!(
                    "Could not find a group called {}",
                    groupname
                )))
            }
        },
        Err(e) => {
            return Err(Error::State(format!(
                "Could not search for group {}: {}",
                groupname, e
            )))
        }
    };

    // check to see if the directory already exists
    let path = Path::new(&dir);

    if path.exists() {
        // directory already exists - check it has the right permissions
        // and user / group ownership
        let metadata = path.metadata()?;

        // check the ownership
        if Uid::from_raw(metadata.uid()) != uid {
            // ownership is wrong
            tracing::error!(
                "Directory '{}' already exists, but has the wrong ownership. Expected '{}', got '{}'",
                    dir, uid, Uid::from_raw(metadata.uid())
                );
        }

        if Gid::from_raw(metadata.gid()) != gid {
            // ownership is wrong
            tracing::error!(
                "Directory '{}' already exists, but has the wrong group ownership. Expected '{}', got '{}'",
                    dir, gid, Gid::from_raw(metadata.gid())
                );
        }

        // check the permissions - we should ignore the sticky bit
        if metadata.permissions().mode() & 0o7777 != permissions {
            // permissions are wrong
            tracing::error!(
                "Directory '{}' already exists, but has the wrong permissions. Expected '{}', got '{}'",
                    dir, unix_mode::to_string(permissions),
                    unix_mode::to_string(metadata.permissions().mode())
                );
        }

        // otherwise the directory is already present and correct
        // It is best to stop now, and not try to do anything,
        // as we should assume that another process has already beaten
        // us to creating the directory
        tracing::info!("Directory already exists with required permissions.");
        return Ok(());
    }

    // use a lock to ensure that only a single task can create directories
    // at a time - this should prevent overloading the filesystem and
    // reduce risk of filesystem-related race conditions
    let now = chrono::Utc::now();
    let _guard = loop {
        match FS_LOCK.try_lock() {
            Ok(guard) => break guard,
            Err(_) => {
                if chrono::Utc::now().signed_duration_since(now).num_seconds() > 15 {
                    return Err(Error::State(
                        "Could not acquire filesystem lock after 15 seconds".to_string(),
                    ));
                }

                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
        }
    };

    // create the directory
    std::fs::create_dir(path).with_context(|| format!("Could not create directory '{}'", dir))?;

    // set the ownership and permissions
    nix::unistd::chown(path, Some(uid), Some(gid))
        .with_context(|| format!("Could not set ownership on directory '{}'", dir))?;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(permissions))
        .with_context(|| format!("Could not set permissions on directory '{}'", dir))?;

    Ok(())
}

///
/// Create the user's home directory at 'homedir'. The directory
/// should be created for the user 'username' and group 'groupname',
/// with unix permissions in 'permissions' (as a string, e.g. "0755")
/// An optional config script can be passed as 'config_script'. If this
/// is passed, then it is executed with the argument being the path
/// to the newly created home directory.
///
pub async fn create_home_dir(
    dir: &str,
    username: &str,
    groupname: &str,
    permissions: &str,
) -> Result<(), Error> {
    create_dir(dir, username, groupname, permissions).await
}

///
/// Create the project directory at 'projectdir'. The directory
/// should be created for the group 'groupname', with unix permissions
/// in 'permissions' (as a string, e.g. "0755"). An optional config
/// script can be passed as 'config_script'. If this is passed, then
/// it is executed with the argument being the path to the newly
/// created project directory.
///
pub async fn create_project_dir(
    dir: &str,
    groupname: &str,
    permissions: &str,
) -> Result<(), Error> {
    create_dir(dir, "root", groupname, permissions).await
}

pub async fn get_project_link(link: &str, project: &str) -> Result<String, Error> {
    // replace either {PROJECT} or {project} with the value of project
    let link = link
        .replace("{PROJECT}", project)
        .replace("{project}", project);

    clean_and_check_path(&link, false).await
}

pub async fn create_project_link(dir: &str, link: &str, project: &str) -> Result<(), Error> {
    // replace either {PROJECT} or {project} with the value of project
    let link = link
        .replace("{PROJECT}", project)
        .replace("{project}", project);

    let link = clean_and_check_path(&link, false).await?;

    let dir = clean_and_check_path(dir, true).await?;

    tracing::info!("Creating link from '{}' to '{}'", dir, link);

    // check to see if the link already exists
    let path = Path::new(&link);

    if path.exists() {
        // link already exists - check it is a link to the correct directory
        let metadata = path.symlink_metadata()?;

        if metadata.file_type().is_symlink() {
            // check the link points to the correct directory
            let target = path.read_link()?.canonicalize()?;

            if target != Path::new(&dir) {
                tracing::error!(
                    "Link '{}' already exists, but points to the wrong directory. Expected '{}', got '{}'",
                        link, dir, target.to_string_lossy()
                );
            }

            // otherwise the link is already present and correct
            // It is best to stop now, and not try to do anything,
            // as we should assume that another process has already beaten
            // us to creating the link
            return Ok(());
        } else {
            tracing::error!("Link '{}' already exists, but is not a symlink", link);
        }
    }

    // create the link
    std::os::unix::fs::symlink(&dir, path)
        .with_context(|| format!("Could not create link '{}' to '{}'", link, dir))?;

    Ok(())
}
