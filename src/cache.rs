//! Cache locations and namespaces. Build/CI state belongs to the repository; dev state belongs
//! to the operating system's standard cache directory and is isolated per configuration file.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use sha1::{Digest as _, Sha1};
use url::Url;

const BUILD_NAMESPACE: &str = "build-v1";
const DEV_NAMESPACE: &str = "dev-v1";
const ARTICLE_NAMESPACE: &str = "articles-v1";
const RENDER_NAMESPACE: &str = "render-v1";
const RENDER_KEY_FILE: &str = ".aggr-build-key";
/// Bump when article extraction semantics change. Raw responses remain reusable across bumps.
const EXTRACTOR_VERSION: &str = "dom-smoothie-0.18-aggr-2";
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
        config_sha,
        data_sha,
        base_url,
        release,
        discussions,
        generation,
    } = input;
    let mut hash = Sha1::new();
    hash_field(&mut hash, b"schema", b"render-v2");
    hash_field(&mut hash, b"version", env!("CARGO_PKG_VERSION").as_bytes());
    // Development builds often keep the package version while renderer code changes. Hash the
    // implementation itself so a prior binary can never make a new binary restore stale HTML.
    for (name, source) in render_implementation_sources() {
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
        hash_file(&mut hash, b"config", &path)?;
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
        hash_tree(&mut hash, &format!("layer-{index}"), dir)?;
    }
    Ok(hex::encode(hash.finalize()))
}

fn render_implementation_sources() -> [(&'static str, &'static str); 19] {
    [
        ("config", include_str!("config.rs")),
        ("source-entries", include_str!("config/source_entries.rs")),
        ("source-formats", include_str!("config/import_formats.rs")),
        ("source-graph", include_str!("config/import_graph.rs")),
        ("defaults", include_str!("../config.default.toml")),
        ("content", include_str!("content.rs")),
        ("media", include_str!("media.rs")),
        ("model", include_str!("model.rs")),
        ("youtube", include_str!("sources/youtube.rs")),
        ("preview", include_str!("preview.rs")),
        ("store", include_str!("store/mod.rs")),
        ("frontmatter", include_str!("store/frontmatter.rs")),
        ("site", include_str!("site/mod.rs")),
        ("derived", include_str!("site/derived.rs")),
        ("context", include_str!("site/context.rs")),
        ("outputs", include_str!("site/outputs.rs")),
        ("pagefind", include_str!("site/pagefind.rs")),
        ("related", include_str!("site/related.rs")),
        ("render", include_str!("site/render.rs")),
    ]
}

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
    let root = cache_root.join(RENDER_NAMESPACE);
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
    let root = cache_root.join(RENDER_NAMESPACE);
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

fn hash_file(hash: &mut Sha1, kind: &[u8], path: &Path) -> Result<()> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    hash_field(hash, kind, path.to_string_lossy().as_bytes());
    hash_field(hash, b"bytes", &bytes);
    Ok(())
}

fn hash_tree(hash: &mut Sha1, kind: &str, root: &Path) -> Result<()> {
    if !root.is_dir() {
        return Ok(());
    }
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(root) {
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
            root: cache_root.join(ARTICLE_NAMESPACE),
        }
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
        if meta.body_hash.len() != 40
            || !meta.body_hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
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
            cache_root
                .join(RENDER_NAMESPACE)
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

    #[test]
    fn render_fingerprint_tracks_loaded_config_and_theme_files() {
        let root = tempdir().unwrap();
        let config_path = root.path().join("aggr.toml");
        std::fs::write(&config_path, "[site]\ntitle = \"one\"\n").unwrap();
        std::fs::create_dir_all(root.path().join("templates")).unwrap();
        std::fs::write(root.path().join("templates/index.html"), "one").unwrap();
        let mut config = crate::config::Config::parse("[site]\ntitle = \"one\"\n").unwrap();
        config.loaded_files = vec![config_path.clone()];
        macro_rules! fingerprint {
            ($discussions:expr) => {
                render_fingerprint(RenderFingerprint {
                    config: &config,
                    project_root: root.path(),
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
    fn render_fingerprint_tracks_config_implementation_and_embedded_defaults() {
        let names = render_implementation_sources()
            .map(|(name, _)| name)
            .into_iter()
            .collect::<Vec<_>>();
        assert!(names.contains(&"config"));
        assert!(names.contains(&"defaults"));
    }
}
