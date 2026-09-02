//! `aggr dev`: refresh locally without a commit, build, then rebuild/live-reload project changes.

use anyhow::{Result, bail};

use super::{Project, build, fetch, server};
use crate::cli::DevArgs;

pub async fn run(project: &Project, args: &DevArgs) -> Result<()> {
    let worktree = project.worktree()?;
    let report = fetch::run(project, &worktree, &args.fetch).await?;
    if report.all_failed() {
        bail!("every source failed");
    }
    println!(
        "dev data: {} new item(s) in the local worktree (not committed or pushed)",
        report.added()
    );
    build::run(project, &args.build)?;
    server::run_with_reload(project, args).await
}
