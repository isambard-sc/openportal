// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Result;
use chrono::Utc;
use once_cell::sync::OnceCell;
use std::collections::HashMap;
use templemeads::agent::Peer;
use templemeads::grammar::{
    PortalIdentifier, ProjectIdentifier, ProjectMapping, UserIdentifier, UserMapping,
};
use templemeads::job::assert_not_expired;
use templemeads::Error;
use tokio::process::Command;

static COMMANDS: OnceCell<Commands> = OnceCell::new();

///
/// Configuration for the Unix commands used by this agent. Each command
/// is stored as a pre-split list of tokens so that prefixes like
/// "docker exec slurmctld useradd" work without any shell quoting issues.
///
pub struct Commands {
    useradd: Vec<String>,
    userdel: Vec<String>,
    groupadd: Vec<String>,
    groupdel: Vec<String>,
    usermod: Vec<String>,
    getent: Vec<String>,
    /// Group that all users managed by this agent are added to, used to
    /// distinguish managed users from pre-existing system accounts.
    managed_group: String,
    /// Extra Unix groups added to every managed user, regardless of instance.
    /// Configured via `system-groups = "groupA,groupB"` in the config file.
    system_groups: Vec<String>,
    /// Extra Unix groups added only to users managed for a specific instance.
    /// Configured via `instance-groups = "instanceA:groupX,instanceA:groupY,instanceB:groupZ"`.
    instance_groups: HashMap<String, Vec<String>>,
}

impl Commands {
    fn parse_cmd(s: &str) -> Vec<String> {
        s.split_whitespace().map(|p| p.to_owned()).collect()
    }

    pub fn new(
        useradd: &str,
        userdel: &str,
        groupadd: &str,
        groupdel: &str,
        usermod: &str,
        getent: &str,
        managed_group: &str,
        system_groups: Vec<String>,
        instance_groups: HashMap<String, Vec<String>>,
    ) -> Self {
        Self {
            useradd: Self::parse_cmd(useradd),
            userdel: Self::parse_cmd(userdel),
            groupadd: Self::parse_cmd(groupadd),
            groupdel: Self::parse_cmd(groupdel),
            usermod: Self::parse_cmd(usermod),
            getent: Self::parse_cmd(getent),
            managed_group: managed_group.to_owned(),
            system_groups,
            instance_groups,
        }
    }
}

pub fn initialise_commands(cmds: Commands) -> Result<()> {
    COMMANDS
        .set(cmds)
        .map_err(|_| anyhow::anyhow!("Commands already initialised"))
}

fn get_commands() -> Result<&'static Commands, Error> {
    COMMANDS
        .get()
        .ok_or_else(|| Error::Call("Commands not initialised".to_owned()))
}

///
/// Run a command built from a pre-tokenised prefix plus additional args.
/// Returns (exit_code, stdout, stderr).
///
async fn run_command(parts: &[String], args: &[&str]) -> Result<(i32, String, String), Error> {
    if parts.is_empty() {
        return Err(Error::Call("Empty command template".to_owned()));
    }

    tracing::debug!("Running command: {} {}", parts.join(" "), args.join(" "));

    let mut cmd = Command::new(&parts[0]);
    for part in &parts[1..] {
        cmd.arg(part);
    }
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().await.map_err(|e| {
        Error::Call(format!(
            "Failed to spawn command {}: {}",
            parts.join(" "),
            e
        ))
    })?;

    let exit_code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    tracing::debug!(
        "Command exit code: {}, stdout: {}, stderr: {}",
        exit_code,
        stdout,
        stderr
    );

    Ok((exit_code, stdout, stderr))
}

///
/// Return the local Unix username for a UserIdentifier.
/// Format: "{username}.{project}"
///
pub fn identifier_to_userid(user: &UserIdentifier) -> String {
    format!("{}.{}", user.username(), user.project())
}

