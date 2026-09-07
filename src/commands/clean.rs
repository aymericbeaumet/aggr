//! Remove disposable local state without fetching sources or touching the append-only archive.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, bail};

use super::{Project, dev::DevLock};
use crate::cli::CleanArgs;
use crate::config::Config;

pub fn run(config_path: &Path, args: &CleanArgs) -> Result<()> {
    run_project(
        &load_project(config_path)?,
        args.out.as_deref(),
        args.dry_run,
    )
}

pub(super) fn load_project(config_path: &Path) -> Result<Project> {
    let config_path = config_path
        .canonicalize()
        .with_context(|| format!("resolving {}", config_path.display()))?;
    let root = config_path
        .parent()
        .context("config file has a parent directory")?
        .to_path_buf();
    // Source documents do not change site/store settings. Cleaning must work offline even when
    // a remote resource is unavailable or a source's credentials have expired.
    let config = Config::parse(&std::fs::read_to_string(&config_path)?)?;
    Ok(Project {
        config,
        sources: Vec::new(),
        config_path,
        repo: crate::git::Repo::discover(&root)?,
        root,
    })
}

pub(super) fn run_project(project: &Project, out: Option<&Path>, dry_run: bool) -> Result<()> {
    let plan = Plan::new(project, out)?;
    let needs_dev_lock = plan.targets.iter().any(|path| path.starts_with(&plan.dev));
    let _lock = if dry_run || !needs_dev_lock {
        DevLock::inspect(&plan.dev, &project.config_path)?
    } else {
        Some(DevLock::acquire(&plan.dev, &project.config_path)?)
    };
    plan.execute(dry_run)
}

pub(super) struct Plan {
    dev: PathBuf,
    targets: BTreeSet<PathBuf>,
}

impl Plan {
    pub(super) fn new(project: &Project, out: Option<&Path>) -> Result<Self> {
        let repo = project.repo.root().canonicalize()?;
        let root = project.root.canonicalize()?;
        let dev = validate_dev_cache(project, &crate::cache::dev(&project.config_path)?)?;
        // The whole repository-local namespace is owned and rebuildable. Removing the parent
        // also clears caches left behind by an older aggr namespace version.
        let build = repo.join(".aggr/cache");
        let output = project.root.join(out.unwrap_or(&project.config.site.out));
        validate_ancestors(&output)?;
        let output = resolve_input(output)?;
        let protected = protected_paths(project, &repo)?;
        let mut candidates = vec![
            build,
            dev.join("data"),
            dev.join("fetch"),
            dev.join("site"),
            dev.join("site.previous"),
            dev.join("tmp"),
        ];
        // The marker is necessary, but not sufficient: even a marked directory cannot be a
        // repository, input directory, archive, or an ancestor of one of those locations.
        validate_target(&output, &repo, &root, &protected)?;
        validate_cache_overlap(&output, &dev, &repo)?;
        if let Ok(marker) = std::fs::symlink_metadata(output.join(".aggr-site")) {
            if marker.file_type().is_symlink() || !marker.is_file() {
                bail!("refusing invalid output marker in {}", output.display());
            }
            validate_output_tree(&output)?;
            candidates.push(output);
        } else if output.exists() {
            println!(
                "keeping {}: no .aggr-site ownership marker",
                output.display()
            );
        }
        validate_dev_lock(&dev)?;
        let mut targets = BTreeSet::new();
        for path in candidates {
            validate_target(&path, &repo, &root, &protected)?;
            match std::fs::symlink_metadata(&path) {
                Ok(metadata) => {
                    if !metadata.is_dir() {
                        bail!("refusing to clean {}: expected a directory", path.display());
                    }
                    validate_tree(&path)?;
                    targets.insert(path);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).with_context(|| format!("inspecting {}", path.display()));
                }
            }
        }
        Ok(Self { dev, targets })
    }

    pub(super) fn execute(self, dry_run: bool) -> Result<()> {
        if self.targets.is_empty() {
            println!("nothing to clean; archived articles are preserved");
        }
        for path in self.targets {
            if dry_run {
                println!("would remove {}", path.display());
            } else {
                validate_ancestors(&path)?;
                validate_tree(&path)?;
                std::fs::remove_dir_all(&path)
                    .with_context(|| format!("removing disposable directory {}", path.display()))?;
                println!("removed {}", path.display());
            }
        }
        if !dry_run {
            println!(
                "archived articles and Git history were not changed; disposable data can be rebuilt"
            );
        }
        Ok(())
    }
}

