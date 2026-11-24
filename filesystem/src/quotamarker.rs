// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! Quota markers: a record, kept under each volume root, of the recursive
//! walk that gave a user or project directory tree its Lustre quota ID.
//!
//! `lfs project -srp <id> <dir>` tags the top directory first and then walks
//! the tree, which can take hours. Reading the top directory's ID therefore
//! cannot tell a finished walk from one that was interrupted - by a restart of
//! the agent, a timeout or a failure - part way through. The marker can:
//!
//! - the walk writes `state = "started"` before it begins, and
//!   `state = "complete"` once `lfs` has finished successfully;
//! - a directory carrying its quota ID with a `complete` marker for that ID is
//!   done;
//! - one with a `started` marker, a marker for another ID, or a marker that
//!   cannot be trusted was interrupted, and is walked again;
//! - one with no marker at all was tagged before markers existed (or by hand),
//!   and is taken to be done - a `complete` marker is then written to record
//!   that.
//!
//! ## Where
//!
//! The marker for `<root>/<relative>` is `<root>/.openportal_quota/<relative>.toml`,
//! e.g. `/projects/.openportal_quota/proj.toml` for `/projects/proj`, and
//! `/home/.openportal_quota/proj/alice.toml` for `/home/proj/alice`. A volume
//! root is the operator's own directory, and `.openportal_quota` and everything
//! in it is created by the agent's user, mode 0700, so the users whose
//! directories are walked cannot reach their markers - not even to delete one.
//! (A marker inside the walked directory could be deleted by its owner, and a
//! directory deleted mid-walk followed by a restart would then look finished.)
//!
//! ## Trust
//!
//! Nothing is trusted unless every directory from `.openportal_quota` down is a
//! real directory (never a symlink), owned by the agent's effective user and not
//! writable by group or others, and the marker is a regular file with one link,
//! owned by the agent's user. Anything else is ignored, and the worst that can
//! then happen is that a walk is repeated. Natively this is enforced one path
//! component at a time with `openat(O_NOFOLLOW)` and `fstat` on what was opened,
//! so nothing can be swapped between checking and using it. The marker is
//! written mode 0400 to a uniquely named temporary file (`O_EXCL`) that is then
//! renamed over it.
//!
//! With `exec-prefix` set (see `crate::filesystem`), the same checks and writes
//! are made by shell commands run through the prefix.

use bradshaw::{DateTime, Utc};
use nix::errno::Errno;
use nix::fcntl::{openat, renameat, OFlag};
use nix::sys::stat::{fstat, mkdirat, Mode, SFlag};
use nix::unistd::{unlinkat, UnlinkatFlags};
use serde::{Deserialize, Serialize};
use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};
use templemeads::Error;

/// The directory, in each volume root, that holds the markers.
pub const MARKER_DIR: &str = ".openportal_quota";

const MARKER_SUFFIX: &str = ".toml";

/// A marker is a few lines of TOML; anything much larger was not written by us.
const MAX_MARKER_BYTES: u64 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WalkState {
    /// The walk was started and has not (yet) been seen to finish.
    Started,

    /// `lfs` finished the walk successfully.
    Complete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuotaMarker {
    /// The quota (Lustre project) ID the walk assigned.
    pub quota_id: u64,

    pub state: WalkState,

    /// When the walk started. Absent for a tree tagged before markers existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started: Option<DateTime<Utc>>,

    /// When the walk completed, or (with no `started`) when an earlier
    /// assignment was first recorded as complete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed: Option<DateTime<Utc>>,
}

impl QuotaMarker {
    pub fn started(quota_id: u64, started: DateTime<Utc>) -> Self {
        Self {
            quota_id,
            state: WalkState::Started,
            started: Some(started),
            completed: None,
        }
    }

    pub fn complete(
        quota_id: u64,
        started: Option<DateTime<Utc>>,
        completed: DateTime<Utc>,
    ) -> Self {
        Self {
            quota_id,
            state: WalkState::Complete,
            started,
            completed: Some(completed),
        }
    }

    /// Whether this marker says the walk for `quota_id` finished.
    pub fn is_complete_for(&self, quota_id: u64) -> bool {
        self.quota_id == quota_id && self.state == WalkState::Complete
    }

    fn to_text(&self) -> Result<String, Error> {
        Ok(format!(
            "# Written by the OpenPortal filesystem agent to record the walk that gave\n\
             # a directory tree its Lustre quota ID. Do not edit: a marker this agent\n\
             # did not write is ignored, and the tree is walked again.\n{}",
            toml::to_string(self).map_err(|e| Error::Failed(format!(
                "Could not serialise the quota marker: {}",
                e
            )))?
        ))
    }
}

impl std::fmt::Display for QuotaMarker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let when = |t: &Option<DateTime<Utc>>| {
            t.map(|t| t.to_rfc3339())
                .unwrap_or_else(|| "unknown".to_string())
        };

        match self.state {
            WalkState::Started => write!(
                f,
                "walk for quota ID {} started at {}, not completed",
                self.quota_id,
                when(&self.started)
            ),
            WalkState::Complete => write!(
                f,
                "walk for quota ID {} started at {}, completed at {}",
                self.quota_id,
                when(&self.started),
                when(&self.completed)
            ),
        }
    }
}

