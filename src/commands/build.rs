//! `aggr build`: render the site from the data worktree (or any ref of the data branch).
//! Plain builds are served from `/`; `--release` builds for the public URL and writes the CNAME.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use chrono::Utc;

use super::Project;
use crate::cli::BuildArgs;

/// Public `aggr build`: rendered output always follows a completed sync.
pub async fn sync_and_run(project: &Project, args: &BuildArgs) -> Result<()> {
    super::sync::run(project, &crate::cli::FetchArgs::default()).await?;
    run(project, args).await.map(|_| ())
}
use crate::site::{self, BuildInfo, Summary};
use crate::store::Store;

pub async fn run(project: &Project, args: &BuildArgs) -> Result<Summary> {
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
    let config_sha = project.config_sha();
    let cache_dir = project.build_cache_dir()?;
    let store = Store::open(&data_dir);
    let now = Utc::now();
    let discussions = resolve_discussions(project, &store, &cache_dir, now).await?;
    let discussions_fingerprint = discussions.fingerprint();
    let fingerprint = crate::cache::render_fingerprint(
        &project.config,
        &project.root,
        config_sha.as_deref(),
        data_sha.as_deref(),
        base_url.as_deref(),
        args.release,
        Some(&discussions_fingerprint),
    )?;
    if let Some(summary) = crate::cache::restore_render(&cache_dir, &fingerprint, &out)? {
        print_summary(summary, &out, base_url.as_deref(), true);
        return Ok(summary);
    }

    let scratch = tempfile::Builder::new()
        .prefix("aggr-build-")
        .tempdir_in(&cache_dir)
        .context("creating render cache staging directory")?;
    let rendered = scratch.path().join("site");
    let info = BuildInfo {
        out: rendered.clone(),
        base_url: base_url.clone(),
        config_sha,
        config_path: project.config_repo_path(),
        data_sha,
        now,
        release: args.release,
        discussions,
    };
    let summary = site::build(
        &project.config,
        &project.sources,
        &store,
        &project.root,
        &info,
    )?;
    crate::cache::store_render(&cache_dir, &fingerprint, &rendered, summary)?;
    crate::cache::restore_render(&cache_dir, &fingerprint, &out)?
        .context("the rendered site disappeared from its cache")?;
    print_summary(summary, &out, base_url.as_deref(), false);
    Ok(summary)
}

fn print_summary(summary: Summary, out: &Path, base_url: Option<&str>, cached: bool) {
    println!(
        "built {} page(s), {} item(s), {} stub(s) {} {}{}",
        summary.pages,
        summary.items,
        summary.stubs,
        if cached { "from cache into" } else { "into" },
        out.display(),
        base_url
            .map(|url| format!(" for {url}"))
            .unwrap_or_default()
    );
}

/// Render dev's isolated store into staging before the server swaps its in-memory snapshot.
pub fn run_ephemeral(
    project: &Project,
    args: &BuildArgs,
    data_dir: &Path,
    out: &Path,
    discussions: crate::discussions::ResolutionSet,
) -> Result<Summary> {
    let store = Store::open(data_dir);
    let now = Utc::now();
    let info = BuildInfo {
        out: out.to_path_buf(),
        base_url: base_url(project, args)?,
        config_sha: project.config_sha(),
        config_path: project.config_repo_path(),
        data_sha: None,
        now,
        release: args.release,
        discussions,
    };
    let summary = site::build(
        &project.config,
        &project.sources,
        &store,
        &project.root,
        &info,
    )?;
    println!(
        "built {} page(s), {} item(s), {} stub(s) for the in-memory dev snapshot",
        summary.pages, summary.items, summary.stubs
    );
    Ok(summary)
}

pub async fn resolve_discussions(
    project: &Project,
    store: &Store,
    cache_dir: &Path,
    now: chrono::DateTime<Utc>,
) -> Result<crate::discussions::ResolutionSet> {
    let items = store.items()?;
    let client = Arc::new(crate::http::Client::new(&project.config.fetch)?);
    crate::discussions::resolve(
        &project.config.site.discussions,
        &items,
        client,
        cache_dir,
        now,
    )
    .await
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
