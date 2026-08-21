// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! Resolving Unix user and group names to their numeric ids.
//!
//! ## Why this does not use libc
//!
//! Release binaries are **statically linked against musl**
//! (`x86_64-unknown-linux-musl` / `aarch64-unknown-linux-musl` - see
//! `.github/workflows/build.yml`), and musl has no NSS implementation at all.
//! `getpwnam_r`/`getgrnam_r` - which is what `nix::unistd::User::from_name` and
//! `Group::from_name` call - therefore read `/etc/passwd` and `/etc/group`, and on a
//! miss make **one** attempt over musl's own minimal nscd-protocol client on
//! `/var/run/nscd/socket`. That is the whole of the lookup path: there is no
//! `nsswitch.conf`, no `sss` module, and no per-source fallback. Two consequences bit
//! us in production:
//!
//! - When `nscd` is not running, musl treats the failed `connect()` as "no nscd here"
//!   and returns *not found* rather than an error. Every group in the directory then
//!   looks as though it does not exist, confidently and instantly.
//! - When `nscd` answers but the exchange does not go cleanly - a short read, a
//!   truncated reply, a version it does not recognise, which is what a saturated
//!   `nscd` thread pool produces - musl's client reports `EIO`. SSSD logs nothing for
//!   these, because the request never reached it.
//!
//! Dynamically linking would not help: the limitation is musl's, not the linker's.
//! So all name resolution goes through the host's `getent`, which is a glibc-dynamic
//! binary and therefore consults every source in the host's `nsswitch.conf` -
//! including `sss` - whether or not `nscd` is healthy. It is also a `tokio::process`
//! call rather than blocking FFI, so it does not stall a worker thread.
//!
//! **Never** resolve a user or group through libc anywhere in this workspace.
//!
//! ## Why the outcome has three cases, not two
//!
//! The old code collapsed "this name definitively does not exist" and "I could not
//! find out whether it exists" into one terminal error, which is what turned a
//! transient `nscd` hiccup into a user-visible job failure. They are kept apart here:
//! an indeterminate lookup is retried a few times and then reported as a temporary
//! failure, while a genuine absence fails immediately.

use templemeads::Error;

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::process::Command;
use tokio::sync::OnceCell;
use tokio::time::timeout;

/// How long a single `getent` invocation is given before it is abandoned.
///
/// A lookup that has not answered within this long is not going to be useful to the
/// job that is waiting for it, and the point of a timeout is that an unresponsive
/// name service cannot pin this task indefinitely - which is exactly what the libc
/// call it replaces could do, having no timeout of its own.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);

/// How many times an *indeterminate* lookup is attempted before giving up.
///
/// Only indeterminate outcomes are retried. A definitive "no such name" is returned
/// on the first attempt, and a success obviously needs no second one.
const LOOKUP_ATTEMPTS: u32 = 3;

/// Base delay between retries, multiplied by the attempt number.
const RETRY_BACKOFF: Duration = Duration::from_millis(250);

/// Where `getent` lives on a normal Linux host.
const STANDARD_GETENT: &str = "/usr/bin/getent";

///
/// The `getent` this process will use for the whole of its life, resolved once on
/// first use. `None` means none was found, and the flat-file fallback is all there is.
///
/// **Locked in deliberately.** Which `getent` a host has is not something that changes
/// under a running agent, so if the one we resolved stops working we say so loudly and
/// fail the lookup until it comes back, rather than quietly moving to a different
/// binary. A `getent` appearing or disappearing mid-run is a sign that something is
/// wrong with the host, and silently resolving names through a *different* program
/// than the one this process started with - possibly one earlier on `PATH`, possibly
/// answering from different sources - is not a failure mode worth having.
///
static GETENT: OnceCell<Option<PathBuf>> = OnceCell::const_new();

///
/// Which name-service database to query.
///
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Database {
    Passwd,
    Group,
}

impl Database {
    /// The `getent` database name.
    fn as_str(&self) -> &'static str {
        match self {
            Database::Passwd => "passwd",
            Database::Group => "group",
        }
    }

    /// The flat file holding local entries for this database.
    fn file(&self) -> &'static str {
        match self {
            Database::Passwd => "/etc/passwd",
            Database::Group => "/etc/group",
        }
    }

    /// What an entry in this database is called, for error messages.
    fn what(&self) -> &'static str {
        match self {
            Database::Passwd => "user",
            Database::Group => "group",
        }
    }
}

