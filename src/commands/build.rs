//! `aggr build`: sync and render live data, or render a pinned data ref without a source sync.
//! Plain builds are served from `/`; `--release` builds for the public URL and writes the CNAME.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use chrono::Utc;

use super::Project;
use crate::cli::BuildArgs;
use crate::discussions::ResolutionSet;
use crate::site::{self, BuildInfo, Summary};
use crate::store::Store;

/// Public `aggr build`: live data is synced first, while a pinned ref is rendered as-is.
/// `--clean` is handled by the dispatcher before the project is loaded.
pub async fn sync_and_run(project: &Project, args: &BuildArgs) -> Result<()> {
    // Validate ownership before sync can create a worktree, write state, or fetch anything.
    // Site rendering and cache restoration both replace the output recursively.
    super::clean::validate_output(project, &out_dir(project, args))?;
    validate_report(project, args)?;
    let started = std::time::Instant::now();
    let discussions = if !args.is_offline() {
        super::sync::run(project, &crate::cli::SyncArgs::default()).await?
    } else {
        None
    };
    let sync_seconds = started.elapsed().as_secs_f64();
    run_prevalidated(project, args, discussions, sync_seconds)
        .await
        .map(|_| ())
}

/// `resolved` carries the discussion matches the preceding sync already computed for this data;
/// without it (a sync stage that was skipped or failed to match) they are resolved here.
async fn run_prevalidated(
    project: &Project,
    args: &BuildArgs,
    resolved: Option<ResolutionSet>,
    sync_seconds: f64,
) -> Result<Summary> {
    let started = std::time::Instant::now();
    let out = out_dir(project, args);
    let base_url = base_url(project, args)?;
    // A build artifact, never something to commit on `main`.
    project.repo.exclude(&out)?;

    let local_sha = if args.is_offline() {
        let reference = args
            .data_ref
            .clone()
            .unwrap_or_else(|| format!("refs/heads/{}", project.config.store.branch));
        Some(match &project.local_data_sha {
            Some(sha) => sha.clone(),
            None => project.repo.local_commit(&reference)?,
        })
    } else {
        None
    };
    let include_media = project.config.defaults.media.images().archives()
        || project.config.defaults.media.previews().archives()
        || project.config.defaults.media.documents().archives()
        || project.sources.iter().any(|source| {
            source.images.archives() || source.previews.archives() || source.documents.archives()
        });
    let checkout = local_sha
        .as_deref()
        .map(|sha| project.repo.local_snapshot(sha, include_media))
        .transpose()?;
    let (data_dir, data_sha) = if let Some(checkout) = &checkout {
        (checkout.dir().to_path_buf(), local_sha.clone())
    } else {
        let worktree = project.worktree()?;
        (worktree.dir().to_path_buf(), worktree.head_sha()?)
    };
    let archive_seconds = started.elapsed().as_secs_f64();
    let config_sha = project.config_sha();
    let cache_dir = project.build_cache_dir()?;
    let mut store = Store::open(&data_dir).with_image_cache(&cache_dir);
    if let Some(snapshot) = &checkout {
        store = store.with_archive_sizes(snapshot.sizes());
    }
    let now = super::offline::clock(&project.repo, local_sha.as_deref())?;
    let generation = site::render_generation(
        &store.retained_items(project.config.defaults.limits, &project.sources, now)?,
        &project.config.site,
        now,
    );
    let discussions = match resolved {
        Some(resolved) => resolved,
        None => super::offline::discussions(&data_dir)?,
    };
    let discussions_fingerprint = discussions.fingerprint();
    let fingerprint = crate::cache::render_fingerprint(crate::cache::RenderFingerprint {
        config: &project.config,
        theme: &site::theme(),
        repo_root: project.repo.root(),
        config_sha: config_sha.as_deref(),
        data_sha: data_sha.as_deref(),
        base_url: base_url.as_deref(),
        release: args.release,
        discussions: Some(&discussions_fingerprint),
        generation: &generation,
    })?;
    let fingerprint = crate::model::sha1_hex(format!(
        "{fingerprint}:{}:{:?}",
        args.hermetic,
        (args.is_offline() || std::env::var_os("SOURCE_DATE_EPOCH").is_some())
            .then_some(now.timestamp())
    ));
    let metrics = Arc::new(std::sync::Mutex::new(site::BuildMetrics::default()));
    // Ephemeral runners never restore full rendered trees, so retaining another one wastes disk.
    let retain_render = std::env::var_os("GITHUB_ACTIONS").is_none();
    if !args.hermetic
        && retain_render
        && let Some(summary) = crate::cache::restore_render(&cache_dir, &fingerprint, &out)?
    {
        site::verify_output_budget(&out, project.config.site.build_max_bytes)?;
        print_summary(summary, &out, base_url.as_deref(), true);
        write_report(
            project,
            args,
            &out,
            &data_sha,
            true,
            sync_seconds,
            archive_seconds,
            started.elapsed().as_secs_f64(),
            &metrics,
        )?;
        trim_cache(&cache_dir, &project.config.cache);
        return Ok(summary);
    }

    let scratch = tempfile::Builder::new()
        .prefix("aggr-build-")
        .tempdir_in(&cache_dir)
        .context("creating render cache staging directory")?;
    let rendered = scratch.path().join("site");
    let info = BuildInfo {
        hermetic: args.hermetic,
        metrics: metrics.clone(),
        out: rendered.clone(),
        base_url: base_url.clone(),
        config_sha,
        config_path: project.config_repo_path(),
        data_sha: data_sha.clone(),
        generation,
        now,
        release: args.release,
        discussions,
        development: false,
        render_cache_key: Some(fingerprint.clone()),
        pagefind_cache: Some(cache_dir.clone()),
    };
    let summary = site::build(
        &project.config,
        &project.sources,
        &store,
        &project.root,
        &info,
    )?;
    if args.hermetic {
        site::hermetic::verify(&rendered, base_url.as_deref())?;
    }
    if retain_render && !args.hermetic {
        crate::cache::store_render(&cache_dir, &fingerprint, &rendered, summary)?;
        crate::cache::restore_render(&cache_dir, &fingerprint, &out)?
            .context("the rendered site disappeared from its cache")?;
    } else {
        site::promote_output(&rendered, &out)?;
    }
    site::verify_output_budget(&out, project.config.site.build_max_bytes)?;
    print_summary(summary, &out, base_url.as_deref(), false);
    write_report(
        project,
        args,
        &out,
        &data_sha,
        false,
        sync_seconds,
        archive_seconds,
        started.elapsed().as_secs_f64(),
        &metrics,
    )?;
    trim_cache(&cache_dir, &project.config.cache);
    Ok(summary)
}

