//! `aggr dev`: immediately serve, then sync/build and live-reload without touching the repository.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read as _, Seek as _, Write as _};
use std::path::Path;

use anyhow::{Context as _, Result, bail};

use super::{Project, server};
use crate::cli::DevArgs;
use crate::store::Store;

pub async fn run(project: &Project, args: &DevArgs) -> Result<()> {
    let persistent = project.dev_cache_dir()?;
    let _lock = DevLock::acquire(&persistent, &project.config_path)?;
    let data = persistent.join("data");
    let cache = persistent.join("fetch");
    let site = persistent.join("site");
    std::fs::create_dir_all(&data).context("creating cached dev store")?;
    std::fs::create_dir_all(&cache).context("creating cached dev fetch state")?;
    Store::open(&data).bootstrap()?;
    let scratch = tempfile::Builder::new().prefix("aggr-dev-").tempdir()?;
    let staging = scratch.path().join("site");
    server::run_with_reload(project, args, data, cache, site, staging).await
}

/// One writer owns a project's reusable dev snapshot. Concurrent old/new processes otherwise
/// race while atomically promoting builds and can make a freshly rendered page appear stale.
#[derive(Debug)]
struct DevLock {
    _file: File,
}

impl DevLock {
    fn acquire(cache: &Path, config: &Path) -> Result<Self> {
        std::fs::create_dir_all(cache)
            .with_context(|| format!("creating dev cache {}", cache.display()))?;
        let path = cache.join("dev.lock");
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("opening dev lock {}", path.display()))?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                let mut owner = String::new();
                let _ = file.read_to_string(&mut owner);
                let owner = owner.trim();
                let suffix = if owner.is_empty() {
                    String::new()
                } else {
                    format!(" (PID {owner})")
                };
                bail!(
                    "another `aggr dev` is already running for {}{suffix}",
                    config.display()
                );
            }
            Err(TryLockError::Error(err)) => {
                return Err(err).with_context(|| format!("locking {}", path.display()));
            }
        }
        file.set_len(0)?;
        file.rewind()?;
        write!(file, "{}", std::process::id())?;
        file.flush()?;
        Ok(Self { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_dev_process_owns_a_project_cache() {
        let dir = tempfile::tempdir().unwrap();
        let first = DevLock::acquire(dir.path(), Path::new("aggr.toml")).unwrap();
        let error = DevLock::acquire(dir.path(), Path::new("aggr.toml")).unwrap_err();
        assert!(
            format!("{error:#}").contains("another `aggr dev` is already running"),
            "{error:#}"
        );
        drop(first);
        DevLock::acquire(dir.path(), Path::new("aggr.toml")).unwrap();
    }
}