/// What reading a directory's marker found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkerRead {
    /// There is no marker.
    Absent,

    /// A marker this agent wrote.
    Ours(QuotaMarker),

    /// Something is there, but it cannot be trusted or read - the reason says
    /// why. Treat it as an interrupted walk.
    Untrusted(String),
}

/// Where the marker for a directory under a volume root lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkerLocation {
    root: PathBuf,

    /// The directories under the root that hold the marker: `MARKER_DIR`, then
    /// the walked directory's parents relative to the root.
    dirs: Vec<OsString>,

    /// The marker's file name: the walked directory's name plus `.toml`.
    file: OsString,
}

impl MarkerLocation {
    /// The marker location for `directory`, which must be strictly under `root`.
    pub fn new(root: &Path, directory: &Path) -> Result<Self, Error> {
        let relative = directory.strip_prefix(root).map_err(|_| {
            Error::Misconfigured(format!(
                "Cannot keep a quota marker for {}: it is not under the volume root {}",
                directory.display(),
                root.display()
            ))
        })?;

        let mut names = Vec::new();

        for component in relative.components() {
            match component {
                Component::Normal(name) => names.push(name.to_os_string()),
                _ => {
                    return Err(Error::Misconfigured(format!(
                        "Cannot keep a quota marker for {}: its path under {} is not plain",
                        directory.display(),
                        root.display()
                    )))
                }
            }
        }

        if names.first().map(OsString::as_os_str) == Some(OsStr::new(MARKER_DIR)) {
            return Err(Error::Misconfigured(format!(
                "Cannot keep a quota marker for {}: it is inside the marker directory",
                directory.display()
            )));
        }

        let Some((last, parents)) = names.split_last() else {
            return Err(Error::Misconfigured(format!(
                "Cannot keep a quota marker for the volume root {} itself",
                root.display()
            )));
        };

        let mut dirs = vec![OsString::from(MARKER_DIR)];
        dirs.extend(parents.iter().cloned());

        let mut file = last.clone();
        file.push(MARKER_SUFFIX);

        Ok(Self {
            root: root.to_path_buf(),
            dirs,
            file,
        })
    }

    /// The marker's full path, for messages and for the remote commands.
    pub fn path(&self) -> PathBuf {
        let mut path = self.root.clone();
        path.extend(&self.dirs);
        path.push(&self.file);
        path
    }

    /// Every directory from `MARKER_DIR` down to the one holding the marker.
    fn dir_paths(&self) -> Vec<PathBuf> {
        let mut path = self.root.clone();
        self.dirs
            .iter()
            .map(|dir| {
                path.push(dir);
                path.clone()
            })
            .collect()
    }
}

impl std::fmt::Display for MarkerLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.path().display())
    }
}

/// Read the marker at `location`.
pub async fn read_marker(location: &MarkerLocation) -> MarkerRead {
    match crate::filesystem::exec_prefix() {
        Some(prefix) => remote::read(prefix, location).await,
        None => native::read(location),
    }
}

