// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use templemeads::Error;

use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
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
pub async fn clean_and_check_path(path: &Path, check_exists: bool) -> Result<PathBuf, Error> {
    let mut path = path.to_owned();

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

    Ok(path)
}

pub async fn create_dir(
    path: &std::path::Path,
    username: &str,
    groupname: &str,
    permissions: &str,
) -> Result<(), Error> {
    let path = clean_and_check_path(path, false).await?;

    // convert the permissions into a u32
    let permissions = clean_and_check_permissions(permissions).await?;

    tracing::info!(
        "Creating directory '{}' for user '{}' and group '{}' with permissions '{}'",
        path.to_string_lossy(),
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
    if path.exists() {
        // directory already exists - check it has the right permissions
        // and user / group ownership
        let metadata = path.metadata()?;

        // check the ownership
        if Uid::from_raw(metadata.uid()) != uid {
            // ownership is wrong
            tracing::error!(
                "Directory '{}' already exists, but has the wrong ownership. Expected '{}', got '{}'",
                    path.to_string_lossy(), uid, Uid::from_raw(metadata.uid())
                );
        }

        if Gid::from_raw(metadata.gid()) != gid {
            // ownership is wrong
            tracing::error!(
                "Directory '{}' already exists, but has the wrong group ownership. Expected '{}', got '{}'",
                    path.to_string_lossy(), gid, Gid::from_raw(metadata.gid())
                );
        }

        // check the permissions - we should ignore the sticky bit
        if metadata.permissions().mode() & 0o7777 != permissions {
            // permissions are wrong
            tracing::error!(
                "Directory '{}' already exists, but has the wrong permissions. Expected '{}', got '{}'",
                    path.to_string_lossy(), unix_mode::to_string(permissions),
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

    // Check if this directory exists in .recycle - if so, restore it
    if let Some(recycle_path) = check_recycle(&path).await? {
        restore_from_recycle(&recycle_path, &path).await?;
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
    std::fs::create_dir(&path)
        .with_context(|| format!("Could not create directory '{}'", path.to_string_lossy()))?;

    // set the ownership and permissions
    nix::unistd::chown(&path, Some(uid), Some(gid)).with_context(|| {
        format!(
            "Could not set ownership on directory '{}'",
            path.to_string_lossy()
        )
    })?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(permissions)).with_context(
        || {
            format!(
                "Could not set permissions on directory '{}'",
                path.to_string_lossy()
            )
        },
    )?;

    Ok(())
}

pub async fn create_link(path: &Path, link: &Path) -> Result<(), Error> {
    let link = clean_and_check_path(link, false).await?;
    let path = clean_and_check_path(path, true).await?;

    tracing::info!(
        "Creating link from '{}' to '{}'",
        path.to_string_lossy(),
        link.to_string_lossy()
    );

    if link.exists() {
        // link already exists - check it is a link to the correct directory
        let metadata = link.symlink_metadata()?;

        if metadata.file_type().is_symlink() {
            // check the link points to the correct directory
            let target = link.read_link()?.canonicalize()?;

            if target != path {
                tracing::error!(
                    "Link '{}' already exists, but points to the wrong directory. Expected '{}', got '{}'",
                        link.to_string_lossy(), path.to_string_lossy(), target.to_string_lossy()
                );
            }

            // otherwise the link is already present and correct
            // It is best to stop now, and not try to do anything,
            // as we should assume that another process has already beaten
            // us to creating the link
            return Ok(());
        } else {
            tracing::error!(
                "Link '{}' already exists, but is not a symlink",
                link.to_string_lossy()
            );
        }
    }

    // create the link
    std::os::unix::fs::symlink(&path, &link).with_context(|| {
        format!(
            "Could not create link '{}' to '{}'",
            link.to_string_lossy(),
            path.to_string_lossy()
        )
    })?;

    Ok(())
}

///
/// Check if a directory exists in the .recycle subdirectory of its parent.
/// Returns Some(recycle_path) if found, None otherwise.
///
async fn check_recycle(path: &Path) -> Result<Option<PathBuf>, Error> {
    let parent = match path.parent() {
        Some(p) => p,
        None => return Ok(None),
    };

    let dir_name = match path.file_name() {
        Some(n) => n.to_string_lossy(),
        None => return Ok(None),
    };

    let recycle_path = parent.join(".recycle").join(dir_name.as_ref());

    if recycle_path.exists() {
        Ok(Some(recycle_path))
    } else {
        Ok(None)
    }
}

///
/// Restore a directory from .recycle by moving it back to its original location.
/// This is used when recreating a directory that was previously recycled.
///
async fn restore_from_recycle(recycle: &Path, target: &Path) -> Result<(), Error> {
    tracing::info!(
        "Restoring '{}' from recycle to '{}'",
        recycle.to_string_lossy(),
        target.to_string_lossy()
    );

    if !recycle.exists() {
        return Err(Error::State(format!(
            "Recycle path '{}' does not exist",
            recycle.to_string_lossy()
        )));
    }

    if target.exists() {
        return Err(Error::State(format!(
            "Target path '{}' already exists, cannot restore from recycle",
            target.to_string_lossy()
        )));
    }

    // Move the directory from recycle back to its original location
    std::fs::rename(recycle, target).with_context(|| {
        format!(
            "Could not restore '{}' from recycle to '{}'",
            recycle.to_string_lossy(),
            target.to_string_lossy()
        )
    })?;

    tracing::info!("Successfully restored directory from recycle");
    Ok(())
}

///
/// Move a directory to the .recycle subdirectory of its parent and update its timestamp.
/// This is a non-destructive way to "remove" directories - they can be restored later
/// or permanently deleted by a separate cleanup process.
///
pub async fn recycle_dir(path: &Path) -> Result<(), Error> {
    let path = clean_and_check_path(path, false).await?;

    if !path.exists() {
        tracing::warn!(
            "Directory '{}' does not exist, nothing to recycle",
            path.to_string_lossy()
        );
        return Ok(());
    }

    let parent = match path.parent() {
        Some(p) => p,
        None => {
            return Err(Error::State(format!(
                "Cannot recycle root directory '{}'",
                path.to_string_lossy()
            )))
        }
    };

    let dir_name = match path.file_name() {
        Some(n) => n,
        None => {
            return Err(Error::State(format!(
                "Cannot determine directory name for '{}'",
                path.to_string_lossy()
            )))
        }
    };

    // Create .recycle directory if it doesn't exist
    let recycle_parent = parent.join(".recycle");
    if !recycle_parent.exists() {
        tracing::info!(
            "Creating recycle directory '{}'",
            recycle_parent.to_string_lossy()
        );
        std::fs::create_dir(&recycle_parent).with_context(|| {
            format!(
                "Could not create recycle directory '{}'",
                recycle_parent.to_string_lossy()
            )
        })?;
    }

    let recycle_path = recycle_parent.join(dir_name);

    // If something already exists in recycle with this name, we need to handle it
    if recycle_path.exists() {
        tracing::warn!(
            "Recycle path '{}' already exists. Removing old recycled directory.",
            recycle_path.to_string_lossy()
        );
        std::fs::remove_dir_all(&recycle_path).with_context(|| {
            format!(
                "Could not remove old recycled directory '{}'",
                recycle_path.to_string_lossy()
            )
        })?;
    }

    tracing::info!(
        "Moving '{}' to recycle '{}'",
        path.to_string_lossy(),
        recycle_path.to_string_lossy()
    );

    // Move the directory to recycle
    std::fs::rename(&path, &recycle_path).with_context(|| {
        format!(
            "Could not move '{}' to recycle '{}'",
            path.to_string_lossy(),
            recycle_path.to_string_lossy()
        )
    })?;

    // Update the timestamp to current time using filetime crate
    // This sets both access and modification times to current time
    let now = filetime::FileTime::now();

    match filetime::set_file_times(&recycle_path, now, now) {
        Ok(_) => tracing::info!("Successfully recycled directory with updated timestamp"),
        Err(e) => {
            tracing::warn!("Could not update timestamp on recycled directory: {}", e);
            // Don't fail here - the directory was successfully recycled
        }
    }

    Ok(())
}
