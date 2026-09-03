// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::{Context, Result};
use once_cell::sync::{Lazy, OnceCell};
use templemeads::Error;

use crate::nameservice;

use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use nix::unistd::{Gid, Uid};

use tokio::sync::Mutex;

static FS_LOCK: Lazy<Arc<Mutex<()>>> = Lazy::new(|| Arc::new(Mutex::new(())));

///
/// The hidden files an account agent is expected to leave in a brand-new home
/// directory, copied from `/etc/skel` when the account was created.
///
/// This is only used to decide how loudly to report removing one. Any *hidden* file is
/// removable when a recycled directory is waiting to take its place - the list says
/// which ones are unsurprising, so that anything else stands out in the log and can be
/// added here (or investigated) rather than passing silently.
///
const EXPECTED_SKEL_FILES: &[&str] = &[".bash_logout", ".bash_profile", ".bashrc"];

///
/// Optional exec prefix for all filesystem operations. When set, every
/// operation that would normally use a Rust stdlib call is instead performed
/// by running an external command prefixed with these tokens.
///
/// Example: ["docker", "exec", "slurmctld"] causes mkdir, chown, chmod, etc.
/// to be executed inside the named container.
///
/// When not set (None), all operations use native Rust stdlib / nix calls.
///
static EXEC_PREFIX: OnceCell<Option<Vec<String>>> = OnceCell::new();

///
/// Configure the exec prefix for remote filesystem operations.
/// Pass None to use native Rust calls (the default / production behaviour).
/// Pass Some(prefix) to redirect every operation through external commands.
///
/// Must be called once before any filesystem operations are performed.
///
pub fn set_exec_prefix(prefix: Option<Vec<String>>) -> Result<()> {
    EXEC_PREFIX
        .set(prefix)
        .map_err(|_| anyhow::anyhow!("exec-prefix has already been set"))
}

/// Return the exec prefix if one has been configured.
fn get_exec_prefix() -> Option<&'static [String]> {
    EXEC_PREFIX.get().and_then(|p| p.as_deref())
}

///
/// Run an external command built from a pre-tokenised prefix plus
/// additional arguments.  Returns (exit_code, stdout, stderr).
///
async fn run_remote(prefix: &[String], args: &[&str]) -> Result<(i32, String, String), Error> {
    if prefix.is_empty() {
        return Err(Error::State("Empty exec-prefix".to_owned()));
    }

    tracing::debug!("Remote: {} {}", prefix.join(" "), args.join(" "));

    let Some((program, extra_args)) = prefix.split_first() else {
        return Err(Error::State("Empty exec-prefix".to_owned()));
    };

    let mut cmd = tokio::process::Command::new(program);
    for p in extra_args {
        cmd.arg(p);
    }
    for a in args {
        cmd.arg(a);
    }

    let output = cmd
        .output()
        .await
        .map_err(|e| Error::State(format!("Failed to spawn '{}': {}", prefix.join(" "), e)))?;

    let exit_code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    tracing::debug!(
        "Remote exit_code={}, stdout={:?}, stderr={:?}",
        exit_code,
        stdout.trim(),
        stderr.trim()
    );

    Ok((exit_code, stdout, stderr))
}

/// Test whether a path exists on the remote system (`test -e`).
async fn remote_exists(prefix: &[String], path: &Path) -> Result<bool, Error> {
    let path_str = path.to_string_lossy();
    let (exit_code, _, _) = run_remote(prefix, &["test", "-e", &path_str]).await?;
    Ok(exit_code == 0)
}

/// Test whether a path is a symlink on the remote system (`test -L`).
async fn remote_is_symlink(prefix: &[String], path: &Path) -> Result<bool, Error> {
    let path_str = path.to_string_lossy();
    let (exit_code, _, _) = run_remote(prefix, &["test", "-L", &path_str]).await?;
    Ok(exit_code == 0)
}

/// Fully resolve `path` on the remote system, following every symlink.
///
/// `readlink -m` rather than `-f`: `-f` requires all but the final component to exist
/// and exits non-zero otherwise, which would reject a legitimate create whose parent
/// has not been made yet. `-m` resolves symlinks wherever they appear and tolerates
/// missing components, matching what `resolve_deepest_existing` does locally. Both are
/// coreutils, so this is available anywhere the `chown`/`mkdir` this module already
/// runs remotely are.
///
/// Note `readlink -m` *succeeds* for a path that does not exist at all, so it cannot
/// itself tell us whether a configured root is present - hence `must_exist`, which
/// adds an explicit directory test. That is the check that catches an unmounted or
/// mistyped volume root.
async fn remote_canonicalize(
    prefix: &[String],
    path: &Path,
    must_exist: bool,
) -> Result<PathBuf, Error> {
    let path_str = path.to_string_lossy();

    let (exit_code, stdout, stderr) =
        run_remote(prefix, &["readlink", "-m", "--", &path_str]).await?;

    let resolved = stdout.trim();

    if exit_code != 0 || resolved.is_empty() {
        return Err(Error::State(format!(
            "Could not resolve '{}' on the remote system: exit code {}, stderr: {}",
            path_str, exit_code, stderr
        )));
    }

    if must_exist {
        // No `--`: not every `test` accepts it (the one in the reference test
        // container reports "binary operator expected"), and the existing
        // `remote_is_symlink` does not use it either. `run_remote` builds an argv
        // array with no shell, and these paths come from validated configuration, so
        // there is nothing to quote around.
        let (exists, _, _) = run_remote(prefix, &["test", "-d", resolved]).await?;

        if exists != 0 {
            return Err(Error::State(format!(
                "The configured volume root '{}' (which resolves to '{}') is not a \
                 directory on the remote system. The volume must exist and be mounted \
                 before directories can be managed inside it.",
                path_str, resolved
            )));
        }
    }

    Ok(PathBuf::from(resolved))
}

/// Read a symlink target on the remote system (`readlink`).
async fn remote_readlink(prefix: &[String], path: &Path) -> Result<String, Error> {
    let path_str = path.to_string_lossy();
    let (exit_code, stdout, stderr) = run_remote(prefix, &["readlink", &path_str]).await?;
    if exit_code != 0 {
        return Err(Error::State(format!(
            "readlink '{}' failed: {}",
            path_str, stderr
        )));
    }
    Ok(stdout.trim().to_owned())
}

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
/// Resolve `path` as far as it exists, and return the fully-resolved form.
///
/// The leaf usually does *not* exist yet - that is the point of `create_dir` - so
/// canonicalising `path` directly would fail. Instead canonicalise the deepest
/// ancestor that does exist and re-append the components below it. Any symlink in the
/// existing prefix is therefore resolved, which is what the root check below needs.
fn resolve_deepest_existing(path: &Path) -> Result<PathBuf, Error> {
    let mut remaining: Vec<std::ffi::OsString> = Vec::new();
    let mut current = path.to_path_buf();

    loop {
        match current.canonicalize() {
            Ok(canonical) => {
                let mut resolved = canonical;
                for component in remaining.iter().rev() {
                    resolved.push(component);
                }
                return Ok(resolved);
            }
            Err(_) => {
                let Some(name) = current.file_name().map(|n| n.to_owned()) else {
                    // Walked all the way up without finding anything that exists.
                    return Err(Error::State(format!(
                        "Could not resolve any existing ancestor of '{}'",
                        path.to_string_lossy()
                    )));
                };

                remaining.push(name);

                match current.parent() {
                    Some(parent) => current = parent.to_path_buf(),
                    None => {
                        return Err(Error::State(format!(
                            "Could not resolve any existing ancestor of '{}'",
                            path.to_string_lossy()
                        )))
                    }
                }
            }
        }
    }
}