///
/// The outcome of a single lookup attempt.
///
#[derive(Clone, Debug, PartialEq, Eq)]
enum Outcome {
    /// The name resolved to this id.
    Found(u32),

    /// Every source configured on this host was consulted, and none of them knows
    /// this name. This is a real answer, and it is authoritative.
    Absent,

    /// We could not find out. The name may or may not exist - nothing has been
    /// learned either way, and the caller should try again later.
    Indeterminate(String),
}

///
/// Resolve a username to its UID.
///
/// Fails with `Error::NotFound` if the host's name sources agree that no such user
/// exists, and `Error::Unavailable` if they could not be asked.
///
pub async fn resolve_uid(username: &str) -> Result<u32, Error> {
    resolve(Database::Passwd, username).await
}

///
/// Resolve a group name to its GID.
///
/// Fails with `Error::NotFound` if the host's name sources agree that no such group
/// exists, and `Error::Unavailable` if they could not be asked.
///
pub async fn resolve_gid(groupname: &str) -> Result<u32, Error> {
    resolve(Database::Group, groupname).await
}

///
/// Resolve a name in `database` to its numeric id, retrying while the answer is
/// indeterminate.
///
async fn resolve(database: Database, name: &str) -> Result<u32, Error> {
    check_name(database, name)?;

    let mut reason = String::new();

    for attempt in 1..=LOOKUP_ATTEMPTS {
        match lookup(database, name).await {
            Outcome::Found(id) => {
                tracing::debug!("Resolved {} '{}' to {}", database.what(), name, id);
                return Ok(id);
            }
            Outcome::Absent => {
                return Err(Error::NotFound(format!(
                    "There is no {} called '{}' - every name source on this host was \
                     asked, and none of them knows it",
                    database.what(),
                    name
                )));
            }
            Outcome::Indeterminate(why) => {
                tracing::warn!(
                    "Could not determine whether the {} '{}' exists (attempt {} of {}): {}",
                    database.what(),
                    name,
                    attempt,
                    LOOKUP_ATTEMPTS,
                    why
                );

                reason = why;

                if attempt < LOOKUP_ATTEMPTS {
                    tokio::time::sleep(RETRY_BACKOFF.saturating_mul(attempt)).await;
                }
            }
        }
    }

    Err(Error::Unavailable(format!(
        "Could not determine whether a {} called '{}' exists - the system name service \
         did not answer after {} attempts ({}). This is a temporary failure, so please \
         retry.",
        database.what(),
        name,
        LOOKUP_ATTEMPTS,
        reason
    )))
}

///
/// Reject a name that we should not be handing to `getent` at all.
///
/// These names all arrive from a peer. The grammar that parsed them is already much
/// stricter than this, so nothing legitimate is turned away here - this is the
/// belt-and-braces check that keeps a name from being read as an option or as more
/// than one field, independent of whatever validation happened upstream.
///
fn check_name(database: Database, name: &str) -> Result<(), Error> {
    if name.is_empty() {
        return Err(Error::Parse(format!(
            "Cannot look up a {} with an empty name",
            database.what()
        )));
    }

    if name.starts_with('-') {
        return Err(Error::Parse(format!(
            "Refusing to look up the {} '{}' - a name cannot begin with '-'",
            database.what(),
            name
        )));
    }

    if let Some(c) = name
        .chars()
        .find(|c| *c == ':' || *c == ',' || *c == '\0' || c.is_whitespace() || c.is_control())
    {
        return Err(Error::Parse(format!(
            "Refusing to look up the {} '{}' - a name cannot contain {:?}",
            database.what(),
            name.escape_debug(),
            c
        )));
    }

    Ok(())
}

///
/// The `getent` this process uses, resolved on first use and then fixed.
///
/// Resolution is by **existence**, not by trying it and seeing whether it works: if
/// `/usr/bin/getent` is there, that is the one, and a host that has it somewhere else
/// gets whatever `PATH` yields - saved as an absolute path, so the program run later
/// cannot depend on `PATH` or on the working directory having changed since.
///
async fn getent() -> Option<&'static Path> {
    GETENT.get_or_init(find_getent).await.as_deref()
}