fn trim_cache(cache_dir: &Path, config: &crate::config::CacheConfig) {
    if let Err(err) = crate::cache::enforce_limits(cache_dir, config) {
        log::warn!("cache limit cleanup skipped: {err:#}");
    }
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

fn report_path(project: &Project, args: &BuildArgs) -> Option<PathBuf> {
    args.report.as_ref().map(|path| project.root.join(path))
}

fn validate_report(project: &Project, args: &BuildArgs) -> Result<()> {
    let Some(path) = report_path(project, args) else {
        return Ok(());
    };
    if std::fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        bail!("build report must not replace a symlink");
    }
    if path.components().any(|part| {
        part == std::path::Component::ParentDir
            || part
                .as_os_str()
                .to_str()
                .is_some_and(|part| part.eq_ignore_ascii_case(".git"))
    }) {
        bail!("build report must be outside generated output, Git metadata, and the archive");
    }
    // Resolve existing ancestors so system aliases such as macOS /tmp work, while an alias
    // into the archive or output receives the same ownership checks as its real path.
    let path = physical_path(&path)?;
    if path.components().any(|part| {
        part.as_os_str()
            .to_str()
            .is_some_and(|part| part.eq_ignore_ascii_case(".git"))
    }) || path.starts_with(physical_path(&out_dir(project, args))?)
        || path.starts_with(physical_path(&project.repo.root().join(".aggr"))?)
        || path.starts_with(physical_path(
            &project.root.join(&project.config.store.dir),
        )?)
    {
        bail!("build report must be outside generated output, Git metadata, and the archive");
    }
    if path.exists() {
        let previous: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)
            .context("refusing to overwrite a file that is not a build report")?;
        if previous.get("report_kind").and_then(|value| value.as_str()) != Some("aggr-build") {
            bail!("refusing to overwrite a file that is not a build report");
        }
    }
    Ok(())
}

