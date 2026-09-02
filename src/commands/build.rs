//! `aggr build`: render the site from the data worktree (or any ref of the data branch).
//! Plain builds are served from `/`; `--release` builds for the public URL and writes the CNAME.

use std::path::PathBuf;

use anyhow::{Result, bail};
use chrono::Utc;

use super::Project;
use crate::cli::BuildArgs;
use crate::site::{self, BuildInfo, Summary};
use crate::store::Store;

pub fn run(project: &Project, args: &BuildArgs) -> Result<Summary> {
    let worktree = project.worktree()?;
    let out = out_dir(project, args);
    let base_url = base_url(project, args)?;
    // A build artifact, never something to commit on `main`.
    project.repo.exclude(&out)?;

    let checkout = args
        .data_ref
        .as_deref()
        .map(|rev| worktree.temp_checkout(rev))
        .transpose()?;
    let (data_dir, data_sha) = match &checkout {
        Some(checkout) => (
            checkout.dir().to_path_buf(),
            Some(checkout.sha().to_string()),
        ),
        None => (worktree.dir().to_path_buf(), worktree.head_sha()?),
    };

    let info = BuildInfo {
        out: out.clone(),
        base_url,
        config_sha: project.config_sha(),
        data_sha,
        now: Utc::now(),
        release: args.release,
    };
    let summary = site::build(
        &project.config,
        &project.sources,
        &Store::open(&data_dir),
        &project.root,
        &info,
    )?;
    println!(
        "built {} page(s), {} item(s), {} stub(s) into {}{}",
        summary.pages,
        summary.items,
        summary.stubs,
        out.display(),
        info.base_url
            .as_deref()
            .map(|url| format!(" for {url}"))
            .unwrap_or_default()
    );
    Ok(summary)
}

/// `--out`, else `[site] out`, both relative to the directory holding `aggr.toml`.
pub fn out_dir(project: &Project, args: &BuildArgs) -> PathBuf {
    let out = args
        .out
        .clone()
        .unwrap_or_else(|| project.config.site.out.clone());
    if out.is_absolute() {
        out
    } else {
        project.root.join(out)
    }
}

/// Where the site will live: `--base-url` always wins; `--release` falls back to `[site] url`
/// and refuses to guess; plain builds have no base URL and are served from `/`.
pub fn base_url(project: &Project, args: &BuildArgs) -> Result<Option<String>> {
    if let Some(url) = non_empty(args.base_url.as_deref()) {
        return Ok(Some(url.to_string()));
    }
    if !args.release {
        return Ok(None);
    }
    match &project.config.site.url {
        Some(url) => Ok(Some(url.to_string())),
        None => {
            bail!("--release needs a public URL: set `[site] url` in aggr.toml or pass --base-url")
        }
    }
}

/// `AGGR_BASE_URL=` (a skipped workflow step) must mean "not set", not "the empty URL".
pub fn non_empty(url: Option<&str>) -> Option<&str> {
    url.map(str::trim).filter(|url| !url.is_empty())
}