///
/// Find a `getent` to lock on to. Called exactly once per process.
///
async fn find_getent() -> Option<PathBuf> {
    let standard = PathBuf::from(STANDARD_GETENT);

    if is_program(&standard).await {
        tracing::info!("Resolving users and groups with '{}'", standard.display());
        return Some(standard);
    }

    // Not at the standard location, so fall back to `PATH` - but only its absolute
    // entries. A relative entry would resolve against whatever the working directory
    // happens to be, which is neither reproducible nor something we want deciding
    // which program resolves the names behind a `chown`.
    let path = std::env::var_os("PATH")?;

    for dir in std::env::split_paths(&path) {
        if !dir.is_absolute() {
            tracing::debug!(
                "Ignoring the relative PATH entry '{}' while looking for getent",
                dir.display()
            );
            continue;
        }

        let candidate = dir.join("getent");

        if is_program(&candidate).await {
            tracing::warn!(
                "'{}' does not exist - resolving users and groups with '{}', found via \
                 PATH. This process will use that one from now on.",
                STANDARD_GETENT,
                candidate.display()
            );
            return Some(candidate);
        }
    }

    tracing::warn!(
        "No 'getent' exists on this host - users and groups can only be resolved from \
         the local files, so no name held only in the directory can be looked up. \
         Every such lookup will fail as indeterminate until a 'getent' is installed \
         and this agent is restarted."
    );

    None
}

///
/// Is `path` a program we can run? An existence check, not a trial run.
///
async fn is_program(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    match tokio::fs::metadata(path).await {
        Ok(metadata) => metadata.is_file() && metadata.permissions().mode() & 0o111 != 0,
        Err(_) => false,
    }
}

///
/// Make one attempt to look `name` up in `database`.
///
/// Uses the `getent` this process locked on to at startup, and *only* that one. If it
/// cannot be run the lookup is indeterminate and says so loudly: the binary that was
/// there when this agent started has gone, which is a problem with the host and not
/// something to paper over by finding another one. Lookups resume as soon as it is
/// back, since the next attempt tries the same path again.
///
async fn lookup(database: Database, name: &str) -> Outcome {
    let Some(program) = getent().await else {
        // No `getent` existed when this process started - already warned about once,
        // at discovery. The flat file holds only local entries, so a hit is
        // trustworthy and a miss is not; see `read_file`.
        return read_file(database, name).await;
    };

    match run_getent(program, database, name).await {
        Ok(outcome) => outcome,
        Err(e) => {
            tracing::error!(
                "'{}' could not be run ({}) - it existed when this agent started, so \
                 something has changed on this host. Refusing to resolve names through \
                 any other program; lookups will fail until it is back.",
                program.display(),
                e
            );

            Outcome::Indeterminate(format!("'{}' could not be run: {}", program.display(), e))
        }
    }
}

///
/// Run one `getent` and interpret its exit status.
///
/// `Err` means the program could not be run at all, which the caller reports as a host
/// problem. Everything else - including a timeout, a signal, or output that makes no
/// sense - is an `Outcome`, because the program did run.
///
async fn run_getent(
    program: &Path,
    database: Database,
    name: &str,
) -> Result<Outcome, std::io::Error> {
    tracing::debug!(
        "Running: {} {} {}",
        program.display(),
        database.as_str(),
        name
    );

    let mut command = Command::new(program);
    command.arg(database.as_str()).arg(name);

    let output = match timeout(LOOKUP_TIMEOUT, command.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => return Err(e),
        Err(_) => {
            return Ok(Outcome::Indeterminate(format!(
                "'{} {} {}' did not answer within {} seconds",
                program.display(),
                database.as_str(),
                name,
                LOOKUP_TIMEOUT.as_secs()
            )))
        }
    };

    // `getent` exits 0 on success, 1 if it was called wrongly or does not know the
    // database, 2 if the key is not in any source, and 3 if enumeration is not
    // supported. Only 2 is a real "no".
    match output.status.code() {
        Some(0) => Ok(parse_entry(
            database,
            name,
            &String::from_utf8_lossy(&output.stdout),
        )),
        Some(2) => Ok(Outcome::Absent),
        Some(code) => Ok(Outcome::Indeterminate(format!(
            "'{} {} {}' exited with code {}: {}",
            program.display(),
            database.as_str(),
            name,
            code,
            String::from_utf8_lossy(&output.stderr).trim()
        ))),
        None => Ok(Outcome::Indeterminate(format!(
            "'{} {} {}' was killed by a signal",
            program.display(),
            database.as_str(),
            name
        ))),
    }
}

