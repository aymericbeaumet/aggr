//! `aggr dev`: immediately serve, then sync/build and live-reload without touching the repository.

use std::path::Path;

use anyhow::{Context as _, Result};

use super::lock::Guard;
use super::{Project, server};
use crate::cli::DevArgs;
use crate::store::Store;

pub async fn run(config_path: &Path, args: &DevArgs) -> Result<()> {
    let project = if args.clean {
        super::clean::load_project(config_path)?
    } else {
        Project::load_offline(config_path).await?
    };
    if args.clean {
        project.validate_layout()?;
    }
    let cleanup = args
        .clean
        .then(|| super::clean::Plan::new(&project, None))
        .transpose()?;
    let persistent = project.dev_cache_dir()?;
    let _lock = DevLock::acquire(&persistent, &project.config_path)?;
    if let Some(cleanup) = cleanup {
        cleanup.execute(false)?;
    }
    let project = if args.clean {
        Project::load_offline(config_path).await?
    } else {
        project
    };
    let data = persistent.join("data");
    let cache = persistent.join("fetch");
    let site = persistent.join("site");
    std::fs::create_dir_all(&data).context("creating cached dev store")?;
    std::fs::create_dir_all(&cache).context("creating cached dev fetch state")?;
    Store::open(&data).bootstrap()?;
    let scratch = dev_scratch(&persistent)?;
    let staging = scratch.join("site");
    server::run_with_reload(project, args, data, cache, site, staging).await
}

fn dev_scratch(cache: &Path) -> Result<std::path::PathBuf> {
    let root = cache.join("tmp");
    super::clean::validate_owned_cache_dir(&root, "dev scratch directory")?;
    if root.exists() {
        std::fs::remove_dir_all(&root).context("clearing stale dev scratch directory")?;
    }
    std::fs::create_dir_all(&root).context("creating dev scratch directory")?;
    Ok(root)
}

/// One writer owns a project's reusable dev snapshot. Concurrent old/new processes otherwise
/// race while atomically promoting builds and can make a freshly rendered page appear stale.
#[derive(Debug)]
pub(super) struct DevLock {
    _guard: Guard,
}

impl DevLock {
    pub(super) fn acquire(cache: &Path, config: &Path) -> Result<Self> {
        super::clean::validate_dev_lock(cache)?;
        std::fs::create_dir_all(cache)
            .with_context(|| format!("creating dev cache {}", cache.display()))?;
        Ok(Self {
            _guard: Guard::acquire(&cache.join("dev.lock"), "dev", config)?,
        })
    }

    pub(super) fn inspect(cache: &Path, config: &Path) -> Result<Option<Self>> {
        super::clean::validate_dev_lock(cache)?;
        Ok(Guard::inspect(&cache.join("dev.lock"), "dev", config)?
            .map(|guard| Self { _guard: guard }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_dev_process_owns_a_project_cache() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().canonicalize().unwrap().join("dev-v1/project");
        let first = DevLock::acquire(&dir, Path::new("aggr.toml")).unwrap();
        let error = DevLock::acquire(&dir, Path::new("aggr.toml")).unwrap_err();
        assert!(
            format!("{error:#}").contains("another `aggr dev` is already running"),
            "{error:#}"
        );
        drop(first);
        DevLock::acquire(&dir, Path::new("aggr.toml")).unwrap();
    }

    #[test]
    fn scratch_workspace_shares_the_dev_cache_filesystem() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().canonicalize().unwrap().join("dev-v1/project");
        std::fs::create_dir_all(cache.join("tmp")).unwrap();
        std::fs::write(cache.join("tmp/stale"), "old").unwrap();
        let scratch = dev_scratch(&cache).unwrap();
        assert_eq!(scratch, cache.join("tmp"));
        assert!(!scratch.join("stale").exists());
    }
}