/// Establish that a normal build may own and replace `out`. This preflight deliberately runs
/// before source synchronization so a bad output setting cannot mutate the archive first.
pub(super) fn validate_output(project: &Project, out: &Path) -> Result<()> {
    let repo = project.repo.root().canonicalize()?;
    let root = project.root.canonicalize()?;
    let dev = canonical_dev_path(&crate::cache::dev(&project.config_path)?)?;
    validate_ancestors(out)?;
    let out = resolve_input(out.to_path_buf())?;
    let protected = protected_paths(project, &repo)?;

    validate_target(&out, &repo, &root, &protected)?;
    validate_cache_overlap(&out, &dev, &repo)?;
    validate_owned_output(&out)
}

/// Validate the persistent project layout before a normal command can create the archive
/// worktree or either repository-owned cache/output namespace.
pub(super) fn validate_project_layout(project: &Project) -> Result<()> {
    let layout = Layout::new(project)?;
    let build = resolve_layout_target(layout.repo.join(".aggr/cache"), "repository build cache")?;
    let output = resolve_input(rooted(&project.root, &project.config.site.out))?;

    if has_git_component(&layout.store) {
        bail!(
            "unsafe project layout: [store] dir {} overlaps Git metadata",
            layout.store.display()
        );
    }
    reject_any_overlap("[store] dir", &layout.store, "config", &layout.configs)?;
    reject_any_overlap("[store] dir", &layout.store, "theme", &layout.themes)?;
    reject_any_overlap("[store] dir", &layout.store, "Git metadata", &layout.git)?;
    reject_any_overlap(
        "[store] dir",
        &layout.store,
        "tracked path",
        &layout.tracked,
    )?;
    reject_overlap("[store] dir", &layout.store, "site output", &output)?;
    reject_overlap(
        "[store] dir",
        &layout.store,
        "repository build cache",
        &build,
    )?;
    reject_any_overlap("repository build cache", &build, "config", &layout.configs)?;
    reject_any_overlap("repository build cache", &build, "theme", &layout.themes)?;
    reject_any_overlap(
        "repository build cache",
        &build,
        "Git metadata",
        &layout.git,
    )?;
    reject_any_overlap(
        "repository build cache",
        &build,
        "tracked path",
        &layout.tracked,
    )?;
    reject_overlap("repository build cache", &build, "site output", &output)
}

/// Resolve and validate the project-specific dev cache before `create_dir_all` or lock creation.
pub(super) fn validate_dev_cache(project: &Project, cache: &Path) -> Result<PathBuf> {
    let layout = Layout::new(project)?;
    let lexical = resolve_layout_target(cache.to_path_buf(), "dev cache")?;
    validate_dev_cache_overlap(&layout, &lexical)?;
    let cache = canonical_dev_path(cache)?;
    validate_dev_cache_overlap(&layout, &cache)?;
    validate_owned_cache_dir(&cache, "dev cache")?;
    Ok(cache)
}

/// Existing cache trees are entirely aggr-owned. A symlink anywhere below the namespace could
/// redirect an otherwise safe cache write or recursive replacement into unrelated user data.
pub(super) fn validate_owned_cache_dir(path: &Path, description: &str) -> Result<()> {
    validate_ancestors(path)?;
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_dir() => {
            bail!(
                "unsafe project layout: {description} {} is not a directory",
                path.display()
            )
        }
        Ok(_) => validate_tree(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("inspecting {}", path.display())),
    }
}

fn validate_dev_cache_overlap(layout: &Layout, cache: &Path) -> Result<()> {
    if has_git_component(cache) {
        bail!(
            "unsafe project layout: dev cache {} overlaps Git metadata",
            cache.display()
        );
    }
    reject_overlap("dev cache", cache, "[store] dir", &layout.store)?;
    reject_any_overlap("dev cache", cache, "config", &layout.configs)?;
    reject_any_overlap("dev cache", cache, "theme", &layout.themes)?;
    reject_any_overlap("dev cache", cache, "Git metadata", &layout.git)?;
    reject_overlap("dev cache", cache, "repository", &layout.repo)?;
    Ok(())
}

