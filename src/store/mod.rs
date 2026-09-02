//! The data branch as a directory tree: per-source state and dedupe keys, the status file,
//! and the `items/` pairs. Everything here is plain files; `git.rs` turns them into commits.

pub mod frontmatter;
pub mod retention;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::model::{FrontMatter, Item};

pub const README: &str = include_str!("branch-readme.md");
pub const GITATTRIBUTES: &str = "\
# aggr data branch. Concurrent runs append to seen.txt; union merge keeps both sides.
sources/*/seen.txt merge=union
* text=auto eol=lf
";

pub struct Store {
    root: PathBuf,
}

/// Per-source fetch state, stored as `sources/<slug>/state.toml`. Only rewritten when it changes,
/// so an unchanged upstream leaves the tree untouched.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceState {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site_url: Option<String>,
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
}

impl Store {
    pub fn open(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Write the branch README and `.gitattributes` when missing. Returns whether anything was
    /// written.
    pub fn bootstrap(&self) -> Result<bool> {
        let mut wrote = false;
        for (name, content) in [("README.md", README), (".gitattributes", GITATTRIBUTES)] {
            let path = self.root.join(name);
            if !path.exists() {
                write(&path, content)?;
                wrote = true;
            }
        }
        Ok(wrote)
    }

    fn source_dir(&self, slug: &str) -> PathBuf {
        self.root.join("sources").join(slug)
    }

    pub fn source_state(&self, slug: &str) -> Result<SourceState> {
        let path = self.source_dir(slug).join("state.toml");
        match fs::read_to_string(&path) {
            Ok(text) => {
                toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(SourceState::default()),
            Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Persist the state if it differs from what is on disk. Returns whether it was written.
    pub fn write_source_state(&self, slug: &str, state: &SourceState) -> Result<bool> {
        let path = self.source_dir(slug).join("state.toml");
        let text = toml::to_string(state).context("serializing state")?;
        if fs::read_to_string(&path).ok().as_deref() == Some(&text) {
            return Ok(false);
        }
        write(&path, &text)?;
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
        let path = self.source_dir(slug).join("seen.txt");
        let day = date.format("%Y-%m-%d");
        let mut text = fs::read_to_string(&path).unwrap_or_default();
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        for key in keys {
            text.push_str(&format!("{key} {day}\n"));
        }
        write(&path, &text)
    }

    pub fn status(&self) -> Result<Status> {
        let path = self.root.join("status.toml");
        match fs::read_to_string(&path) {
            Ok(text) => {
                toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Status::default()),
            Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn write_status(&self, status: &Status) -> Result<()> {
        let path = self.root.join("status.toml");
        if status.errors.is_empty() {
            if path.exists() {
                fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
            }
            return Ok(());
        }
        let text = toml::to_string(status).context("serializing status")?;
        write(&path, &text)
    }

    pub fn stem_exists(&self, dir: &str, stem: &str) -> bool {
        self.root.join(dir).join(format!("{stem}.md")).exists()
    }

    pub fn write_item(&self, item: NewItem<'_>) -> Result<()> {
        let dir = self.root.join(item.dir);
        let md = dir.join(format!("{}.md", item.stem));
        write(&md, &frontmatter::render(item.front, item.body)?)?;
        if let Some(html) = item.html {
            write(&dir.join(format!("{}.html", item.stem)), html)?;
        }
        Ok(())
    }

    /// Delete an item's `.md` and `.html` (retention). Its dedupe keys stay in `seen.txt`.
    pub fn remove_item(&self, path: &str) -> Result<()> {
        for ext in ["md", "html"] {
            let file = self.root.join(format!("{path}.{ext}"));
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
        let file = self.root.join(format!("{path}.md"));
        let text =
            fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
        let (front, body) = frontmatter::parse::<FrontMatter>(&text)
            .with_context(|| format!("parsing {}", file.display()))?;
        Ok(Item {
            path: path.to_string(),
            front,
            body: body.to_string(),
        })
    }

    /// Every item under `items/`, unsorted. Files that fail to parse are logged and skipped so
    /// one hand edit never takes the site down.
    pub fn items(&self) -> Result<Vec<Item>> {
        let mut items = Vec::new();
        for path in self.item_paths()? {
            match self.read_item(&path) {
                Ok(item) => items.push(item),
                Err(err) => log::warn!("skipping {path}.md: {err:#}"),
            }
        }
        Ok(items)
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

    /// The exact bytes of an item's `.md`, for blob hashes and raw copies.
    pub fn item_bytes(&self, path: &str) -> Result<Vec<u8>> {
        let file = self.root.join(format!("{path}.md"));
        fs::read(&file).with_context(|| format!("reading {}", file.display()))
    }

    pub fn read_html(&self, path: &str) -> Result<Option<String>> {
        let file = self.root.join(format!("{path}.html"));
        match fs::read_to_string(&file) {
            Ok(text) => Ok(Some(text)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err).with_context(|| format!("reading {}", file.display())),
        }
    }
}

fn write(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(path, content).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 2, 12, 0, 0).unwrap()
    }

    fn known(slugs: &[&str]) -> BTreeSet<String> {
        slugs.iter().map(|s| s.to_string()).collect()
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
    }

    #[test]
    fn state_is_written_only_when_changed() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path());
        assert_eq!(store.source_state("x").unwrap(), SourceState::default());
        let state = SourceState {
            url: "https://a.b/feed".into(),
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
            })
            .unwrap();
        assert!(store.stem_exists("items/x/2026/09", "2026-09-02-hello"));
        let items = store.items().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].path, "items/x/2026/09/2026-09-02-hello");
        assert_eq!(items[0].front, front);
        assert_eq!(items[0].body, "Body\n");
        assert_eq!(
            store.read_html(&items[0].path).unwrap().as_deref(),
            Some("<p>Body</p>")
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
}
