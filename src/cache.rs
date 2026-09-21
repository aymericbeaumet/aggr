//! Cache locations and namespaces. Build/CI state belongs to the repository; dev state belongs
//! to the operating system's standard cache directory and is isolated per configuration file.

use std::collections::BTreeSet;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use chrono::{DateTime, Utc};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use sha1::{Digest as _, Sha1};
use url::Url;

const BUILD_NAMESPACE: &str = "build-v1";
const DEV_NAMESPACE: &str = "dev-v1";
pub(crate) const RENDER_KEY_FILE: &str = ".aggr-build-key";

/// Every directory aggr creates under a build or dev cache root. The reusable workflow persists
/// the derived-state subset between runners, so a namespace's name is an on-disk contract: rename
/// one only together with a format change, never for tidiness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Namespace {
    /// Raw original-page responses and their extractions. Private to the machine that fetched
    /// them: never uploaded to a shared cache.
    Articles,
    /// The last rendered site, keyed by [`render_fingerprint`].
    Render,
    /// Discussion lookups with timestamped backoff.
    Discussions,
    /// The Pagefind index keyed by its input fingerprint.
    Pagefind,
    /// Validation receipts for retained images, keyed by every input byte.
    ValidatedImages,
    /// Checksummed deployment image projections; full-quality masters remain in the archive.
    DeploymentMedia,
    /// Backoff markers for image downloads that failed, per media implementation generation.
    ImageFailures,
    /// Backoff markers for feed-only captures awaiting their original page.
    CaptureRetries,
    /// Backoff markers for recording-duration probes.
    RecordingDuration,
    /// Feed parser receipts that gate conditional GET after a parser change.
    FeedParsing,
}

impl Namespace {
    /// Test-only: unit tests pin the reusable workflow's cache list to this registry.
    #[cfg(test)]
    pub const ALL: [Namespace; 10] = [
        Namespace::Articles,
        Namespace::Render,
        Namespace::Discussions,
        Namespace::Pagefind,
        Namespace::ValidatedImages,
        Namespace::DeploymentMedia,
        Namespace::ImageFailures,
        Namespace::CaptureRetries,
        Namespace::RecordingDuration,
        Namespace::FeedParsing,
    ];

    pub fn dir_name(&self) -> &'static str {
        match self {
            Namespace::Articles => "articles-v1",
            Namespace::Render => "render-v1",
            Namespace::Discussions => "discussions-v1",
            Namespace::Pagefind => "pagefind-v1",
            Namespace::ValidatedImages => "validated-images-v2",
            Namespace::DeploymentMedia => "deployment-media-v1",
            Namespace::ImageFailures => "image-failures-v1",
            Namespace::CaptureRetries => "capture-retries-v1",
            Namespace::RecordingDuration => "recording-duration-v1",
            Namespace::FeedParsing => "feed-parsing",
        }
    }

    pub fn dir(&self, root: &Path) -> PathBuf {
        root.join(self.dir_name())
    }

    /// Whether the reusable workflow may persist this namespace between runners. Derived state
    /// about already-published bytes is safe and self-invalidating; raw upstream responses are
    /// private, and the rendered site is too large and too short-lived to be worth uploading.
    #[cfg(test)]
    pub fn ci_cached(&self) -> bool {
        !matches!(self, Namespace::Articles | Namespace::Render)
    }
}

/// Repository-relative directories the reusable workflow restores and saves, in registry order.
/// Test-only: `.github/workflows/aggr.yml` is checked against this list.
#[cfg(test)]
pub fn ci_cached_paths() -> Vec<String> {
    Namespace::ALL
        .iter()
        .filter(|namespace| namespace.ci_cached())
        .map(|namespace| format!(".aggr/cache/{BUILD_NAMESPACE}/{}", namespace.dir_name()))
        .collect()
}

/// Bump when article extraction semantics change. Raw responses remain reusable across bumps.
const EXTRACTOR_VERSION: &str = "dom-smoothie-0.18-aggr-14";
const MAX_ARTICLE_METADATA_BYTES: usize = 64 * 1024;
pub(crate) const MAX_ARTICLE_BODY_BYTES: usize = 16 * 1024 * 1024;
const MAX_EXTRACTED_ARTICLE_BYTES: usize = 16 * 1024 * 1024;

/// Persistent cache used by `aggr build` and `aggr sync`, safely scoped to one repository.
pub fn build(repo_root: &Path) -> PathBuf {
    repo_root.join(".aggr").join("cache").join(BUILD_NAMESPACE)
}

/// Persistent cache used by `aggr dev`, safely scoped to one canonical configuration path.
pub fn dev(config_path: &Path) -> Result<PathBuf> {
    let base = match std::env::var_os("AGGR_CACHE_DIR") {
        Some(path) => PathBuf::from(path),
        None => ProjectDirs::from("", "", "aggr")
            .context("finding the operating system cache directory")?
            .cache_dir()
            .to_path_buf(),
    };
    Ok(dev_under(&base, config_path))
}

/// Exact key for reusable rendered output. The data commit, effective config files, layered theme,
/// output URL and embedded defaults all participate; wall-clock time deliberately does not.
pub struct RenderFingerprint<'a> {
    pub config: &'a crate::config::Config,
    pub project_root: &'a Path,
    /// Loaded config files are named relative to this root so a checkout elsewhere (another
    /// clone, a CI runner) shares the fingerprint of the same bytes.
    pub repo_root: &'a Path,
    pub config_sha: Option<&'a str>,
    pub data_sha: Option<&'a str>,
    pub base_url: Option<&'a str>,
    pub release: bool,
    pub discussions: Option<&'a str>,
    pub generation: &'a str,
}

pub fn render_fingerprint(input: RenderFingerprint<'_>) -> Result<String> {
    let RenderFingerprint {
        config,
        project_root,
        repo_root,
        config_sha,
        data_sha,
        base_url,
        release,
        discussions,
        generation,
    } = input;
    let mut hash = Sha1::new();
    hash_field(&mut hash, b"schema", b"render-v4");
    hash_field(&mut hash, b"version", env!("CARGO_PKG_VERSION").as_bytes());
    // Development builds often keep the package version while renderer code changes. Hash the
    // implementation itself so a prior binary can never make a new binary restore stale HTML.
    for (name, _, source) in render_implementation_sources() {
        hash_field(&mut hash, name.as_bytes(), source.as_bytes());
    }
    hash_field(
        &mut hash,
        b"embedded-theme",
        crate::site::render::default_theme_hash().as_bytes(),
    );
    hash_field(
        &mut hash,
        b"config-sha",
        config_sha.unwrap_or("").as_bytes(),
    );
    hash_field(&mut hash, b"data-sha", data_sha.unwrap_or("").as_bytes());
    hash_field(&mut hash, b"base-url", base_url.unwrap_or("").as_bytes());
    hash_field(&mut hash, b"release", if release { b"1" } else { b"0" });
    hash_field(
        &mut hash,
        b"discussions",
        discussions.unwrap_or("").as_bytes(),
    );
    hash_field(&mut hash, b"generation", generation.as_bytes());
    hash_field(
        &mut hash,
        b"repository",
        config.repository().unwrap_or_default().as_bytes(),
    );

    let mut config_files = config.loaded_files.clone();
    config_files.sort();
    for path in config_files {
        hash_file(
            &mut hash,
            b"config",
            &path,
            &portable_name(&path, repo_root),
        )?;
    }
    let mut remote_configs = config.loaded_remote.clone();
    remote_configs.sort();
    for (identity, digest) in remote_configs {
        hash_field(&mut hash, identity.as_bytes(), digest.as_bytes());
    }
    for (index, dir) in crate::site::theme_layers(config, project_root)?
        .dirs
        .iter()
        .enumerate()
    {
        // A layer can be the project root itself (a project that overrides `templates/`), and
        // the renderer only ever reads these two subdirectories of a layer. Hashing the whole
        // layer would walk the repository, its caches and the previous output on every build.
        for kind in LAYER_KINDS {
            hash_tree(&mut hash, &format!("layer-{index}/{kind}"), &dir.join(kind))?;
        }
    }
    Ok(hex::encode(hash.finalize()))
}