struct Layout {
    repo: PathBuf,
    store: PathBuf,
    configs: Vec<PathBuf>,
    themes: Vec<PathBuf>,
    git: Vec<PathBuf>,
    tracked: Vec<PathBuf>,
}

impl Layout {
    fn new(project: &Project) -> Result<Self> {
        let repo = project.repo.root().canonicalize()?;
        let store = resolve_layout_target(rooted(&repo, &project.config.store.dir), "[store] dir")?;

        let mut configs = project.config.loaded_files.clone();
        if configs.is_empty() {
            // The offline clean loader deliberately skips source document expansion.
            protect_local_sources(&project.config_path, &mut BTreeSet::new(), &mut configs)?;
        } else {
            configs.push(project.config_path.clone());
        }
        let configs = resolve_paths(configs)?;

        let mut themes = vec![
            project.root.join("templates"),
            project.root.join("static"),
            Path::new(env!("CARGO_MANIFEST_DIR")).join("themes/default"),
        ];
        if project.config.site.theme != "default" {
            themes.push(rooted(&project.root, Path::new(&project.config.site.theme)));
        }
        let themes = resolve_paths(themes)?;

        let mut git = vec![repo.join(".git")];
        git.extend(git_metadata_paths(&repo)?);
        let git = resolve_paths(git)?;
        let tracked = resolve_paths(tracked_paths(&repo)?)?;

        Ok(Self {
            repo,
            store,
            configs,
            themes,
            git,
            tracked,
        })
    }
}

fn rooted(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn resolve_paths(paths: Vec<PathBuf>) -> Result<Vec<PathBuf>> {
    let paths = paths
        .into_iter()
        .map(resolve_input)
        .collect::<Result<BTreeSet<_>>>()?;
    Ok(paths.into_iter().collect())
}

fn reject_any_overlap(
    subject: &str,
    path: &Path,
    protected_name: &str,
    protected: &[PathBuf],
) -> Result<()> {
    if let Some(other) = protected
        .iter()
        .find(|other| paths_overlap(path, other.as_path()))
    {
        reject_overlap(subject, path, protected_name, other)?;
    }
    Ok(())
}

fn reject_overlap(subject: &str, path: &Path, protected_name: &str, other: &Path) -> Result<()> {
    if paths_overlap(path, other) {
        bail!(
            "unsafe project layout: protected path conflict: {subject} {} overlaps {protected_name} {}",
            path.display(),
            other.display()
        );
    }
    Ok(())
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn has_git_component(path: &Path) -> bool {
    path.components()
        .any(|component| is_git_name(component.as_os_str()))
}

/// Resolve a path exactly as the filesystem would while refusing every existing symlink in the
/// path. Parent components remain supported because archive worktrees may live outside the repo.
fn resolve_layout_target(path: PathBuf, description: &str) -> Result<PathBuf> {
    let mut resolved = PathBuf::new();
    for component in std::path::absolute(&path)?.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                resolved.push(component);
            }
            Component::ParentDir => {
                resolved.pop();
            }
            Component::CurDir => {}
            other => {
                resolved.push(other);
                match std::fs::symlink_metadata(&resolved) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        bail!(
                            "unsafe project layout: refusing {description} through symlink {}",
                            resolved.display()
                        );
                    }
                    Ok(_) => {}
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                        ) => {}
                    Err(error) => {
                        return Err(error)
                            .with_context(|| format!("inspecting {}", resolved.display()));
                    }
                }
            }
        }
    }
    Ok(resolved)
}

fn protected_paths(project: &Project, repo: &Path) -> Result<Vec<PathBuf>> {
    let mut protected = vec![
        repo.join(".git"),
        repo.join(&project.config.store.dir),
        project.config_path.clone(),
        project.root.join("templates"),
        project.root.join("static"),
    ];
    protected.extend(git_metadata_paths(repo)?);
    if project.config.site.theme != "default" {
        protected.push(project.root.join(&project.config.site.theme));
    }
    protected.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("themes/default"));
    protected.extend(project.config.loaded_files.iter().cloned());
    protect_local_sources(&project.config_path, &mut BTreeSet::new(), &mut protected)?;
    protected.extend(tracked_paths(repo)?);
    let protected = protected
        .into_iter()
        .map(resolve_input)
        .collect::<Result<BTreeSet<_>>>()?;
    Ok(protected.into_iter().collect())
}

