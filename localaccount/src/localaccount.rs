// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! # op-localaccount — a testing agent
//!
//! **This agent is intended for testing only.** It manages Unix accounts and
//! groups directly with `useradd`/`groupadd`/… (typically `docker exec`'d into
//! a containerised test Slurm cluster), rather than through a managed directory
//! service like FreeIPA. Production account management should use `op-freeipa`.
//!
//! It is nonetheless written to be safe if it is mistakenly deployed against a
//! real system: it only ever removes accounts and groups it manages — a user
//! must be a member of the managed group before it will `userdel` them
//! (`is_protected_user`), and a group must have a normal (non-system) GID and
//! not be a configured system/managed group before it will `groupdel` it
//! (`is_protected_project`). See docs/specifications/security-review.md
//! (finding F13).

use anyhow::Result;
use chrono::Utc;
use greatwestern::grammar::{ProjectIdentifier, ProjectMapping, UserIdentifier, UserMapping};
use once_cell::sync::OnceCell;
use std::collections::HashMap;
use templemeads::agent::Peer;
use templemeads::job::assert_not_expired;
use templemeads::portal_identifier::PortalIdentifier;
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
    gpasswd: Vec<String>,
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

    // ignore too many arguments warning for this constructor,
    // since it's more ergonomic to construct the Commands struct directly
    // from the config file with all fields specified.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        useradd: &str,
        userdel: &str,
        groupadd: &str,
        groupdel: &str,
        usermod: &str,
        getent: &str,
        gpasswd: &str,
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
            gpasswd: Self::parse_cmd(gpasswd),
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

    let Some((program, extra_args)) = parts.split_first() else {
        return Err(Error::Call("Empty command template".to_owned()));
    };

    let mut cmd = Command::new(program);
    for part in extra_args {
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

/// The portal names that map a project to a *bare* Unix group name rather than
/// the usual `{portal}.{project}`. These exist so that an operator can point
/// this agent at pre-existing groups via configuration.
const INTERNAL_PORTALS: [&str; 3] = ["openportal", "system", "instance"];

///
/// Return the local Unix group name for a ProjectIdentifier.
/// Format: "{portal}.{project}", except for internal portals
/// (openportal, system, instance) which use just "{project}".
///
fn identifier_to_projectid(project: &ProjectIdentifier) -> String {
    if INTERNAL_PORTALS.contains(&project.portal().as_str()) {
        project.project().to_string()
    } else {
        format!("{}.{}", project.portal(), project.project())
    }
}

///
/// Refuse an identifier that arrived from a peer and names an internal portal.
///
/// An internal portal maps to a *bare* group name (see
/// `identifier_to_projectid`), so `bob.docker.system` resolves to the group
/// `docker` - and the add path would then put the new account into the host's
/// real `docker` group, which is root-equivalent on most hosts. `wheel`,
/// `sudo`, `shadow` and `adm` behave identically. Only configuration should
/// name these groups, never a Job. `op-freeipa` already refuses internal
/// portals this way in `force_get_user`/`get_users`; this brings
/// `op-localaccount` into line. See
/// `docs/specifications/security-review-2.md` (finding R13).
///
fn assert_not_internal_portal(project: &ProjectIdentifier) -> Result<(), Error> {
    if INTERNAL_PORTALS.contains(&project.portal().as_str()) {
        tracing::warn!(
            "Refusing to act on '{}': '{}' is an internal portal name, which \
             maps to a bare Unix group and may only be named by configuration.",
            project,
            project.portal()
        );

        return Err(Error::Call(format!(
            "Refusing to act on '{}' - '{}' is an internal portal name",
            project,
            project.portal()
        )));
    }

    Ok(())
}

///
/// Return true if `group_name` already exists with a system GID (below
/// `MANAGED_GID_MIN`) - i.e. it is a pre-existing group this agent did not
/// create and must not adopt. A group that does not exist is not a system
/// group; an unparseable GID fails safe (treated as one).
///
/// Defence in depth behind `assert_not_internal_portal`: that stops the
/// bare-name collision at its source, and this stops the add path adopting a
/// pre-existing privileged group by any other route. See
/// `docs/specifications/security-review-2.md` (finding R13).
///
async fn is_system_group(group_name: &str) -> Result<bool, Error> {
    let cmds = get_commands()?;

    let (exit_code, stdout, _) = run_command(&cmds.getent, &["group", group_name]).await?;

    if exit_code != 0 {
        // Does not exist - this agent will create it, so it is ours.
        return Ok(false);
    }

    // getent group output: groupname:x:gid:member1,member2,...
    match stdout
        .trim()
        .split(':')
        .nth(2)
        .and_then(|g| g.trim().parse::<u64>().ok())
    {
        Some(gid) => Ok(gid < MANAGED_GID_MIN),
        // Could not parse the GID - fail safe.
        None => Ok(true),
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

    let (gc_exit, _, stderr) = run_command(&cmds.groupadd, &["--", group_name]).await?;
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
    //
    // Checked against `is_system_group` because this is the one group in the
    // list that is *derived from a peer-supplied identifier* rather than named
    // by configuration - so it is the one that can collide with a pre-existing
    // privileged group. See
    // `docs/specifications/security-review-2.md` (finding R13).
    let project_group = identifier_to_projectid(&user.project_identifier());

    if is_system_group(&project_group).await? {
        tracing::warn!(
            "Refusing to add user '{}' to existing system group '{}'",
            local_user,
            project_group
        );
        return Err(Error::Call(format!(
            "Refusing to add user '{}' to existing system group '{}'",
            local_user, project_group
        )));
    }

    groups.push(project_group);

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
        run_command(&cmds.usermod, &["-aG", &groups_str, "--", local_user]).await?;

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
    assert_not_internal_portal(project)?;

    let group_name = identifier_to_projectid(project);

    if is_system_group(&group_name).await? {
        tracing::warn!(
            "Refusing to adopt existing system group '{}' as a project group",
            group_name
        );
        return Err(Error::Call(format!(
            "Refusing to adopt existing system group '{}' as a project group",
            group_name
        )));
    }

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

    let mapping =
        ProjectMapping::new(project, &group_name).map_err(|e| Error::Call(e.to_string()))?;

    // Refuse to delete a group this agent does not manage (finding F13). This
    // guards against a crafted ProjectIdentifier whose derived group name
    // collides with a system group (e.g. a project identifier "docker.system"
    // maps to the bare group name "docker"). See `is_protected_project`.
    if is_protected_project(project, expires).await? {
        tracing::warn!(
            "Ignoring request to remove group '{}' as it is not an OpenPortal-managed \
             project group",
            group_name
        );
        return Ok(mapping);
    }

    tracing::info!("Removing project group: {}", group_name);

    let (exit_code, _, stderr) = run_command(&cmds.groupdel, &["--", &group_name]).await?;

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

    Ok(mapping)
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
    assert_not_internal_portal(&user.project_identifier())?;

    let local_user = identifier_to_userid(user);
    let local_group = get_primary_group_name(user);
    let cmds = get_commands()?;

    // blocked users must be explicitly unblocked - don't re-enable them here
    if is_blocked_user(user, expires).await? {
        tracing::info!(
            "User {} is blocked - not re-adding. Use unblock_user to unblock.",
            local_user
        );
        return UserMapping::new(user, &local_user, &local_group)
            .map_err(|e| Error::Call(e.to_string()));
    }

    let default_home = format!("/home/{}", local_user);
    let homedir_str = homedir.as_deref().unwrap_or(&default_home);

    // `useradd -m` creates this directory (and any missing parents) as root and
    // chowns it to the new account, and on this path the value came back over
    // the wire from the peer - so validate it before handing it over. See
    // `docs/specifications/security-review-2.md` (finding R13).
    check_homedir(homedir_str)?;

    tracing::info!("Adding user: {}", local_user);

    let (exit_code, _, stderr) = run_command(
        &cmds.useradd,
        // `--` ends option parsing so a name can never be read as a flag
        // (defence-in-depth on top of identifier validation, finding F15).
        &[
            "-d",
            homedir_str,
            "-m",
            "-s",
            "/bin/bash",
            "--",
            &local_user,
        ],
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
/// Remove a user from the local system.
/// Idempotent: succeeds silently if the user did not exist.
/// Note: the home directory is intentionally NOT removed here — home directories
/// are managed separately by the filesystem agent, which recycles them rather
/// than deleting them.
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

    // Refuse to delete an account this agent does not manage (finding F13).
    // Mirrors the guard block_user/unblock_user already apply: a managed user
    // is a member of the managed group; anything else is a pre-existing system
    // account we must never touch. `is_protected_user` returns false for a
    // non-existent user, so removal stays idempotent for accounts we did
    // create but that are already gone.
    if is_protected_user(user, expires).await? {
        tracing::warn!(
            "Ignoring request to remove {} as they are not managed by this agent",
            local_user
        );
        return Ok(mapping);
    }

    tracing::info!("Removing user: {}", local_user);

    let (exit_code, _, stderr) = run_command(&cmds.userdel, &["--", &local_user]).await?;

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
///
/// Check that `homedir` is a plausible home directory before it is handed to
/// `usermod -d` or `useradd -d ... -m`.
///
/// The home directory is a bare `String` on the wire, and on the `AddUser` path
/// it is supplied by the *peer* (via `get_home_dir`). `useradd -m` will create
/// a non-existent directory - including missing parents - as root and chown it
/// to the new account, so an unvalidated value lets a peer choose where that
/// happens. `op-filesystem` applies an equivalent check
/// (`clean_and_check_path`) to every path it touches; this brings the
/// shadow-utils path into line. See
/// `docs/specifications/security-review-2.md` (finding R13).
///
fn check_homedir(homedir: &str) -> Result<(), Error> {
    let path = std::path::Path::new(homedir);

    if homedir.trim().is_empty() {
        return Err(Error::Call("Home directory cannot be empty".to_owned()));
    }

    if !path.is_absolute() {
        return Err(Error::Call(format!(
            "Home directory '{}' is not an absolute path",
            homedir
        )));
    }

    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(Error::Call(format!(
            "Home directory '{}' contains a '..' component",
            homedir
        )));
    }

    // Same sensitive-location list `op-filesystem::clean_and_check_path` uses.
    for sensitive in [
        "/etc", "/var", "/usr", "/bin", "/sbin", "/lib", "/lib64", "/boot", "/root", "/dev",
        "/proc", "/sys", "/run", "/tmp",
    ] {
        if path.starts_with(sensitive) {
            return Err(Error::Call(format!(
                "Home directory '{}' is in a sensitive location",
                homedir
            )));
        }
    }

    if path == std::path::Path::new("/") {
        return Err(Error::Call(
            "Home directory cannot be the root directory".to_owned(),
        ));
    }

    Ok(())
}

pub async fn update_homedir(
    user: &UserIdentifier,
    homedir: &str,
    expires: &chrono::DateTime<Utc>,
) -> Result<(), Error> {
    assert_not_expired(expires)?;
    assert_not_internal_portal(&user.project_identifier())?;
    check_homedir(homedir)?;

    let local_user = identifier_to_userid(user);
    let cmds = get_commands()?;

    // Refuse to retarget an account this agent does not manage - the same guard
    // `remove_user`/`block_user`/`unblock_user` already apply, and that
    // `op-freeipa::update_homedir` applies. Its absence here was an
    // inconsistency, not a design choice. See
    // `docs/specifications/security-review-2.md` (finding R13).
    if is_protected_user(user, expires).await? {
        tracing::warn!(
            "Ignoring request to update the home directory of {} as they are \
             not managed by this agent",
            local_user
        );
        return Ok(());
    }

    tracing::info!("Updating home directory for {}: {}", local_user, homedir);

    let (exit_code, _, stderr) =
        run_command(&cmds.usermod, &["-d", homedir, "--", &local_user]).await?;

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

        // `strip_prefix` rather than slicing at `prefix.len()` - see
        // docs/specifications/security-review-2.md (finding R1).
        let Some(project_name) = group_name.strip_prefix(&prefix) else {
            continue;
        };

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

    let (exit_code, stdout, stderr) = run_command(&cmds.getent, &["group", &group_name]).await?;

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
        if let Some((username_part, project_part)) = member.split_once('.') {
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
/// Return the name of the blocked group: "{managed_group}.blocked".
/// Membership of this group is the source of truth for whether a user
/// is blocked, mirroring the "openportal.blocked" convention in FreeIPA.
///
fn blocked_group_name(cmds: &Commands) -> String {
    format!("{}.blocked", cmds.managed_group)
}

///
/// Return true if the local Unix user is a member of the given group.
///
async fn is_user_in_group(
    local_user: &str,
    group: &str,
    expires: &chrono::DateTime<Utc>,
) -> Result<bool, Error> {
    assert_not_expired(expires)?;

    let cmds = get_commands()?;

    let (exit_code, stdout, _) = run_command(&cmds.getent, &["group", group]).await?;

    if exit_code != 0 {
        return Ok(false);
    }

    let line = stdout.trim();
    let members_field = line.splitn(4, ':').nth(3).unwrap_or("");

    Ok(members_field.split(',').any(|m| m.trim() == local_user))
}

pub async fn is_blocked_user(
    user: &UserIdentifier,
    expires: &chrono::DateTime<Utc>,
) -> Result<bool, Error> {
    assert_not_expired(expires)?;

    let local_user = identifier_to_userid(user);
    let cmds = get_commands()?;
    let blocked_group = blocked_group_name(cmds);

    is_user_in_group(&local_user, &blocked_group, expires).await
}

///
/// Block a managed user by adding them to the blocked group and locking
/// their Unix account. Idempotent: returns the mapping without error if
/// the user is already blocked.
///
pub async fn block_user(
    user: &UserIdentifier,
    expires: &chrono::DateTime<Utc>,
) -> Result<UserMapping, Error> {
    assert_not_expired(expires)?;

    let local_user = identifier_to_userid(user);
    let local_group = get_primary_group_name(user);
    let cmds = get_commands()?;
    let blocked_group = blocked_group_name(cmds);

    let mapping = UserMapping::new(user, &local_user, &local_group)
        .map_err(|e| Error::Call(e.to_string()))?;

    if is_protected_user(user, expires).await? {
        tracing::warn!(
            "Ignoring request to block {} as they are not managed by this agent",
            local_user
        );
        return Ok(mapping);
    }

    if is_blocked_user(user, expires).await? {
        tracing::info!("User {} is already blocked - nothing to do.", local_user);
        return Ok(mapping);
    }

    ensure_group_exists(&blocked_group, expires).await?;

    let (exit_code, _, stderr) =
        run_command(&cmds.usermod, &["-aG", &blocked_group, "--", &local_user]).await?;

    if exit_code != 0 {
        return Err(Error::Call(format!(
            "usermod -aG {} failed for '{}': exit code {}, stderr: {}",
            blocked_group, local_user, exit_code, stderr
        )));
    }

    let (exit_code, _, stderr) = run_command(&cmds.usermod, &["-L", "--", &local_user]).await?;

    if exit_code != 0 {
        return Err(Error::Call(format!(
            "usermod -L failed for '{}': exit code {}, stderr: {}",
            local_user, exit_code, stderr
        )));
    }

    tracing::info!("Blocked user: {}", local_user);

    Ok(mapping)
}

///
/// Unblock a previously blocked user by removing them from the blocked group
/// and unlocking their Unix account. Idempotent: returns the mapping without
/// error if the user is not currently blocked.
///
pub async fn unblock_user(
    user: &UserIdentifier,
    expires: &chrono::DateTime<Utc>,
) -> Result<UserMapping, Error> {
    assert_not_expired(expires)?;

    let local_user = identifier_to_userid(user);
    let local_group = get_primary_group_name(user);
    let cmds = get_commands()?;
    let blocked_group = blocked_group_name(cmds);

    let mapping = UserMapping::new(user, &local_user, &local_group)
        .map_err(|e| Error::Call(e.to_string()))?;

    if is_protected_user(user, expires).await? {
        tracing::warn!(
            "Ignoring request to unblock {} as they are not managed by this agent",
            local_user
        );
        return Ok(mapping);
    }

    if !is_blocked_user(user, expires).await? {
        tracing::info!("User {} is not blocked - nothing to do.", local_user);
        return Ok(mapping);
    }

    let (exit_code, _, stderr) =
        run_command(&cmds.gpasswd, &["-d", &local_user, &blocked_group]).await?;

    if exit_code != 0 {
        return Err(Error::Call(format!(
            "gpasswd -d failed for '{}' from group '{}': exit code {}, stderr: {}",
            local_user, blocked_group, exit_code, stderr
        )));
    }

    let (exit_code, _, stderr) = run_command(&cmds.usermod, &["-U", "--", &local_user]).await?;

    if exit_code != 0 {
        return Err(Error::Call(format!(
            "usermod -U failed for '{}': exit code {}, stderr: {}",
            local_user, exit_code, stderr
        )));
    }

    tracing::info!("Unblocked user: {}", local_user);

    Ok(mapping)
}

/// Minimum GID this agent treats as a "normal" (non-system) group. Groups
/// with a GID below this are OS/system groups (`wheel`, `sudo`, `docker`, …)
/// created by the distribution, never OpenPortal project groups (which
/// `groupadd` allocates from the normal range), so this agent must never
/// remove them regardless of name. 1000 is the usual `GID_MIN` on Linux.
const MANAGED_GID_MIN: u64 = 1000;

///
/// Return true if the project's Unix group is "protected" — i.e. it must not
/// be removed by this agent because it is a system group or a
/// specially-configured group rather than an OpenPortal-managed project group.
///
/// A group is protected if it: has a GID below `MANAGED_GID_MIN` (a system
/// group); has a GID that cannot be parsed (fail safe); or is the managed
/// group, the blocked group, or one of the configured system groups. A group
/// that does not exist is *not* protected (there is nothing to remove, so
/// removal stays idempotent). This guards against a crafted `ProjectIdentifier`
/// whose derived group name collides with a real system group — see
/// docs/specifications/security-review.md (finding F13).
///
pub async fn is_protected_project(
    project: &ProjectIdentifier,
    expires: &chrono::DateTime<Utc>,
) -> Result<bool, Error> {
    assert_not_expired(expires)?;

    let group_name = identifier_to_projectid(project);
    let cmds = get_commands()?;

    let (exit_code, stdout, _) = run_command(&cmds.getent, &["group", &group_name]).await?;

    if exit_code != 0 {
        // Group does not exist — nothing to protect (removal is a no-op).
        return Ok(false);
    }

    // getent group output: groupname:x:gid:member1,member2,...
    let line = stdout.trim();
    match line
        .split(':')
        .nth(2)
        .and_then(|g| g.trim().parse::<u64>().ok())
    {
        Some(gid) if gid < MANAGED_GID_MIN => return Ok(true),
        Some(_) => {}
        // Could not parse the GID — refuse to remove, to be safe.
        None => return Ok(true),
    }

    // Never remove specially-configured groups, even with a normal GID.
    if group_name == cmds.managed_group
        || group_name == blocked_group_name(cmds)
        || cmds.system_groups.iter().any(|g| g == &group_name)
    {
        return Ok(true);
    }

    Ok(false)
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

    let (exit_code, stdout, _) = run_command(&cmds.getent, &["group", &cmds.managed_group]).await?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_homedir_rejects_dangerous_paths() {
        // Regression test for finding R13. The home directory is a bare
        // `String` on the wire and, on the AddUser path, is supplied by the
        // *peer* - then handed to `useradd -d ... -m`, which creates the
        // directory (and missing parents) as root and chowns it to the new
        // account.
        assert!(check_homedir("/home/bob.proj").is_ok());
        assert!(check_homedir("/data/projects/proj/bob").is_ok());

        for bad in [
            "",
            "   ",
            "home/bob",            // relative
            "../../root",          // relative with traversal
            "/home/../etc/cron.d", // traversal
            "/etc",
            "/etc/cron.d/x",
            "/var/lib/x",
            "/usr/bin",
            "/root",
            "/dev/shm/x",
            "/proc/self",
            "/sys/x",
            "/run/x",
            "/tmp/x",
            "/boot/x",
            "/",
        ] {
            assert!(
                check_homedir(bad).is_err(),
                "{:?} must be rejected as a home directory",
                bad
            );
        }
    }

    #[test]
    fn test_internal_portal_identifiers_are_refused() {
        // The bare-group-name collision finding R13 is really about:
        // `bob.docker.system` resolves to the group `docker`, which is
        // root-equivalent on most hosts. Only configuration may name these.
        for portal in ["openportal", "system", "instance"] {
            let project = ProjectIdentifier::parse(&format!("docker.{}", portal))
                .unwrap_or_else(|e| unreachable!("project: {:?}", e));

            assert!(
                assert_not_internal_portal(&project).is_err(),
                "portal {:?} must be refused when it arrives from a peer",
                portal
            );

            // ...and it is exactly the identifier that maps to a bare group.
            assert_eq!(identifier_to_projectid(&project), "docker");
        }

        // A normal portal is unaffected, and keeps the qualified group name.
        let project = ProjectIdentifier::parse("proj.brics")
            .unwrap_or_else(|e| unreachable!("project: {:?}", e));
        assert!(assert_not_internal_portal(&project).is_ok());
        assert_eq!(identifier_to_projectid(&project), "brics.proj");
    }
}