/// The subdirectories of a theme layer the renderer reads (`Layers::read` and `Layers::names`).
const LAYER_KINDS: [&str; 2] = ["templates", "static"];

/// Directory names a tree hash skips wherever it meets them: the repository, aggr's own state and
/// a previous build output are never rendered, and walking them would read every cached file.
const UNHASHED_DIRS: [&str; 3] = [".git", ".aggr", "_site"];

/// `(hash label, path relative to `src/`, contents)` for every module whose code can change
/// rendered bytes. A unit test walks `src/` and fails on any file that is neither listed here nor
/// in [`RENDER_INDEPENDENT_SOURCES`], so a new module must be classified before it ships.
fn render_implementation_sources() -> &'static [(&'static str, &'static str, &'static str)] {
    macro_rules! source {
        ($name:literal, $path:literal) => {
            ($name, $path, include_str!($path))
        };
    }
    &[
        source!("config", "config.rs"),
        source!("language", "config/language.rs"),
        source!("preferences", "config/preferences.rs"),
        source!("repository-url", "config/repository_url.rs"),
        source!("source-entries", "config/source_entries.rs"),
        source!("source-formats", "config/import_formats.rs"),
        source!("source-graph", "config/import_graph.rs"),
        source!("defaults", "../config.default.toml"),
        source!("content", "content.rs"),
        source!("content-access", "content/access.rs"),
        source!("content-aggregator", "content/aggregator.rs"),
        source!("content-cleanup", "content/cleanup.rs"),
        source!("content-extract", "content/extract.rs"),
        source!("content-markdown", "content/markdown.rs"),
        source!("content-module", "content/module.rs"),
        source!("content-normalize", "content/normalize.rs"),
        source!("content-math", "content/math.rs"),
        source!("content-render", "content/render.rs"),
        source!("content-resources", "content/resources.rs"),
        source!("content-scan", "content/scan.rs"),
        source!("content-strip", "content/strip.rs"),
        source!("content-highlight", "content_highlight.rs"),
        source!("discussions", "discussions.rs"),
        source!("media", "media.rs"),
        source!("platform", "platform.rs"),
        source!("media-stored-cache", "media/stored_cache.rs"),
        source!("media-placeholder", "media/placeholder.rs"),
        source!("media-srcset", "media/srcset.rs"),
        source!("media-vector", "media/vector.rs"),
        source!("media-duration", "media_duration.rs"),
        source!("model", "model.rs"),
        source!("youtube", "sources/youtube.rs"),
        source!("preview", "preview.rs"),
        source!("pdf-preview", "preview/pdf.rs"),
        source!("store", "store/mod.rs"),
        source!("frontmatter", "store/frontmatter.rs"),
        source!("site", "site/mod.rs"),
        source!("source-index", "site/source_index.rs"),
        source!("assets", "site/assets.rs"),
        source!("budget", "site/budget.rs"),
        source!("compressed-media", "site/compressed_media.rs"),
        source!("page", "site/page.rs"),
        source!("directory", "site/directory.rs"),
        source!("output-dir", "site/output_dir.rs"),
        source!("context", "site/context.rs"),
        source!("display", "site/display.rs"),
        source!("document", "site/document.rs"),
        source!("document-storage", "document.rs"),
        source!("interactive", "site/interactive.rs"),
        source!("native-media", "site/native_media.rs"),
        source!("item-type", "site/item_type.rs"),
        source!("outputs", "site/outputs.rs"),
        source!("pagefind", "site/pagefind.rs"),
        source!("parallel", "site/parallel.rs"),
        source!("related", "site/related.rs"),
        source!("render", "site/render.rs"),
        source!("video", "site/video.rs"),
        source!("threads", "threads.rs"),
        source!("thread-embeds", "threads/embeds.rs"),
        source!("thread-x", "threads/x.rs"),
    ]
}

/// Source files (relative to `src/`, a trailing `/` matching a whole directory) that cannot change
/// rendered bytes, with the reason each one is safe to leave out of the fingerprint. Anything
/// they influence reaches the renderer through inputs that are fingerprinted on their own: the
/// data commit, the loaded config files, the base URL, the release flag and the generation.
#[cfg(test)]
const RENDER_INDEPENDENT_SOURCES: &[&str] = &[
    // Process entry, argument parsing and command orchestration. Every rendering decision they
    // make (output URL, release mode, pinned data ref) is an explicit fingerprint field.
    "main.rs",
    "cli.rs",
    "commands/",
    // This module: the fingerprint, the raw-response cache and the cache layout. Changing how a
    // key is computed is deliberately versioned through the `schema` field instead.
    "cache.rs",
    // Git plumbing decides which commit is rendered; the commit itself is the `data-sha` field.
    "git.rs",
    // HTTP fetches bytes into the data branch and the private article cache; what was fetched
    // is covered by `data-sha`, and the build never renders a live response.
    "http.rs",
    "http/transport.rs",
    // Source engines produce raw items that sync persists before any build reads them. Only
    // `sources/youtube.rs` is consulted at render time (video ids, posters, short links) and it
    // is listed above; its `duration.rs` submodule serves ingestion-only duration and caption
    // probes.
    "sources/",
    // Retention plans remove files from the checkout in sync; the build sees the resulting tree.
    "store/retention.rs",
];

#[derive(Serialize, Deserialize)]
struct RenderManifest {
    fingerprint: String,
    pages: usize,
    items: usize,
    stubs: usize,
}

/// Restore a matching site into `out`. If `out` already carries this key the operation is an
/// O(1) no-op; otherwise it copies plain files so Pages artifacts never contain hardlinks.
pub fn restore_render(
    cache_root: &Path,
    fingerprint: &str,
    out: &Path,
) -> Result<Option<crate::site::Summary>> {
    let root = Namespace::Render.dir(cache_root);
    let manifest_path = root.join("manifest.toml");
    let manifest = match std::fs::read_to_string(&manifest_path)
        .ok()
        .and_then(|text| toml::from_str::<RenderManifest>(&text).ok())
    {
        Some(manifest) if manifest.fingerprint == fingerprint => manifest,
        _ => return Ok(None),
    };
    let cached = root.join("site");
    if !cached.join(".aggr-site").is_file() {
        return Ok(None);
    }
    if std::fs::read_to_string(cached.join(RENDER_KEY_FILE))
        .ok()
        .is_none_or(|key| key != fingerprint)
    {
        return Ok(None);
    }
    if std::fs::read_to_string(out.join(RENDER_KEY_FILE))
        .ok()
        .is_some_and(|key| key == fingerprint)
        && out.join(".aggr-site").is_file()
    {
        return Ok(Some(manifest.summary()));
    }

    crate::site::prepare_out_dir(out)?;
    copy_tree(&cached, out)?;
    write(&out.join(RENDER_KEY_FILE), fingerprint.as_bytes())?;
    Ok(Some(manifest.summary()))
}