fn git_metadata_paths(repo: &Path) -> Result<Vec<PathBuf>> {
    ["--absolute-git-dir", "--git-common-dir"]
        .into_iter()
        .map(|query| {
            let output = Command::new("git")
                .args(["rev-parse", query])
                .current_dir(repo)
                .output()
                .with_context(|| format!("locating Git metadata with {query}"))?;
            if !output.status.success() {
                bail!("cannot establish protected Git metadata");
            }
            let value = std::str::from_utf8(&output.stdout)
                .context("Git metadata path is not UTF-8")?
                .trim();
            let path = PathBuf::from(value);
            Ok(if path.is_absolute() {
                path
            } else {
                repo.join(path)
            })
        })
        .collect()
}

fn validate_cache_overlap(output: &Path, dev: &Path, repo: &Path) -> Result<()> {
    let repository_cache = resolve_input(repo.join(".aggr/cache"))?;
    if [dev, repository_cache.as_path()]
        .iter()
        .any(|namespace| namespace.starts_with(output) || output.starts_with(namespace))
    {
        bail!(
            "refusing output overlapping a cache namespace: {}",
            output.display()
        );
    }
    Ok(())
}

fn validate_owned_output(output: &Path) -> Result<()> {
    validate_ancestors(output)?;
    let metadata = match std::fs::symlink_metadata(output) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("inspecting {}", output.display()));
        }
    };
    if !metadata.is_dir() {
        bail!(
            "refusing to replace {}: expected a directory",
            output.display()
        );
    }
    if std::fs::read_dir(output)?.next().is_none() {
        return Ok(());
    }
    let marker = std::fs::symlink_metadata(output.join(".aggr-site")).map_err(|error| {
        anyhow::anyhow!(
            "refusing to clear {}: not an aggr output directory (no .aggr-site marker): {error}",
            output.display()
        )
    })?;
    if marker.file_type().is_symlink() || !marker.is_file() {
        bail!("refusing invalid output marker in {}", output.display());
    }
    validate_output_tree(output)
}

fn validate_target(path: &Path, repo: &Path, root: &Path, protected: &[PathBuf]) -> Result<()> {
    validate_ancestors(path)?;
    let absolute = resolve_input(path.to_path_buf())?;
    if absolute.parent().is_none()
        || absolute
            .components()
            .any(|component| is_git_name(component.as_os_str()))
        || repo.starts_with(&absolute)
        || root.starts_with(&absolute)
        || protected
            .iter()
            .any(|input| input.starts_with(&absolute) || absolute.starts_with(input))
    {
        bail!("refusing to clean protected path {}", path.display());
    }
    Ok(())
}

fn validate_output_tree(path: &Path) -> Result<()> {
    validate_tree(path)?;
    for entry in walkdir::WalkDir::new(path).follow_links(false) {
        let entry = entry?;
        if is_git_name(entry.file_name()) {
            bail!(
                "refusing generated output containing Git metadata: {}",
                entry.path().display()
            );
        }
    }
    Ok(())
}

fn is_git_name(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| name.eq_ignore_ascii_case(".git"))
}

fn resolve_input(path: PathBuf) -> Result<PathBuf> {
    let mut resolved = PathBuf::new();
    for component in std::path::absolute(&path)?.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                resolved.push(component);
            }
            Component::ParentDir => {
                resolved.pop();
            }
            Component::CurDir => {}
            other => {
                resolved.push(other);
                match std::fs::symlink_metadata(&resolved) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        resolved = resolved.canonicalize().with_context(|| {
                            format!("resolving protected input {}", resolved.display())
                        })?;
                    }
                    Ok(_) => {}
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                        ) => {}
                    Err(error) => {
                        return Err(error)
                            .with_context(|| format!("inspecting {}", resolved.display()));
                    }
                }
            }
        }
    }
    Ok(resolved)
}