fn physical_path(path: &Path) -> Result<PathBuf> {
    match path.canonicalize() {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path.parent().context("build report path has no parent")?;
            let name = path
                .file_name()
                .context("build report path has no filename")?;
            Ok(physical_path(parent)?.join(name))
        }
        Err(error) => Err(error).context("resolving build report path"),
    }
}

#[allow(clippy::too_many_arguments)]
fn write_report(
    project: &Project,
    args: &BuildArgs,
    out: &Path,
    data_sha: &Option<String>,
    cached: bool,
    sync_seconds: f64,
    archive_seconds: f64,
    render_seconds: f64,
    metrics: &std::sync::Mutex<site::BuildMetrics>,
) -> Result<()> {
    let Some(path) = report_path(project, args) else {
        return Ok(());
    };
    let output_bytes = walkdir::WalkDir::new(out)
        .into_iter()
        .try_fold(0_u64, |bytes, entry| {
            let entry = entry?;
            Ok::<_, anyhow::Error>(
                bytes
                    + if entry.file_type().is_file() {
                        entry.metadata()?.len()
                    } else {
                        0
                    },
            )
        })?;
    let metrics = metrics.lock().unwrap_or_else(|error| error.into_inner());
    let report = serde_json::json!({
        "schema_version": 1, "report_kind": "aggr-build", "version": env!("CARGO_PKG_VERSION"),
        "archive_sha": data_sha, "offline": args.is_offline(), "hermetic": args.hermetic,
        "media_cache": site::compressed_media::cache_statistics(),
        "render_cache_hit": cached, "sync_seconds": sync_seconds,
        "archive_seconds": archive_seconds, "render_seconds": (render_seconds - archive_seconds).max(0.0),
        "total_seconds": sync_seconds + render_seconds, "output_bytes": output_bytes,
        "render_passes": metrics.passes, "retry_count": metrics.passes.len().saturating_sub(1),
    });
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_vec_pretty(&report)?)?;
    project.repo.exclude(&path)?;
    Ok(())
}

/// Render dev's isolated store into staging before the server swaps its in-memory snapshot.
pub fn run_ephemeral(
    project: &Project,
    args: &BuildArgs,
    data_dir: &Path,
    out: &Path,
    cache_dir: &Path,
    discussions: ResolutionSet,
) -> Result<Summary> {
    let store = Store::open(data_dir).with_image_cache(cache_dir);
    let now = Utc::now();
    let generation = site::render_generation(
        &store.retained_items(project.config.defaults.limits, &project.sources, now)?,
        &project.config.site,
        now,
    );
    let info = BuildInfo {
        hermetic: false,
        metrics: Default::default(),
        out: out.to_path_buf(),
        base_url: base_url(project, args)?,
        config_sha: project.config_sha(),
        config_path: project.config_repo_path(),
        data_sha: None,
        generation,
        now,
        release: args.release,
        discussions,
        development: true,
        render_cache_key: None,
        pagefind_cache: Some(cache_dir.to_path_buf()),
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
) -> Result<ResolutionSet> {
    // Mirrors the provider check inside `discussions::resolve`: without a lookup provider there
    // is nothing to match, so the archive is not parsed only to be discarded.
    if project
        .config
        .networks
        .iter()
        .all(|network| network.provider.is_none())
    {
        return Ok(ResolutionSet::default());
    }
    let items = store.items()?;
    let client = Arc::new(crate::http::Client::new(&project.config.fetch)?);
    crate::discussions::resolve(&project.config.networks, &items, client, cache_dir, now).await
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