/// Promote a freshly rendered temporary directory to the single current cache entry.
pub fn store_render(
    cache_root: &Path,
    fingerprint: &str,
    rendered: &Path,
    summary: crate::site::Summary,
) -> Result<()> {
    write(&rendered.join(RENDER_KEY_FILE), fingerprint.as_bytes())?;
    let root = Namespace::Render.dir(cache_root);
    std::fs::create_dir_all(&root).with_context(|| format!("creating {}", root.display()))?;
    let current = root.join("site");
    let previous = root.join("previous");
    if previous.exists() {
        std::fs::remove_dir_all(&previous)
            .with_context(|| format!("clearing {}", previous.display()))?;
    }
    if current.exists() {
        std::fs::rename(&current, &previous)
            .with_context(|| format!("moving cached site {} aside", current.display()))?;
    }
    if let Err(err) = std::fs::rename(rendered, &current) {
        if previous.exists() {
            let _ = std::fs::rename(&previous, &current);
        }
        return Err(err).with_context(|| format!("caching rendered site at {}", current.display()));
    }
    if previous.exists() {
        std::fs::remove_dir_all(&previous)
            .with_context(|| format!("clearing {}", previous.display()))?;
    }
    let manifest = RenderManifest {
        fingerprint: fingerprint.to_string(),
        pages: summary.pages,
        items: summary.items,
        stubs: summary.stubs,
    };
    write(
        &root.join("manifest.toml"),
        toml::to_string(&manifest)
            .context("serializing render cache metadata")?
            .as_bytes(),
    )
}

impl RenderManifest {
    fn summary(&self) -> crate::site::Summary {
        crate::site::Summary {
            pages: self.pages,
            items: self.items,
            stubs: self.stubs,
        }
    }
}

fn hash_field(hash: &mut Sha1, name: &[u8], value: &[u8]) {
    hash.update((name.len() as u64).to_le_bytes());
    hash.update(name);
    hash.update((value.len() as u64).to_le_bytes());
    hash.update(value);
}

fn hash_file(hash: &mut Sha1, kind: &[u8], path: &Path, name: &str) -> Result<()> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    hash_field(hash, kind, name.as_bytes());
    hash_field(hash, b"bytes", &bytes);
    Ok(())
}

/// The name a loaded file contributes to the fingerprint: its path under the repository with
/// `/` separators on every platform, or just its file name when it lives outside the repository.
fn portable_name(path: &Path, repo_root: &Path) -> String {
    let relative = match path.strip_prefix(repo_root) {
        Ok(relative) => relative,
        Err(_) => path.file_name().map(Path::new).unwrap_or(path),
    };
    relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn hash_tree(hash: &mut Sha1, kind: &str, root: &Path) -> Result<()> {
    if !root.is_dir() {
        return Ok(());
    }
    let mut files = Vec::new();
    let walk = walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| {
            entry.depth() == 0
                || !entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| UNHASHED_DIRS.contains(&name))
        });
    for entry in walk {
        let entry = entry.with_context(|| format!("walking {}", root.display()))?;
        if entry.file_type().is_file() {
            files.push(entry.path().to_path_buf());
        }
    }
    files.sort();
    for file in files {
        let relative = file.strip_prefix(root).unwrap_or(&file);
        hash_field(hash, kind.as_bytes(), relative.to_string_lossy().as_bytes());
        hash_field(
            hash,
            b"bytes",
            &std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?,
        );
    }
    Ok(())
}

pub(crate) fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    for entry in walkdir::WalkDir::new(from).min_depth(1) {
        let entry = entry.with_context(|| format!("walking {}", from.display()))?;
        let relative = entry
            .path()
            .strip_prefix(from)
            .expect("walk entry under root");
        let destination = to.join(relative);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&destination)
                .with_context(|| format!("creating {}", destination.display()))?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            std::fs::copy(entry.path(), &destination)
                .with_context(|| format!("copying {}", destination.display()))?;
        }
    }
    Ok(())
}

fn dev_under(base: &Path, config_path: &Path) -> PathBuf {
    let identity = config_path.to_string_lossy().replace('\\', "/");
    let hash = crate::model::sha1_hex(identity.as_bytes());
    let name = config_path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .map(slug::slugify)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "reader".to_string());
    base.join(DEV_NAMESPACE)
        .join(format!("{name}-{}", &hash[..16]))
}

/// Private, persistent origin-page cache. Build and dev pass different roots, so their state can
/// never bleed into each other.
pub struct ArticleCache {
    root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ArticleResponse {
    pub bytes: Vec<u8>,
    pub final_url: Url,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub body_hash: String,
    pub content_type: Option<String>,
}

impl ArticleResponse {
    pub fn html_text(&self) -> String {
        crate::http::decode_html(&self.bytes, self.content_type.as_deref())
    }

    pub fn extraction_key(&self) -> String {
        crate::model::sha1_hex(format!("{}\0{:?}", self.body_hash, self.content_type))
    }
}

#[derive(Serialize, Deserialize)]
struct ArticleMeta {
    final_url: String,
    etag: Option<String>,
    last_modified: Option<String>,
    body_hash: String,
    #[serde(default)]
    content_type: Option<String>,
}

impl ArticleCache {
    pub fn new(cache_root: &Path) -> Self {
        Self {
            root: Namespace::Articles.dir(cache_root),
        }
    }

    pub(crate) fn archive_retry_ready(&self, url: &Url, now: i64) -> Result<bool> {
        let path = self
            .root
            .join("archive-retries")
            .join(crate::model::sha1_hex(url.as_str()));
        let Some(bytes) = read_bounded_regular(&path, 32)? else {
            return Ok(true);
        };
        let attempted = std::str::from_utf8(&bytes)
            .ok()
            .and_then(|value| value.parse::<i64>().ok());
        Ok(attempted
            .is_none_or(|attempted| now < attempted || now.saturating_sub(attempted) >= 86_400))
    }

    pub(crate) fn record_archive_failure(&self, url: &Url, now: i64) -> Result<()> {
        let path = self
            .root
            .join("archive-retries")
            .join(crate::model::sha1_hex(url.as_str()));
        write(&path, now.to_string().as_bytes())
    }

    pub fn load(&self, url: &Url, headers: &[(String, String)]) -> Result<Option<ArticleResponse>> {
        let key = article_key(url, headers);
        let meta_path = self.root.join("entries").join(format!("{key}.toml"));
        let Some(metadata) = read_bounded_regular(&meta_path, MAX_ARTICLE_METADATA_BYTES)
            .with_context(|| format!("reading {}", meta_path.display()))?
        else {
            return Ok(None);
        };
        let Ok(text) = std::str::from_utf8(&metadata) else {
            return Ok(None);
        };
        let Ok(meta) = toml::from_str::<ArticleMeta>(text) else {
            return Ok(None);
        };
        if !is_sha1_hex(&meta.body_hash) {
            return Ok(None);
        }
        let body_path = self
            .root
            .join("bodies")
            .join(format!("{}.html", meta.body_hash));
        let Some(bytes) = read_bounded_regular(&body_path, MAX_ARTICLE_BODY_BYTES)
            .with_context(|| format!("reading {}", body_path.display()))?
        else {
            return Ok(None);
        };
        if crate::model::sha1_hex(&bytes) != meta.body_hash {
            log::warn!(
                "ignoring corrupt cached article body {}",
                body_path.display()
            );
            return Ok(None);
        }
        let Ok(final_url) = Url::parse(&meta.final_url) else {
            return Ok(None);
        };
        if !matches!(final_url.scheme(), "http" | "https") {
            return Ok(None);
        }
        Ok(Some(ArticleResponse {
            bytes,
            final_url,
            etag: meta.etag,
            last_modified: meta.last_modified,
            body_hash: meta.body_hash,
            content_type: meta.content_type,
        }))
    }

