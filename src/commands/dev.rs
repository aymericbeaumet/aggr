//! `aggr dev`: immediately serve, then sync/build and live-reload without touching the repository.

use anyhow::{Context as _, Result};

use super::{Project, server};
use crate::cli::DevArgs;
use crate::store::Store;

pub async fn run(project: &Project, args: &DevArgs) -> Result<()> {
    let persistent = project.dev_cache_dir()?;
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