pub(super) fn validate_dev_lock(cache: &Path) -> Result<()> {
    validate_ancestors(&canonical_dev_path(cache)?.join("dev.lock"))
}

fn canonical_dev_path(path: &Path) -> Result<PathBuf> {
    validate_ancestors(path)?;
    let namespace = path.parent().context("dev cache has a namespace")?;
    let base = namespace
        .parent()
        .context("dev cache has a base directory")?;
    let mut existing = std::path::absolute(base)?;
    let mut missing = Vec::new();
    while !existing.exists() {
        missing.push(
            existing
                .file_name()
                .context("cache base has an existing ancestor")?
                .to_os_string(),
        );
        existing.pop();
    }
    let mut resolved = existing.canonicalize()?;
    for component in missing.into_iter().rev() {
        resolved.push(component);
    }
    resolved.push(
        namespace
            .file_name()
            .context("dev cache namespace has a name")?,
    );
    resolved.push(path.file_name().context("dev cache has a project name")?);
    Ok(resolved)
}

fn validate_ancestors(path: &Path) -> Result<()> {
    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        bail!(
            "refusing cleanup path with parent traversal: {}",
            path.display()
        );
    }
    let mut current = PathBuf::new();
    for component in std::path::absolute(path)?.components() {
        if matches!(component, Component::Prefix(_) | Component::RootDir) {
            current.push(component);
            continue;
        }
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                bail!("refusing cleanup through symlink {}", current.display());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(error).with_context(|| format!("inspecting {}", current.display()));
            }
        }
    }
    Ok(())
}

fn validate_tree(path: &Path) -> Result<()> {
    for entry in walkdir::WalkDir::new(path).follow_links(false) {
        let entry = entry?;
        if entry.file_type().is_symlink() {
            bail!("refusing cleanup of symlink {}", entry.path().display());
        }
    }
    Ok(())
}

fn tracked_paths(repo: &Path) -> Result<Vec<PathBuf>> {
    let output = Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(repo)
        .output()
        .context("listing protected tracked files")?;
    if !output.status.success() {
        bail!("cannot establish protected tracked files before cleanup");
    }
    output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(
            |entry| Ok(repo.join(std::str::from_utf8(entry).context("tracked path is not UTF-8")?)),
        )
        .collect()
}

fn protect_local_sources(
    path: &Path,
    seen: &mut BTreeSet<PathBuf>,
    protected: &mut Vec<PathBuf>,
) -> Result<()> {
    let root_config = seen.is_empty();
    let path = path.canonicalize()?;
    if !seen.insert(path.clone()) {
        return Ok(());
    }
    if seen.len() > 256 {
        bail!("too many local source documents to establish cleanup safety");
    }
    protected.push(path.clone());
    let bytes = std::fs::read(&path)?;
    let sources = if root_config {
        Config::parse(std::str::from_utf8(&bytes)?)?.sources
    } else if bytes.iter().all(u8::is_ascii_whitespace) {
        Vec::new()
    } else {
        let location = url::Url::from_file_path(&path)
            .map_err(|_| anyhow::anyhow!("invalid source document path {}", path.display()))?;
        Config::parse_source_document(&bytes, &location)?
    };
    let root = path
        .parent()
        .context("source document has a parent directory")?;
    for source in sources {
        if source.kind.as_deref() == Some("aggr") {
            continue;
        }
        if let Some(pattern) = source.url {
            // Unavailable credentials must not prevent offline cleanup. A configured local
            // path whose variables are available still needs the same protection as a literal.
            let Ok(pattern) = crate::config::expand_env(&pattern, &|key| std::env::var(key).ok())
            else {
                continue;
            };
            let pattern = match url::Url::parse(&pattern) {
                Ok(url) if url.scheme() == "file" => url
                    .to_file_path()
                    .map_err(|_| anyhow::anyhow!("invalid local source URL {url}"))?,
                Ok(_) => continue,
                Err(_) => root.join(pattern),
            };
            for entry in glob::glob(&pattern.to_string_lossy())? {
                protect_local_sources(&entry?, seen, protected)?;
            }
        }
    }
    Ok(())
}