///
/// Return the local Unix group name for a ProjectIdentifier.
/// Format: "{portal}.{project}", except for internal portals
/// (openportal, system, instance) which use just "{project}".
///
fn identifier_to_projectid(project: &ProjectIdentifier) -> String {
    let system_portals = ["openportal", "system", "instance"];
    if system_portals.contains(&project.portal().as_str()) {
        project.project().to_string()
    } else {
        format!("{}.{}", project.portal(), project.project())
    }
}

///
/// Return the name of the primary Unix group for a user.
/// This is the project group: "{portal}.{project}".
///
pub fn get_primary_group_name(user: &UserIdentifier) -> String {
    identifier_to_projectid(&user.project_identifier())
}

///
/// Return the name of the auto-generated per-instance group for the
/// given instance peer.  Mirrors freeipa's get_op_instance_group naming:
/// "op-{peer}" with all non-alphanumeric characters replaced by "_".
///
fn instance_group_name(instance: &Peer) -> String {
    format!("op-{}", instance)
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' => c,
            _ => '_',
        })
        .collect()
}

///
/// Ensure the named Unix group exists, creating it with groupadd if not.
/// Idempotent: succeeds silently if the group already exists (exit code 9).
///
async fn ensure_group_exists(
    group_name: &str,
    expires: &chrono::DateTime<Utc>,
) -> Result<(), Error> {
    assert_not_expired(expires)?;

    let cmds = get_commands()?;

    let (exit_code, _, _) = run_command(&cmds.getent, &["group", group_name]).await?;
    if exit_code == 0 {
        return Ok(());
    }

    let (gc_exit, _, stderr) = run_command(&cmds.groupadd, &[group_name]).await?;
    match gc_exit {
        0 => tracing::info!("Created group: {}", group_name),
        9 => tracing::debug!("Group already exists: {}", group_name),
        _ => {
            return Err(Error::Call(format!(
                "groupadd failed for '{}': exit code {}, stderr: {}",
                group_name, gc_exit, stderr
            )));
        }
    }

    Ok(())
}

///
/// Ensure the user is a member of all groups they should belong to, creating
/// any groups that do not yet exist.  The full set of groups is:
///
///   - project group  ({portal}.{project})
///   - managed group  (default: "openportal")
///   - auto instance group  (op-{instance}, sanitised)
///   - system groups  (from config `system-groups`)
///   - per-instance groups  (from config `instance-groups` for this peer)
///
async fn sync_groups(
    local_user: &str,
    user: &UserIdentifier,
    instance: &Peer,
    expires: &chrono::DateTime<Utc>,
) -> Result<(), Error> {
    assert_not_expired(expires)?;

    let cmds = get_commands()?;

    let mut groups: Vec<String> = Vec::new();

    // 1. Project group — the group that represents this user's project.
    groups.push(identifier_to_projectid(&user.project_identifier()));

    // 2. Managed group — marks the user as managed by this agent.
    groups.push(cmds.managed_group.clone());

    // 3. Per-instance group — one group per instance that this user belongs to.
    groups.push(instance_group_name(instance));

    // 4. System groups — extra groups applied to all managed users.
    groups.extend(cmds.system_groups.clone());

    // 5. Instance-specific groups — extra groups for this particular instance.
    if let Some(ig) = cmds.instance_groups.get(instance.name()) {
        groups.extend(ig.clone());
    }

    // Deduplicate while preserving order.
    let mut seen = std::collections::HashSet::new();
    groups.retain(|g| seen.insert(g.clone()));

    // Ensure every group exists before we try to add the user to it.
    for group in &groups {
        ensure_group_exists(group, expires).await?;
    }

    let groups_str = groups.join(",");
    tracing::info!("Syncing user '{}' into groups: {}", local_user, groups_str);

    let (exit_code, _, stderr) =
        run_command(&cmds.usermod, &["-aG", &groups_str, local_user]).await?;

    if exit_code != 0 {
        return Err(Error::Call(format!(
            "usermod -aG failed for '{}': exit code {}, stderr: {}",
            local_user, exit_code, stderr
        )));
    }

    Ok(())
}

