// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::{Context, Result};

use templemeads::Error;

use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use nix::unistd::{Gid, Group, Uid, User};

///
/// Create the user's home directory at 'homedir'. The directory
/// should be created for the user 'username' and group 'groupname',
/// with unix permissions in 'permissions' (as a string, e.g. "0755")
/// An optional config script can be passed as 'config_script'. If this
/// is passed, then it is executed with the argument being the path
/// to the newly created home directory.
///
pub async fn create_home_dir(
    homedir: &str,
    username: &str,
    groupname: &str,
    permissions: &str,
    config_script: &Option<String>,
) -> Result<(), Error> {
    // convert the permissions into a u32
    let permissions = u32::from_str_radix(permissions, 8)
        .with_context(|| format!("Could not convert permissions '{}' into a u32", permissions))?;

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
    let path = Path::new(homedir);

    if path.exists() {
        // directory already exists - check it has the right permissions
        // and user / group ownership
        let metadata = path.metadata()?;

        // check the ownership
        if Uid::from_raw(metadata.uid()) != uid {
            // ownership is wrong
            return Err(Error::State(
                format!(
                    "Home directory '{}' already exists, but has the wrong ownership. Expected '{}', got '{}'",
                    homedir, uid, Uid::from_raw(metadata.uid())
                )
            ));
        }

        if Gid::from_raw(metadata.gid()) != gid {
            // ownership is wrong
            return Err(Error::State(
                format!(
                    "Home directory '{}' already exists, but has the wrong group ownership. Expected '{}', got '{}'",
                    homedir, gid, Gid::from_raw(metadata.gid())
                )
            ));
        }

        // check the permissions
        if permissions != metadata.permissions().mode() {
            // permissions are wrong
            return Err(Error::State(
                format!(
                    "Home directory '{}' already exists, but has the wrong permissions. Expected '{}', got '{}'",
                    homedir, unix_mode::to_string(permissions),
                    unix_mode::to_string(metadata.permissions().mode())
                )
            ));
        }

        // otherwise the directory is already present and correct
        // It is best to stop now, and not try to do anything,
        // as we should assume that another process has already beaten
        // us to creating the directory
        return Ok(());
    }

    // create the directory
    std::fs::create_dir(homedir)
        .with_context(|| format!("Could not create home directory '{}'", homedir))?;

    // set the ownership and permissions
    nix::unistd::chown(homedir, Some(uid), Some(gid))
        .with_context(|| format!("Could not set ownership on home directory '{}'", homedir))?;

    std::fs::set_permissions(homedir, std::fs::Permissions::from_mode(permissions))
        .with_context(|| format!("Could not set permissions on home directory '{}'", homedir))?;

    // run the config script if it is present
    if let Some(script) = config_script {
        let output = tokio::process::Command::new(script)
            .arg(homedir)
            .output()
            .await
            .with_context(|| format!("Could not run config script '{}'", script))?;

        if !output.status.success() {
            return Err(Error::State(format!(
                "Config script '{}' failed with status '{}'",
                script, output.status
            )));
        }
    }

    Ok(())
}
