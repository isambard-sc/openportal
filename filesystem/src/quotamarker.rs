// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! The `.openportal_quota` marker: a record, kept in a user or project
//! directory, of the recursive walk that gave its tree a Lustre quota ID.
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
//! The marker lives in a directory its user or project members can write to,
//! so nothing in it is trusted unless it is a regular file, with one link,
//! owned by the agent's own effective user. Anything else is ignored, and the
//! worst that can then happen is that a walk is repeated. The agent writes it
//! mode 0400, through a uniquely named temporary file opened with `O_EXCL` and
//! `O_NOFOLLOW` that is then renamed over the marker, so a planted symlink or
//! file is replaced rather than followed. It also records when the walk
//! started and completed (UTC), for operators.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use templemeads::Error;

/// The marker's file name, in the top directory of the walked tree.
pub const MARKER_NAME: &str = ".openportal_quota";

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

pub fn marker_path(directory: &Path) -> PathBuf {
    directory.join(MARKER_NAME)
}

fn effective_uid() -> u32 {
    nix::unistd::geteuid().as_raw()
}

/// Read the marker in `directory`.
///
/// The checks are made on the opened file, not on the path, so the file
/// cannot be swapped between checking and reading.
pub fn read_marker(directory: &Path) -> MarkerRead {
    let path = marker_path(directory);

    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
        .open(&path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound => return MarkerRead::Absent,
        Err(e) => {
            return MarkerRead::Untrusted(format!("could not open {}: {}", path.display(), e))
        }
    };

    let metadata = match file.metadata() {
        Ok(metadata) => metadata,
        Err(e) => {
            return MarkerRead::Untrusted(format!("could not stat {}: {}", path.display(), e))
        }
    };

    if !metadata.file_type().is_file() {
        return MarkerRead::Untrusted(format!("{} is not a regular file", path.display()));
    }

    if metadata.uid() != effective_uid() {
        return MarkerRead::Untrusted(format!(
            "{} is owned by uid {}, not by this agent (uid {})",
            path.display(),
            metadata.uid(),
            effective_uid()
        ));
    }

    if metadata.nlink() != 1 {
        return MarkerRead::Untrusted(format!(
            "{} has {} links, not 1",
            path.display(),
            metadata.nlink()
        ));
    }

    let mut text = String::new();

    if let Err(e) = file.take(MAX_MARKER_BYTES).read_to_string(&mut text) {
        return MarkerRead::Untrusted(format!("could not read {}: {}", path.display(), e));
    }

    match toml::from_str::<QuotaMarker>(&text) {
        Ok(marker) => MarkerRead::Ours(marker),
        Err(e) => MarkerRead::Untrusted(format!("could not parse {}: {}", path.display(), e)),
    }
}

/// Write `marker` into `directory`, replacing any marker already there.
pub fn write_marker(directory: &Path, marker: &QuotaMarker) -> Result<(), Error> {
    let text = format!(
        "# Written by the OpenPortal filesystem agent to record the walk that gave\n\
         # this directory tree its Lustre quota ID. Do not edit: a marker this\n\
         # agent did not write is ignored, and the tree is walked again.\n{}",
        toml::to_string(marker)
            .map_err(|e| Error::Failed(format!("Could not serialise the quota marker: {}", e)))?
    );

    let target = marker_path(directory);
    let temp = directory.join(format!(
        "{}.{:016x}.tmp",
        MARKER_NAME,
        rand::random::<u64>()
    ));

    let write = || -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o400)
            .custom_flags(nix::libc::O_NOFOLLOW)
            .open(&temp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, &target)
    };

    write().map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        Error::Failed(format!(
            "Could not write the quota marker {}: {}",
            target.display(),
            e
        ))
    })
}