    pub fn store(
        &self,
        requested_url: &Url,
        headers: &[(String, String)],
        body: &crate::http::Body,
    ) -> Result<ArticleResponse> {
        let body_hash = crate::model::sha1_hex(&body.bytes);
        let body_path = self.root.join("bodies").join(format!("{body_hash}.html"));
        if read_bounded_regular(&body_path, MAX_ARTICLE_BODY_BYTES)?.as_deref()
            != Some(body.bytes.as_slice())
        {
            write(&body_path, &body.bytes)?;
        }

        let response = ArticleResponse {
            bytes: body.bytes.clone(),
            final_url: body.final_url.clone(),
            etag: body.etag.clone(),
            last_modified: body.last_modified.clone(),
            body_hash,
            content_type: body.content_type.clone(),
        };
        let meta = ArticleMeta {
            final_url: response.final_url.to_string(),
            etag: response.etag.clone(),
            last_modified: response.last_modified.clone(),
            body_hash: response.body_hash.clone(),
            content_type: response.content_type.clone(),
        };
        let meta_path = self
            .root
            .join("entries")
            .join(format!("{}.toml", article_key(requested_url, headers)));
        write(
            &meta_path,
            toml::to_string(&meta)
                .context("serializing article cache metadata")?
                .as_bytes(),
        )?;
        Ok(response)
    }

    pub fn extracted(
        &self,
        body_hash: &str,
        final_url: &Url,
    ) -> Result<Option<crate::content::ExtractedArticle>> {
        let path = self.extracted_path(body_hash, final_url);
        let Some(content) = read_bounded_regular(&path, MAX_EXTRACTED_ARTICLE_BYTES)
            .with_context(|| format!("reading {}", path.display()))?
        else {
            return Ok(None);
        };
        Ok(serde_json::from_slice(&content).ok())
    }

    pub fn store_extracted(
        &self,
        body_hash: &str,
        final_url: &Url,
        content: &crate::content::ExtractedArticle,
    ) -> Result<()> {
        write(
            &self.extracted_path(body_hash, final_url),
            &serde_json::to_vec(content)?,
        )
    }

    fn extracted_path(&self, body_hash: &str, final_url: &Url) -> PathBuf {
        self.root
            .join("extracted")
            .join(EXTRACTOR_VERSION)
            .join(format!("{body_hash}-{}.json", article_key(final_url, &[])))
    }
}

fn read_bounded_regular(path: &Path, max_bytes: usize) -> Result<Option<Vec<u8>>> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("inspecting {}", path.display())),
    };
    if !metadata.file_type().is_file() || metadata.len() > max_bytes as u64 {
        return Ok(None);
    }
    let capacity = usize::try_from(metadata.len()).context("cached file is too large")?;
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let opened = file
        .metadata()
        .with_context(|| format!("inspecting open cache file {}", path.display()))?;
    if !opened.is_file() || opened.len() > max_bytes as u64 {
        return Ok(None);
    }
    let mut bytes = Vec::with_capacity(capacity);
    file.take(max_bytes.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading {}", path.display()))?;
    Ok((bytes.len() <= max_bytes).then_some(bytes))
}

fn article_key(url: &Url, headers: &[(String, String)]) -> String {
    let mut url = url.clone();
    url.set_fragment(None);
    let mut headers = headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.as_str()))
        .collect::<Vec<_>>();
    headers.sort();
    crate::model::sha1_hex(format!("response-v2\0{}\0{headers:?}", url.as_str()))
}

pub(crate) fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("cache file has a parent directory")?;
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let mut file = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("creating a temporary file under {}", parent.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("writing temporary file for {}", path.display()))?;
    file.flush()
        .with_context(|| format!("flushing temporary file for {}", path.display()))?;
    file.persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

fn is_sha1_hex(text: &str) -> bool {
    text.len() == 40 && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Marker under the cache root recording the last [`sweep`]. One pass a day is plenty for a cache
/// that grows by a handful of files per run.
const SWEEP_MARKER: &str = ".last-sweep";

/// What one [`sweep`] removed. Counts cover successful deletions only; failures are logged.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SweepReport {
    /// The previous sweep is less than a day old: nothing was inspected or removed.
    pub throttled: bool,
    /// `articles-v1/bodies/<sha1>.html` files no entry referenced.
    pub bodies_removed: usize,
    /// `articles-v1/extracted/<version>/` directories of a previous extractor version.
    pub extractor_versions_removed: usize,
    /// `image-failures-v1/<generation>/` directories of a previous media implementation.
    pub image_generations_removed: usize,
    /// Paths that could not be removed; the next pass retries them.
    pub failures: usize,
}

impl std::fmt::Display for SweepReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.throttled {
            return write!(f, "throttled");
        }
        write!(
            f,
            "removed {} article bodies, {} extractor versions, {} image-failure generations; \
             {} failures",
            self.bodies_removed,
            self.extractor_versions_removed,
            self.image_generations_removed,
            self.failures
        )
    }
}

/// Remove cache files nothing can read any more: article bodies no `entries/*.toml` references,
/// extractions of a previous [`EXTRACTOR_VERSION`] and, when the caller passes the current media
/// generation, image-failure markers of a previous one. Bodies have no age limit: an entry that
/// still points at one keeps it, however old. Runs at most once a day, remembered in
/// `<cache_root>/.last-sweep`. Best-effort: a path that cannot be removed is logged at debug and
/// left for the next pass. The only errors are unreadable directory listings, because an
/// unreadable `entries/` would make every body look unreferenced.
pub fn sweep(cache_root: &Path, image_generation: Option<&str>) -> Result<SweepReport> {
    sweep_at(cache_root, image_generation, Utc::now())
}

fn sweep_at(
    cache_root: &Path,
    image_generation: Option<&str>,
    now: DateTime<Utc>,
) -> Result<SweepReport> {
    let marker = cache_root.join(SWEEP_MARKER);
    let last = std::fs::read_to_string(&marker).ok();
    if !sweep_due(last.as_deref(), now) {
        return Ok(SweepReport {
            throttled: true,
            ..SweepReport::default()
        });
    }

    let mut report = SweepReport::default();
    let articles = Namespace::Articles.dir(cache_root);
    let referenced = referenced_bodies(&articles.join("entries"))?;
    for entry in directory_entries(&articles.join("bodies"))? {
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.path();
        let Some(hash) = body_hash_of(&path) else {
            continue;
        };
        if referenced.contains(hash) {
            continue;
        }
        if remove_path(&path, false, &mut report.failures) {
            report.bodies_removed += 1;
        }
    }
    report.extractor_versions_removed = remove_stale_directories(
        &articles.join("extracted"),
        EXTRACTOR_VERSION,
        &mut report.failures,
    )?;
    if let Some(current) = image_generation {
        report.image_generations_removed = remove_stale_directories(
            &Namespace::ImageFailures.dir(cache_root),
            current,
            &mut report.failures,
        )?;
    }
    write(&marker, now.to_rfc3339().as_bytes())?;
    Ok(report)
}