///
/// Add a project (Unix group) for the given ProjectIdentifier.
/// Idempotent: succeeds silently if the group already exists.
///
pub async fn add_project(
    project: &ProjectIdentifier,
    expires: &chrono::DateTime<Utc>,
) -> Result<ProjectMapping, Error> {
    assert_not_expired(expires)?;

    let group_name = identifier_to_projectid(project);

    tracing::info!("Adding project group: {}", group_name);

    ensure_group_exists(&group_name, expires).await?;

    ProjectMapping::new(project, &group_name).map_err(|e| Error::Call(e.to_string()))
}

///
/// Remove the project (Unix group) for the given ProjectIdentifier.
/// Idempotent: succeeds silently if the group did not exist.
///
pub async fn remove_project(
    project: &ProjectIdentifier,
    expires: &chrono::DateTime<Utc>,
) -> Result<ProjectMapping, Error> {
    assert_not_expired(expires)?;

    let group_name = identifier_to_projectid(project);
    let cmds = get_commands()?;

    tracing::info!("Removing project group: {}", group_name);

    let (exit_code, _, stderr) = run_command(&cmds.groupdel, &[&group_name]).await?;

    match exit_code {
        0 => {
            tracing::info!("Project group removed: {}", group_name);
        }
        6 => {
            tracing::warn!("Project group did not exist: {}", group_name);
        }
        _ => {
            return Err(Error::Call(format!(
                "groupdel failed for '{}': exit code {}, stderr: {}",
                group_name, exit_code, stderr
            )));
        }
    }

    ProjectMapping::new(project, &group_name).map_err(|e| Error::Call(e.to_string()))
}

///
/// Add a user to the local system for the given instance.
///
/// All required groups (project, managed, per-instance, system, and any
/// instance-specific groups from config) are created if they do not yet
/// exist, then the user is added to all of them.  The supplied homedir is
/// used; if None a default of /home/{local_user} is used.
///
pub async fn add_user(
    user: &UserIdentifier,
    instance: &Peer,
    homedir: &Option<String>,
    expires: &chrono::DateTime<Utc>,
) -> Result<UserMapping, Error> {
    assert_not_expired(expires)?;

    let local_user = identifier_to_userid(user);
    let local_group = get_primary_group_name(user);
    let cmds = get_commands()?;

    let default_home = format!("/home/{}", local_user);
    let homedir_str = homedir.as_deref().unwrap_or(&default_home);

    tracing::info!("Adding user: {}", local_user);

    let (exit_code, _, stderr) = run_command(
        &cmds.useradd,
        &["-d", homedir_str, "-m", "-s", "/bin/bash", &local_user],
    )
    .await?;

    match exit_code {
        0 => {
            tracing::info!("User created: {}", local_user);
        }
        9 => {
            tracing::warn!("User already exists, will sync groups: {}", local_user);
        }
        _ => {
            return Err(Error::Call(format!(
                "useradd failed for '{}': exit code {}, stderr: {}",
                local_user, exit_code, stderr
            )));
        }
    }

    // Now create all required groups and add the user to them.
    sync_groups(&local_user, user, instance, expires).await?;

    UserMapping::new(user, &local_user, &local_group).map_err(|e| Error::Call(e.to_string()))
}

///
/// Remove a user from the local system (and remove their home directory).
/// Idempotent: succeeds silently if the user did not exist.
///
pub async fn remove_user(
    user: &UserIdentifier,
    expires: &chrono::DateTime<Utc>,
) -> Result<UserMapping, Error> {
    assert_not_expired(expires)?;

    let local_user = identifier_to_userid(user);
    let local_group = get_primary_group_name(user);
    let cmds = get_commands()?;

    let mapping = UserMapping::new(user, &local_user, &local_group)
        .map_err(|e| Error::Call(e.to_string()))?;

    tracing::info!("Removing user: {}", local_user);

    let (exit_code, _, stderr) = run_command(&cmds.userdel, &["-r", &local_user]).await?;

    match exit_code {
        0 => {
            tracing::info!("User removed: {}", local_user);
        }
        6 => {
            tracing::warn!("User did not exist: {}", local_user);
        }
        _ => {
            return Err(Error::Call(format!(
                "userdel failed for '{}': exit code {}, stderr: {}",
                local_user, exit_code, stderr
            )));
        }
    }

    Ok(mapping)
}

