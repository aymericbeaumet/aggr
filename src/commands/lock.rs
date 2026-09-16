//! One process owns a piece of shared on-disk state at a time. `aggr dev` guards its reusable
//! snapshot with it; sync, build and clean guard a repository's `.aggr/` directory, where two
//! concurrent runs would otherwise race on `git worktree add`, `prune` and the data rebase.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read as _, Seek as _, Write as _};
use std::path::Path;

use anyhow::{Context as _, Result, bail};

/// An exclusive advisory lock on one file, released when dropped or when the process ends.
/// Acquisition never waits: a second command fails at once with the holder's identity.
#[derive(Debug)]
pub struct Guard {
    _file: File,
}

impl Guard {
    /// Take `path` on behalf of `aggr <subject>` working on `target`, creating the lock file
    /// when it does not exist yet.
    pub fn acquire(path: &Path, subject: &str, target: &Path) -> Result<Self> {
        Self::open(path, subject, target, false)?.context("creating lock file")
    }

    /// Fail when another process holds `path`, without creating the file when it is absent.
    pub fn inspect(path: &Path, subject: &str, target: &Path) -> Result<Option<Self>> {
        Self::open(path, subject, target, true)
    }

    fn open(path: &Path, subject: &str, target: &Path, read_only: bool) -> Result<Option<Self>> {
        let mut file = match OpenOptions::new()
            .create(!read_only)
            .read(true)
            .write(!read_only)
            .truncate(false)
            .open(path)
        {
            Ok(file) => file,
            Err(error) if read_only && error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(error) => {
                return Err(error).with_context(|| format!("opening lock {}", path.display()));
            }
        };
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                let mut recorded = String::new();
                let _ = file.read_to_string(&mut recorded);
                bail!("{}", conflict_message(&recorded, subject, target));
            }
            Err(TryLockError::Error(err)) => {
                return Err(err).with_context(|| format!("locking {}", path.display()));
            }
        }
        if !read_only {
            file.set_len(0)?;
            file.rewind()?;
            write!(file, "{} {subject}", std::process::id())?;
            file.flush()?;
        }
        Ok(Some(Self { _file: file }))
    }
}

/// The error a newcomer sees. The holder recorded `<pid> <subject>`; an older holder wrote the
/// PID alone, and an interrupted write may have left nothing, so every part is optional and the
/// newcomer's own subject stands in when the holder's is unknown.
fn conflict_message(recorded: &str, subject: &str, target: &Path) -> String {
    let mut parts = recorded.trim().splitn(2, ' ');
    let pid = parts.next().filter(|pid| !pid.is_empty());
    let holder = parts
        .next()
        .map(str::trim)
        .filter(|holder| !holder.is_empty())
        .unwrap_or(subject);
    let suffix = pid.map(|pid| format!(" (PID {pid})")).unwrap_or_default();
    format!(
        "another `aggr {holder}` is already running for {}{suffix}",
        target.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_lock_refuses_a_second_owner_and_names_the_holder() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("aggr.lock");
        let target = Path::new("/repo");
        let first = Guard::acquire(&path, "sync", target).unwrap();
        let error = Guard::acquire(&path, "build", target).unwrap_err();
        assert_eq!(
            format!("{error:#}"),
            format!(
                "another `aggr sync` is already running for /repo (PID {})",
                std::process::id()
            )
        );
        assert!(Guard::inspect(&path, "clean", target).is_err());
        drop(first);
        Guard::acquire(&path, "build", target).unwrap();
        assert!(
            Guard::inspect(&temp.path().join("absent.lock"), "clean", target)
                .unwrap()
                .is_none()
        );
        assert!(!temp.path().join("absent.lock").exists());
    }

    #[test]
    fn conflict_messages_tolerate_older_and_empty_records() {
        let target = Path::new("aggr.toml");
        assert_eq!(
            conflict_message("", "dev", target),
            "another `aggr dev` is already running for aggr.toml"
        );
        assert_eq!(
            conflict_message("4242\n", "dev", target),
            "another `aggr dev` is already running for aggr.toml (PID 4242)"
        );
        assert_eq!(
            conflict_message("4242 build\n", "sync", target),
            "another `aggr build` is already running for aggr.toml (PID 4242)"
        );
    }
}