/// Whether a sweep is due given the marker's contents. A missing or unreadable marker and one
/// older than a day are due. So is one from the future (the clock went backwards): a single pass
/// rewrites it instead of silencing the sweep until the clock catches up.
fn sweep_due(marker: Option<&str>, now: DateTime<Utc>) -> bool {
    let Some(last) = marker.and_then(|text| DateTime::parse_from_rfc3339(text.trim()).ok()) else {
        return true;
    };
    let last = last.with_timezone(&Utc);
    last > now || now - last >= chrono::Duration::hours(24)
}

/// Every valid `body_hash` named by an `entries/*.toml`. An entry that cannot be parsed references
/// nothing: [`ArticleCache::load`] ignores it too, so the body it pointed at is unreachable.
fn referenced_bodies(entries: &Path) -> Result<BTreeSet<String>> {
    let mut referenced = BTreeSet::new();
    for entry in directory_entries(entries)? {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "toml") {
            continue;
        }
        let Some(bytes) = read_bounded_regular(&path, MAX_ARTICLE_METADATA_BYTES)
            .with_context(|| format!("reading {}", path.display()))?
        else {
            continue;
        };
        let Some(meta) = std::str::from_utf8(&bytes)
            .ok()
            .and_then(|text| toml::from_str::<ArticleMeta>(text).ok())
        else {
            continue;
        };
        if is_sha1_hex(&meta.body_hash) {
            referenced.insert(meta.body_hash);
        }
    }
    Ok(referenced)
}

/// The hash a `bodies/<sha1>.html` file stores, or `None` for anything else in the directory.
fn body_hash_of(path: &Path) -> Option<&str> {
    let hash = path.file_name()?.to_str()?.strip_suffix(".html")?;
    is_sha1_hex(hash).then_some(hash)
}

/// Remove every subdirectory of `parent` except `current`, returning how many went.
fn remove_stale_directories(parent: &Path, current: &str, failures: &mut usize) -> Result<usize> {
    let mut removed = 0;
    for entry in directory_entries(parent)? {
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) || entry.file_name() == current {
            continue;
        }
        if remove_path(&entry.path(), true, failures) {
            removed += 1;
        }
    }
    Ok(removed)
}

/// The entries of a cache directory in path order; a directory that does not exist is empty.
fn directory_entries(dir: &Path) -> Result<Vec<std::fs::DirEntry>> {
    let listing = match std::fs::read_dir(dir) {
        Ok(listing) => listing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("listing {}", dir.display())),
    };
    let mut entries = listing
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("listing {}", dir.display()))?;
    entries.sort_by_key(std::fs::DirEntry::path);
    Ok(entries)
}