///
/// Pull the numeric id out of `getent` output.
///
/// `passwd` entries are `name:passwd:uid:gid:...` and `group` entries are
/// `name:passwd:gid:members`, so the id wanted is the third field either way.
///
/// The name in the entry is deliberately **not** checked against the name asked for.
/// A host with `use_fully_qualified_names` set, or a source with its own idea of the
/// canonical spelling, legitimately answers with a different string, and turning
/// those into failures would recreate the class of bug this module exists to fix.
///
fn parse_entry(database: Database, name: &str, stdout: &str) -> Outcome {
    // More than one source can answer. The first entry wins, which is the same
    // precedence `getent` itself applied when it ordered them.
    let Some(entry) = stdout.lines().find(|line| !line.trim().is_empty()) else {
        return Outcome::Indeterminate(format!(
            "'getent {} {}' succeeded but returned nothing",
            database.as_str(),
            name
        ));
    };

    let Some(id) = entry.split(':').nth(2) else {
        return Outcome::Indeterminate(format!(
            "'getent {} {}' returned an entry with no id field: {}",
            database.as_str(),
            name,
            entry.escape_debug()
        ));
    };

    match id.trim().parse::<u32>() {
        Ok(id) => Outcome::Found(id),
        Err(e) => Outcome::Indeterminate(format!(
            "'getent {} {}' returned an unparseable id '{}': {}",
            database.as_str(),
            name,
            id.escape_debug(),
            e
        )),
    }
}