/// Write `marker` at `location`, replacing any marker already there.
pub async fn write_marker(location: &MarkerLocation, marker: &QuotaMarker) -> Result<(), Error> {
    match crate::filesystem::exec_prefix() {
        Some(prefix) => remote::write(prefix, location, marker).await,
        None => native::write(location, marker),
    }
}

/// Remove the marker at `location`, if it is one this agent wrote. Anything
/// else is left alone.
pub async fn remove_marker(location: &MarkerLocation) -> Result<(), Error> {
    match crate::filesystem::exec_prefix() {
        Some(prefix) => remote::remove(prefix, location).await,
        None => native::remove(location),
    }
}

fn effective_uid() -> u32 {
    nix::unistd::geteuid().as_raw()
}

mod native {
    use super::*;

    const DIR_FLAGS: OFlag = OFlag::O_RDONLY
        .union(OFlag::O_DIRECTORY)
        .union(OFlag::O_NOFOLLOW)
        .union(OFlag::O_CLOEXEC);

    /// Open one directory of the marker tree under `parent`, creating it
    /// (mode 0700) if asked, and check it can be trusted. `Ok(None)` if it
    /// does not exist.
    fn open_dir(
        parent: impl AsFd,
        name: &OsStr,
        path: &Path,
        create: bool,
    ) -> Result<Option<OwnedFd>, String> {
        if create {
            match mkdirat(&parent, name, Mode::S_IRWXU) {
                Ok(()) | Err(Errno::EEXIST) => {}
                Err(e) => return Err(format!("could not create {}: {}", path.display(), e)),
            }
        }

        let fd = match openat(&parent, name, DIR_FLAGS, Mode::empty()) {
            Ok(fd) => fd,
            Err(Errno::ENOENT) => return Ok(None),
            Err(Errno::ELOOP) => return Err(format!("{} is a symlink", path.display())),
            Err(Errno::ENOTDIR) => return Err(format!("{} is not a directory", path.display())),
            Err(e) => return Err(format!("could not open {}: {}", path.display(), e)),
        };

        let stat = fstat(&fd).map_err(|e| format!("could not stat {}: {}", path.display(), e))?;

        if stat.st_uid != effective_uid() {
            return Err(format!(
                "{} is owned by uid {}, not by this agent (uid {})",
                path.display(),
                stat.st_uid,
                effective_uid()
            ));
        }

        if stat.st_mode & 0o022 != 0 {
            return Err(format!(
                "{} is writable by group or others (mode {:o})",
                path.display(),
                stat.st_mode & 0o7777
            ));
        }

        Ok(Some(fd))
    }

    /// Open the directory that holds the marker, checking every directory on
    /// the way from `MARKER_DIR`. The root itself is the operator's.
    fn open_marker_dir(location: &MarkerLocation, create: bool) -> Result<Option<OwnedFd>, String> {
        let root = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(nix::libc::O_DIRECTORY | nix::libc::O_CLOEXEC)
            .open(&location.root)
            .map_err(|e| format!("could not open {}: {}", location.root.display(), e))?;

        let mut current: OwnedFd = root.into();

        for (name, path) in location.dirs.iter().zip(location.dir_paths()) {
            match open_dir(&current, name, &path, create)? {
                Some(fd) => current = fd,
                None => return Ok(None),
            }
        }

        Ok(Some(current))
    }

    pub(super) fn read(location: &MarkerLocation) -> MarkerRead {
        let path = location.path();

        let dir = match open_marker_dir(location, false) {
            Ok(Some(dir)) => dir,
            Ok(None) => return MarkerRead::Absent,
            Err(reason) => return MarkerRead::Untrusted(reason),
        };

        let fd = match openat(
            &dir,
            location.file.as_os_str(),
            OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(Errno::ENOENT) => return MarkerRead::Absent,
            Err(Errno::ELOOP) => {
                return MarkerRead::Untrusted(format!("{} is a symlink", path.display()))
            }
            Err(e) => {
                return MarkerRead::Untrusted(format!("could not open {}: {}", path.display(), e))
            }
        };

        let stat = match fstat(&fd) {
            Ok(stat) => stat,
            Err(e) => {
                return MarkerRead::Untrusted(format!("could not stat {}: {}", path.display(), e))
            }
        };

        if SFlag::from_bits_truncate(stat.st_mode & SFlag::S_IFMT.bits()) != SFlag::S_IFREG {
            return MarkerRead::Untrusted(format!("{} is not a regular file", path.display()));
        }

        if stat.st_uid != effective_uid() {
            return MarkerRead::Untrusted(format!(
                "{} is owned by uid {}, not by this agent (uid {})",
                path.display(),
                stat.st_uid,
                effective_uid()
            ));
        }

        if stat.st_nlink != 1 {
            return MarkerRead::Untrusted(format!(
                "{} has {} links, not 1",
                path.display(),
                stat.st_nlink
            ));
        }

        let mut text = String::new();

        if let Err(e) = std::fs::File::from(fd)
            .take(MAX_MARKER_BYTES)
            .read_to_string(&mut text)
        {
            return MarkerRead::Untrusted(format!("could not read {}: {}", path.display(), e));
        }

        parse(&path, &text)
    }