///
/// Update the home directory for a user.
///
pub async fn update_homedir(
    user: &UserIdentifier,
    homedir: &str,
    expires: &chrono::DateTime<Utc>,
) -> Result<(), Error> {
    assert_not_expired(expires)?;

    let local_user = identifier_to_userid(user);
    let cmds = get_commands()?;

    tracing::info!("Updating home directory for {}: {}", local_user, homedir);

    let (exit_code, _, stderr) =
        run_command(&cmds.usermod, &["-d", homedir, &local_user]).await?;

    if exit_code != 0 {
        return Err(Error::Call(format!(
            "usermod -d failed for '{}': exit code {}, stderr: {}",
            local_user, exit_code, stderr
        )));
    }

    Ok(())
}

///
/// Return all project mappings for the given portal by scanning
/// `getent group` output for groups named "{portal}.{project}".
///
pub async fn get_groups(
    portal: &PortalIdentifier,
    expires: &chrono::DateTime<Utc>,
) -> Result<Vec<ProjectMapping>, Error> {
    assert_not_expired(expires)?;

    let cmds = get_commands()?;
    let prefix = format!("{}.", portal.portal());

    let (exit_code, stdout, stderr) = run_command(&cmds.getent, &["group"]).await?;

    if exit_code != 0 {
        return Err(Error::Call(format!(
            "getent group failed: exit code {}, stderr: {}",
            exit_code, stderr
        )));
    }

    let mut mappings = Vec::new();

    for line in stdout.lines() {
        // getent group output: groupname:x:gid:member1,member2,...
        let group_name = match line.split(':').next() {
            Some(n) => n,
            None => continue,
        };

        if !group_name.starts_with(&prefix) {
            continue;
        }

        let project_name = &group_name[prefix.len()..];
        if project_name.is_empty() {
            continue;
        }

        // Reconstruct ProjectIdentifier from "{project}.{portal}"
        let project_id_str = format!("{}.{}", project_name, portal.portal());
        match ProjectIdentifier::parse(&project_id_str) {
            Ok(project) => match ProjectMapping::new(&project, group_name) {
                Ok(mapping) => mappings.push(mapping),
                Err(e) => {
                    tracing::warn!("Could not create mapping for group '{}': {}", group_name, e)
                }
            },
            Err(e) => tracing::warn!(
                "Could not parse project identifier '{}': {}",
                project_id_str,
                e
            ),
        }
    }

    Ok(mappings)
}