/// Remove one cache path, logging instead of failing: the next sweep tries again.
fn remove_path(path: &Path, directory: bool, failures: &mut usize) -> bool {
    let result = if directory {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    match result {
        Ok(()) => true,
        Err(error) => {
            log::debug!("cache sweep: leaving {}: {error}", path.display());
            *failures += 1;
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::Body;
    use tempfile::tempdir;

    #[cfg(unix)]
    #[test]
    fn bounded_cache_reads_reject_oversized_and_non_regular_files() {
        let root = tempdir().unwrap();
        let file = root.path().join("entry");
        std::fs::write(&file, b"1234").unwrap();
        assert_eq!(
            read_bounded_regular(&file, 4).unwrap(),
            Some(b"1234".to_vec())
        );
        assert_eq!(read_bounded_regular(&file, 3).unwrap(), None);
        assert_eq!(read_bounded_regular(root.path(), 10).unwrap(), None);

        let link = root.path().join("link");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert_eq!(read_bounded_regular(&link, 10).unwrap(), None);
        assert_eq!(
            read_bounded_regular(&root.path().join("missing"), 10).unwrap(),
            None
        );
    }

    #[test]
    fn build_and_dev_namespaces_are_explicit_and_disjoint() {
        assert_eq!(
            build(Path::new("/repo")),
            Path::new("/repo/.aggr/cache/build-v1")
        );
        let a = dev_under(Path::new("/cache/aggr"), Path::new("/one/aggr.toml"));
        let b = dev_under(Path::new("/cache/aggr"), Path::new("/two/aggr.toml"));
        assert!(a.starts_with("/cache/aggr/dev-v1"));
        assert_ne!(a, b);
        assert!(!a.starts_with("/repo"));
    }

    #[test]
    fn article_responses_and_extractions_survive_processes() {
        let root = tempdir().unwrap();
        let cache = ArticleCache::new(root.path());
        let requested = Url::parse("https://www.example.com/post?utm_source=x").unwrap();
        let body = Body {
            bytes: b"<article>complete</article>".to_vec(),
            etag: Some("\"v1\"".into()),
            last_modified: None,
            final_url: Url::parse("https://example.com/post").unwrap(),
            content_type: Some("text/html; charset=utf-8".into()),
        };
        let stored = cache.store(&requested, &[], &body).unwrap();
        cache
            .store_extracted(
                &stored.body_hash,
                &stored.final_url,
                &crate::content::ExtractedArticle {
                    html: "<article>clean</article>".into(),
                    image: None,
                    labels: Vec::new(),
                },
            )
            .unwrap();

        let equivalent = requested.clone();
        let loaded = ArticleCache::new(root.path())
            .load(&equivalent, &[])
            .unwrap()
            .unwrap();
        assert_eq!(loaded.bytes, body.bytes);
        assert_eq!(loaded.etag.as_deref(), Some("\"v1\""));
        assert_eq!(
            cache
                .extracted(&loaded.body_hash, &loaded.final_url)
                .unwrap()
                .map(|article| article.html)
                .as_deref(),
            Some("<article>clean</article>")
        );
        assert!(
            cache
                .extracted(
                    &loaded.body_hash,
                    &Url::parse("https://elsewhere.example/post").unwrap()
                )
                .unwrap()
                .is_none()
        );
        assert!(
            cache
                .load(&requested, &[("Authorization".into(), "different".into())])
                .unwrap()
                .is_none()
        );
        assert_eq!(loaded.content_type, body.content_type);
        let mut different_encoding = loaded.clone();
        different_encoding.content_type = Some("text/html; charset=windows-1252".into());
        assert_ne!(loaded.extraction_key(), different_encoding.extraction_key());
    }

    #[test]
    fn article_cache_keeps_http_identity_distinct_from_dedupe_identity() {
        let original = Url::parse("http://www.example.com:8080/post?ref=a").unwrap();
        let key = article_key(&original, &[]);
        for other in [
            "http://www.example.com:8081/post?ref=a",
            "https://www.example.com:8080/post?ref=a",
            "http://www.example.com:8080/post?ref=b",
        ] {
            assert_ne!(key, article_key(&Url::parse(other).unwrap(), &[]));
        }
        assert_eq!(
            key,
            article_key(
                &Url::parse("http://www.example.com:8080/post?ref=a#part").unwrap(),
                &[]
            )
        );
    }

    #[test]
    fn disposable_article_cache_recovers_from_corruption_without_storing_headers() {
        let root = tempdir().unwrap();
        let cache = ArticleCache::new(root.path());
        let url = Url::parse("https://example.com/article").unwrap();
        let headers = vec![("Authorization".into(), "Bearer private-test-secret".into())];
        let body = Body {
            bytes: b"<p>article</p>".to_vec(),
            etag: None,
            last_modified: None,
            final_url: url.clone(),
            content_type: Some("text/html".into()),
        };
        let response = cache.store(&url, &headers, &body).unwrap();
        let metadata = cache
            .root
            .join("entries")
            .join(format!("{}.toml", article_key(&url, &headers)));
        assert!(
            !std::fs::read_to_string(&metadata)
                .unwrap()
                .contains("private-test-secret")
        );
        let body_path = cache
            .root
            .join("bodies")
            .join(format!("{}.html", response.body_hash));
        std::fs::write(&body_path, "corrupt").unwrap();
        assert!(cache.load(&url, &headers).unwrap().is_none());
        cache.store(&url, &headers, &body).unwrap();
        assert_eq!(
            cache.load(&url, &headers).unwrap().unwrap().bytes,
            body.bytes
        );

        std::fs::write(&metadata, "invalid toml").unwrap();
        assert!(cache.load(&url, &headers).unwrap().is_none());
        cache.store(&url, &headers, &body).unwrap();
        let text = std::fs::read_to_string(&metadata).unwrap();
        std::fs::write(
            &metadata,
            text.replace(&response.body_hash, "../../outside"),
        )
        .unwrap();
        assert!(cache.load(&url, &headers).unwrap().is_none());

        let extracted = crate::content::ExtractedArticle {
            html: "<p>article</p>".into(),
            image: None,
            labels: Vec::new(),
        };
        cache
            .store_extracted(&response.extraction_key(), &url, &extracted)
            .unwrap();
        std::fs::write(
            cache.extracted_path(&response.extraction_key(), &url),
            "invalid json",
        )
        .unwrap();
        assert!(
            cache
                .extracted(&response.extraction_key(), &url)
                .unwrap()
                .is_none()
        );
        cache
            .store_extracted(&response.extraction_key(), &url, &extracted)
            .unwrap();
        assert_eq!(
            cache
                .extracted(&response.extraction_key(), &url)
                .unwrap()
                .unwrap()
                .html,
            extracted.html
        );
    }

    #[test]
    fn rendered_sites_restore_and_short_circuit_matching_output() {
        let root = tempdir().unwrap();
        let cache_root = root.path().join("cache");
        let rendered = root.path().join("staging");
        std::fs::create_dir_all(&rendered).unwrap();
        std::fs::write(rendered.join(".aggr-site"), "1").unwrap();
        std::fs::write(rendered.join("index.html"), "first").unwrap();
        let summary = crate::site::Summary {
            pages: 3,
            items: 2,
            stubs: 1,
        };
        store_render(&cache_root, "abc", &rendered, summary).unwrap();

        let out = root.path().join("out");
        assert_eq!(
            restore_render(&cache_root, "abc", &out).unwrap(),
            Some(summary)
        );
        assert_eq!(
            std::fs::read_to_string(out.join("index.html")).unwrap(),
            "first"
        );
        assert_eq!(
            restore_render(&cache_root, "abc", &out).unwrap(),
            Some(summary)
        );
        assert!(
            restore_render(&cache_root, "different", &out)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn render_restore_rejects_a_manifest_and_site_from_different_generations() {
        let root = tempdir().unwrap();
        let cache_root = root.path().join("cache");
        let rendered = root.path().join("staging");
        std::fs::create_dir_all(&rendered).unwrap();
        std::fs::write(rendered.join(".aggr-site"), "1").unwrap();
        std::fs::write(rendered.join("index.html"), "cached generation").unwrap();
        let summary = crate::site::Summary {
            pages: 1,
            items: 1,
            stubs: 0,
        };
        store_render(&cache_root, "manifest-generation", &rendered, summary).unwrap();
        std::fs::write(
            Namespace::Render
                .dir(&cache_root)
                .join("site")
                .join(RENDER_KEY_FILE),
            "site-generation",
        )
        .unwrap();

        let out = root.path().join("out");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join(".aggr-site"), "1").unwrap();
        std::fs::write(out.join("index.html"), "untouched output").unwrap();

        assert!(
            restore_render(&cache_root, "manifest-generation", &out)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            std::fs::read_to_string(out.join("index.html")).unwrap(),
            "untouched output"
        );
    }

    /// A minimal project: `aggr.toml` importing `sources.toml`, plus one project template.
    fn write_fixture(root: &Path) -> Vec<PathBuf> {
        std::fs::write(root.join("aggr.toml"), "[site]\ntitle = \"one\"\n").unwrap();
        std::fs::write(root.join("sources.toml"), "[[sources]]\nurl = \"x\"\n").unwrap();
        std::fs::create_dir_all(root.join("templates")).unwrap();
        std::fs::write(root.join("templates/index.html"), "one").unwrap();
        vec![root.join("aggr.toml"), root.join("sources.toml")]
    }

    #[test]
    fn render_fingerprint_tracks_loaded_config_and_theme_files() {
        let root = tempdir().unwrap();
        let config_path = root.path().join("aggr.toml");
        let mut config = crate::config::Config::parse("[site]\ntitle = \"one\"\n").unwrap();
        config.loaded_files = write_fixture(root.path());
        macro_rules! fingerprint {
            ($discussions:expr) => {
                render_fingerprint(RenderFingerprint {
                    config: &config,
                    project_root: root.path(),
                    repo_root: root.path(),
                    config_sha: Some("config"),
                    data_sha: Some("data"),
                    base_url: None,
                    release: false,
                    discussions: $discussions,
                    generation: "generation",
                })
                .unwrap()
            };
        }
        let first = fingerprint!(None);

        std::fs::write(root.path().join("templates/index.html"), "two").unwrap();
        let theme_changed = fingerprint!(None);
        assert_ne!(first, theme_changed);

        std::fs::write(&config_path, "[site]\ntitle = \"two\"\n").unwrap();
        let config_changed = fingerprint!(None);
        assert_ne!(theme_changed, config_changed);

        config
            .loaded_remote
            .push(("github:o/r@main/aggr.toml".into(), "body-v1".into()));
        let remote_changed = fingerprint!(None);
        assert_ne!(config_changed, remote_changed);

        let discussion_changed = fingerprint!(Some("new-direct-link"));
        assert_ne!(remote_changed, discussion_changed);
    }

    #[test]
    fn render_fingerprint_is_the_same_for_the_same_checkout_at_another_path() {
        fn fingerprint(root: &Path, loaded_files: Vec<PathBuf>) -> String {
            let mut config = crate::config::Config::parse("[site]\ntitle = \"one\"\n").unwrap();
            config.loaded_files = loaded_files;
            render_fingerprint(RenderFingerprint {
                config: &config,
                project_root: root,
                repo_root: root,
                config_sha: Some("config"),
                data_sha: Some("data"),
                base_url: None,
                release: false,
                discussions: None,
                generation: "generation",
            })
            .unwrap()
        }

        let first = tempdir().unwrap();
        let second = tempdir().unwrap();
        assert_ne!(first.path(), second.path());
        let original = fingerprint(first.path(), write_fixture(first.path()));
        let copied = fingerprint(second.path(), write_fixture(second.path()));
        assert_eq!(original, copied);

        // Same bytes under a different imported file name is a different config graph.
        std::fs::rename(
            second.path().join("sources.toml"),
            second.path().join("feeds.toml"),
        )
        .unwrap();
        let renamed = fingerprint(
            second.path(),
            vec![
                second.path().join("aggr.toml"),
                second.path().join("feeds.toml"),
            ],
        );
        assert_ne!(copied, renamed);
    }

    #[test]
    fn portable_names_are_repository_relative_with_forward_slashes() {
        let repo = Path::new("/srv/checkout");
        assert_eq!(
            portable_name(&repo.join("config").join("feeds.toml"), repo),
            "config/feeds.toml"
        );
        assert_eq!(portable_name(&repo.join("aggr.toml"), repo), "aggr.toml");
        assert_eq!(
            portable_name(Path::new("/elsewhere/shared/aggr.toml"), repo),
            "aggr.toml"
        );
    }

    #[test]
    fn render_fingerprint_tracks_config_implementation_and_embedded_defaults() {
        let names = render_implementation_sources()
            .iter()
            .map(|(name, _, _)| *name)
            .collect::<Vec<_>>();
        assert!(names.contains(&"config"));
        assert!(names.contains(&"defaults"));
        let unique = names.iter().collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            unique.len(),
            names.len(),
            "duplicate hash labels: {names:?}"
        );
    }

    #[test]
    fn every_source_file_is_classified_for_the_render_fingerprint() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let tracked = render_implementation_sources()
            .iter()
            .map(|(_, path, _)| *path)
            .collect::<std::collections::BTreeSet<_>>();
        for tracked in &tracked {
            assert!(
                src.join(tracked).is_file(),
                "fingerprinted source {tracked} does not exist"
            );
        }
        for allowed in RENDER_INDEPENDENT_SOURCES {
            assert!(
                !tracked.contains(allowed),
                "{allowed} is both fingerprinted and declared render-independent"
            );
            let path = src.join(allowed);
            assert!(
                if allowed.ends_with('/') {
                    path.is_dir()
                } else {
                    path.is_file()
                },
                "render-independent entry {allowed} does not exist"
            );
        }

        let mut unclassified = Vec::new();
        for entry in walkdir::WalkDir::new(&src) {
            let entry = entry.unwrap();
            if !entry.file_type().is_file()
                || entry.path().extension().is_none_or(|ext| ext != "rs")
            {
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(&src)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let allowed = RENDER_INDEPENDENT_SOURCES.iter().any(|allowed| {
                if allowed.ends_with('/') {
                    relative.starts_with(allowed)
                } else {
                    relative == *allowed
                }
            });
            if !tracked.contains(relative.as_str()) && !allowed {
                unclassified.push(relative);
            }
        }
        assert!(
            unclassified.is_empty(),
            "classify these files in render_implementation_sources() or \
             RENDER_INDEPENDENT_SOURCES: {unclassified:?}"
        );
    }

    #[test]
    fn cache_namespaces_keep_their_historical_directory_names() {
        let names = Namespace::ALL
            .iter()
            .map(Namespace::dir_name)
            .collect::<Vec<_>>();
        let unique = names.iter().collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), names.len(), "duplicate namespaces: {names:?}");
        // Renaming a namespace orphans every cache written by earlier releases.
        assert_eq!(Namespace::Articles.dir_name(), "articles-v1");
        assert_eq!(Namespace::Render.dir_name(), "render-v1");
        assert_eq!(Namespace::Discussions.dir_name(), "discussions-v1");
        assert_eq!(Namespace::Pagefind.dir_name(), "pagefind-v1");
        assert_eq!(Namespace::ValidatedImages.dir_name(), "validated-images-v2");
        assert_eq!(Namespace::ImageFailures.dir_name(), "image-failures-v1");
        assert_eq!(Namespace::CaptureRetries.dir_name(), "capture-retries-v1");
        assert_eq!(
            Namespace::RecordingDuration.dir_name(),
            "recording-duration-v1"
        );
        assert_eq!(Namespace::FeedParsing.dir_name(), "feed-parsing");
        assert_eq!(
            Namespace::Pagefind.dir(Path::new("/repo/.aggr/cache/build-v1")),
            Path::new("/repo/.aggr/cache/build-v1/pagefind-v1")
        );
    }

    #[test]
    fn ci_caches_derived_state_but_never_private_responses_or_the_rendered_site() {
        let excluded = Namespace::ALL
            .iter()
            .filter(|namespace| !namespace.ci_cached())
            .copied()
            .collect::<Vec<_>>();
        assert_eq!(excluded, [Namespace::Articles, Namespace::Render]);
        let paths = ci_cached_paths();
        assert_eq!(paths.len(), Namespace::ALL.len() - excluded.len());
        for path in &paths {
            assert!(
                path.starts_with(".aggr/cache/build-v1/"),
                "{path} is not under the build cache"
            );
            assert!(
                Path::new("/repo")
                    .join(path)
                    .starts_with(build(Path::new("/repo"))),
                "{path} does not resolve under build()"
            );
        }
        assert!(paths.contains(&".aggr/cache/build-v1/validated-images-v2".to_string()));
        assert!(!paths.iter().any(|path| path.contains("render-v1")));
        assert!(!paths.iter().any(|path| path.contains("articles-v1")));
    }

    #[test]
    fn render_fingerprint_hashes_only_the_theme_subdirectories_of_a_project_root_layer() {
        let root = tempdir().unwrap();
        let mut config = crate::config::Config::parse("[site]\ntitle = \"one\"\n").unwrap();
        config.loaded_files = write_fixture(root.path());
        let fingerprint = || {
            render_fingerprint(RenderFingerprint {
                config: &config,
                project_root: root.path(),
                repo_root: root.path(),
                config_sha: Some("config"),
                data_sha: Some("data"),
                base_url: None,
                release: false,
                discussions: None,
                generation: "generation",
            })
            .unwrap()
        };
        let initial = fingerprint();

        // The project root is layer 0 because it carries `templates/`. Everything else in it, and
        // repository state that strays under a theme directory, is not rendered.
        for unrelated in [
            ".git/objects/pack/large.pack",
            ".aggr/cache/build-v1/articles-v1/bodies/a.html",
            "_site/index.html",
            "items/demo/2026/09/post.md",
            "notes/draft.md",
            "templates/.git/HEAD",
            "static/.aggr/junk",
            "templates/_site/index.html",
        ] {
            let path = root.path().join(unrelated);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, format!("unrelated {unrelated}")).unwrap();
        }
        assert_eq!(fingerprint(), initial);

        std::fs::create_dir_all(root.path().join("static")).unwrap();
        std::fs::write(root.path().join("static/style.css"), "body{}").unwrap();
        let static_changed = fingerprint();
        assert_ne!(static_changed, initial);

        std::fs::write(root.path().join("templates/index.html"), "two").unwrap();
        assert_ne!(fingerprint(), static_changed);
    }

    /// Store one article under `url` with `body` and an extraction of the current version.
    fn seed_article(cache: &ArticleCache, url: &str, body: &[u8]) -> ArticleResponse {
        let url = Url::parse(url).unwrap();
        let stored = cache
            .store(
                &url,
                &[],
                &Body {
                    bytes: body.to_vec(),
                    etag: None,
                    last_modified: None,
                    final_url: url.clone(),
                    content_type: Some("text/html".into()),
                },
            )
            .unwrap();
        cache
            .store_extracted(
                &stored.body_hash,
                &stored.final_url,
                &crate::content::ExtractedArticle {
                    html: String::from_utf8_lossy(body).into_owned(),
                    image: None,
                    labels: Vec::new(),
                },
            )
            .unwrap();
        stored
    }

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "x").unwrap();
    }

    #[test]
    fn sweep_removes_unreferenced_bodies_and_stale_generations_but_keeps_live_state() {
        let root = tempdir().unwrap();
        let cache = ArticleCache::new(root.path());
        let kept = seed_article(&cache, "https://example.com/kept", b"<p>kept</p>");
        let replaced = seed_article(&cache, "https://example.com/replaced", b"<p>old</p>");
        // The entry now points at a new body: the old one is unreferenced.
        let current = seed_article(&cache, "https://example.com/replaced", b"<p>new</p>");
        assert_ne!(replaced.body_hash, current.body_hash);
        let bodies = cache.root.join("bodies");
        let orphan = bodies.join(format!("{}.html", crate::model::sha1_hex(b"orphan")));
        touch(&orphan);
        // Not a body file: left alone whatever it is.
        touch(&bodies.join("notes.txt"));
        let extracted = cache.root.join("extracted");
        touch(
            &extracted
                .join("dom-smoothie-0.17-aggr-1")
                .join("stale.json"),
        );
        touch(&extracted.join("README"));
        let failures = Namespace::ImageFailures.dir(root.path());
        touch(&failures.join("previous-generation").join("marker"));
        touch(&failures.join("current-generation").join("marker"));

        let now = Utc::now();
        let report = sweep_at(root.path(), Some("current-generation"), now).unwrap();
        assert_eq!(
            report,
            SweepReport {
                throttled: false,
                bodies_removed: 2,
                extractor_versions_removed: 1,
                image_generations_removed: 1,
                failures: 0,
            }
        );

        assert!(bodies.join(format!("{}.html", kept.body_hash)).is_file());
        assert!(bodies.join(format!("{}.html", current.body_hash)).is_file());
        assert!(!bodies.join(format!("{}.html", replaced.body_hash)).exists());
        assert!(!orphan.exists());
        assert!(bodies.join("notes.txt").is_file());
        assert!(!extracted.join("dom-smoothie-0.17-aggr-1").exists());
        assert!(extracted.join(EXTRACTOR_VERSION).is_dir());
        assert!(extracted.join("README").is_file());
        assert!(!failures.join("previous-generation").exists());
        assert!(failures.join("current-generation").join("marker").is_file());
        assert_eq!(
            std::fs::read_to_string(root.path().join(SWEEP_MARKER)).unwrap(),
            now.to_rfc3339()
        );

        // Everything an entry still points at loads exactly as before.
        let loaded = cache
            .load(&Url::parse("https://example.com/kept").unwrap(), &[])
            .unwrap()
            .unwrap();
        assert_eq!(loaded.bytes, b"<p>kept</p>");
        assert_eq!(
            cache
                .extracted(&kept.body_hash, &kept.final_url)
                .unwrap()
                .map(|article| article.html)
                .as_deref(),
            Some("<p>kept</p>")
        );
        assert_eq!(
            cache
                .load(&Url::parse("https://example.com/replaced").unwrap(), &[])
                .unwrap()
                .unwrap()
                .bytes,
            b"<p>new</p>"
        );
    }

    #[test]
    fn sweep_leaves_image_failure_generations_alone_without_the_current_one() {
        let root = tempdir().unwrap();
        let failures = Namespace::ImageFailures.dir(root.path());
        touch(&failures.join("generation-a").join("marker"));
        touch(&failures.join("generation-b").join("marker"));
        let report = sweep_at(root.path(), None, Utc::now()).unwrap();
        assert_eq!(report.image_generations_removed, 0);
        assert_eq!(report.failures, 0);
        assert!(failures.join("generation-a").join("marker").is_file());
        assert!(failures.join("generation-b").join("marker").is_file());
    }

    #[test]
    fn sweep_runs_at_most_once_a_day() {
        let root = tempdir().unwrap();
        let bodies = Namespace::Articles.dir(root.path()).join("bodies");
        let now = Utc::now();
        assert!(!sweep_at(root.path(), None, now).unwrap().throttled);

        let orphan = bodies.join(format!("{}.html", crate::model::sha1_hex(b"later")));
        touch(&orphan);
        let soon = sweep_at(root.path(), None, now + chrono::Duration::hours(23)).unwrap();
        assert!(soon.throttled);
        assert_eq!(soon.bodies_removed, 0);
        assert!(orphan.is_file(), "a throttled sweep touches nothing");

        let due = sweep_at(root.path(), None, now + chrono::Duration::hours(24)).unwrap();
        assert!(!due.throttled);
        assert_eq!(due.bodies_removed, 1);
        assert!(!orphan.exists());

        // The public entry point reads the wall clock: a first pass runs, one right after does not.
        let fresh = tempdir().unwrap();
        assert!(!sweep(fresh.path(), None).unwrap().throttled);
        assert!(sweep(fresh.path(), None).unwrap().throttled);

        assert!(sweep_due(None, now));
        assert!(sweep_due(Some("not a timestamp"), now));
        assert!(sweep_due(Some(""), now));
        let hour = chrono::Duration::hours(1);
        assert!(!sweep_due(Some(&(now - hour).to_rfc3339()), now));
        assert!(!sweep_due(
            Some(&format!("{}\n", (now - hour * 23).to_rfc3339())),
            now
        ));
        assert!(sweep_due(Some(&(now - hour * 25).to_rfc3339()), now));
        assert!(
            sweep_due(Some(&(now + hour).to_rfc3339()), now),
            "a marker from the future is rewritten, not trusted"
        );
    }

    #[cfg(unix)]
    #[test]
    fn sweep_reports_a_directory_it_cannot_remove_and_continues() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempdir().unwrap();
        let articles = Namespace::Articles.dir(root.path());
        let locked = articles.join("extracted").join("locked-version");
        touch(&locked.join("stale.json"));
        let removable = articles.join("extracted").join("removable-version");
        touch(&removable.join("stale.json"));
        let orphan = articles
            .join("bodies")
            .join(format!("{}.html", crate::model::sha1_hex(b"orphan")));
        touch(&orphan);
        // Without write permission on the directory its children cannot be unlinked.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();

        let result = sweep_at(root.path(), None, Utc::now());
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        let report = result.unwrap();
        assert_eq!(report.failures, 1);
        assert_eq!(report.extractor_versions_removed, 1);
        assert_eq!(report.bodies_removed, 1);
        assert!(
            locked.join("stale.json").is_file(),
            "left for the next pass"
        );
        assert!(!removable.exists());
        assert!(!orphan.exists());
        assert!(
            root.path().join(SWEEP_MARKER).is_file(),
            "a partial pass still counts as today's sweep"
        );
        assert_eq!(
            report.to_string(),
            "removed 1 article bodies, 1 extractor versions, 0 image-failure generations; 1 failures"
        );
    }
}