/// Remove the marker from `directory`, if it is one this agent wrote.
/// Anything else is left alone.
pub fn remove_marker(directory: &Path) -> Result<(), Error> {
    match read_marker(directory) {
        MarkerRead::Ours(_) => {
            let path = marker_path(directory);
            std::fs::remove_file(&path).map_err(|e| {
                Error::Failed(format!(
                    "Could not remove the quota marker {}: {}",
                    path.display(),
                    e
                ))
            })
        }
        MarkerRead::Absent | MarkerRead::Untrusted(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a test fixture (no key material).
    fn put(path: impl AsRef<Path>, text: &str) {
        std::fs::File::create(path)
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("op-quota-marker-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_marker_round_trip() {
        let dir = temp_dir("round-trip");

        assert_eq!(read_marker(&dir), MarkerRead::Absent);

        let started = Utc::now();
        let marker = QuotaMarker::started(500, started);
        write_marker(&dir, &marker).unwrap();
        assert_eq!(read_marker(&dir), MarkerRead::Ours(marker.clone()));
        assert!(!marker.is_complete_for(500));

        let metadata = std::fs::symlink_metadata(marker_path(&dir)).unwrap();
        assert_eq!(metadata.mode() & 0o777, 0o400);

        // replacing a read-only marker works, because it is renamed over
        let marker = QuotaMarker::complete(500, Some(started), Utc::now());
        write_marker(&dir, &marker).unwrap();
        assert_eq!(read_marker(&dir), MarkerRead::Ours(marker.clone()));
        assert!(marker.is_complete_for(500));
        assert!(!marker.is_complete_for(501));

        // no temporary files are left behind
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from(MARKER_NAME)]);

        let text = std::fs::read_to_string(marker_path(&dir)).unwrap();
        assert!(text.contains("quota_id = 500"));
        assert!(text.contains("state = \"complete\""));

        remove_marker(&dir).unwrap();
        assert_eq!(read_marker(&dir), MarkerRead::Absent);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_marker_symlink_is_not_followed() {
        let dir = temp_dir("symlink");
        let elsewhere = dir.join("elsewhere");

        put(&elsewhere, "quota_id = 500\nstate = \"complete\"\n");
        std::os::unix::fs::symlink(&elsewhere, marker_path(&dir)).unwrap();

        assert!(matches!(read_marker(&dir), MarkerRead::Untrusted(_)));

        // writing replaces the link itself, and leaves its target alone
        write_marker(&dir, &QuotaMarker::started(7, Utc::now())).unwrap();
        assert!(std::fs::symlink_metadata(marker_path(&dir))
            .unwrap()
            .file_type()
            .is_file());
        assert_eq!(
            std::fs::read_to_string(&elsewhere).unwrap(),
            "quota_id = 500\nstate = \"complete\"\n"
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_marker_that_is_not_ours_is_untrusted() {
        let dir = temp_dir("untrusted");

        // not a regular file
        std::fs::create_dir(marker_path(&dir)).unwrap();
        assert!(matches!(read_marker(&dir), MarkerRead::Untrusted(_)));
        std::fs::remove_dir(marker_path(&dir)).unwrap();

        // garbage, or a field we never write
        put(marker_path(&dir), "not toml at all [");
        assert!(matches!(read_marker(&dir), MarkerRead::Untrusted(_)));
        put(
            marker_path(&dir),
            "quota_id = 5\nstate = \"complete\"\nextra = 1\n",
        );
        assert!(matches!(read_marker(&dir), MarkerRead::Untrusted(_)));

        // a second link to a marker
        put(marker_path(&dir), "quota_id = 5\nstate = \"complete\"\n");
        assert!(matches!(read_marker(&dir), MarkerRead::Ours(_)));
        std::fs::hard_link(marker_path(&dir), dir.join("other")).unwrap();
        assert!(matches!(read_marker(&dir), MarkerRead::Untrusted(_)));

        // an untrusted marker is not removed
        remove_marker(&dir).unwrap();
        assert!(marker_path(&dir).exists());

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
