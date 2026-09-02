//! `aggr dev`: immediately serve, then sync/build and live-reload without touching the repository.

use anyhow::{Context as _, Result};

use super::{Project, server};
use crate::cli::DevArgs;
use crate::store::Store;

pub async fn run(project: &Project, args: &DevArgs) -> Result<()> {
    let scratch = tempfile::Builder::new().prefix("aggr-dev-").tempdir()?;
    let data = scratch.path().join("data");
    let cache = scratch.path().join("cache");
    let out = scratch.path().join("site");
    std::fs::create_dir_all(&data).context("creating transient dev store")?;
    std::fs::create_dir_all(&cache).context("creating transient dev cache")?;
    Store::open(&data).bootstrap()?;
    let served = server::run_with_reload(
        project,
        args,
        data,
        cache,
        out,
        scratch.path().to_path_buf(),
    )
    .await;
    let cleaned = scratch
        .close()
        .context("removing the transient dev workspace");
    served?;
    cleaned?;
    println!("transient dev data removed");
    Ok(())
}
