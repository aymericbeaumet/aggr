//! One process owns a piece of shared on-disk state at a time. `aggr dev` guards its reusable
//! snapshot with it; sync, build and clean guard a repository's `.aggr/` directory, where two
//! concurrent runs would otherwise race on `git worktree add`, `prune` and the data rebase.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read as _, Seek as _, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

/// An exclusive advisory lock on one file, released when dropped or when the process ends.
/// Acquisition never waits: a second command fails at once with the holder's identity.
#[derive(Debug)]
pub struct Guard {
    _file: File,
    record: PathBuf,
}

impl Drop for Guard {
    /// The record says who holds the lock, so it goes when the lock does. Leaving it behind let a
    /// later conflict name a process that had already exited, and the wrong subcommand with it.
    /// This runs before `_file` is dropped, so the lock is never free while a stale record stands.
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.record);
    }
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
                // Windows locks are mandatory: the locked file cannot be read by another handle,
                // so the holder's identity is also kept in a sidecar the lock does not cover.
                let mut recorded = std::fs::read_to_string(record_path(path)).unwrap_or_default();
                if recorded.trim().is_empty() {
                    let _ = file.read_to_string(&mut recorded);
                }
                bail!("{}", conflict_message(&recorded, subject, target));
            }
            Err(TryLockError::Error(err)) => {
                return Err(err).with_context(|| format!("locking {}", path.display()));
            }
        }
        let record = format!("{} {subject}", std::process::id());
        if !read_only {
            file.set_len(0)?;
            file.rewind()?;
            write!(file, "{record}")?;
            file.flush()?;
        }
        // `inspect` holds the lock just as firmly as `acquire` does, so it owes a newcomer the
        // same answer about who is holding it. Only the locked file's own body is left alone,
        // because a read-only handle cannot write to it.
        let record_path = record_path(path);
        std::fs::write(&record_path, record)
            .with_context(|| format!("recording the holder of {}", path.display()))?;
        Ok(Some(Self {
            _file: file,
            record: record_path,
        }))
    }
}

/// `aggr.lock.holder` beside `aggr.lock`.
fn record_path(path: &Path) -> PathBuf {
    let mut record = path.as_os_str().to_owned();
    record.push(".holder");
    PathBuf::from(record)
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
        // A released lock leaves nobody to name. Keeping the record let the next conflict report
        // a process that had already exited, under whatever subcommand ran last.
        assert!(!record_path(&path).exists());

        // `inspect` holds the lock too, so it has to say so: a newcomer was told the last
        // `acquire` still held it, naming a dead PID and the wrong subcommand.
        let inspected = Guard::inspect(&path, "clean", target).unwrap().unwrap();
        assert_eq!(
            format!("{:#}", Guard::acquire(&path, "build", target).unwrap_err()),
            format!(
                "another `aggr clean` is already running for /repo (PID {})",
                std::process::id()
            )
        );
        drop(inspected);

        Guard::acquire(&path, "build", target).unwrap();
        assert!(
            Guard::inspect(&temp.path().join("absent.lock"), "clean", target)
                .unwrap()
                .is_none()
        );
        assert!(!temp.path().join("absent.lock").exists());
        assert!(!temp.path().join("absent.lock.holder").exists());
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