/// Assert that `path` really resolves inside `root`, canonicalising both **at the
/// time of the operation**.
///
/// The deny-list below checks the *unresolved* string, so with `check_exists: false`
/// (which every path that gets written to used to use) a symlink anywhere in the path
/// silently relocated the operation. Since `chown` and `set_permissions` follow
/// symlinks, that was a route to root handing ownership of a directory outside the
/// tree to an unprivileged user.
///
/// Checked live rather than from a set of roots canonicalised at startup, because the
/// volume has to be mounted for the operation to succeed anyway - so there is no
/// automounter to race, and no stale pre-mount resolution to worry about.
///
/// This is deliberately paired with the `AT_SYMLINK_NOFOLLOW` ownership change in
/// `create_dir_native`: this check catches a symlink that is *already* in place
/// (misconfiguration, or a writable volume root), and nofollow catches one planted
/// between this check and the operation. Neither alone is sufficient. See
/// `docs/specifications/security-review-2.md` (finding R33).
async fn assert_within_roots(path: &Path, roots: &[PathBuf]) -> Result<(), Error> {
    let mut last_error = None;
    let mut resolved_path = None;

    for root in roots {
        match assert_within_root(path, root).await {
            Ok(_) => return Ok(()),
            Err(e) => {
                if resolved_path.is_none() {
                    resolved_path = Some(e.to_string());
                }
                last_error = Some(e);
            }
        }
    }

    tracing::error!(
        "Refusing to operate on '{}': it does not resolve inside any configured volume \
         root ({}). A symlink in the path, or a misconfigured root, would put this \
         outside the managed tree.",
        path.to_string_lossy(),
        roots
            .iter()
            .map(|r| r.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(", ")
    );

    Err(last_error.unwrap_or_else(|| {
        Error::State(format!(
            "The path '{}' is not inside any configured volume root",
            path.to_string_lossy()
        ))
    }))
}

async fn assert_within_root(path: &Path, root: &Path) -> Result<PathBuf, Error> {
    // Resolve on whichever system actually owns these paths. With an `exec-prefix`
    // configured, op-filesystem runs somewhere else entirely - the paths exist only on
    // the target - so canonicalising locally would fail on every path, or worse,
    // resolve against an unrelated local filesystem. Every other filesystem operation
    // in this module already routes through `run_remote` for exactly this reason; this
    // check has to as well.
    let (canonical_root, resolved) = match get_exec_prefix() {
        Some(prefix) => (
            remote_canonicalize(prefix, root, true).await?,
            remote_canonicalize(prefix, path, false).await?,
        ),
        None => (
            root.canonicalize().map_err(|e| {
                Error::State(format!(
                    "Could not resolve the configured volume root '{}': {}. The volume \
                     must exist and be mounted before directories can be managed \
                     inside it.",
                    root.to_string_lossy(),
                    e
                ))
            })?,
            resolve_deepest_existing(path)?,
        ),
    };

    if !resolved.starts_with(&canonical_root) {
        // Debug, not error: with several configured roots this is the expected
        // outcome for all but one of them. `assert_within_roots` logs once, at error
        // level, only if *every* root rejects the path.
        tracing::debug!(
            "'{}' resolves to '{}', which is not inside volume root '{}' ('{}')",
            path.to_string_lossy(),
            resolved.to_string_lossy(),
            root.to_string_lossy(),
            canonical_root.to_string_lossy()
        );

        return Err(Error::State(format!(
            "The path '{}' resolves to '{}', outside the volume root '{}'",
            path.to_string_lossy(),
            resolved.to_string_lossy(),
            canonical_root.to_string_lossy()
        )));
    }

    Ok(resolved)
}

pub async fn clean_and_check_path(
    path: &Path,
    roots: &[PathBuf],
    check_exists: bool,
) -> Result<PathBuf, Error> {
    let mut path = path.to_owned();

    // Reject relative paths and any '..' components up front (finding F15), so
    // that neither the sensitive-location check below nor the eventual mkdir can
    // be steered out of the intended tree by traversal - even when
    // `check_exists` is false and no canonicalisation runs. Configured volume
    // paths are always absolute, so this never rejects a legitimate path.
    if !path.is_absolute() {
        return Err(Error::State(format!(
            "The path '{}' is not absolute.",
            path.to_string_lossy()
        )));
    }

    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(Error::State(format!(
            "The path '{}' contains a '..' component.",
            path.to_string_lossy()
        )));
    }

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

    // Verify the path really resolves inside one of the permitted volume roots,
    // resolving symlinks live - see `assert_within_roots`. An empty slice means the
    // caller has no root to check against and only the deny-list below applies.
    //
    // The resolved form is used **only** to decide containment; it is deliberately
    // *not* substituted for `path`. Several callers must act on the path exactly as
    // given: `create_link` and `remove_link` operate on the symlink itself, and
    // `recycle_dir` must move the symlink rather than whatever it points at.
    // Substituting the resolved path made `create_link` try to create the link at its
    // own target - so restoring a recycled project, whose directory still contained
    // the symlink from the first run, tried to link '/public/aiproject' to itself.
    if !roots.is_empty() {
        assert_within_roots(&path, roots).await?;
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

/// Create the directory `path`, owned by `username`:`groupname` with `permissions`.
///
/// Returns `true` if this call brought the directory into existence - either by
/// creating it fresh, or by restoring it from `.recycle` - and `false` if it was
/// already there and was left as it was found. Callers use that to tell a genuine
/// creation from a repeated `add_local_project` / `add_local_user`, which must not
/// re-apply anything that an operator may have changed since - see the default quota
/// handling in `main.rs`.
pub async fn create_dir(
    path: &std::path::Path,
    roots: &[PathBuf],
    username: &str,
    groupname: &str,
    permissions: &str,
) -> Result<bool, Error> {
    let path = clean_and_check_path(path, roots, false).await?;

    // convert the permissions into a u32
    let permissions = clean_and_check_permissions(permissions).await?;

    tracing::info!(
        "Creating directory '{}' for user '{}' and group '{}' with permissions '{}'",
        path.to_string_lossy(),
        username,
        groupname,
        unix_mode::to_string(permissions)
    );

    match get_exec_prefix() {
        Some(prefix) => create_dir_remote(&path, username, groupname, permissions, prefix).await,
        None => create_dir_native(&path, username, groupname, permissions).await,
    }
}

///
/// Open `path` as a directory **without following symlinks**, so ownership and
/// permissions can be set on the file descriptor rather than on a path that could be
/// swapped underneath us between the check and the act.
///
/// `nix::unistd::chown` and `std::fs::set_permissions` both follow, so operating on a
/// path would let anything that replaced `path` have ownership of the symlink's
/// *target* transferred to it. `O_NOFOLLOW` makes the open itself fail if `path` is a
/// symlink, and `O_DIRECTORY` if it is not a directory - stronger than a nofollow path
/// operation, which still resolves the path once more. See
/// `docs/specifications/security-review-2.md` (finding R33).
///
fn open_dir_nofollow(path: &Path) -> Result<std::fs::File, Error> {
    use std::os::unix::fs::OpenOptionsExt;

    Ok(std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_DIRECTORY)
        .open(path)
        .with_context(|| {
            format!(
                "Could not open the directory '{}' to set its ownership - it is not a \
                 directory, or it is a symlink",
                path.to_string_lossy()
            )
        })?)
}

async fn create_dir_native(
    path: &Path,
    username: &str,
    groupname: &str,
    permissions: u32,
) -> Result<bool, Error> {
    // Resolve the names to ids. This goes through `getent` rather than libc - see
    // `crate::nameservice` for why a static musl binary cannot use `getpwnam_r` /
    // `getgrnam_r`, and why a lookup that fails to answer must not be reported as a
    // name that does not exist.
    let uid = Uid::from_raw(nameservice::resolve_uid(username).await?);
    let gid = Gid::from_raw(nameservice::resolve_gid(groupname).await?);

    // check to see if the directory already exists
    if path.exists() {
        // A directory being here does not mean it is the one that should be here. An
        // account agent that creates a home directory when it creates the account
        // (`useradd -m`) leaves an empty one holding nothing but `/etc/skel` copies,
        // and that used to be enough to stop the recycled directory - the one with the
        // user's actual files in it - from ever being restored. So if something is
        // waiting in `.recycle`, and what is here holds nothing real, prefer the
        // recycled one.
        if let Some(recycle_path) = check_recycle_native(path).await? {
            if clear_placeholder_dir_native(path).await? {
                restore_from_recycle_native(&recycle_path, path, uid, gid).await?;
                return Ok(true);
            }
        }

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
        return Ok(false);
    }

    // Check if this directory exists in .recycle - if so, restore it
    if let Some(recycle_path) = check_recycle_native(path).await? {
        restore_from_recycle_native(&recycle_path, path, uid, gid).await?;
        return Ok(true);
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

    // Create the directory.
    //
    // **This must stay `create_dir`, not `create_dir_all`.** Non-recursive creation
    // fails with `EEXIST` if the final component is already a symlink, which is what
    // stops an unprivileged user who can write to this directory's parent from
    // pre-planting a symlink and having the `chown` below hand them ownership of its
    // target. `create_dir_all` traverses symlinked components happily, so making this
    // recursive - an easy-looking fix for "parent doesn't exist" - would reopen a
    // local privilege escalation. Create parents explicitly instead, each one checked.
    // See docs/specifications/security-review-2.md (finding R33).
    std::fs::create_dir(path)
        .with_context(|| format!("Could not create directory '{}'", path.to_string_lossy()))?;

    // Set the ownership and permissions **without following symlinks**.
    //
    // `nix::unistd::chown` and `std::fs::set_permissions` both follow, so if anything
    // replaced `path` between the create above and here, ownership of the symlink's
    // *target* would be transferred. `create_dir` succeeding means `path` was a real
    // directory a moment ago, and the nofollow variants mean that is still the thing
    // being modified - closing the race that the resolve-then-act check in
    // `assert_within_root` can only narrow. See finding R33.
    // Open the directory we just created with `O_NOFOLLOW | O_DIRECTORY` and operate
    // on the **file descriptor**, so the target cannot be swapped underneath us at
    // all - stronger than a nofollow path operation, which still resolves the path
    // once more. `O_NOFOLLOW` makes the open itself fail if `path` is a symlink.
    let dir = open_dir_nofollow(path)?;

    nix::unistd::fchown(&dir, Some(uid), Some(gid)).with_context(|| {
        format!(
            "Could not set ownership on directory '{}'",
            path.to_string_lossy()
        )
    })?;

    // `permissions` is already validated by `clean_and_check_permissions`, so it fits a
    // mode_t; `try_into` rather than a cast so a future widening cannot silently
    // truncate it.
    //
    // `mode_t` is **platform-dependent** - `u16` on Darwin, `u32` on Linux - so this is
    // a real fallible narrowing on macOS and an identity conversion on Linux, where
    // clippy therefore reports it as useless. Allowed rather than removed: dropping
    // `try_into` would fail to compile on Darwin, and replacing it with `as` would
    // reintroduce the silent truncation this is here to prevent.
    #[allow(clippy::useless_conversion)]
    let mode_bits: nix::libc::mode_t = permissions.try_into().with_context(|| {
        format!(
            "Permissions {:o} do not fit a mode_t for directory '{}'",
            permissions,
            path.to_string_lossy()
        )
    })?;

    nix::sys::stat::fchmod(&dir, nix::sys::stat::Mode::from_bits_truncate(mode_bits))
        .with_context(|| {
            format!(
                "Could not set permissions on directory '{}'",
                path.to_string_lossy()
            )
        })?;

    Ok(true)
}

async fn create_dir_remote(
    path: &Path,
    username: &str,
    groupname: &str,
    permissions: u32,
    prefix: &[String],
) -> Result<bool, Error> {
    let path_str = path.to_string_lossy();

    // Check if the directory already exists on the remote.
    let already_exists = remote_exists(prefix, path).await?;

    // Check if this directory exists in .recycle - if so, restore it. A directory that
    // is already here does not stop that: if it holds nothing real it is the empty home
    // an account agent leaves behind, and the recycled copy is the one that matters.
    // See `clear_placeholder_dir_native`.
    if let Some(recycle_path) = check_recycle_remote(path, prefix).await? {
        if !already_exists || clear_placeholder_dir_remote(path, prefix).await? {
            restore_from_recycle_remote(&recycle_path, path, username, groupname, prefix).await?;
            return Ok(true);
        }
    }

    if already_exists {
        tracing::info!("Directory already exists (remote): {}", path_str);
        return Ok(false);
    }

    // Serialise directory creation with the same lock used by the native path.
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

    // mkdir
    let (exit_code, _, stderr) = run_remote(prefix, &["mkdir", &path_str]).await?;
    if exit_code != 0 {
        return Err(Error::State(format!(
            "mkdir '{}' failed: exit code {}, stderr: {}",
            path_str, exit_code, stderr
        )));
    }

    // chown user:group path
    let owner = format!("{}:{}", username, groupname);
    // `-h` so a symlink is never followed - the remote counterpart of the
    // `O_NOFOLLOW` open in `create_dir_native`. See finding R33.
    let (exit_code, _, stderr) = run_remote(prefix, &["chown", "-h", &owner, &path_str]).await?;
    if exit_code != 0 {
        return Err(Error::State(format!(
            "chown '{}' '{}' failed: exit code {}, stderr: {}",
            owner, path_str, exit_code, stderr
        )));
    }

    // chmod mode path  (e.g. "0755")
    let mode_str = format!("{:04o}", permissions);
    let (exit_code, _, stderr) = run_remote(prefix, &["chmod", &mode_str, &path_str]).await?;
    if exit_code != 0 {
        return Err(Error::State(format!(
            "chmod '{}' '{}' failed: exit code {}, stderr: {}",
            mode_str, path_str, exit_code, stderr
        )));
    }

    Ok(true)
}

/// Create a symlink at `link` pointing to `path`.
///
/// `roots` is every configured volume root, not one - the two paths legitimately live
/// under *different* roots. A `links` entry such as
/// `links = ["", "/projects/{PROJECT}/public"]` on a volume rooted at
/// `["/projects", "/public"]` means the directory `/public/aiproject` is linked from
/// `/projects/aiproject/public`, so checking both against a single root wrongly
/// refuses one of them. Both must be inside the managed tree; neither has to be
/// inside the *same* part of it. See finding R33.
pub async fn create_link(path: &Path, link: &Path, roots: &[PathBuf]) -> Result<(), Error> {
    match get_exec_prefix() {
        Some(prefix) => {
            // In remote mode skip the local path-existence check; validate
            // only the security constraints (no /etc, /tmp, …).
            let link = clean_and_check_path(link, roots, false).await?;
            let path = clean_and_check_path(path, roots, false).await?;
            create_link_remote(&path, &link, prefix).await
        }
        None => {
            let link = clean_and_check_path(link, roots, false).await?;
            let path = clean_and_check_path(path, roots, true).await?;
            create_link_native(&path, &link).await
        }
    }
}

///
/// Remove a symlink. Silently does nothing if the path does not exist or is
/// not a symlink. In prefix/remote mode the check and removal run on the
/// remote system via the exec prefix.
///
pub async fn remove_link(link: &Path, roots: &[PathBuf]) -> Result<(), Error> {
    let link = clean_and_check_path(link, roots, false).await?;

    match get_exec_prefix() {
        Some(prefix) => {
            if !remote_is_symlink(prefix, &link).await? {
                return Ok(());
            }
            let link_str = link.to_string_lossy();
            tracing::info!("Removing symlink (remote): '{}'", link_str);
            let (exit_code, _, stderr) = run_remote(prefix, &["rm", "-f", &link_str]).await?;
            if exit_code != 0 {
                tracing::warn!(
                    "Could not remove symlink (remote) '{}': exit code {}, stderr: {}",
                    link_str,
                    exit_code,
                    stderr
                );
            }
        }
        None => {
            if !link.is_symlink() {
                return Ok(());
            }
            tracing::info!("Removing symlink: '{}'", link.to_string_lossy());
            match std::fs::remove_file(&link) {
                Ok(_) => {}
                Err(e) => tracing::warn!(
                    "Could not remove symlink '{}': {}",
                    link.to_string_lossy(),
                    e
                ),
            }
        }
    }

    Ok(())
}

async fn create_link_native(path: &Path, link: &Path) -> Result<(), Error> {
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
    std::os::unix::fs::symlink(path, link).with_context(|| {
        format!(
            "Could not create link '{}' to '{}'",
            link.to_string_lossy(),
            path.to_string_lossy()
        )
    })?;

    Ok(())
}

async fn create_link_remote(path: &Path, link: &Path, prefix: &[String]) -> Result<(), Error> {
    let path_str = path.to_string_lossy();
    let link_str = link.to_string_lossy();

    tracing::info!(
        "Creating link (remote) from '{}' to '{}'",
        path_str,
        link_str
    );

    if remote_exists(prefix, link).await? {
        if remote_is_symlink(prefix, link).await? {
            // Check the target matches.
            let target = remote_readlink(prefix, link).await?;
            if target != path_str.as_ref() {
                tracing::error!(
                    "Link '{}' already exists, but points to '{}' not '{}'",
                    link_str,
                    target,
                    path_str
                );
            }
            // Link exists and is correct (or we've warned above).
            return Ok(());
        } else {
            tracing::error!("Link '{}' already exists but is not a symlink", link_str);
        }
    }

    // ln -s path link
    let (exit_code, _, stderr) = run_remote(prefix, &["ln", "-s", &path_str, &link_str]).await?;
    if exit_code != 0 {
        return Err(Error::State(format!(
            "ln -s '{}' '{}' failed: exit code {}, stderr: {}",
            path_str, link_str, exit_code, stderr
        )));
    }

    Ok(())
}

///
/// Decide whether `path` is a placeholder that a recycled directory should replace, and
/// if it is, remove it so the restore can proceed. Returns whether it did.
///
/// "Placeholder" means it holds nothing a person put there:
///
/// - **Any non-hidden entry** means something real is already here. Nothing is removed
///   and the recycled copy is left where it is - the alternative is deleting a user's
///   files because a stale copy happened to be in `.recycle`.
/// - **A hidden directory or symlink** is refused too. Removing it would mean recursing,
///   and something has put more than skel files here, so this is not the empty home
///   this is meant to recognise.
/// - **Hidden regular files** are the skel copies (`.bashrc` and friends). They are
///   removed one at a time, each one logged, and then the directory itself with
///   `remove_dir` - the non-recursive one, so it *cannot* take a subtree with it even if
///   the checks above are ever wrong.
///
/// If the final `remove_dir` fails after the files are gone, all that has been lost is
/// a few regenerable skel copies, and the next attempt finds an empty directory and
/// gets further. Nothing is destroyed that the account agent will not put back.
///
async fn clear_placeholder_dir_native(path: &Path) -> Result<bool, Error> {
    // `read_dir` yields neither "." nor "..", so there is nothing to filter out here.
    let entries = std::fs::read_dir(path).with_context(|| {
        format!(
            "Could not list the contents of '{}'",
            path.to_string_lossy()
        )
    })?;

    let mut hidden = Vec::new();

    for entry in entries {
        let entry = entry
            .with_context(|| format!("Could not read an entry of '{}'", path.to_string_lossy()))?;

        let name = entry.file_name().to_string_lossy().into_owned();

        if !name.starts_with('.') {
            tracing::info!(
                "'{}' already contains '{}', so it holds real content - keeping it and \
                 leaving the recycled copy alone",
                path.to_string_lossy(),
                name
            );
            return Ok(false);
        }

        // `file_type` on the directory entry, which does not follow symlinks.
        let file_type = entry.file_type().with_context(|| {
            format!(
                "Could not determine the type of '{}'",
                entry.path().to_string_lossy()
            )
        })?;

        if !file_type.is_file() {
            tracing::warn!(
                "'{}' contains the hidden {} '{}' - not something a newly created \
                 account leaves behind, so keeping this directory and leaving the \
                 recycled copy alone rather than recursing into it",
                path.to_string_lossy(),
                if file_type.is_dir() {
                    "directory"
                } else {
                    "symlink"
                },
                name
            );
            return Ok(false);
        }

        hidden.push((name, entry.path()));
    }

    tracing::warn!(
        "'{}' holds only {} hidden file(s) and a recycled copy of it exists, so it is \
         the empty home directory an account agent leaves behind. Removing it and \
         restoring the recycled directory in its place.",
        path.to_string_lossy(),
        hidden.len()
    );

    for (name, file) in &hidden {
        if EXPECTED_SKEL_FILES.contains(&name.as_str()) {
            tracing::info!("Removing the skel file '{}'", file.to_string_lossy());
        } else {
            // Removed all the same - it is hidden, and a recycled directory is waiting -
            // but said loudly, because it is not one of the files this expects.
            tracing::warn!(
                "Removing the unexpected hidden file '{}' - it is not one of {:?}. If \
                 this is a normal part of a new account on this system, add it to \
                 EXPECTED_SKEL_FILES.",
                file.to_string_lossy(),
                EXPECTED_SKEL_FILES
            );
        }

        std::fs::remove_file(file)
            .with_context(|| format!("Could not remove '{}'", file.to_string_lossy()))?;
    }

    // `remove_dir`, never `remove_dir_all`: if anything is left, this fails loudly
    // rather than deleting a subtree.
    std::fs::remove_dir(path).with_context(|| {
        format!(
            "Could not remove the now-empty directory '{}'",
            path.to_string_lossy()
        )
    })?;

    Ok(true)
}

///
/// Check if a directory exists in the .recycle subdirectory of its parent.
/// Returns Some(recycle_path) if found, None otherwise.
///
async fn check_recycle_native(path: &Path) -> Result<Option<PathBuf>, Error> {
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
/// The remote counterpart of `clear_placeholder_dir_native` - see there for the rules
/// and why they are what they are.
///
/// `ls -A -F` does the listing: `-A` leaves out "." and "..", and `-F` marks each name
/// with its type, so a directory or a symlink can be told apart from a regular file
/// without a `stat` per entry.
///
async fn clear_placeholder_dir_remote(path: &Path, prefix: &[String]) -> Result<bool, Error> {
    let path_str = path.to_string_lossy();

    let (exit_code, stdout, stderr) = run_remote(prefix, &["ls", "-A", "-F", &path_str]).await?;

    if exit_code != 0 {
        return Err(Error::State(format!(
            "ls -A -F '{}' failed: exit code {}, stderr: {}",
            path_str, exit_code, stderr
        )));
    }

    let mut hidden = Vec::new();

    for line in stdout.lines() {
        let entry = line.trim_end();

        if entry.is_empty() {
            continue;
        }

        // A trailing '/' marks a directory and '@' a symlink; the other indicators
        // ('*' executable, '=' socket, '|' FIFO, '>' door) all mark things that are
        // still files as far as `rm` is concerned, so strip them and carry on.
        let (name, is_file) = match entry.strip_suffix(['/', '@']) {
            Some(_) => (entry, false),
            None => (entry.trim_end_matches(['*', '=', '|', '>']), true),
        };

        if !name.starts_with('.') {
            tracing::info!(
                "'{}' already contains '{}', so it holds real content - keeping it and \
                 leaving the recycled copy alone",
                path_str,
                name
            );
            return Ok(false);
        }

        if !is_file {
            tracing::warn!(
                "'{}' contains the hidden entry '{}', which is not a regular file - not \
                 something a newly created account leaves behind, so keeping this \
                 directory and leaving the recycled copy alone",
                path_str,
                name
            );
            return Ok(false);
        }

        hidden.push(name.to_string());
    }

    tracing::warn!(
        "'{}' holds only {} hidden file(s) and a recycled copy of it exists, so it is \
         the empty home directory an account agent leaves behind. Removing it and \
         restoring the recycled directory in its place.",
        path_str,
        hidden.len()
    );

    for name in &hidden {
        let file = path.join(name);
        let file_str = file.to_string_lossy();

        if EXPECTED_SKEL_FILES.contains(&name.as_str()) {
            tracing::info!("Removing the skel file '{}'", file_str);
        } else {
            tracing::warn!(
                "Removing the unexpected hidden file '{}' - it is not one of {:?}. If \
                 this is a normal part of a new account on this system, add it to \
                 EXPECTED_SKEL_FILES.",
                file_str,
                EXPECTED_SKEL_FILES
            );
        }

        // `--` so a name can never be read as an option.
        let (exit_code, _, stderr) = run_remote(prefix, &["rm", "-f", "--", &file_str]).await?;

        if exit_code != 0 {
            return Err(Error::State(format!(
                "rm '{}' failed: exit code {}, stderr: {}",
                file_str, exit_code, stderr
            )));
        }
    }

    // `rmdir`, not `rm -r`: if anything is left this fails loudly rather than deleting a
    // subtree.
    let (exit_code, _, stderr) = run_remote(prefix, &["rmdir", "--", &path_str]).await?;

    if exit_code != 0 {
        return Err(Error::State(format!(
            "rmdir '{}' failed: exit code {}, stderr: {}",
            path_str, exit_code, stderr
        )));
    }

    Ok(true)
}

async fn check_recycle_remote(path: &Path, prefix: &[String]) -> Result<Option<PathBuf>, Error> {
    let parent = match path.parent() {
        Some(p) => p,
        None => return Ok(None),
    };

    let dir_name = match path.file_name() {
        Some(n) => n.to_string_lossy(),
        None => return Ok(None),
    };

    let recycle_path = parent.join(".recycle").join(dir_name.as_ref());

    if remote_exists(prefix, &recycle_path).await? {
        Ok(Some(recycle_path))
    } else {
        Ok(None)
    }
}

///
/// Restore a directory from .recycle by moving it back to its original location.
/// This is used when recreating a directory that was previously recycled.
///
async fn restore_from_recycle_native(
    recycle: &Path,
    target: &Path,
    uid: Uid,
    gid: Gid,
) -> Result<(), Error> {
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

    // A recycled directory carries the ownership it had when it was recycled, which is
    // not necessarily the ownership it should have now - see
    // `correct_restored_ownership_native`.
    correct_restored_ownership_native(target, uid, gid).await?;

    tracing::info!("Successfully restored directory from recycle");
    Ok(())
}

///
/// Check that a directory just restored from `.recycle` is owned by who it should be,
/// and correct it if not.
///
/// A recycled directory keeps the uid and gid it had when it was recycled. That is
/// usually still right, but not always: an account agent that *deletes* an account
/// rather than disabling it (`op-localaccount` runs `userdel`, where `op-freeipa`
/// disables) frees its uid, and recreating the account later can allocate a different
/// one. The restored directory then belongs to a uid its owner no longer has - or, if
/// the old uid has since been reused, to somebody else entirely. Restoring used to
/// move the directory back and stop there, so this went unnoticed until a user could
/// not read their own home directory.
///
/// **Only the directory itself is corrected here.** Everything inside it still carries
/// the old ownership, and fixing that means walking a tree of unbounded size - which
/// does not belong inside a job with an answering deadline. The warning below says so
/// explicitly, with the ids needed to put it right, rather than leaving a half-fixed
/// tree looking fully fixed.
///
async fn correct_restored_ownership_native(path: &Path, uid: Uid, gid: Gid) -> Result<(), Error> {
    // `symlink_metadata` so a symlink is described rather than followed.
    let metadata = std::fs::symlink_metadata(path).with_context(|| {
        format!(
            "Could not read the ownership of the restored path '{}'",
            path.to_string_lossy()
        )
    })?;

    if metadata.file_type().is_symlink() {
        // `recycle_dir` moves a symlink as a symlink, so one can legitimately come
        // back. Chowning it would transfer ownership of whatever it points at, which
        // is exactly what finding R33 was about, so leave it alone and say so.
        tracing::warn!(
            "Restored path '{}' is a symlink, not a directory - leaving its ownership \
             untouched",
            path.to_string_lossy()
        );
        return Ok(());
    }

    let current_uid = Uid::from_raw(metadata.uid());
    let current_gid = Gid::from_raw(metadata.gid());

    if current_uid == uid && current_gid == gid {
        tracing::debug!(
            "Restored directory '{}' is already owned by {}:{}",
            path.to_string_lossy(),
            uid,
            gid
        );
        return Ok(());
    }

    tracing::warn!(
        "Restored directory '{}' is owned by {}:{} but should be owned by {}:{}. This \
         happens when an account is deleted and recreated with a different uid or gid \
         rather than being disabled and re-enabled. Correcting the directory itself \
         now - but note that ITS CONTENTS ARE STILL OWNED BY {}:{} and are not changed \
         here, so a recursive chown may be needed before the owner can use them.",
        path.to_string_lossy(),
        current_uid,
        current_gid,
        uid,
        gid,
        current_uid,
        current_gid
    );

    let dir = open_dir_nofollow(path)?;

    nix::unistd::fchown(&dir, Some(uid), Some(gid)).with_context(|| {
        format!(
            "Could not correct the ownership of the restored directory '{}'",
            path.to_string_lossy()
        )
    })?;

    tracing::info!(
        "Corrected the ownership of restored directory '{}' to {}:{}",
        path.to_string_lossy(),
        uid,
        gid
    );

    Ok(())
}

async fn restore_from_recycle_remote(
    recycle: &Path,
    target: &Path,
    username: &str,
    groupname: &str,
    prefix: &[String],
) -> Result<(), Error> {
    let recycle_str = recycle.to_string_lossy();
    let target_str = target.to_string_lossy();

    tracing::info!(
        "Restoring (remote) '{}' from recycle to '{}'",
        recycle_str,
        target_str
    );

    let (exit_code, _, stderr) = run_remote(prefix, &["mv", &recycle_str, &target_str]).await?;
    if exit_code != 0 {
        return Err(Error::State(format!(
            "mv '{}' '{}' failed: exit code {}, stderr: {}",
            recycle_str, target_str, exit_code, stderr
        )));
    }

    correct_restored_ownership_remote(target, username, groupname, prefix).await?;

    tracing::info!("Successfully restored directory from recycle (remote)");
    Ok(())
}

///
/// The remote counterpart of `correct_restored_ownership_native` - see there for why a
/// restored directory can have the wrong owner, and for why only the directory itself
/// is corrected.
///
/// Ownership is compared by **name** rather than by id, since that is what this side
/// has: `stat -c '%U:%G'` prints names, falling back to numbers for an id that no
/// longer resolves - which is itself a mismatch, and so gets corrected.
///
async fn correct_restored_ownership_remote(
    path: &Path,
    username: &str,
    groupname: &str,
    prefix: &[String],
) -> Result<(), Error> {
    let path_str = path.to_string_lossy();
    let owner = format!("{}:{}", username, groupname);

    let (exit_code, stdout, stderr) =
        run_remote(prefix, &["stat", "-c", "%U:%G", &path_str]).await?;

    if exit_code == 0 {
        let current = stdout.trim();

        if current == owner {
            tracing::debug!(
                "Restored directory '{}' is already owned by {}",
                path_str,
                owner
            );
            return Ok(());
        }

        tracing::warn!(
            "Restored directory '{}' is owned by {} but should be owned by {}. This \
             happens when an account is deleted and recreated with a different uid or \
             gid rather than being disabled and re-enabled. Correcting the directory \
             itself now - but note that ITS CONTENTS ARE STILL OWNED BY {} and are not \
             changed here, so a recursive chown may be needed before the owner can use \
             them.",
            path_str,
            current,
            owner,
            current
        );
    } else {
        // Could not find out. Correct it anyway - an unnecessary chown to the ownership
        // it should already have costs nothing, where skipping a necessary one leaves a
        // user locked out of their own directory.
        tracing::warn!(
            "Could not read the ownership of the restored directory '{}' (stat exited \
             {}: {}) - setting it to {} regardless",
            path_str,
            exit_code,
            stderr.trim(),
            owner
        );
    }

    // `-h` so a symlink is never followed - `recycle_dir` moves a symlink as a symlink,
    // so one can legitimately come back here. See finding R33.
    let (exit_code, _, stderr) = run_remote(prefix, &["chown", "-h", &owner, &path_str]).await?;

    if exit_code != 0 {
        return Err(Error::State(format!(
            "chown '{}' '{}' failed while correcting a restored directory: exit code \
             {}, stderr: {}",
            owner, path_str, exit_code, stderr
        )));
    }

    tracing::info!(
        "Corrected the ownership of restored directory '{}' to {}",
        path_str,
        owner
    );

    Ok(())
}

///
/// Move a directory to the .recycle subdirectory of its parent and update its timestamp.
/// This is a non-destructive way to "remove" directories - they can be restored later
/// or permanently deleted by a separate cleanup process.
///
pub async fn recycle_dir(path: &Path, roots: &[PathBuf]) -> Result<(), Error> {
    let path = clean_and_check_path(path, roots, false).await?;

    match get_exec_prefix() {
        Some(prefix) => recycle_dir_remote(&path, prefix).await,
        None => recycle_dir_native(&path).await,
    }
}

async fn recycle_dir_native(path: &Path) -> Result<(), Error> {
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
    std::fs::rename(path, &recycle_path).with_context(|| {
        format!(
            "Could not move '{}' to recycle '{}'",
            path.to_string_lossy(),
            recycle_path.to_string_lossy()
        )
    })?;

    // Update the timestamp to current time using filetime crate
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

async fn recycle_dir_remote(path: &Path, prefix: &[String]) -> Result<(), Error> {
    let path_str = path.to_string_lossy();

    if !remote_exists(prefix, path).await? {
        tracing::warn!(
            "Directory '{}' does not exist (remote), nothing to recycle",
            path_str
        );
        return Ok(());
    }

    let parent = match path.parent() {
        Some(p) => p,
        None => {
            return Err(Error::State(format!(
                "Cannot recycle root directory '{}'",
                path_str
            )))
        }
    };

    let dir_name = match path.file_name() {
        Some(n) => n.to_string_lossy(),
        None => {
            return Err(Error::State(format!(
                "Cannot determine directory name for '{}'",
                path_str
            )))
        }
    };

    // Create .recycle directory if it doesn't exist on the remote.
    let recycle_parent = parent.join(".recycle");
    let recycle_parent_str = recycle_parent.to_string_lossy();

    if !remote_exists(prefix, &recycle_parent).await? {
        tracing::info!(
            "Creating recycle directory (remote): '{}'",
            recycle_parent_str
        );
        let (exit_code, _, stderr) = run_remote(prefix, &["mkdir", &recycle_parent_str]).await?;
        if exit_code != 0 {
            return Err(Error::State(format!(
                "mkdir '{}' failed: exit code {}, stderr: {}",
                recycle_parent_str, exit_code, stderr
            )));
        }
    }

    let recycle_path = recycle_parent.join(dir_name.as_ref());
    let recycle_path_str = recycle_path.to_string_lossy();

    // If something already exists in recycle with this name, remove it.
    if remote_exists(prefix, &recycle_path).await? {
        tracing::warn!(
            "Recycle path '{}' already exists (remote). Removing.",
            recycle_path_str
        );
        let (exit_code, _, stderr) = run_remote(prefix, &["rm", "-rf", &recycle_path_str]).await?;
        if exit_code != 0 {
            return Err(Error::State(format!(
                "rm -rf '{}' failed: exit code {}, stderr: {}",
                recycle_path_str, exit_code, stderr
            )));
        }
    }

    tracing::info!(
        "Moving (remote) '{}' to recycle '{}'",
        path_str,
        recycle_path_str
    );

    // Move the directory to recycle.
    let (exit_code, _, stderr) = run_remote(prefix, &["mv", &path_str, &recycle_path_str]).await?;
    if exit_code != 0 {
        return Err(Error::State(format!(
            "mv '{}' '{}' failed: exit code {}, stderr: {}",
            path_str, recycle_path_str, exit_code, stderr
        )));
    }

    // Update the timestamp (touch).
    match run_remote(prefix, &["touch", &recycle_path_str]).await {
        Ok((0, _, _)) => {
            tracing::info!("Successfully recycled directory (remote) with updated timestamp")
        }
        Ok((code, _, err)) => tracing::warn!(
            "Could not update timestamp on recycled directory (remote): exit code {}, {}",
            code,
            err
        ),
        Err(e) => tracing::warn!(
            "Could not update timestamp on recycled directory (remote): {}",
            e
        ),
    }

    Ok(())
}

///
/// Return whether `path` is currently present on the managed filesystem.
///
/// A symlink counts as present even when it dangles: `create_link` /
/// `remove_link` treat the link itself as the thing that exists, and
/// `recycle_dir` moves a link rather than its target, so a link left behind
/// means the removal has not finished.
///
/// This is only ever a read, but it still goes through `clean_and_check_path`
/// like every write does. The paths come from a `UserMapping`/`ProjectMapping`
/// that arrived over the wire, and without the guard this would be a way to
/// ask the agent whether an arbitrary path outside the configured volume
/// roots exists.
///
pub async fn path_exists(path: &Path, roots: &[PathBuf]) -> Result<bool, Error> {
    let path = clean_and_check_path(path, roots, false).await?;

    match get_exec_prefix() {
        Some(prefix) => {
            if remote_exists(prefix, &path).await? {
                return Ok(true);
            }

            // `test -e` follows symlinks, so a dangling one reads as absent.
            remote_is_symlink(prefix, &path).await
        }
        None => Ok(path.exists() || path.is_symlink()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a directory with the given entries. A name ending in '/' is made a
    /// directory, everything else an empty file.
    fn make_dir(base: &Path, entries: &[&str]) -> PathBuf {
        std::fs::create_dir_all(base).expect("mkdir base");

        for entry in entries {
            match entry.strip_suffix('/') {
                Some(dir) => std::fs::create_dir(base.join(dir)).expect("mkdir entry"),
                None => {
                    std::fs::File::create(base.join(entry)).expect("create entry");
                }
            }
        }

        base.to_path_buf()
    }

    #[tokio::test]
    async fn test_path_exists_reports_directories_files_and_dangling_links() {
        // `path_exists` is what decides the filesystem agent's answer to
        // `is_local_user_added` / `is_local_user_removed`, so each of these
        // cases is a different answer to "did the add or remove finish?".
        // Not `temp_dir()` like the tests around this one: those call the
        // `*_native` helpers directly, whereas `path_exists` goes through
        // `clean_and_check_path`, which refuses '/tmp' as a sensitive location.
        // The build directory is somewhere writable that is not on that list.
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("op-exists-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);

        let root = make_dir(&base.join("root"), &["a_file"]);
        let roots = vec![root.clone()];

        let dir = make_dir(&root.join("a_dir"), &[]);

        assert!(
            path_exists(&dir, &roots).await.expect("dir"),
            "a directory that is there must read as present"
        );
        assert!(
            path_exists(&root.join("a_file"), &roots)
                .await
                .expect("file"),
            "a file that is there must read as present"
        );
        assert!(
            !path_exists(&root.join("nope"), &roots)
                .await
                .expect("missing"),
            "a path that was never created must read as absent"
        );

        // A recycled directory leaves its symlink behind, and that link is
        // exactly the evidence that the removal has not finished - so a
        // dangling link must not read as absent the way `test -e` would have it.
        let link = root.join("a_link");
        std::os::unix::fs::symlink(root.join("nope"), &link).expect("symlink");

        assert!(
            path_exists(&link, &roots).await.expect("dangling link"),
            "a dangling symlink must still read as present"
        );

        // The guard is the same one every write goes through: a mapping that
        // arrived over the wire must not be usable to probe outside the roots.
        assert!(
            path_exists(Path::new("/etc/shadow"), &roots).await.is_err(),
            "a path outside the configured roots must be refused, not answered"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn test_placeholder_with_only_skel_files_is_cleared() {
        let base = std::env::temp_dir().join(format!("op-ph-skel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);

        let home = make_dir(
            &base.join("fred"),
            &[".bashrc", ".bash_logout", ".bash_profile"],
        );

        assert!(
            clear_placeholder_dir_native(&home).await.expect("clear"),
            "a home holding only skel files must be cleared"
        );
        assert!(!home.exists(), "the placeholder directory must be gone");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn test_an_empty_placeholder_is_cleared() {
        let base = std::env::temp_dir().join(format!("op-ph-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);

        let home = make_dir(&base.join("fred"), &[]);

        assert!(clear_placeholder_dir_native(&home).await.expect("clear"));
        assert!(!home.exists());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn test_a_directory_with_real_files_is_never_cleared() {
        // The case that must never go wrong: something non-hidden is a person's work,
        // and a stale copy in `.recycle` must not be an excuse to delete it.
        let base = std::env::temp_dir().join(format!("op-ph-real-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);

        let home = make_dir(&base.join("fred"), &[".bashrc", "thesis.tex"]);

        assert!(
            !clear_placeholder_dir_native(&home).await.expect("clear"),
            "a home holding real files must be kept"
        );
        assert!(home.join("thesis.tex").exists(), "nothing may be removed");
        assert!(home.join(".bashrc").exists(), "not even the skel files");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn test_a_hidden_directory_is_never_cleared() {
        // `.ssh` is the one that matters - clearing it would need recursion, and it
        // holds credentials rather than skel defaults.
        let base = std::env::temp_dir().join(format!("op-ph-hidden-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);

        let home = make_dir(&base.join("fred"), &[".bashrc", ".ssh/"]);
        std::fs::File::create(home.join(".ssh").join("authorized_keys")).expect("create key");

        assert!(
            !clear_placeholder_dir_native(&home).await.expect("clear"),
            "a home holding a hidden directory must be kept"
        );
        assert!(home.join(".ssh").join("authorized_keys").exists());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn test_a_hidden_symlink_is_never_cleared() {
        let base = std::env::temp_dir().join(format!("op-ph-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);

        let home = make_dir(&base.join("fred"), &[".bashrc"]);
        let elsewhere = make_dir(&base.join("elsewhere"), &["secret"]);
        std::os::unix::fs::symlink(&elsewhere, home.join(".hidden-link")).expect("symlink");

        assert!(
            !clear_placeholder_dir_native(&home).await.expect("clear"),
            "a home holding a hidden symlink must be kept"
        );
        assert!(
            elsewhere.join("secret").exists(),
            "the symlink's target must be untouched"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn test_an_unexpected_hidden_file_is_still_cleared() {
        // Not on the expected list, so it is logged loudly - but a hidden file with a
        // recycled directory waiting is still a placeholder, or the fix would stop
        // working on any host whose /etc/skel differs.
        let base = std::env::temp_dir().join(format!("op-ph-unexp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);

        let home = make_dir(&base.join("fred"), &[".bashrc", ".zshrc"]);

        assert!(clear_placeholder_dir_native(&home).await.expect("clear"));
        assert!(!home.exists());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn test_restore_from_recycle_keeps_ownership_that_is_already_right() {
        let base = std::env::temp_dir().join(format!("op-restore-ok-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);

        let recycle = base.join(".recycle").join("fred");
        let target = base.join("fred");
        std::fs::create_dir_all(&recycle).expect("mkdir recycle/fred");
        std::fs::create_dir(recycle.join("keep-me")).expect("mkdir keep-me");

        let uid = nix::unistd::getuid();
        let gid = nix::unistd::getgid();

        restore_from_recycle_native(&recycle, &target, uid, gid)
            .await
            .expect("restore");

        assert!(target.join("keep-me").exists(), "contents must come back");
        assert!(!recycle.exists(), "the recycled copy must be gone");

        let metadata = std::fs::symlink_metadata(&target).expect("stat");
        assert_eq!(Uid::from_raw(metadata.uid()), uid);
        assert_eq!(Gid::from_raw(metadata.gid()), gid);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn test_correct_restored_ownership_leaves_a_symlink_alone() {
        // `recycle_dir` moves a symlink as a symlink, so one can come back here.
        // Chowning it would transfer ownership of its *target*, which is finding R33 -
        // so it must be left alone even when the ownership does not match, and the
        // mismatch reported rather than acted on.
        let base = std::env::temp_dir().join(format!("op-restore-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("mkdir base");

        let elsewhere = base.join("elsewhere");
        std::fs::create_dir(&elsewhere).expect("mkdir elsewhere");

        let link = base.join("fred");
        std::os::unix::fs::symlink(&elsewhere, &link).expect("symlink");

        // Deliberately mismatched ids - root, which this test is not.
        correct_restored_ownership_native(&link, Uid::from_raw(0), Gid::from_raw(0))
            .await
            .expect("a symlink must be reported, not treated as an error");

        assert!(
            std::fs::symlink_metadata(&link)
                .expect("stat")
                .file_type()
                .is_symlink(),
            "the symlink must still be a symlink"
        );
        assert_eq!(
            std::fs::read_link(&link).expect("read_link"),
            elsewhere,
            "the symlink must still point where it did"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn test_resolve_deepest_existing_resolves_symlinks_in_the_prefix() {
        // The path being created does not exist yet, so it cannot be canonicalised
        // directly - resolve the deepest existing ancestor and re-append the rest. Any
        // symlink in that existing prefix is therefore resolved, which is what lets
        // `assert_within_root` see where the operation would really land. See
        // docs/specifications/security-review-2.md (finding R33).
        let base = std::env::temp_dir().join(format!("op-r33-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);

        let root = base.join("root");
        let outside = base.join("outside");
        std::fs::create_dir_all(root.join("proj")).expect("mkdir root/proj");
        std::fs::create_dir_all(&outside).expect("mkdir outside");

        // A leaf that does not exist yet still resolves.
        let target = root.join("proj").join("newuser");
        let resolved = resolve_deepest_existing(&target).expect("resolve");
        assert!(resolved.ends_with("proj/newuser"));
        assert!(resolved.starts_with(root.canonicalize().expect("canonicalize root")));

        // Now make the *parent* a symlink pointing outside the root - the shape of the
        // escalation. The resolved path must reveal that.
        std::fs::remove_dir(root.join("proj")).expect("rmdir");
        std::os::unix::fs::symlink(&outside, root.join("proj")).expect("symlink");

        let resolved = resolve_deepest_existing(&target).expect("resolve via symlink");
        assert!(
            resolved.starts_with(outside.canonicalize().expect("canonicalize outside")),
            "a symlinked parent must resolve to its target, got {:?}",
            resolved
        );
        assert!(
            !resolved.starts_with(root.canonicalize().expect("canonicalize root")),
            "the resolved path must no longer look like it is inside the root"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn test_a_symlink_pointing_into_another_configured_root_is_allowed() {
        // The shape after restoring a recycled project: the restored directory still
        // contains the symlink from the first run, so the link path *is* a symlink
        // pointing into a different root of the same volume. That must be allowed.
        //
        // Note `assert_within_roots` returns `()`. That is deliberate and is itself the
        // fix for a bug this caught: resolution decides containment only, and must
        // never be substituted for the path the operation acts on. `create_link` and
        // `remove_link` act on the symlink itself, and `recycle_dir` must move the
        // symlink rather than its target - substituting the resolved form made
        // `create_link` try to link '/public/aiproject' to itself. Returning nothing
        // makes that mistake unrepresentable. See
        // docs/specifications/security-review-2.md (finding R33).
        let base = std::env::temp_dir().join(format!("op-r33-sub-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);

        let projects = base.join("projects");
        let public = base.join("public");
        let outside = base.join("outside");
        std::fs::create_dir_all(projects.join("aiproject")).expect("mkdir projects");
        std::fs::create_dir_all(public.join("aiproject")).expect("mkdir public");
        std::fs::create_dir_all(&outside).expect("mkdir outside");

        let link = projects.join("aiproject").join("public");
        std::os::unix::fs::symlink(public.join("aiproject"), &link).expect("symlink");

        let roots = vec![projects.clone(), public.clone()];

        assert!(
            assert_within_roots(&link, &roots).await.is_ok(),
            "a symlink pointing into another configured root must be allowed"
        );

        // ...whereas one pointing out of the managed tree is still refused.
        let escape = projects.join("aiproject").join("escape");
        std::os::unix::fs::symlink(&outside, &escape).expect("symlink");
        assert!(
            assert_within_roots(&escape, &roots).await.is_err(),
            "a symlink pointing outside every configured root must be refused"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn test_assert_within_root_refuses_a_path_that_escapes_via_a_symlink() {
        let base = std::env::temp_dir().join(format!("op-r33-esc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);

        let root = base.join("root");
        let outside = base.join("outside");
        std::fs::create_dir_all(&root).expect("mkdir root");
        std::fs::create_dir_all(&outside).expect("mkdir outside");

        // An ordinary path inside the root is fine.
        assert!(
            assert_within_roots(&root.join("proj"), std::slice::from_ref(&root))
                .await
                .is_ok()
        );

        // A symlinked component pointing out of the tree is refused.
        std::os::unix::fs::symlink(&outside, root.join("escape")).expect("symlink");
        assert!(assert_within_root(&root.join("escape").join("dir"), &root)
            .await
            .is_err());

        // A missing root is reported rather than silently passing.
        assert!(
            assert_within_root(&root.join("proj"), &base.join("nosuchroot"))
                .await
                .is_err()
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// The name of the user and group running the tests, as `getent` knows them.
    /// `None` if they cannot be looked up, in which case the caller skips - a build
    /// environment where `id` is unavailable is not a failure of the code under test.
    fn current_user_and_group() -> Option<(String, String)> {
        let run = |args: &[&str]| {
            let output = std::process::Command::new("id").args(args).output().ok()?;
            match output.status.success() {
                true => Some(String::from_utf8_lossy(&output.stdout).trim().to_string()),
                false => None,
            }
        };

        Some((run(&["-un"])?, run(&["-gn"])?))
    }

    #[tokio::test]
    async fn test_create_dir_reports_whether_it_created_the_directory() {
        // The default quota is applied only to a directory this agent has just brought
        // into existence, so `create_dir` must tell a real creation from a repeated
        // `add_local_project` / `add_local_user` finding the directory already there.
        let Some((user, group)) = current_user_and_group() else {
            return;
        };

        let base = std::env::temp_dir().join(format!("op-create-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("mkdir base");

        let path = base.join("fred");

        assert!(
            create_dir_native(&path, &user, &group, 0o755)
                .await
                .expect("create"),
            "creating the directory must be reported as a creation"
        );

        assert!(
            !create_dir_native(&path, &user, &group, 0o755)
                .await
                .expect("create again"),
            "finding the directory already there must not be reported as a creation"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn test_create_dir_counts_a_restore_from_recycle_as_a_creation() {
        // A restored directory is one that was not here a moment ago, so it counts as
        // brought into existence. Its quota, which removal leaves in place, is what
        // then stops the default from being re-applied over it.
        let Some((user, group)) = current_user_and_group() else {
            return;
        };

        let base = std::env::temp_dir().join(format!("op-create-restore-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);

        let recycle = base.join(".recycle").join("fred");
        std::fs::create_dir_all(&recycle).expect("mkdir recycle/fred");
        std::fs::create_dir(recycle.join("keep-me")).expect("mkdir keep-me");

        let path = base.join("fred");

        assert!(
            create_dir_native(&path, &user, &group, 0o755)
                .await
                .expect("restore"),
            "restoring the directory must be reported as a creation"
        );
        assert!(path.join("keep-me").exists(), "contents must come back");

        let _ = std::fs::remove_dir_all(&base);
    }
}
