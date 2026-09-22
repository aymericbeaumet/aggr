//! The data branch as a directory tree: per-source state and dedupe keys, the status file,
//! and the `items/` pairs. Everything here is plain files; `git.rs` turns them into commits.

pub mod frontmatter;
pub mod retention;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::model::{ArticleImage, FrontMatter, Item};

pub const README: &str = include_str!("branch-readme.md");
pub const GITATTRIBUTES: &str = "\
# aggr data branch. Concurrent runs append to seen.txt; union merge keeps both sides.
sources/*/seen.txt merge=union
* text=auto eol=lf
*.pdf -text
";
/// Atomic writes stage `.tmpXXXXXX` files (tempfile's default prefix) beside their target; a run
/// killed before `persist` leaves one behind, and `git add -A` must never commit it.
pub const GITIGNORE: &str = "\
# aggr data branch. Interrupted atomic writes leave temporary files; they never belong in history.
.tmp*
";

pub struct Store {
    root: PathBuf,
    image_cache: Option<crate::media::StoredAssetCache>,
}

/// Per-source fetch state, stored as `sources/<slug>/state.toml`. Only rewritten when it changes,
/// so an unchanged upstream leaves the tree untouched.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceState {
    /// Hash of unexpanded fetch inputs. A change invalidates the discovered endpoint below while
    /// literal credentials and `${ENV}` values never enter the append-only history.
    pub identity: String,
    /// Feed endpoint discovered from a website, or the final page URL for HTML fallback.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site_url: Option<String>,
    /// Canonical BCP 47 tag the publisher declares for the whole source; items inherit it at
    /// build time, so archives that predate the field pick it up on their next build.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_hash: Option<String>,
}

/// `status.toml`: only sources currently in error. A healthy source has no entry, so the file
/// changes exactly on ok↔error transitions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Status {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub errors: BTreeMap<String, SourceError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceError {
    pub message: String,
    pub since: DateTime<Utc>,
}

/// Outcome of fetching one source, as far as the status file is concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Error(String),
}

impl Status {
    /// Apply this run's outcomes. Sources absent from `outcomes` (removed from the config, or not
    /// selected) are left as they are; `known` prunes entries for sources no longer configured.
    /// Returns whether anything changed.
    pub fn apply(
        &mut self,
        outcomes: &BTreeMap<String, Outcome>,
        known: &BTreeSet<String>,
        now: DateTime<Utc>,
    ) -> bool {
        let before = self.clone();
        for (slug, outcome) in outcomes {
            match outcome {
                Outcome::Ok => {
                    self.errors.remove(slug);
                }
                Outcome::Error(message) => {
                    self.errors.entry(slug.clone()).or_insert(SourceError {
                        message: message.clone(),
                        since: now,
                    });
                }
            }
        }
        self.errors.retain(|slug, _| known.contains(slug));
        *self != before
    }
}

/// A new item ready to be written as a `.md`/`.html` pair.
pub struct NewItem<'a> {
    pub dir: &'a str,
    pub stem: &'a str,
    pub front: &'a FrontMatter,
    pub body: &'a str,
    pub html: Option<&'a str>,
    pub preview: Option<&'a [u8]>,
    pub images: &'a [crate::media::Asset],
}

#[derive(Debug, Clone)]
pub struct StoredImage {
    pub metadata: ArticleImage,
    pub original: Vec<u8>,
    pub variants: Vec<Vec<u8>>,
}

pub(crate) const MAX_STORED_HTML_BYTES: usize = 16 * 1024 * 1024;

/// Each source transaction owns its attributes file, so another source's rollback cannot
/// remove the rule that protects already-retained PDF bytes from Git's line-ending conversion.
pub(crate) fn document_attributes_path(directory: &Path) -> Result<PathBuf> {
    let mut components = directory.components();
    if components.next() != Some(Component::Normal(std::ffi::OsStr::new("items"))) {
        bail!("document directory is outside items");
    }
    let Some(Component::Normal(source)) = components.next() else {
        bail!("document directory has no owning source");
    };
    Ok(Path::new("items").join(source).join(".gitattributes"))
}

struct StoredMediaBudget {
    files: usize,
    bytes: usize,
}

impl StoredMediaBudget {
    fn new(limits: &crate::media::MediaLimits) -> Self {
        let files_per_asset = crate::media::MAX_STORED_RENDITIONS + 1;
        Self {
            files: limits.max_assets.saturating_mul(files_per_asset),
            bytes: limits.max_article_bytes,
        }
    }

    fn reserve(&mut self, bytes: usize) -> bool {
        if self.files == 0 || bytes > self.bytes {
            return false;
        }
        self.files -= 1;
        self.bytes -= bytes;
        true
    }
}

impl Store {
    pub fn open(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            image_cache: None,
        }
    }

    /// Reuse validated image results only in the caller's existing disposable cache boundary.
    pub fn with_image_cache(mut self, root: impl AsRef<Path>) -> Self {
        self.image_cache = Some(crate::media::StoredAssetCache::new(root));
        self
    }

    /// The validation receipt cache this store reuses, when one was attached.
    pub fn image_cache(&self) -> Option<&crate::media::StoredAssetCache> {
        self.image_cache.as_ref()
    }

    /// Write the branch README, `.gitattributes` and `.gitignore` when missing. Each file is
    /// checked on its own, so a branch created by an older version gains the files it lacks.
    /// Returns whether anything was written.
    pub fn bootstrap(&self) -> Result<bool> {
        let mut wrote = false;
        for (name, content) in [
            ("README.md", README),
            (".gitattributes", GITATTRIBUTES),
            (".gitignore", GITIGNORE),
        ] {
            let path = self.checked_path(Path::new(name))?;
            if !path.exists() {
                self.write_text(Path::new(name), content)?;
                wrote = true;
            }
        }
        Ok(wrote)
    }

    fn source_dir(&self, slug: &str) -> PathBuf {
        self.root.join("sources").join(slug)
    }

    /// Per-source fetch state. A file that fails to parse is logged and read as absent so one
    /// hand edit never wedges every future sync; the next write replaces it with a well-formed one.
    pub fn source_state(&self, slug: &str) -> Result<SourceState> {
        let path = self.source_dir(slug).join("state.toml");
        match fs::read_to_string(&path) {
            Ok(text) => Ok(parse_regenerated(&path, &text)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(SourceState::default()),
            Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Persist the state if it differs from what is on disk. Returns whether it was written.
    pub fn write_source_state(&self, slug: &str, state: &SourceState) -> Result<bool> {
        let relative = Path::new("sources").join(slug).join("state.toml");
        let path = self.checked_path(&relative)?;
        let text = toml::to_string(state).context("serializing state")?;
        if fs::read_to_string(&path).ok().as_deref() == Some(&text) {
            return Ok(false);
        }
        self.write_text(&relative, &text)?;
        Ok(true)
    }

    /// Dedupe keys already recorded for a source.
    pub fn seen(&self, slug: &str) -> Result<BTreeSet<String>> {
        let path = self.source_dir(slug).join("seen.txt");
        match fs::read_to_string(&path) {
            Ok(text) => Ok(text
                .lines()
                .filter_map(|line| line.split_whitespace().next())
                .map(str::to_string)
                .collect()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(BTreeSet::new()),
            Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn append_seen(&self, slug: &str, keys: &[String], date: DateTime<Utc>) -> Result<()> {
        if keys.is_empty() {
            return Ok(());
        }
        let relative = Path::new("sources").join(slug).join("seen.txt");
        let path = self.checked_path(&relative)?;
        let day = date.format("%Y-%m-%d");
        let mut text = fs::read_to_string(&path).unwrap_or_default();
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        for key in keys {
            text.push_str(&format!("{key} {day}\n"));
        }
        self.write_text(&relative, &text)
    }

    /// Sources currently in error. A file that fails to parse is logged and read as healthy so
    /// one hand edit never wedges every future sync; the next transition rewrites it.
    pub fn status(&self) -> Result<Status> {
        let path = self.root.join("status.toml");
        match fs::read_to_string(&path) {
            Ok(text) => Ok(parse_regenerated(&path, &text)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Status::default()),
            Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn write_status(&self, status: &Status) -> Result<()> {
        let relative = Path::new("status.toml");
        let path = self.checked_path(relative)?;
        if status.errors.is_empty() {
            if path.exists() {
                fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
            }
            return Ok(());
        }
        let text = toml::to_string(status).context("serializing status")?;
        self.write_text(relative, &text)
    }

    pub fn stem_exists(&self, dir: &str, stem: &str) -> bool {
        self.root.join(dir).join(format!("{stem}.md")).exists()
    }

    /// A stat-only repair hint: optional renditions never require downloading a usable master
    /// again. Writes and rendering still validate bytes; presence alone is not integrity proof.
    pub fn image_files_present(&self, item_path: &str, metadata: &ArticleImage) -> bool {
        (|| -> Result<bool> {
            if metadata.variants.len() > crate::media::MAX_STORED_RENDITIONS {
                return Ok(false);
            }
            let directory = self.stored_image_directory(item_path, metadata)?;
            let relative = directory.strip_prefix(normalized_absolute(&self.root)?)?;
            let path = self.checked_path(&relative.join(&metadata.original.file))?;
            let file = fs::symlink_metadata(path)?;
            Ok(file.file_type().is_file()
                && file.len() > 0
                && file.len() <= crate::media::MediaLimits::default().max_file_bytes as u64)
        })()
        .unwrap_or(false)
    }

    /// Retained-byte estimate for bounded repair downloads, without reading or decoding images.
    pub fn image_bytes_present(&self, item_path: &str, metadata: &ArticleImage) -> usize {
        if !self.image_files_present(item_path, metadata) {
            return 0;
        }
        let Ok(directory) = self.stored_image_directory(item_path, metadata) else {
            return 0;
        };
        let Ok(root) = normalized_absolute(&self.root) else {
            return 0;
        };
        let Ok(relative) = directory.strip_prefix(root) else {
            return 0;
        };
        std::iter::once(&metadata.original)
            .chain(&metadata.variants)
            .filter_map(|image| {
                let path = self.checked_path(&relative.join(&image.file)).ok()?;
                let file = fs::symlink_metadata(path).ok()?;
                (file.file_type().is_file()
                    && file.len() <= crate::media::MediaLimits::default().max_file_bytes as u64)
                    .then(|| usize::try_from(file.len()).ok())
                    .flatten()
            })
            .sum()
    }

    pub fn write_item(&self, item: NewItem<'_>) -> Result<()> {
        self.write_item_with_document(item, None)
    }

    pub fn write_item_with_document(
        &self,
        item: NewItem<'_>,
        document: Option<&crate::document::Asset>,
    ) -> Result<()> {
        let limits = crate::media::MediaLimits::default();
        if item.front.images.len() > limits.max_assets {
            bail!("article has too many stored images");
        }
        let relative_dir = PathBuf::from(item.dir);
        let md = relative_dir.join(format!("{}.md", item.stem));
        let mut targets = vec![md.clone()];
        let item_path = format!("{}/{}", item.dir, item.stem);
        let mut front = item.front.clone();
        let mut document_attributes = None;
        if let Some(asset) = document {
            let metadata = asset.metadata(item.stem);
            crate::document::validate_stored(&asset.bytes, &metadata, item.stem)?;
            if front
                .document
                .as_ref()
                .is_some_and(|value| *value != metadata)
            {
                bail!("document metadata does not match supplied bytes");
            }
            targets.push(relative_dir.join(&metadata.file));
            let relative = document_attributes_path(&relative_dir)?;
            let path = self.checked_path(&relative)?;
            let mut attributes = match fs::read_to_string(&path) {
                Ok(attributes) => attributes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(error) => return Err(error).context("reading document Git attributes"),
            };
            if attributes.lines().last() != Some("*.pdf -text") {
                if !attributes.is_empty() && !attributes.ends_with('\n') {
                    attributes.push('\n');
                }
                attributes.push_str("*.pdf -text\n");
                document_attributes = Some((relative.clone(), attributes));
            }
            targets.push(relative);
            front.document = Some(metadata);
        } else {
            let existing = self.read_item(&item_path).ok();
            if front.document.is_some()
                && front.document
                    != existing
                        .as_ref()
                        .and_then(|item| item.front.document.clone())
            {
                bail!("retained document metadata changed without supplied bytes");
            }
            front.document = match existing {
                Some(existing) => self
                    .read_document(&existing)?
                    .map(|asset| asset.metadata(item.stem)),
                None => None,
            };
        }

        if let Some(preview) = &item.front.preview {
            if !preview.is_valid_for(item.stem) {
                bail!("invalid preview companion name");
            }
            if let Some(bytes) = item.preview {
                crate::preview::validate_stored(bytes, preview)?;
            }
            let relative = relative_dir.join(&preview.file);
            let file = self.checked_path(&relative)?;
            if item.preview.is_none() && !file.is_file() {
                bail!("preview companion is missing");
            }
            targets.push(relative);
        } else if item.preview.is_some() {
            bail!("preview bytes have no metadata");
        }

        let existing = if item.front.images.len() > item.images.len() {
            Some(
                self.read_item(&item_path)
                    .context("reading retained image metadata")?,
            )
        } else {
            None
        };
        let mut supplied = BTreeMap::new();
        for image in item.images {
            if supplied.insert(image.source_url.as_str(), image).is_some() {
                bail!("duplicate supplied article image source");
            }
        }
        let mut sources = BTreeSet::new();
        let mut image_files = BTreeMap::new();
        let mut budget = StoredMediaBudget::new(&limits);
        for metadata in &item.front.images {
            if metadata.variants.len() > crate::media::MAX_STORED_RENDITIONS
                || !metadata.is_valid_for(item.stem)
                || !sources.insert(metadata.source.as_str())
            {
                bail!("invalid or duplicate article image metadata");
            }
            if let Some(image) = supplied.remove(metadata.source.as_str()) {
                if image.metadata(item.stem) != *metadata {
                    bail!("article image metadata does not match its bytes");
                }
                for companion in image.files(item.stem) {
                    if !budget.reserve(companion.bytes.len()) {
                        bail!("stored article media exceeds the item budget");
                    }
                    crate::media::validate_stored(
                        companion.bytes,
                        &companion.metadata,
                        companion.kind,
                    )?;
                    let relative = relative_dir.join(&companion.metadata.file);
                    if !self.image_companion_matches(&relative, companion.bytes)? {
                        if image_files
                            .insert(relative.clone(), companion.bytes)
                            .is_some_and(|bytes| bytes != companion.bytes)
                        {
                            bail!("conflicting article image companion bytes");
                        }
                        targets.push(relative);
                    }
                }
            } else {
                if !existing
                    .as_ref()
                    .is_some_and(|existing| existing.front.images.contains(metadata))
                {
                    bail!("retained article image metadata changed without supplied bytes");
                }
                self.validate_retained_image(&item_path, metadata, &mut budget)?;
            }
        }
        if !supplied.is_empty() {
            bail!("supplied article image has no metadata");
        }

        // Markdown is the item pair's visibility marker: write the optional sibling first, then
        // atomically publish the `.md`. A crash can leave an ignored orphan HTML file, never a
        // Markdown record pointing at a partial companion.
        if let Some(html) = item.html {
            if html.len() > MAX_STORED_HTML_BYTES {
                bail!("stored HTML companion exceeds the size limit");
            }
            targets.push(relative_dir.join(format!("{}.html", item.stem)));
        }
        for target in &targets {
            self.checked_path(target)?;
        }

        let markdown = frontmatter::render(&front, item.body)?;
        if let Some((relative, attributes)) = document_attributes {
            self.write_text(&relative, &attributes)?;
        }
        if let (Some(asset), Some(metadata)) = (document, &front.document) {
            let relative = relative_dir.join(&metadata.file);
            let path = self.checked_path(&relative)?;
            let unchanged = fs::symlink_metadata(&path).is_ok_and(|metadata| {
                metadata.file_type().is_file() && metadata.len() == asset.bytes.len() as u64
            }) && fs::read(&path).is_ok_and(|bytes| bytes == asset.bytes);
            if !unchanged {
                self.write_bytes(&relative, &asset.bytes)?;
            }
        }
        if let (Some(preview), Some(bytes)) = (&item.front.preview, item.preview) {
            self.write_bytes(&relative_dir.join(&preview.file), bytes)?;
        }
        for (relative, bytes) in image_files {
            self.write_image_companion(&relative, bytes)?;
        }
        if let Some(html) = item.html {
            self.write_text(&relative_dir.join(format!("{}.html", item.stem)), html)?;
        }
        let path = self.checked_path(&md)?;
        if !fs::read(&path).is_ok_and(|bytes| bytes == markdown.as_bytes()) {
            self.write_text(&md, &markdown)?;
        }
        Ok(())
    }

    /// Delete an item's companions and Markdown. Its dedupe keys stay in `seen.txt`.
    pub fn remove_item(&self, path: &str) -> Result<()> {
        for extension in ["md", "html"] {
            self.checked_path(Path::new(&format!("{path}.{extension}")))?;
        }
        if let Ok(item) = self.read_item(path)
            && let Some(preview) = &item.front.preview
            && let Some(stem) = Path::new(path).file_name().and_then(|name| name.to_str())
            && preview.is_valid_for(stem)
        {
            let parent = Path::new(path).parent().unwrap_or_else(|| Path::new(""));
            let relative = parent.join(&preview.file);
            let file = self.checked_path(&relative)?;
            match fs::remove_file(&file) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => {
                    return Err(err).with_context(|| format!("removing {}", file.display()));
                }
            }
        }
        if let Ok(item) = self.read_item(path)
            && let Some(stem) = Path::new(path).file_name().and_then(|name| name.to_str())
        {
            let parent = Path::new(path).parent().unwrap_or_else(|| Path::new(""));
            if let Some(document) = item
                .front
                .document
                .as_ref()
                .filter(|document| document.is_valid_for(stem))
            {
                let file = self.checked_path(&parent.join(&document.file))?;
                match fs::remove_file(&file) {
                    Ok(()) => {}
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                    Err(err) => {
                        return Err(err).with_context(|| format!("removing {}", file.display()));
                    }
                }
            }
            for image in item
                .front
                .images
                .iter()
                .filter(|image| image.is_valid_for(stem))
            {
                for file in std::iter::once(&image.original).chain(&image.variants) {
                    let path = self.checked_path(&parent.join(&file.file))?;
                    match fs::remove_file(&path) {
                        Ok(()) => {}
                        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                        Err(err) => {
                            return Err(err)
                                .with_context(|| format!("removing {}", path.display()));
                        }
                    }
                }
            }
        }
        for ext in ["md", "html"] {
            let file = self.checked_path(Path::new(&format!("{path}.{ext}")))?;
            match fs::remove_file(&file) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => {
                    return Err(err).with_context(|| format!("removing {}", file.display()));
                }
            }
        }
        Ok(())
    }

    pub fn read_item(&self, path: &str) -> Result<Item> {
        let file = self.checked_path(Path::new(&format!("{path}.md")))?;
        let text =
            fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
        let (mut front, body) = frontmatter::parse::<FrontMatter>(&text)
            .with_context(|| format!("parsing {}", file.display()))?;
        let stem = file
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if front
            .preview
            .as_ref()
            .is_some_and(|preview| !preview.is_valid_for(stem))
        {
            log::debug!("ignoring invalid preview metadata for {path}");
            front.preview = None;
        }
        let mut image_sources = BTreeSet::new();
        if front
            .document
            .as_ref()
            .is_some_and(|document| !document.is_valid_for(stem))
        {
            log::debug!("ignoring invalid document metadata for {path}");
            front.document = None;
        }
        front.images.retain(|image| {
            let valid = image.is_valid_for(stem) && image_sources.insert(image.source.clone());
            if !valid {
                log::debug!("ignoring invalid article image metadata for {path}");
            }
            valid
        });
        Ok(Item {
            path: path.to_string(),
            front,
            body: body.to_string(),
        })
    }

    /// Every item under `items/`, unsorted. Files that fail to parse are logged and skipped so
    /// one hand edit never takes the site down.
    /// Every archived item. Each is an independent file read and parse, so the archive is read on
    /// the shared worker pool and the results keep their sorted path order.
    pub fn items(&self) -> Result<Vec<Item>> {
        let paths = self.item_paths()?;
        let read = crate::site::parallel::map(&paths, |path| {
            Ok(match self.read_item(path) {
                Ok(item) => Some(item),
                Err(err) => {
                    log::warn!("skipping {path}.md: {err:#}");
                    None
                }
            })
        })?;
        Ok(read.into_iter().flatten().collect())
    }

    /// Relative item paths (without extension), for stub generation and sorting.
    pub fn item_paths(&self) -> Result<Vec<String>> {
        let items_dir = self.root.join("items");
        if !items_dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut paths = Vec::new();
        for entry in walkdir::WalkDir::new(&items_dir).sort_by_file_name() {
            let entry = entry.context("walking items")?;
            let path = entry.path();
            if entry.file_type().is_file() && path.extension().is_some_and(|ext| ext == "md") {
                let rel = path
                    .strip_prefix(&self.root)
                    .expect("under root")
                    .with_extension("");
                paths.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
        Ok(paths)
    }

    pub fn read_html(&self, item: &Item) -> Result<Option<String>> {
        let Some(companion) = item.front.html.as_deref() else {
            return Ok(None);
        };
        let result = (|| -> Result<String> {
            let path = Path::new(&item.path);
            let stem = path
                .file_name()
                .and_then(|name| name.to_str())
                .context("stored HTML has no owning item")?;
            if path
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
                || companion != format!("{stem}.html")
            {
                bail!("invalid stored HTML companion metadata");
            }
            let relative = path
                .parent()
                .unwrap_or_else(|| Path::new(""))
                .join(companion);
            let file = self.checked_path(&relative)?;
            let metadata = fs::symlink_metadata(&file)?;
            if !metadata.file_type().is_file() || metadata.len() > MAX_STORED_HTML_BYTES as u64 {
                bail!("invalid stored HTML companion");
            }
            if !file.canonicalize()?.starts_with(self.root.canonicalize()?) {
                bail!("stored HTML escapes the store");
            }
            use std::io::Read as _;
            let mut bytes = Vec::with_capacity(metadata.len() as usize);
            fs::File::open(&file)?
                .take((MAX_STORED_HTML_BYTES + 1) as u64)
                .read_to_end(&mut bytes)?;
            if bytes.len() > MAX_STORED_HTML_BYTES {
                bail!("stored HTML companion exceeds the size limit");
            }
            String::from_utf8(bytes).context("stored HTML companion is not UTF-8")
        })();
        match result {
            Ok(html) => Ok(Some(html)),
            Err(error) => {
                log::debug!(
                    "ignoring unavailable stored HTML for {}: {error:#}",
                    item.path
                );
                Ok(None)
            }
        }
    }

    /// Optional documents degrade to their publisher URL if missing, unsafe or corrupted.
    pub fn read_document(&self, item: &Item) -> Result<Option<crate::document::Asset>> {
        let Some(document) = &item.front.document else {
            return Ok(None);
        };
        let result = (|| -> Result<crate::document::Asset> {
            let path = Path::new(&item.path);
            let stem = path
                .file_name()
                .and_then(|name| name.to_str())
                .context("stored document has no owning item")?;
            if !document.is_valid_for(stem) {
                bail!("invalid stored document metadata");
            }
            let relative = path
                .parent()
                .unwrap_or_else(|| Path::new(""))
                .join(&document.file);
            let file = self.checked_path(&relative)?;
            let metadata = fs::symlink_metadata(&file)?;
            if !metadata.file_type().is_file()
                || metadata.len() > crate::document::MAX_DOCUMENT_BYTES as u64
            {
                bail!("invalid stored document companion");
            }
            if !file.canonicalize()?.starts_with(self.root.canonicalize()?) {
                bail!("stored document escapes the store");
            }
            use std::io::Read as _;
            let mut bytes = Vec::with_capacity(metadata.len() as usize);
            fs::File::open(&file)?
                .take((crate::document::MAX_DOCUMENT_BYTES + 1) as u64)
                .read_to_end(&mut bytes)?;
            crate::document::validate_stored(&bytes, document, stem)?;
            Ok(crate::document::Asset {
                source_url: document.source.clone(),
                bytes,
            })
        })();
        match result {
            Ok(asset) => Ok(Some(asset)),
            Err(error) => {
                log::debug!(
                    "ignoring unavailable stored document for {}: {error:#}",
                    item.path
                );
                Ok(None)
            }
        }
    }

    /// Repair only absent companions; unsafe paths and other filesystem failures remain errors.
    pub fn preview_file_present(
        &self,
        item_path: &str,
        preview: &crate::model::Preview,
    ) -> Result<bool> {
        let path = Path::new(item_path);
        let stem = path
            .file_name()
            .and_then(|value| value.to_str())
            .context("preview item path has no filename")?;
        if !preview.is_valid_for(stem) {
            bail!("invalid preview metadata for {item_path}");
        }
        let file = self.checked_path(
            &path
                .parent()
                .unwrap_or_else(|| Path::new(""))
                .join(&preview.file),
        )?;
        match fs::symlink_metadata(&file) {
            Ok(metadata) if metadata.file_type().is_file() => Ok(true),
            Ok(_) => bail!(
                "preview companion is not a regular file: {}",
                file.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error).with_context(|| format!("inspecting preview for {item_path}")),
        }
    }

    /// Optional image failures never prevent an otherwise readable article from being built.
    pub fn read_preview(&self, item: &Item) -> Result<Option<Vec<u8>>> {
        let Some(preview) = &item.front.preview else {
            return Ok(None);
        };
        let path = Path::new(&item.path);
        let Some(stem) = path.file_name().and_then(|name| name.to_str()) else {
            return Ok(None);
        };
        if !preview.is_valid_for(stem)
            || path
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Ok(None);
        }
        let relative = path
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .join(&preview.file);
        let file = match self.checked_path(&relative) {
            Ok(file) => file,
            Err(error) => {
                log::debug!("ignoring unavailable preview for {}: {error:#}", item.path);
                return Ok(None);
            }
        };
        let result = (|| -> Result<Vec<u8>> {
            let metadata = fs::symlink_metadata(&file)?;
            if !metadata.file_type().is_file() || metadata.len() > crate::preview::MAX_BYTES as u64
            {
                bail!("invalid preview file");
            }
            if !file.canonicalize()?.starts_with(self.root.canonicalize()?) {
                bail!("preview escapes the store");
            }
            use std::io::Read as _;
            let mut bytes = Vec::new();
            fs::File::open(&file)?
                .take((crate::preview::MAX_BYTES + 1) as u64)
                .read_to_end(&mut bytes)?;
            crate::preview::validate_stored(&bytes, preview)?;
            Ok(bytes)
        })();
        match result {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) => {
                log::debug!("ignoring unavailable preview for {}: {error:#}", item.path);
                Ok(None)
            }
        }
    }

    /// Invalid optional companions degrade to the publisher URLs retained in Markdown.
    pub fn read_images(&self, item: &Item) -> Result<Vec<StoredImage>> {
        let limits = crate::media::MediaLimits::default();
        let mut budget = StoredMediaBudget::new(&limits);
        let mut images = Vec::new();
        for metadata in item.front.images.iter().take(limits.max_assets) {
            match self.read_available_stored_image(&item.path, metadata, &mut budget) {
                Ok(image) => images.push(image),
                Err(error) => {
                    log::debug!(
                        "ignoring unavailable article image for {}: {error:#}",
                        item.path
                    );
                }
            }
        }
        Ok(images)
    }

    /// Read and reconstruct build-ready article images in one bounded validation/decode pass.
    /// Missing or corrupt optional renditions are ignored; an invalid master drops only its image.
    pub fn read_image_assets(&self, item: &Item) -> Result<Vec<crate::media::Asset>> {
        let limits = crate::media::MediaLimits::default();
        let mut budget = StoredMediaBudget::new(&limits);
        let mut images = Vec::new();
        for metadata in item.front.images.iter().take(limits.max_assets) {
            let restored = self
                .read_available_image_bytes(&item.path, metadata, &mut budget)
                .and_then(|stored| match &self.image_cache {
                    Some(cache) => {
                        cache.restore(&stored.metadata, stored.original, stored.variants)
                    }
                    None => crate::media::Asset::from_stored(
                        &stored.metadata,
                        stored.original,
                        stored.variants,
                    ),
                });
            match restored {
                Ok(image) => images.push(image),
                Err(error) => log::debug!(
                    "ignoring unavailable article image for {}: {error:#}",
                    item.path
                ),
            }
        }
        Ok(images)
    }

    fn validate_retained_image(
        &self,
        item_path: &str,
        metadata: &ArticleImage,
        budget: &mut StoredMediaBudget,
    ) -> Result<()> {
        if metadata.variants.len() > crate::media::MAX_STORED_RENDITIONS {
            bail!("stored article image has too many renditions");
        }
        let directory = self.stored_image_directory(item_path, metadata)?;
        for (index, file) in std::iter::once(&metadata.original)
            .chain(&metadata.variants)
            .enumerate()
        {
            let bytes = match self.read_image_bytes(&directory, file, budget) {
                Ok(bytes) => bytes,
                Err(error)
                    if index > 0
                        && error
                            .downcast_ref::<std::io::Error>()
                            .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
                {
                    continue;
                }
                Err(error) => return Err(error),
            };
            // Metadata must already match the archive. Verify its immutable content address,
            // without decoding unchanged masters and recomputing their responsive renditions.
            let hash = crate::model::sha1_hex(&bytes);
            let stored_hash = file
                .file
                .rsplit_once(".image-")
                .and_then(|(_, suffix)| suffix.split_once('.'))
                .map(|(hash, _)| hash);
            if stored_hash != Some(&hash[..12]) {
                bail!("retained article image bytes do not match their content address");
            }
        }
        Ok(())
    }

    fn read_available_stored_image(
        &self,
        item_path: &str,
        metadata: &ArticleImage,
        budget: &mut StoredMediaBudget,
    ) -> Result<StoredImage> {
        let max_renditions = crate::media::MAX_STORED_RENDITIONS;
        if metadata.variants.len() > max_renditions {
            bail!("stored article image has too many renditions");
        }
        let directory = self.stored_image_directory(item_path, metadata)?;
        let original = self.read_image_file(
            &directory,
            &metadata.original,
            crate::media::StoredKind::Master,
            budget,
        )?;
        let mut available = metadata.clone();
        available.variants.clear();
        let mut variants = Vec::with_capacity(metadata.variants.len().min(max_renditions));
        for file in metadata.variants.iter().take(max_renditions) {
            match self.read_image_file(
                &directory,
                file,
                crate::media::StoredKind::Rendition,
                budget,
            ) {
                Ok(bytes) => {
                    available.variants.push(file.clone());
                    variants.push(bytes);
                }
                Err(error) => log::debug!(
                    "ignoring unavailable article image rendition for {item_path}: {error:#}"
                ),
            }
        }
        Ok(StoredImage {
            metadata: available,
            original,
            variants,
        })
    }

    fn read_available_image_bytes(
        &self,
        item_path: &str,
        metadata: &ArticleImage,
        budget: &mut StoredMediaBudget,
    ) -> Result<StoredImage> {
        let limits = crate::media::MediaLimits::default();
        let max_renditions = crate::media::MAX_STORED_RENDITIONS;
        if metadata.variants.len() > max_renditions {
            bail!("stored article image has too many renditions");
        }
        let directory = self.stored_image_directory(item_path, metadata)?;
        let original = self.read_image_bytes(&directory, &metadata.original, budget)?;
        let mut retained = original.len();
        let mut available = metadata.clone();
        available.variants.clear();
        let mut variants = Vec::with_capacity(metadata.variants.len().min(max_renditions));
        for file in metadata.variants.iter().take(max_renditions) {
            match self.read_image_bytes(&directory, file, budget) {
                Ok(bytes)
                    if retained
                        .checked_add(bytes.len())
                        .is_some_and(|total| total <= limits.max_article_bytes) =>
                {
                    retained += bytes.len();
                    available.variants.push(file.clone());
                    variants.push(bytes);
                }
                Ok(_) => log::debug!(
                    "ignoring article image rendition beyond the size limit for {item_path}"
                ),
                Err(error) => log::debug!(
                    "ignoring unavailable article image rendition for {item_path}: {error:#}"
                ),
            }
        }
        Ok(StoredImage {
            metadata: available,
            original,
            variants,
        })
    }

    fn stored_image_directory(&self, item_path: &str, metadata: &ArticleImage) -> Result<PathBuf> {
        let path = Path::new(item_path);
        let stem = path
            .file_name()
            .and_then(|name| name.to_str())
            .context("article image has no owning item")?;
        if !metadata.is_valid_for(stem)
            || path
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            bail!("invalid article image metadata");
        }
        Ok(normalized_absolute(&self.root)?.join(path.parent().unwrap_or_else(|| Path::new(""))))
    }

    fn read_image_file(
        &self,
        directory: &Path,
        file: &crate::model::ImageFile,
        kind: crate::media::StoredKind,
        budget: &mut StoredMediaBudget,
    ) -> Result<Vec<u8>> {
        let bytes = self.read_image_bytes(directory, file, budget)?;
        crate::media::validate_stored(&bytes, file, kind)?;
        Ok(bytes)
    }

    fn read_image_bytes(
        &self,
        directory: &Path,
        file: &crate::model::ImageFile,
        budget: &mut StoredMediaBudget,
    ) -> Result<Vec<u8>> {
        let directory = directory
            .strip_prefix(normalized_absolute(&self.root)?)
            .context("article image directory escapes the store")?;
        let path = self.checked_path(&directory.join(&file.file))?;
        let filesystem = fs::symlink_metadata(&path)?;
        let max = crate::media::MediaLimits::default().max_file_bytes;
        if !filesystem.file_type().is_file() || filesystem.len() > max as u64 {
            bail!("invalid article image companion");
        }
        let length = usize::try_from(filesystem.len()).context("article image is too large")?;
        if !budget.reserve(length) {
            bail!("stored article media exceeds the item budget");
        }
        if !path.canonicalize()?.starts_with(self.root.canonicalize()?) {
            bail!("article image escapes the store");
        }
        use std::io::Read as _;
        let mut bytes = Vec::with_capacity(length);
        fs::File::open(&path)?
            .take((length + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > length {
            bail!("article image changed while it was being read");
        }
        Ok(bytes)
    }

    fn checked_path(&self, relative: &Path) -> Result<PathBuf> {
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            bail!(
                "store path must be relative and contained: {}",
                relative.display()
            );
        }

        let root = normalized_absolute(&self.root)?;
        match fs::symlink_metadata(&root) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                bail!("store root is not a regular directory: {}", root.display())
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("inspecting {}", root.display()));
            }
        }

        let mut parent = root.clone();
        for component in relative
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .components()
        {
            let Component::Normal(component) = component else {
                unreachable!("relative path components were validated")
            };
            parent.push(component);
            match fs::symlink_metadata(&parent) {
                Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                    bail!(
                        "store write parent is not a regular directory: {}",
                        parent.display()
                    )
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error) => {
                    return Err(error).with_context(|| format!("inspecting {}", parent.display()));
                }
            }
        }

        let path = root.join(relative);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                bail!("store target is not a regular file: {}", path.display())
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("inspecting {}", path.display()));
            }
        }
        Ok(path)
    }

    fn write_text(&self, relative: &Path, content: &str) -> Result<()> {
        self.write_bytes(relative, content.as_bytes())
    }

    fn image_companion_matches(&self, relative: &Path, content: &[u8]) -> Result<bool> {
        let path = self.checked_path(relative)?;
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
            Ok(metadata)
                if !metadata.file_type().is_file() || metadata.len() != content.len() as u64 =>
            {
                bail!("immutable article image companion has different bytes");
            }
            Ok(_) => {}
        }
        use std::io::Read as _;
        let mut existing = Vec::with_capacity(content.len());
        fs::File::open(path)?
            .take(content.len() as u64 + 1)
            .read_to_end(&mut existing)?;
        if existing != content {
            bail!("immutable article image companion has different bytes");
        }
        Ok(true)
    }

    fn write_image_companion(&self, relative: &Path, content: &[u8]) -> Result<()> {
        if self.image_companion_matches(relative, content)? {
            return Ok(());
        }
        let path = self.checked_path(relative)?;
        let parent = path.parent().context("image companion has a parent")?;
        fs::create_dir_all(parent)?;
        let path = self.checked_path(relative)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(content)?;
        temporary.flush()?;
        match temporary.persist_noclobber(&path) {
            Ok(_) => Ok(()),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                if self.image_companion_matches(relative, content)? {
                    Ok(())
                } else {
                    bail!("article image companion disappeared during publication")
                }
            }
            Err(error) => Err(error.error.into()),
        }
    }

    fn write_bytes(&self, relative: &Path, content: &[u8]) -> Result<()> {
        let path = self.checked_path(relative)?;
        let parent = path.parent().context("store file has a parent directory")?;
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        let path = self.checked_path(relative)?;
        atomic_write_bytes(&path, content)
    }
}

/// Parse a regenerated TOML file (`status.toml`, `state.toml`). Unlike items, these hold nothing
/// the next sync cannot rebuild, so a malformed file degrades to the default with a warning.
fn parse_regenerated<T: Default + serde::de::DeserializeOwned>(path: &Path, text: &str) -> T {
    match toml::from_str(text) {
        Ok(value) => value,
        Err(err) => {
            log::warn!(
                "ignoring malformed {}; it will be regenerated: {err}",
                path.display()
            );
            T::default()
        }
    }
}

fn normalized_absolute(path: &Path) -> Result<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in std::path::absolute(path)?.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component),
        }
    }
    Ok(normalized)
}

#[cfg(test)]
fn write(path: &Path, content: &str) -> Result<()> {
    write_bytes(path, content.as_bytes())
}

#[cfg(test)]
fn write_bytes(path: &Path, content: &[u8]) -> Result<()> {
    atomic_write_bytes(path, content)
}

fn atomic_write_bytes(path: &Path, content: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        let mut file = tempfile::NamedTempFile::new_in(parent)
            .with_context(|| format!("creating a temporary file under {}", parent.display()))?;
        file.write_all(content)
            .with_context(|| format!("writing temporary file for {}", path.display()))?;
        file.flush()
            .with_context(|| format!("flushing temporary file for {}", path.display()))?;
        file.persist(path)
            .map_err(|err| err.error)
            .with_context(|| format!("replacing {}", path.display()))?;
        return Ok(());
    }
    fs::write(path, content).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn document_asset() -> crate::document::Asset {
        crate::document::Asset {
            source_url: "https://publisher.example/paper.pdf".into(),
            bytes: b"%PDF-1.7\narchived paper".to_vec(),
        }
    }

    fn write_document_item(
        store: &Store,
        front: &FrontMatter,
        document: Option<&crate::document::Asset>,
    ) -> Result<()> {
        store.write_item_with_document(
            NewItem {
                dir: "items/blog",
                stem: "paper",
                front,
                body: "Readable abstract",
                html: None,
                preview: None,
                images: &[],
            },
            document,
        )
    }

    #[test]
    fn document_roundtrip_survives_body_refresh_and_retention_removes_only_owned_companion() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path());
        let asset = document_asset();
        write_document_item(&store, &FrontMatter::default(), Some(&asset)).unwrap();
        let path = directory
            .path()
            .join("items/blog")
            .join(asset.metadata("paper").file);
        let timestamp = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(timestamp)
            .unwrap();
        write_document_item(&store, &FrontMatter::default(), None).unwrap();
        let item = store.read_item("items/blog/paper").unwrap();
        assert_eq!(item.front.document, Some(asset.metadata("paper")));
        assert_eq!(store.read_document(&item).unwrap(), Some(asset));
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), timestamp);
        let other = directory.path().join("items/blog/other.pdf");
        fs::write(&other, b"unrelated").unwrap();
        store.remove_item(&item.path).unwrap();
        assert!(!path.exists());
        assert!(other.exists());
    }

    #[test]
    fn invalid_document_write_leaves_no_markdown_and_corrupt_optional_document_degrades() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path());
        let mut asset = document_asset();
        asset.bytes = b"<html>challenge</html>".to_vec();
        assert!(write_document_item(&store, &FrontMatter::default(), Some(&asset)).is_err());
        assert!(!directory.path().join("items/blog/paper.md").exists());
        let asset = document_asset();
        write_document_item(&store, &FrontMatter::default(), Some(&asset)).unwrap();
        let item = store.read_item("items/blog/paper").unwrap();
        let path = directory
            .path()
            .join("items/blog")
            .join(asset.metadata("paper").file);
        fs::write(&path, b"%PDF-tampered").unwrap();
        assert_eq!(store.read_document(&item).unwrap(), None);
        write_document_item(&store, &item.front, None).unwrap();
        assert!(
            store
                .read_item(&item.path)
                .unwrap()
                .front
                .document
                .is_none()
        );
    }

    #[test]
    fn document_metadata_cannot_attach_an_unrelated_file() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path());
        let front = FrontMatter {
            document: Some(document_asset().metadata("other")),
            ..Default::default()
        };
        assert!(write_document_item(&store, &front, None).is_err());
        assert!(write_document_item(&store, &front, Some(&document_asset())).is_err());
        let parsed: FrontMatter =
            serde_yaml_ng::from_str("document: [invalid]\ntitle: Paper").unwrap();
        assert!(parsed.document.is_none());
    }

    #[test]
    fn document_git_roundtrip_keeps_crlf_bytes_and_preserves_source_attributes() {
        use std::process::Command;
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path());
        fs::create_dir_all(directory.path().join("items/blog")).unwrap();
        // An existing branch predates the PDF-specific root rule.
        fs::write(
            directory.path().join(".gitattributes"),
            "* text=auto eol=lf\n",
        )
        .unwrap();
        let attributes = directory.path().join("items/blog/.gitattributes");
        fs::write(&attributes, "*.md linguist-language=Markdown").unwrap();
        let asset = crate::document::Asset {
            source_url: "https://example.com/paper.pdf".into(),
            bytes: b"%PDF-1.7\r\nASCII PDF with CRLF\r\n%%EOF\r\n".to_vec(),
        };
        write_document_item(&store, &FrontMatter::default(), Some(&asset)).unwrap();
        assert_eq!(
            fs::read_to_string(&attributes).unwrap(),
            "*.md linguist-language=Markdown\n*.pdf -text\n"
        );
        write_document_item(&store, &FrontMatter::default(), Some(&asset)).unwrap();
        assert_eq!(
            fs::read_to_string(&attributes)
                .unwrap()
                .matches("*.pdf -text")
                .count(),
            1
        );
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(directory.path())
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            output.stdout
        };
        git(&["init", "--quiet"]);
        let path = format!("items/blog/{}", asset.metadata("paper").file);
        git(&["add", "--", &path]);
        assert_eq!(git(&["show", &format!(":{path}")]), asset.bytes);
    }

    #[cfg(unix)]
    #[test]
    fn document_symlink_cannot_be_read_written_or_followed_during_removal() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path());
        let asset = document_asset();
        write_document_item(&store, &FrontMatter::default(), Some(&asset)).unwrap();
        let item = store.read_item("items/blog/paper").unwrap();
        let path = directory
            .path()
            .join("items/blog")
            .join(asset.metadata("paper").file);
        let external = outside.path().join("paper.pdf");
        fs::write(&external, &asset.bytes).unwrap();
        fs::remove_file(&path).unwrap();
        symlink(&external, &path).unwrap();
        assert!(store.read_document(&item).unwrap().is_none());
        assert!(write_document_item(&store, &item.front, Some(&asset)).is_err());
        assert!(store.remove_item(&item.path).is_err());
        assert_eq!(fs::read(&external).unwrap(), asset.bytes);
    }

    fn repair_asset(source: &str, seed: u32) -> crate::media::Asset {
        let image =
            image::DynamicImage::ImageRgb8(image::ImageBuffer::from_fn(640, 400, |x, y| {
                image::Rgb([
                    ((x + seed) % 251) as u8,
                    ((y + seed) % 239) as u8,
                    ((x * y + seed) % 241) as u8,
                ])
            }));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        crate::media::prepare_asset(
            &crate::media::Candidate {
                url: source.parse().unwrap(),
                alt: None,
            },
            bytes.into_inner(),
            &crate::media::MediaLimits::default(),
        )
        .unwrap()
    }

    #[test]
    fn image_subset_write_preserves_companions_without_decoding_or_rewriting_existing_images() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path());
        let original = repair_asset("https://publisher.example/one.png", 1);
        let new = repair_asset("https://publisher.example/two.png", 2);
        let mut front = FrontMatter {
            images: vec![original.metadata("article")],
            ..Default::default()
        };
        let write = |front: &FrontMatter, images: &[crate::media::Asset], html| {
            store.write_item(NewItem {
                dir: "items/blog/2026/09",
                stem: "article",
                front,
                body: "Preserved markdown",
                html,
                preview: None,
                images,
            })
        };
        write(
            &front,
            std::slice::from_ref(&original),
            Some("<p>Preserved HTML</p>"),
        )
        .unwrap();
        let directory = dir.path().join("items/blog/2026/09");
        let timestamp = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
        for file in original.files("article") {
            fs::File::options()
                .write(true)
                .open(directory.join(file.metadata.file))
                .unwrap()
                .set_modified(timestamp)
                .unwrap();
        }
        front.images.push(new.metadata("article"));
        crate::media::reset_stored_decode_count();
        write(&front, std::slice::from_ref(&new), None).unwrap();
        assert_eq!(
            crate::media::stored_decode_count(),
            new.files("article").len()
        );
        for file in original.files("article") {
            let path = directory.join(file.metadata.file);
            assert_eq!(fs::read(&path).unwrap(), file.bytes);
            assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), timestamp);
        }
        assert_eq!(
            fs::read_to_string(directory.join("article.html")).unwrap(),
            "<p>Preserved HTML</p>"
        );
        assert_eq!(
            store
                .read_item("items/blog/2026/09/article")
                .unwrap()
                .front
                .images,
            front.images
        );
        let md = directory.join("article.md");
        fs::File::options()
            .write(true)
            .open(&md)
            .unwrap()
            .set_modified(timestamp)
            .unwrap();
        crate::media::reset_stored_decode_count();
        write(&front, &[], None).unwrap();
        assert_eq!(crate::media::stored_decode_count(), 0);
        assert_eq!(fs::metadata(md).unwrap().modified().unwrap(), timestamp);
    }

    #[test]
    fn image_subset_rejects_forged_unlisted_duplicate_and_corrupt_inputs_before_publication() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path());
        let original = repair_asset("https://publisher.example/one.png", 1);
        let new = repair_asset("https://publisher.example/two.png", 2);
        let original_front = FrontMatter {
            images: vec![original.metadata("article")],
            ..Default::default()
        };
        let write = |front: &FrontMatter, images: &[crate::media::Asset]| {
            store.write_item(NewItem {
                dir: "items/blog/2026/09",
                stem: "article",
                front,
                body: "Preserved",
                html: None,
                preview: None,
                images,
            })
        };
        write(&original_front, std::slice::from_ref(&original)).unwrap();
        let mut front = original_front.clone();
        front.images.push(new.metadata("article"));
        let md = dir.path().join("items/blog/2026/09/article.md");
        let saved = fs::read(&md).unwrap();
        let mut forged = front.clone();
        forged.images[0].color = Some("#123456".into());
        assert!(write(&forged, std::slice::from_ref(&new)).is_err());
        assert!(write(&original_front, std::slice::from_ref(&new)).is_err());
        assert!(write(&front, &[new.clone(), new.clone()]).is_err());
        let mut duplicated = front.clone();
        duplicated.images.push(original.metadata("article"));
        assert!(write(&duplicated, std::slice::from_ref(&new)).is_err());
        let original_path = dir
            .path()
            .join("items/blog/2026/09")
            .join(&original_front.images[0].original.file);
        fs::write(&original_path, b"corrupt").unwrap();
        assert!(write(&front, std::slice::from_ref(&new)).is_err());
        assert!(
            write(&original_front, std::slice::from_ref(&original)).is_err(),
            "existing immutable filenames cannot be overwritten"
        );
        assert_eq!(fs::read(&original_path).unwrap(), b"corrupt");
        assert_eq!(fs::read(md).unwrap(), saved);
        for file in new.files("article") {
            assert!(
                !dir.path()
                    .join("items/blog/2026/09")
                    .join(file.metadata.file)
                    .exists()
            );
        }
    }

    #[test]
    fn image_repair_presence_and_subset_writes_distinguish_missing_optional_and_required_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path());
        let original = repair_asset("https://publisher.example/one.png", 1);
        let new = repair_asset("https://publisher.example/two.png", 2);
        let mut front = FrontMatter {
            images: vec![original.metadata("article")],
            ..Default::default()
        };
        let write = |front: &FrontMatter, images: &[crate::media::Asset]| {
            store.write_item(NewItem {
                dir: "items/blog/2026/09",
                stem: "article",
                front,
                body: "Preserved",
                html: None,
                preview: None,
                images,
            })
        };
        write(&front, std::slice::from_ref(&original)).unwrap();
        let item_path = "items/blog/2026/09/article";
        let directory = dir.path().join("items/blog/2026/09");
        assert!(store.image_files_present(item_path, &front.images[0]));
        let original_bytes: usize = original
            .files("article")
            .iter()
            .map(|file| file.bytes.len())
            .sum();
        assert_eq!(
            store.image_bytes_present(item_path, &front.images[0]),
            original_bytes
        );
        let optional = directory.join(&front.images[0].variants[0].file);
        let optional_bytes = fs::metadata(&optional).unwrap().len() as usize;
        fs::remove_file(optional).unwrap();
        assert!(store.image_files_present(item_path, &front.images[0]));
        assert_eq!(
            store.image_bytes_present(item_path, &front.images[0]),
            original_bytes - optional_bytes
        );
        front.images.push(new.metadata("article"));
        write(&front, std::slice::from_ref(&new)).unwrap();
        assert_eq!(
            store.read_item(item_path).unwrap().front.images,
            front.images
        );
        let required = directory.join(&front.images[0].original.file);
        fs::remove_file(&required).unwrap();
        assert!(!store.image_files_present(item_path, &front.images[0]));
        assert!(write(&front, &[]).is_err());
        fs::create_dir(&required).unwrap();
        assert!(!store.image_files_present(item_path, &front.images[0]));
        fs::remove_dir(&required).unwrap();
        fs::write(&required, []).unwrap();
        assert!(!store.image_files_present(item_path, &front.images[0]));
        assert_eq!(store.image_bytes_present(item_path, &front.images[0]), 0);
        #[cfg(unix)]
        {
            fs::remove_file(&required).unwrap();
            let outside = dir.path().join("outside.png");
            fs::write(&outside, &original.master_bytes).unwrap();
            std::os::unix::fs::symlink(&outside, &required).unwrap();
            assert!(!store.image_files_present(item_path, &front.images[0]));
            assert_eq!(store.image_bytes_present(item_path, &front.images[0]), 0);
            assert!(write(&front, &[]).is_err());
            assert_eq!(fs::read(outside).unwrap(), original.master_bytes);
        }
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 2, 12, 0, 0).unwrap()
    }

    fn known(slugs: &[&str]) -> BTreeSet<String> {
        slugs.iter().map(|s| s.to_string()).collect()
    }

    fn thumbnail() -> crate::preview::Thumbnail {
        let image = image::DynamicImage::new_rgb8(20, 10);
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        crate::preview::thumbnail(&bytes.into_inner(), Some("Example".into())).unwrap()
    }

    fn stored_asset(minimum_bytes: usize) -> crate::media::Asset {
        let image = image::DynamicImage::new_rgb8(640, 400);
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let mut bytes = bytes.into_inner();
        bytes.resize(minimum_bytes.max(bytes.len()), 0);
        crate::media::prepare_asset(
            &crate::media::Candidate {
                url: url::Url::parse("https://publisher.example/image.png").unwrap(),
                alt: Some("Diagram".into()),
            },
            bytes,
            &crate::media::MediaLimits::default(),
        )
        .unwrap()
    }

    fn item_with_repeated_asset(root: &Path, asset: &crate::media::Asset, count: usize) -> Item {
        let directory = root.join("items/blog/2026/09");
        fs::create_dir_all(&directory).unwrap();
        for companion in asset.files("article") {
            fs::write(directory.join(&companion.metadata.file), companion.bytes).unwrap();
        }
        let images = (0..count)
            .map(|index| {
                let mut metadata = asset.metadata("article");
                metadata.source = format!("https://publisher.example/image-{index}.png");
                metadata
            })
            .collect();
        Item {
            path: "items/blog/2026/09/article".into(),
            front: FrontMatter {
                images,
                ..Default::default()
            },
            body: "body".into(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn data_branch_writes_refuse_traversal_and_symlinked_parents() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("store");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let store = Store::open(&root);
        let front = FrontMatter::default();

        let escaped = store.write_item(NewItem {
            dir: "../outside",
            stem: "escaped",
            front: &front,
            body: "must stay inside",
            html: None,
            preview: None,
            images: &[],
        });
        assert!(escaped.is_err());
        assert!(!outside.join("escaped.md").exists());

        std::os::unix::fs::symlink(&outside, root.join("items")).unwrap();
        let redirected = store.write_item(NewItem {
            dir: "items/blog",
            stem: "redirected",
            front: &front,
            body: "must not follow the link",
            html: None,
            preview: None,
            images: &[],
        });
        assert!(redirected.is_err());
        assert!(!outside.join("blog/redirected.md").exists());

        std::os::unix::fs::symlink(&outside, root.join("sources")).unwrap();
        assert!(
            store
                .write_source_state("redirected", &SourceState::default())
                .is_err()
        );
        assert!(!outside.join("redirected/state.toml").exists());
    }

    #[test]
    fn stored_media_readers_share_item_count_and_byte_budgets() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path());
        let limits = crate::media::MediaLimits::default();

        let small = stored_asset(0);
        let many = item_with_repeated_asset(temp.path(), &small, limits.max_assets + 1);
        assert_eq!(store.read_images(&many).unwrap().len(), limits.max_assets);
        assert_eq!(
            store.read_image_assets(&many).unwrap().len(),
            limits.max_assets
        );

        let large = stored_asset(limits.max_file_bytes);
        let oversized = item_with_repeated_asset(
            temp.path(),
            &large,
            limits.max_article_bytes / limits.max_file_bytes + 1,
        );
        let stored = store.read_images(&oversized).unwrap();
        let stored_bytes = stored
            .iter()
            .map(|image| image.original.len() + image.variants.iter().map(Vec::len).sum::<usize>())
            .sum::<usize>();
        assert!(stored_bytes <= limits.max_article_bytes);
        assert!(stored.len() < oversized.front.images.len());
        drop(stored);

        let assets = store.read_image_assets(&oversized).unwrap();
        let archived_bytes = assets
            .iter()
            .map(|asset| {
                asset.master_bytes.len()
                    + asset
                        .renditions
                        .iter()
                        .map(|rendition| rendition.bytes.len())
                        .sum::<usize>()
            })
            .sum::<usize>();
        assert!(archived_bytes <= limits.max_article_bytes);
        assert!(assets.len() < oversized.front.images.len());
    }

    #[test]
    fn preview_roundtrips_and_retention_removes_its_companion() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path());
        let preview = thumbnail();
        let front = FrontMatter {
            preview: Some(preview.metadata("article")),
            ..Default::default()
        };
        store
            .write_item(NewItem {
                dir: "items/blog/2026/09",
                stem: "article",
                front: &front,
                body: "Readable body",
                html: None,
                preview: Some(&preview.bytes),
                images: &[],
            })
            .unwrap();
        let item = store.read_item("items/blog/2026/09/article").unwrap();
        assert_eq!(store.read_preview(&item).unwrap(), Some(preview.bytes));
        let image_path = dir
            .path()
            .join("items/blog/2026/09")
            .join(&front.preview.as_ref().unwrap().file);
        assert!(image_path.is_file());
        store.remove_item(&item.path).unwrap();
        assert!(!image_path.exists());
        assert!(store.items().unwrap().is_empty());
    }

    #[test]
    fn article_images_roundtrip_and_retention_removes_current_companions() {
        let dir = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).with_image_cache(cache.path());
        let image = image::DynamicImage::new_rgb8(640, 400);
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let candidate = crate::media::Candidate {
            url: url::Url::parse("https://publisher.example/image.png").unwrap(),
            alt: Some("Diagram".into()),
        };
        let asset = crate::media::prepare_asset(
            &candidate,
            bytes.into_inner(),
            &crate::media::MediaLimits::default(),
        )
        .unwrap();
        let front = FrontMatter {
            images: vec![asset.metadata("article")],
            ..Default::default()
        };
        store
            .write_item(NewItem {
                dir: "items/blog/2026/09",
                stem: "article",
                front: &front,
                body: "![Diagram](https://publisher.example/image.png)",
                html: None,
                preview: None,
                images: std::slice::from_ref(&asset),
            })
            .unwrap();

        let item = store.read_item("items/blog/2026/09/article").unwrap();
        let stored = store.read_images(&item).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].original, asset.master_bytes);
        assert_eq!(stored[0].variants.len(), asset.renditions.len());
        crate::media::reset_stored_decode_count();
        let rebuilt = store.read_image_assets(&item).unwrap();
        assert_eq!(rebuilt.len(), 1);
        assert_eq!(rebuilt[0].master_bytes, asset.master_bytes);
        assert_eq!(rebuilt[0].renditions, asset.renditions);
        assert_eq!(
            crate::media::stored_decode_count(),
            1 + asset.renditions.len()
        );
        crate::media::reset_stored_decode_count();
        let reopened = Store::open(dir.path()).with_image_cache(cache.path());
        assert_eq!(reopened.read_image_assets(&item).unwrap(), rebuilt);
        assert_eq!(crate::media::stored_decode_count(), 0);
        let files = asset
            .files("article")
            .into_iter()
            .map(|file| {
                dir.path()
                    .join("items/blog/2026/09")
                    .join(file.metadata.file)
            })
            .collect::<Vec<_>>();
        assert!(files.iter().all(|file| file.is_file()));

        store.remove_item(&item.path).unwrap();
        assert!(files.iter().all(|file| !file.exists()));
        assert!(store.items().unwrap().is_empty());
        assert!(
            reopened.read_image_assets(&item).unwrap().is_empty(),
            "a cached receipt cannot resurrect removed image files"
        );
    }

    #[test]
    fn unavailable_optional_rendition_keeps_the_exact_master() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path());
        let image = image::DynamicImage::new_rgb8(640, 400);
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let asset = crate::media::prepare_asset(
            &crate::media::Candidate {
                url: url::Url::parse("https://publisher.example/image.png").unwrap(),
                alt: Some("Diagram".into()),
            },
            bytes.into_inner(),
            &crate::media::MediaLimits::default(),
        )
        .unwrap();
        assert!(!asset.renditions.is_empty());
        let front = FrontMatter {
            images: vec![asset.metadata("article")],
            ..Default::default()
        };
        store
            .write_item(NewItem {
                dir: "items/blog/2026/09",
                stem: "article",
                front: &front,
                body: "![Diagram](https://publisher.example/image.png)",
                html: None,
                preview: None,
                images: std::slice::from_ref(&asset),
            })
            .unwrap();

        let missing = dir
            .path()
            .join("items/blog/2026/09")
            .join(&front.images[0].variants[0].file);
        std::fs::remove_file(missing).unwrap();
        let item = store.read_item("items/blog/2026/09/article").unwrap();
        let stored = store.read_images(&item).unwrap();

        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].original, asset.master_bytes);
        assert_eq!(stored[0].variants.len(), asset.renditions.len() - 1);
        assert_eq!(stored[0].metadata.variants.len(), stored[0].variants.len());
        let rebuilt = store.read_image_assets(&item).unwrap();
        assert_eq!(rebuilt.len(), 1);
        assert_eq!(rebuilt[0].master_bytes, asset.master_bytes);
    }

    #[test]
    fn invalid_preview_never_publishes_markdown_or_escapes_its_item() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path());
        let preview = thumbnail();
        let mut front = FrontMatter {
            preview: Some(preview.metadata("article")),
            ..Default::default()
        };
        front.preview.as_mut().unwrap().file = "../outside.jpg".into();
        assert!(
            store
                .write_item(NewItem {
                    dir: "items/blog/2026/09",
                    stem: "article",
                    front: &front,
                    body: "body",
                    html: None,
                    preview: Some(&preview.bytes),
                    images: &[],
                })
                .is_err()
        );
        assert!(!store.stem_exists("items/blog/2026/09", "article"));
        assert!(!dir.path().join("items/blog/2026/outside.jpg").exists());
    }

    #[test]
    fn missing_or_oversized_preview_is_optional_when_reading() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path());
        let preview = thumbnail();
        let item = Item {
            path: "items/blog/2026/09/article".into(),
            front: FrontMatter {
                preview: Some(preview.metadata("article")),
                ..Default::default()
            },
            body: "body".into(),
        };
        assert_eq!(store.read_preview(&item).unwrap(), None);
        let file = dir
            .path()
            .join("items/blog/2026/09")
            .join(&item.front.preview.as_ref().unwrap().file);
        write_bytes(&file, &vec![0; crate::preview::MAX_BYTES + 1]).unwrap();
        assert_eq!(store.read_preview(&item).unwrap(), None);
    }

    #[test]
    fn malformed_preview_metadata_does_not_hide_the_article() {
        let (front, body) = frontmatter::parse::<FrontMatter>(
            "---\ntitle: Readable\npreview: { file: broken.jpg, width: wrong }\n---\nStill readable\n",
        ).unwrap();
        assert_eq!(front.title, "Readable");
        assert!(front.preview.is_none());
        assert_eq!(body.trim(), "Still readable");
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_symlink_preview_even_when_target_is_valid() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path());
        let preview = thumbnail();
        let item = Item {
            path: "items/blog/2026/09/article".into(),
            front: FrontMatter {
                preview: Some(preview.metadata("article")),
                ..Default::default()
            },
            body: "body".into(),
        };
        let target = dir.path().join("target.jpg");
        write_bytes(&target, &preview.bytes).unwrap();
        let file = dir
            .path()
            .join("items/blog/2026/09")
            .join(&item.front.preview.as_ref().unwrap().file);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(target, file).unwrap();
        assert_eq!(store.read_preview(&item).unwrap(), None);
    }

    #[test]
    fn status_changes_only_on_transitions() {
        let mut status = Status::default();
        let all = known(&["a", "b"]);
        let ok = BTreeMap::from([("a".to_string(), Outcome::Ok)]);
        assert!(!status.apply(&ok, &all, now()), "ok → ok is not a change");

        let err = BTreeMap::from([("a".to_string(), Outcome::Error("boom".into()))]);
        assert!(status.apply(&err, &all, now()));
        assert_eq!(status.errors["a"].message, "boom");

        let err2 = BTreeMap::from([("a".to_string(), Outcome::Error("other".into()))]);
        assert!(
            !status.apply(&err2, &all, now()),
            "error → error keeps the first message"
        );
        assert_eq!(status.errors["a"].message, "boom");

        assert!(status.apply(&ok, &all, now()));
        assert!(status.errors.is_empty());
    }

    #[test]
    fn status_prunes_unknown_sources() {
        let mut status = Status::default();
        let err = BTreeMap::from([("gone".to_string(), Outcome::Error("x".into()))]);
        assert!(status.apply(&err, &known(&["gone"]), now()));
        assert!(status.apply(&BTreeMap::new(), &known(&["other"]), now()));
        assert!(status.errors.is_empty());
    }

    #[test]
    fn bootstrap_writes_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path());
        assert!(store.bootstrap().unwrap());
        assert!(!store.bootstrap().unwrap());
        assert!(
            fs::read_to_string(dir.path().join(".gitattributes"))
                .unwrap()
                .contains("merge=union")
        );
        assert_eq!(
            fs::read_to_string(dir.path().join(".gitignore")).unwrap(),
            GITIGNORE
        );
        assert!(GITIGNORE.lines().any(|line| line == ".tmp*"));
    }

    #[test]
    fn bootstrap_adds_missing_files_to_an_existing_branch() {
        // A branch created before `.gitignore` existed has README and .gitattributes only; the
        // next bootstrap adds the missing file without touching the two it already has.
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("README.md"), "hand-edited\n").unwrap();
        fs::write(dir.path().join(".gitattributes"), GITATTRIBUTES).unwrap();
        let store = Store::open(dir.path());
        assert!(store.bootstrap().unwrap());
        assert_eq!(
            fs::read_to_string(dir.path().join("README.md")).unwrap(),
            "hand-edited\n"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join(".gitignore")).unwrap(),
            GITIGNORE
        );
        assert!(!store.bootstrap().unwrap());
    }

    #[test]
    fn state_identity_does_not_use_obsolete_url_fields() {
        let state: SourceState =
            toml::from_str("identity='current-hash'\nurl='https://obsolete.example/feed'\n")
                .unwrap();
        assert_eq!(state.identity, "current-hash");
        let obsolete: SourceState =
            toml::from_str("url='https://obsolete.example/feed'\n").unwrap();
        assert!(obsolete.identity.is_empty());
    }

    #[test]
    fn state_language_is_additive() {
        let before: SourceState =
            toml::from_str("identity='h'\ntitle='T'\nsite_url='https://t.example/'\n").unwrap();
        assert_eq!(
            before.language, None,
            "state written before the field parses"
        );
        let state = SourceState {
            identity: "h".into(),
            language: Some("fr-FR".into()),
            ..Default::default()
        };
        let text = toml::to_string(&state).unwrap();
        assert!(text.contains("language = \"fr-FR\"\n"), "{text}");
        assert_eq!(toml::from_str::<SourceState>(&text).unwrap(), state);
        assert!(
            !toml::to_string(&SourceState::default())
                .unwrap()
                .contains("language"),
            "an unknown language is not serialised"
        );
    }

    #[test]
    fn state_is_written_only_when_changed() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path());
        assert_eq!(store.source_state("x").unwrap(), SourceState::default());
        let state = SourceState {
            identity: "source-hash".into(),
            etag: Some("\"abc\"".into()),
            ..Default::default()
        };
        assert!(store.write_source_state("x", &state).unwrap());
        assert!(!store.write_source_state("x", &state).unwrap());
        assert_eq!(store.source_state("x").unwrap(), state);
    }

    #[test]
    fn seen_keys_append() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path());
        assert!(store.seen("x").unwrap().is_empty());
        store
            .append_seen("x", &["k1".into(), "k2".into()], now())
            .unwrap();
        store.append_seen("x", &["k3".into()], now()).unwrap();
        assert_eq!(store.seen("x").unwrap(), known(&["k1", "k2", "k3"]));
        let text = fs::read_to_string(dir.path().join("sources/x/seen.txt")).unwrap();
        assert_eq!(text, "k1 2026-09-02\nk2 2026-09-02\nk3 2026-09-02\n");
    }

    #[test]
    fn malformed_status_reads_as_healthy_and_is_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path());
        fs::write(dir.path().join("status.toml"), "not = = toml\n").unwrap();
        assert_eq!(store.status().unwrap(), Status::default());

        let mut status = Status::default();
        status.errors.insert(
            "a".into(),
            SourceError {
                message: "x".into(),
                since: now(),
            },
        );
        store.write_status(&status).unwrap();
        let text = fs::read_to_string(dir.path().join("status.toml")).unwrap();
        assert_eq!(toml::from_str::<Status>(&text).unwrap(), status);
        assert_eq!(store.status().unwrap(), status);
    }

    #[test]
    fn malformed_source_state_reads_as_absent_and_is_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path());
        let path = dir.path().join("sources/x/state.toml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "not = = toml\n").unwrap();
        assert_eq!(store.source_state("x").unwrap(), SourceState::default());

        let state = SourceState {
            identity: "source-hash".into(),
            resolved_url: Some("https://example.net/feed.xml".into()),
            ..Default::default()
        };
        assert!(store.write_source_state("x", &state).unwrap());
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(toml::from_str::<SourceState>(&text).unwrap(), state);
        assert_eq!(store.source_state("x").unwrap(), state);
    }

    #[test]
    fn unreadable_state_is_still_an_error() {
        // Only malformed content is recoverable; a directory where the file should be is not.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path());
        fs::create_dir_all(dir.path().join("status.toml")).unwrap();
        assert!(store.status().is_err());
        fs::create_dir_all(dir.path().join("sources/x/state.toml")).unwrap();
        assert!(store.source_state("x").is_err());
    }

    #[test]
    fn status_file_disappears_when_healthy() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path());
        let mut status = Status::default();
        status.errors.insert(
            "a".into(),
            SourceError {
                message: "x".into(),
                since: now(),
            },
        );
        store.write_status(&status).unwrap();
        assert_eq!(store.status().unwrap(), status);
        store.write_status(&Status::default()).unwrap();
        assert!(!dir.path().join("status.toml").exists());
    }

    #[test]
    fn items_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path());
        let front = FrontMatter {
            title: "Hello".into(),
            link: "https://a.b/c".into(),
            source: "x".into(),
            first_seen: now(),
            html: Some("2026-09-02-hello.html".into()),
            ..Default::default()
        };
        store
            .write_item(NewItem {
                dir: "items/x/2026/09",
                stem: "2026-09-02-hello",
                front: &front,
                body: "Body\n",
                html: Some("<p>Body</p>"),
                preview: None,
                images: &[],
            })
            .unwrap();
        assert!(store.stem_exists("items/x/2026/09", "2026-09-02-hello"));
        let items = store.items().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].path, "items/x/2026/09/2026-09-02-hello");
        assert_eq!(items[0].front, front);
        assert_eq!(items[0].body, "Body\n");
        assert_eq!(
            store.read_html(&items[0]).unwrap().as_deref(),
            Some("<p>Body</p>")
        );
        assert!(
            walkdir::WalkDir::new(dir.path())
                .min_depth(1)
                .into_iter()
                .filter_map(Result::ok)
                .all(|entry| !entry.file_name().to_string_lossy().starts_with(".tmp"))
        );
    }

    #[test]
    fn broken_items_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path());
        write(
            &dir.path().join("items/x/2026/09/bad.md"),
            "no front matter\n",
        )
        .unwrap();
        assert!(store.items().unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn stored_html_requires_declared_regular_bounded_companions() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside.html");
        fs::write(&outside, "<p>host secret</p>").unwrap();
        let store_root = dir.path().join("store");
        let item_dir = store_root.join("items/x/2026/09");
        fs::create_dir_all(&item_dir).unwrap();
        let mut front = FrontMatter {
            title: "Unsafe".into(),
            link: "https://example.test/unsafe".into(),
            source: "x".into(),
            first_seen: now(),
            html: Some("unsafe.html".into()),
            ..Default::default()
        };
        write(
            &item_dir.join("unsafe.md"),
            &frontmatter::render(&front, "Safe Markdown").unwrap(),
        )
        .unwrap();
        std::os::unix::fs::symlink(&outside, item_dir.join("unsafe.html")).unwrap();
        let store = Store::open(&store_root);
        let item = store.read_item("items/x/2026/09/unsafe").unwrap();
        assert_eq!(store.read_html(&item).unwrap(), None);

        fs::remove_file(item_dir.join("unsafe.html")).unwrap();
        fs::write(item_dir.join("unsafe.html"), "<p>declared</p>").unwrap();
        front.html = None;
        write(
            &item_dir.join("unsafe.md"),
            &frontmatter::render(&front, "Safe Markdown").unwrap(),
        )
        .unwrap();
        let item = store.read_item("items/x/2026/09/unsafe").unwrap();
        assert_eq!(store.read_html(&item).unwrap(), None);
    }
}