///
/// Return user mappings for all members of the given project's Unix group.
///
pub async fn get_users(
    project: &ProjectIdentifier,
    expires: &chrono::DateTime<Utc>,
) -> Result<Vec<UserMapping>, Error> {
    assert_not_expired(expires)?;

    let group_name = identifier_to_projectid(project);
    let cmds = get_commands()?;

    let (exit_code, stdout, stderr) =
        run_command(&cmds.getent, &["group", &group_name]).await?;

    match exit_code {
        0 => {}
        2 => {
            // Group does not exist — return empty list.
            return Ok(Vec::new());
        }
        _ => {
            return Err(Error::Call(format!(
                "getent group '{}' failed: exit code {}, stderr: {}",
                group_name, exit_code, stderr
            )));
        }
    }

    // Output line: groupname:x:gid:user1,user2,...
    let line = stdout.trim();
    let members_field = line.splitn(4, ':').nth(3).unwrap_or("");

    if members_field.is_empty() {
        return Ok(Vec::new());
    }

    let mut mappings = Vec::new();

    for member in members_field.split(',') {
        let member = member.trim();
        if member.is_empty() {
            continue;
        }

        // Unix username format: "{username}.{project}" (neither part contains dots)
        if let Some(dot_pos) = member.find('.') {
            let username_part = &member[..dot_pos];
            let project_part = &member[dot_pos + 1..];

            if project_part != project.project() {
                // Belongs to a different project — skip.
                continue;
            }

            let user_id_str = format!("{}.{}.{}", username_part, project_part, project.portal());
            match UserIdentifier::parse(&user_id_str) {
                Ok(user_id) => {
                    let local_group = get_primary_group_name(&user_id);
                    match UserMapping::new(&user_id, member, &local_group) {
                        Ok(mapping) => mappings.push(mapping),
                        Err(e) => {
                            tracing::warn!("Could not create user mapping for '{}': {}", member, e)
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Could not parse user identifier '{}': {}", user_id_str, e)
                }
            }
        }
    }

    Ok(mappings)
}

///
/// Return the ProjectMapping for a project, or an error if it does not exist.
///
pub async fn get_project_mapping(
    project: &ProjectIdentifier,
    expires: &chrono::DateTime<Utc>,
) -> Result<ProjectMapping, Error> {
    assert_not_expired(expires)?;

    if !is_existing_project(project, expires).await? {
        return Err(Error::Call(format!("Project does not exist: {}", project)));
    }

    let group_name = identifier_to_projectid(project);
    ProjectMapping::new(project, &group_name).map_err(|e| Error::Call(e.to_string()))
}

///
/// Return the UserMapping for a user, or an error if they do not exist.
///
pub async fn get_user_mapping(
    user: &UserIdentifier,
    expires: &chrono::DateTime<Utc>,
) -> Result<UserMapping, Error> {
    assert_not_expired(expires)?;

    if !is_existing_user(user, expires).await? {
        return Err(Error::Call(format!("User does not exist: {}", user)));
    }

    let local_user = identifier_to_userid(user);
    let local_group = get_primary_group_name(user);
    UserMapping::new(user, &local_user, &local_group).map_err(|e| Error::Call(e.to_string()))
}

///
/// Return true if the local Unix user for the given identifier exists.
///
pub async fn is_existing_user(
    user: &UserIdentifier,
    expires: &chrono::DateTime<Utc>,
) -> Result<bool, Error> {
    assert_not_expired(expires)?;

    let local_user = identifier_to_userid(user);
    let cmds = get_commands()?;

    let (exit_code, _, _) = run_command(&cmds.getent, &["passwd", &local_user]).await?;

    Ok(exit_code == 0)
}

///
/// Return true if the local Unix group for the given project exists.
///
pub async fn is_existing_project(
    project: &ProjectIdentifier,
    expires: &chrono::DateTime<Utc>,
) -> Result<bool, Error> {
    assert_not_expired(expires)?;

    let group_name = identifier_to_projectid(project);
    let cmds = get_commands()?;

    let (exit_code, _, _) = run_command(&cmds.getent, &["group", &group_name]).await?;

    Ok(exit_code == 0)
}

///
/// Return true if the user is "protected" — i.e. the user exists on the
/// system but was NOT created by this agent. Managed users are identified
/// by membership of the managed group (default: "openportal").
///
pub async fn is_protected_user(
    user: &UserIdentifier,
    expires: &chrono::DateTime<Utc>,
) -> Result<bool, Error> {
    assert_not_expired(expires)?;

    if !is_existing_user(user, expires).await? {
        return Ok(false);
    }

    let local_user = identifier_to_userid(user);
    let cmds = get_commands()?;

    let (exit_code, stdout, _) =
        run_command(&cmds.getent, &["group", &cmds.managed_group]).await?;

    if exit_code != 0 {
        // Managed group doesn't exist — user must be unmanaged/protected.
        return Ok(true);
    }

    // Output: groupname:x:gid:member1,member2,...
    let line = stdout.trim();
    let members_field = line.splitn(4, ':').nth(3).unwrap_or("");

    let is_managed = members_field
        .split(',')
        .any(|m| m.trim() == local_user.as_str());

    Ok(!is_managed)
}
