//! `aggr dev`: refresh data once, build once, then rebuild and live-reload on project changes.

use anyhow::Result;

use super::{Project, build, server, sync};
use crate::cli::DevArgs;

pub async fn run(project: &Project, args: &DevArgs) -> Result<()> {
    sync::run(project, &args.fetch).await?;
    build::run(project, &args.build)?;
    server::run_with_reload(project, args).await
}