    pub(super) fn write(location: &MarkerLocation, marker: &QuotaMarker) -> Result<(), Error> {
        let path = location.path();
        let failed = |reason: String| {
            Error::Failed(format!(
                "Could not write the quota marker {}: {}",
                path.display(),
                reason
            ))
        };

        let text = marker.to_text()?;

        let dir = open_marker_dir(location, true)
            .map_err(&failed)?
            .ok_or_else(|| failed("its directory could not be created".to_string()))?;

        let mut temp = location.file.clone();
        temp.push(format!(".{:016x}.tmp", rand::random::<u64>()));

        let fd = openat(
            &dir,
            temp.as_os_str(),
            OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            Mode::S_IRUSR,
        )
        .map_err(|e| failed(e.to_string()))?;

        let mut file = std::fs::File::from(fd);

        let written = file
            .write_all(text.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|e| e.to_string())
            .and_then(|()| {
                renameat(&dir, temp.as_os_str(), &dir, location.file.as_os_str())
                    .map_err(|e| e.to_string())
            });

        if let Err(reason) = written {
            let _ = unlinkat(&dir, temp.as_os_str(), UnlinkatFlags::NoRemoveDir);
            return Err(failed(reason));
        }

        Ok(())
    }

    pub(super) fn remove(location: &MarkerLocation) -> Result<(), Error> {
        let MarkerRead::Ours(_) = read(location) else {
            return Ok(());
        };

        let path = location.path();
        let failed = |reason: String| {
            Error::Failed(format!(
                "Could not remove the quota marker {}: {}",
                path.display(),
                reason
            ))
        };

        let Some(dir) = open_marker_dir(location, false).map_err(&failed)? else {
            return Ok(());
        };

        match unlinkat(&dir, location.file.as_os_str(), UnlinkatFlags::NoRemoveDir) {
            Ok(()) | Err(Errno::ENOENT) => Ok(()),
            Err(e) => Err(failed(e.to_string())),
        }
    }
}

/// The same operations, as shell commands run through the `exec-prefix`.
/// Paths and contents are passed as arguments, never put into the script.
mod remote {
    use super::*;

    /// Prints `ABSENT`, `UNTRUSTED <reason>`, or `OK` followed by the marker.
    const READ: &str = r#"
# GNU stat, or BSD stat (macOS) where there is no -c: "<GNU format>" "<BSD format>" path
st() { stat -c "$1" "$3" 2>/dev/null || stat -f "$2" "$3"; }
uid=$(id -u) || exit 1
f=$1
shift
for d in "$@"; do
    if [ -L "$d" ]; then echo "UNTRUSTED $d is a symlink"; exit 0; fi
    if [ ! -e "$d" ]; then echo ABSENT; exit 0; fi
    if [ ! -d "$d" ]; then echo "UNTRUSTED $d is not a directory"; exit 0; fi
    s=$(st '%u %a' '%u %Mp%Lp' "$d") || exit 1
    set -- $s
    if [ "$1" != "$uid" ]; then echo "UNTRUSTED $d is owned by uid $1"; exit 0; fi
    case "$2" in *[2367]?|*[2367]) echo "UNTRUSTED $d is writable by group or others"; exit 0;; esac