///
/// Look `name` up by reading the flat file for `database`, for hosts with no `getent`.
///
/// A miss here is `Indeterminate`, not `Absent`: the file holds only local entries, so
/// not being in it says nothing about whether the directory knows the name. Claiming
/// otherwise is precisely the mistake that made a missing `nscd` look like a missing
/// group.
///
async fn read_file(database: Database, name: &str) -> Outcome {
    let path = database.file();

    let contents = match tokio::fs::read_to_string(path).await {
        Ok(contents) => contents,
        Err(e) => {
            return Outcome::Indeterminate(format!("could not read {}: {}", path, e));
        }
    };

    for line in contents.lines() {
        let line = line.trim();

        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let mut fields = line.split(':');

        if fields.next() != Some(name) {
            continue;
        }

        let Some(id) = fields.nth(1) else {
            continue;
        };

        return match id.trim().parse::<u32>() {
            Ok(id) => Outcome::Found(id),
            Err(e) => Outcome::Indeterminate(format!(
                "{} holds an unparseable id '{}' for '{}': {}",
                path,
                id.escape_debug(),
                name,
                e
            )),
        };
    }

    Outcome::Indeterminate(format!(
        "there is no 'getent' on this host and no {} called '{}' in {}, so the \
         directory could not be consulted",
        database.what(),
        name,
        path
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_passwd_entry() {
        assert_eq!(
            parse_entry(
                Database::Passwd,
                "fred",
                "fred:x:1001:1002:Fred:/home/fred:/bin/bash\n"
            ),
            Outcome::Found(1001)
        );
    }

    #[test]
    fn test_parse_group_entry() {
        assert_eq!(
            parse_entry(
                Database::Group,
                "acme.proj01",
                "acme.proj01:*:5000:fred,jane\n"
            ),
            Outcome::Found(5000)
        );
    }

    #[test]
    fn test_parse_takes_first_entry() {
        assert_eq!(
            parse_entry(
                Database::Group,
                "staff",
                "staff:*:50:fred\nstaff:*:1000:jane\n"
            ),
            Outcome::Found(50)
        );
    }

    #[test]
    fn test_parse_accepts_a_differently_spelled_name() {
        // SSSD with `use_fully_qualified_names` answers like this - it must not be
        // treated as a failure.
        assert_eq!(
            parse_entry(
                Database::Group,
                "acme.proj01",
                "acme.proj01@ipa.example.com:*:5000:\n"
            ),
            Outcome::Found(5000)
        );
    }

    #[test]
    fn test_parse_empty_output_is_indeterminate() {
        assert!(matches!(
            parse_entry(Database::Group, "acme.proj01", "\n \n"),
            Outcome::Indeterminate(_)
        ));
    }

    #[test]
    fn test_parse_missing_id_field_is_indeterminate() {
        assert!(matches!(
            parse_entry(Database::Group, "acme.proj01", "acme.proj01:*\n"),
            Outcome::Indeterminate(_)
        ));
    }

    #[test]
    fn test_parse_unparseable_id_is_indeterminate() {
        // Never `Found(0)`, which would be `root`.
        assert!(matches!(
            parse_entry(Database::Group, "acme.proj01", "acme.proj01:*:not_a_gid:\n"),
            Outcome::Indeterminate(_)
        ));

        assert!(matches!(
            parse_entry(Database::Group, "acme.proj01", "acme.proj01:*:-1:\n"),
            Outcome::Indeterminate(_)
        ));
    }

    #[test]
    fn test_check_name_accepts_real_names() {
        assert!(check_name(Database::Group, "acme.proj01").is_ok());
        assert!(check_name(Database::Passwd, "fred.bloggs").is_ok());
        assert!(check_name(Database::Group, "acme.proj01@ipa.example.com").is_ok());
    }

    #[test]
    fn test_check_name_rejects_dangerous_names() {
        for name in [
            "",
            "-V",
            "--help",
            "acme:proj01",
            "acme proj01",
            "acme,proj01",
            "acme\nproj01",
            "acme\0proj01",
        ] {
            assert!(
                check_name(Database::Group, name).is_err(),
                "should have rejected {:?}",
                name
            );
        }
    }

    #[tokio::test]
    async fn test_is_program_accepts_a_real_program() {
        assert!(is_program(Path::new("/bin/sh")).await);
    }

    #[tokio::test]
    async fn test_is_program_rejects_a_non_executable_file() {
        assert!(!is_program(Path::new("/etc/passwd")).await);
    }

    #[tokio::test]
    async fn test_is_program_rejects_a_directory() {
        // Has the executable bit set, but is not something we can run - which is what
        // stops a `PATH` entry containing a *directory* called `getent` being locked
        // on to.
        assert!(!is_program(Path::new("/usr/bin")).await);
    }

    #[tokio::test]
    async fn test_is_program_rejects_a_missing_path() {
        assert!(!is_program(Path::new("/nonexistent/openportal/getent")).await);
    }

    #[tokio::test]
    async fn test_getent_is_locked_in() {
        // Whatever this host has, the answer must be stable for the life of the
        // process and must be an absolute path - the point of resolving once is that
        // the program used cannot change under a running agent, nor depend on `PATH`
        // or the working directory at the moment of a lookup.
        let first = getent().await;
        let second = getent().await;

        assert_eq!(first, second);

        if let Some(program) = first {
            assert!(
                program.is_absolute(),
                "{} is not absolute",
                program.display()
            );
        }
    }

    #[tokio::test]
    async fn test_resolve_root() {
        // An end-to-end exercise of the whole path - `getent` if the host has it, the
        // flat-file fallback if it does not. `root` is uid 0 and is in `/etc/passwd`
        // on every platform this builds for, so this holds whether or not there is a
        // directory to consult.
        assert_eq!(resolve_uid("root").await.expect("root must resolve"), 0);
    }

    #[tokio::test]
    async fn test_resolve_never_finds_an_impossible_name() {
        // Whatever this host has - a real `getent`, none at all, a working directory
        // or a broken one - a name that cannot exist must never resolve, and must
        // never panic. Which of the two failures it is depends on the host, so both
        // are accepted here.
        let result =
            resolve_gid("openportal.no.such.group.a8f3c1e0-0000-4000-8000-000000000000").await;

        assert!(matches!(
            result,
            Err(Error::NotFound(_)) | Err(Error::Unavailable(_))
        ));
    }
}
