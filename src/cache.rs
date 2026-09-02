//! Cache locations and namespaces. Build/CI state belongs to the repository; dev state belongs
//! to the operating system's standard cache directory and is isolated per configuration file.

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
const EXTRACTOR_VERSION: &str = "dom-smoothie-0.18-aggr-1";

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
pub fn render_fingerprint(
    config: &crate::config::Config,
    project_root: &Path,
    config_sha: Option<&str>,
    data_sha: Option<&str>,
    base_url: Option<&str>,
    release: bool,
) -> Result<String> {
    let mut hash = Sha1::new();
    hash_field(&mut hash, b"schema", b"render-v1");
    hash_field(&mut hash, b"version", env!("CARGO_PKG_VERSION").as_bytes());
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
        b"repository",
        config.repository().unwrap_or_default().as_bytes(),
    );

    let mut config_files = config.loaded_files.clone();
    config_files.sort();
    for path in config_files {
        hash_file(&mut hash, b"config", &path)?;
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

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
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
}

#[derive(Serialize, Deserialize)]
struct ArticleMeta {
    final_url: String,
    etag: Option<String>,
    last_modified: Option<String>,
    body_hash: String,
}

impl ArticleCache {
    pub fn new(cache_root: &Path) -> Self {
        Self {
            root: cache_root.join(ARTICLE_NAMESPACE),
        }
    }

    pub fn load(&self, url: &Url) -> Result<Option<ArticleResponse>> {
        let key = article_key(url);
        let meta_path = self.root.join("entries").join(format!("{key}.toml"));
        let text = match std::fs::read_to_string(&meta_path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(err).with_context(|| format!("reading {}", meta_path.display()));
            }
        };
        let meta: ArticleMeta =
            toml::from_str(&text).with_context(|| format!("parsing {}", meta_path.display()))?;
        let body_path = self
            .root
            .join("bodies")
            .join(format!("{}.html", meta.body_hash));
        let bytes = match std::fs::read(&body_path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(err).with_context(|| format!("reading {}", body_path.display()));
            }
        };
        if crate::model::sha1_hex(&bytes) != meta.body_hash {
            log::warn!(
                "ignoring corrupt cached article body {}",
                body_path.display()
            );
            return Ok(None);
        }
        let final_url = Url::parse(&meta.final_url)
            .with_context(|| format!("invalid cached article URL in {}", meta_path.display()))?;
        Ok(Some(ArticleResponse {
            bytes,
            final_url,
            etag: meta.etag,
            last_modified: meta.last_modified,
            body_hash: meta.body_hash,
        }))
    }

    pub fn store(&self, requested_url: &Url, body: &crate::http::Body) -> Result<ArticleResponse> {
        let body_hash = crate::model::sha1_hex(&body.bytes);
        let body_path = self.root.join("bodies").join(format!("{body_hash}.html"));
        write_if_missing(&body_path, &body.bytes)?;

        let response = ArticleResponse {
            bytes: body.bytes.clone(),
            final_url: body.final_url.clone(),
            etag: body.etag.clone(),
            last_modified: body.last_modified.clone(),
            body_hash,
        };
        let meta = ArticleMeta {
            final_url: response.final_url.to_string(),
            etag: response.etag.clone(),
            last_modified: response.last_modified.clone(),
            body_hash: response.body_hash.clone(),
        };
        let meta_path = self
            .root
            .join("entries")
            .join(format!("{}.toml", article_key(requested_url)));
        write(
            &meta_path,
            toml::to_string(&meta)
                .context("serializing article cache metadata")?
                .as_bytes(),
        )?;
        Ok(response)
    }

    pub fn extracted(&self, body_hash: &str) -> Result<Option<String>> {
        let path = self.extracted_path(body_hash);
        match std::fs::read_to_string(&path) {
            Ok(content) => Ok(Some(content)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn store_extracted(&self, body_hash: &str, content: &str) -> Result<()> {
        write_if_missing(&self.extracted_path(body_hash), content.as_bytes())
    }

    fn extracted_path(&self, body_hash: &str) -> PathBuf {
        self.root
            .join("extracted")
            .join(EXTRACTOR_VERSION)
            .join(format!("{body_hash}.html"))
    }
}

fn article_key(url: &Url) -> String {
    crate::model::sha1_hex(crate::model::normalize_link(url.as_str()).as_bytes())
}

fn write_if_missing(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.is_file() {
        return Ok(());
    }
    write(path, bytes)
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("cache file has a parent directory")?;
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::Body;
    use tempfile::tempdir;

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
        };
        let stored = cache.store(&requested, &body).unwrap();
        cache
            .store_extracted(&stored.body_hash, "<article>clean</article>")
            .unwrap();

        let equivalent = Url::parse("https://example.com/post").unwrap();
        let loaded = ArticleCache::new(root.path())
            .load(&equivalent)
            .unwrap()
            .unwrap();
        assert_eq!(loaded.bytes, body.bytes);
        assert_eq!(loaded.etag.as_deref(), Some("\"v1\""));
        assert_eq!(
            cache.extracted(&loaded.body_hash).unwrap().as_deref(),
            Some("<article>clean</article>")
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
    fn render_fingerprint_tracks_loaded_config_and_theme_files() {
        let root = tempdir().unwrap();
        let config_path = root.path().join("aggr.toml");
        std::fs::write(&config_path, "[site]\ntitle = \"one\"\n").unwrap();
        std::fs::create_dir_all(root.path().join("templates")).unwrap();
        std::fs::write(root.path().join("templates/index.html"), "one").unwrap();
        let mut config = crate::config::Config::parse("[site]\ntitle = \"one\"\n").unwrap();
        config.loaded_files = vec![config_path.clone()];
        let first = render_fingerprint(
            &config,
            root.path(),
            Some("config"),
            Some("data"),
            None,
            false,
        )
        .unwrap();

        std::fs::write(root.path().join("templates/index.html"), "two").unwrap();
        let theme_changed = render_fingerprint(
            &config,
            root.path(),
            Some("config"),
            Some("data"),
            None,
            false,
        )
        .unwrap();
        assert_ne!(first, theme_changed);

        std::fs::write(&config_path, "[site]\ntitle = \"two\"\n").unwrap();
        let config_changed = render_fingerprint(
            &config,
            root.path(),
            Some("config"),
            Some("data"),
            None,
            false,
        )
        .unwrap();
        assert_ne!(theme_changed, config_changed);
    }
}