done
if [ -L "$f" ]; then echo "UNTRUSTED $f is a symlink"; exit 0; fi
if [ ! -e "$f" ]; then echo ABSENT; exit 0; fi
if [ ! -f "$f" ]; then echo "UNTRUSTED $f is not a regular file"; exit 0; fi
s=$(st '%u %h' '%u %l' "$f") || exit 1
set -- $s
if [ "$1" != "$uid" ]; then echo "UNTRUSTED $f is owned by uid $1"; exit 0; fi
if [ "$2" != 1 ]; then echo "UNTRUSTED $f has $2 links, not 1"; exit 0; fi
echo OK
head -c 4096 "$f"
"#;

    /// Creates the marker directories (0700) as needed, checks them, and
    /// writes the marker (0400) through a temporary file and a rename.
    const WRITE: &str = r#"
# GNU stat, or BSD stat (macOS) where there is no -c: "<GNU format>" "<BSD format>" path
st() { stat -c "$1" "$3" 2>/dev/null || stat -f "$2" "$3"; }
uid=$(id -u) || exit 1
content=$1
f=$2
shift 2
for d in "$@"; do
    if [ -L "$d" ]; then echo "$d is a symlink" >&2; exit 1; fi
    if [ ! -e "$d" ]; then mkdir -m 700 "$d" || exit 1; fi
    if [ ! -d "$d" ]; then echo "$d is not a directory" >&2; exit 1; fi
    s=$(st '%u %a' '%u %Mp%Lp' "$d") || exit 1
    set -- $s
    if [ "$1" != "$uid" ]; then echo "$d is owned by uid $1" >&2; exit 1; fi
    case "$2" in *[2367]?|*[2367]) echo "$d is writable by group or others" >&2; exit 1;; esac
done
t="$f.$$.tmp"
(set -C; umask 377; printf '%s' "$content" > "$t") || exit 1
mv -f "$t" "$f" || { rm -f "$t"; exit 1; }
"#;

    async fn run(prefix: &[String], script: &str, args: &[String]) -> Result<String, String> {
        let mut all: Vec<&str> = vec!["sh", "-c", script, "sh"];
        all.extend(args.iter().map(String::as_str));

        let (code, stdout, stderr) = crate::filesystem::run_with_prefix(prefix, &all)
            .await
            .map_err(|e| e.to_string())?;

        if code != 0 {
            return Err(stderr.trim().to_string());
        }

        Ok(stdout)
    }

    fn paths(location: &MarkerLocation) -> Vec<String> {
        location
            .dir_paths()
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    }

    pub(super) async fn read(prefix: &[String], location: &MarkerLocation) -> MarkerRead {
        let path = location.path();

        let mut args = vec![path.to_string_lossy().into_owned()];
        args.extend(paths(location));

        let output = match run(prefix, READ, &args).await {
            Ok(output) => output,
            Err(e) => {
                return MarkerRead::Untrusted(format!("could not read {}: {}", path.display(), e))
            }
        };

        let (status, text) = output.split_once('\n').unwrap_or((output.as_str(), ""));

        match status {
            "OK" => parse(&path, text),
            "ABSENT" => MarkerRead::Absent,
            other => MarkerRead::Untrusted(
                other
                    .strip_prefix("UNTRUSTED ")
                    .unwrap_or(other)
                    .to_string(),
            ),
        }
    }

    pub(super) async fn write(
        prefix: &[String],
        location: &MarkerLocation,
        marker: &QuotaMarker,
    ) -> Result<(), Error> {
        let path = location.path();

        let mut args = vec![marker.to_text()?, path.to_string_lossy().into_owned()];
        args.extend(paths(location));

        run(prefix, WRITE, &args).await.map(|_| ()).map_err(|e| {
            Error::Failed(format!(
                "Could not write the quota marker {}: {}",
                path.display(),
                e
            ))
        })
    }

    pub(super) async fn remove(prefix: &[String], location: &MarkerLocation) -> Result<(), Error> {
        let MarkerRead::Ours(_) = read(prefix, location).await else {
            return Ok(());
        };

        let path = location.path().to_string_lossy().into_owned();

        run(prefix, "rm -f -- \"$1\"", std::slice::from_ref(&path))
            .await
            .map(|_| ())
            .map_err(|e| {
                Error::Failed(format!("Could not remove the quota marker {}: {}", path, e))
            })
    }
}

fn parse(path: &Path, text: &str) -> MarkerRead {
    match toml::from_str::<QuotaMarker>(text) {
        Ok(marker) => MarkerRead::Ours(marker),
        Err(e) => MarkerRead::Untrusted(format!("could not parse {}: {}", path.display(), e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    /// Write a test fixture (no key material).
    fn put(path: impl AsRef<Path>, text: &str) {
        std::fs::File::create(path)
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }

    fn temp_root(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("op-quota-marker-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Run the remote commands locally: `env` is a prefix that changes nothing.
    fn local_prefix() -> Vec<String> {
        vec!["env".to_string()]
    }

    async fn read_via(remote: bool, location: &MarkerLocation) -> MarkerRead {
        if remote {
            remote::read(&local_prefix(), location).await
        } else {
            native::read(location)
        }
    }

    async fn write_via(
        remote: bool,
        location: &MarkerLocation,
        marker: &QuotaMarker,
    ) -> Result<(), Error> {
        if remote {
            remote::write(&local_prefix(), location, marker).await
        } else {
            native::write(location, marker)
        }
    }

    async fn remove_via(remote: bool, location: &MarkerLocation) -> Result<(), Error> {
        if remote {
            remote::remove(&local_prefix(), location).await
        } else {
            native::remove(location)
        }
    }

    #[test]
    fn test_marker_location() {
        let root = Path::new("/projects");

        let location = MarkerLocation::new(root, Path::new("/projects/proj")).unwrap();
        assert_eq!(
            location.path(),
            PathBuf::from("/projects/.openportal_quota/proj.toml")
        );

        let location =
            MarkerLocation::new(Path::new("/home"), Path::new("/home/proj/alice")).unwrap();
        assert_eq!(
            location.path(),
            PathBuf::from("/home/.openportal_quota/proj/alice.toml")
        );
        assert_eq!(
            location.dir_paths(),
            vec![
                PathBuf::from("/home/.openportal_quota"),
                PathBuf::from("/home/.openportal_quota/proj")
            ]
        );

        // not under the root, the root itself, or inside the marker directory
        assert!(MarkerLocation::new(root, Path::new("/scratch/proj")).is_err());
        assert!(MarkerLocation::new(root, root).is_err());
        assert!(MarkerLocation::new(root, Path::new("/projects/../etc")).is_err());
        assert!(MarkerLocation::new(root, Path::new("/projects/.openportal_quota/x")).is_err());
    }

    /// The same round trip, natively and through an exec prefix.
    async fn round_trip(name: &str, remote: bool) {
        let root = temp_root(name);
        let walked = root.join("proj").join("alice");
        std::fs::create_dir_all(&walked).unwrap();
        let location = MarkerLocation::new(&root, &walked).unwrap();

        assert_eq!(read_via(remote, &location).await, MarkerRead::Absent);

        let started = Utc::now();
        let marker = QuotaMarker::started(500, started);
        write_via(remote, &location, &marker).await.unwrap();
        assert_eq!(
            read_via(remote, &location).await,
            MarkerRead::Ours(marker.clone())
        );
        assert!(!marker.is_complete_for(500));

        // nothing appears in the walked directory itself
        assert_eq!(std::fs::read_dir(&walked).unwrap().count(), 0);

        let metadata = std::fs::symlink_metadata(location.path()).unwrap();
        assert_eq!(metadata.mode() & 0o777, 0o400);
        for dir in location.dir_paths() {
            let metadata = std::fs::symlink_metadata(&dir).unwrap();
            assert_eq!(metadata.mode() & 0o777, 0o700, "{}", dir.display());
        }

        // replacing a read-only marker works, because it is renamed over
        let marker = QuotaMarker::complete(500, Some(started), Utc::now());
        write_via(remote, &location, &marker).await.unwrap();
        assert_eq!(
            read_via(remote, &location).await,
            MarkerRead::Ours(marker.clone())
        );
        assert!(marker.is_complete_for(500));
        assert!(!marker.is_complete_for(501));

        // no temporary files are left behind
        let names: Vec<_> = std::fs::read_dir(location.path().parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![OsString::from("alice.toml")]);

        let text = std::fs::read_to_string(location.path()).unwrap();
        assert!(text.contains("quota_id = 500"));
        assert!(text.contains("state = \"complete\""));

        remove_via(remote, &location).await.unwrap();
        assert_eq!(read_via(remote, &location).await, MarkerRead::Absent);

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[tokio::test]
    async fn test_marker_round_trip() {
        round_trip("round-trip", false).await;
    }

    #[tokio::test]
    async fn test_marker_round_trip_through_exec_prefix() {
        round_trip("round-trip-remote", true).await;
    }

    /// What must never be trusted, natively and through an exec prefix.
    async fn untrusted(name: &str, remote: bool) {
        let root = temp_root(name);
        let walked = root.join("proj");
        std::fs::create_dir_all(&walked).unwrap();
        let location = MarkerLocation::new(&root, &walked).unwrap();
        let marker_dir = root.join(MARKER_DIR);

        let assert_untrusted = |result: MarkerRead| {
            assert!(
                matches!(result, MarkerRead::Untrusted(_)),
                "{}: {:?}",
                name,
                result
            );
        };

        // the marker directory is a symlink to somewhere else
        let elsewhere = root.join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        put(
            elsewhere.join("proj.toml"),
            "quota_id = 7\nstate = \"complete\"\n",
        );
        std::os::unix::fs::symlink(&elsewhere, &marker_dir).unwrap();
        assert_untrusted(read_via(remote, &location).await);
        assert!(
            write_via(remote, &location, &QuotaMarker::started(7, Utc::now()))
                .await
                .is_err()
        );
        std::fs::remove_file(&marker_dir).unwrap();

        // the marker directory is writable by others
        std::fs::create_dir(&marker_dir).unwrap();
        std::fs::set_permissions(&marker_dir, std::fs::Permissions::from_mode(0o777)).unwrap();
        put(location.path(), "quota_id = 7\nstate = \"complete\"\n");
        assert_untrusted(read_via(remote, &location).await);
        assert!(
            write_via(remote, &location, &QuotaMarker::started(7, Utc::now()))
                .await
                .is_err()
        );
        std::fs::set_permissions(&marker_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(matches!(
            read_via(remote, &location).await,
            MarkerRead::Ours(_)
        ));

        // the marker is a symlink: not followed, and replaced by a write
        std::fs::remove_file(location.path()).unwrap();
        std::os::unix::fs::symlink(elsewhere.join("proj.toml"), location.path()).unwrap();
        assert_untrusted(read_via(remote, &location).await);
        write_via(remote, &location, &QuotaMarker::started(7, Utc::now()))
            .await
            .unwrap();
        assert!(std::fs::symlink_metadata(location.path())
            .unwrap()
            .file_type()
            .is_file());
        assert_eq!(
            std::fs::read_to_string(elsewhere.join("proj.toml")).unwrap(),
            "quota_id = 7\nstate = \"complete\"\n"
        );

        // garbage, or a field we never write
        std::fs::remove_file(location.path()).unwrap();
        put(location.path(), "not toml at all [");
        assert_untrusted(read_via(remote, &location).await);
        put(
            location.path(),
            "quota_id = 5\nstate = \"complete\"\nextra = 1\n",
        );
        assert_untrusted(read_via(remote, &location).await);

        // not a regular file
        std::fs::remove_file(location.path()).unwrap();
        std::fs::create_dir(location.path()).unwrap();
        assert_untrusted(read_via(remote, &location).await);
        std::fs::remove_dir(location.path()).unwrap();

        // a second link to a marker, which is then not removed
        put(location.path(), "quota_id = 5\nstate = \"complete\"\n");
        assert!(matches!(
            read_via(remote, &location).await,
            MarkerRead::Ours(_)
        ));
        std::fs::hard_link(location.path(), marker_dir.join("other")).unwrap();
        assert_untrusted(read_via(remote, &location).await);
        remove_via(remote, &location).await.unwrap();
        assert!(location.path().exists());

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[tokio::test]
    async fn test_marker_that_is_not_ours_is_untrusted() {
        untrusted("untrusted", false).await;
    }

    #[tokio::test]
    async fn test_marker_that_is_not_ours_is_untrusted_through_exec_prefix() {
        untrusted("untrusted-remote", true).await;
    }
}
