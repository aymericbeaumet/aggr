//! Per-source file transaction: every store path a source is about to write is snapshotted
//! first, so a source that fails midway leaves the archive exactly as it found it.

use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context as _, Result, bail};

use super::plan::Planned;
use crate::model::RawItem;

pub(super) struct SourceTransaction {
    root: PathBuf,
    backup: Option<tempfile::TempDir>,
    tracked: HashSet<PathBuf>,
    snapshots: Vec<FileSnapshot>,
    finished: bool,
}

struct FileSnapshot {
    target: PathBuf,
    backup: Option<PathBuf>,
}

impl SourceTransaction {
    pub(super) fn new(root: &Path) -> Result<Self> {
        let metadata = fs::symlink_metadata(root)
            .with_context(|| format!("inspecting store root {}", root.display()))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            bail!("store root is not a regular directory: {}", root.display());
        }
        Ok(Self {
            root: root
                .canonicalize()
                .with_context(|| format!("resolving store root {}", root.display()))?,
            backup: None,
            tracked: HashSet::new(),
            snapshots: Vec::new(),
            finished: false,
        })
    }

    fn track_relative(&mut self, relative: impl AsRef<Path>) -> Result<()> {
        let relative = relative.as_ref();
        let target = self.checked_target(relative)?;
        if !self.tracked.insert(target.clone()) {
            return Ok(());
        }
        let backup = match fs::symlink_metadata(&target) {
            Ok(metadata) => {
                debug_assert!(metadata.file_type().is_file());
                if self.backup.is_none() {
                    self.backup =
                        Some(tempfile::tempdir().context("creating source transaction backup")?);
                }
                let backup = self
                    .backup
                    .as_ref()
                    .context("source transaction backup was not initialized")?
                    .path()
                    .join(self.snapshots.len().to_string());
                fs::copy(&target, &backup)
                    .with_context(|| format!("backing up {}", target.display()))?;
                Some(backup)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(error).with_context(|| format!("inspecting {}", target.display()));
            }
        };
        self.snapshots.push(FileSnapshot { target, backup });
        Ok(())
    }

    fn checked_target(&self, relative: &Path) -> Result<PathBuf> {
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            bail!("invalid source transaction path: {}", relative.display());
        }
        let mut parent = self.root.clone();
        for component in relative
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .components()
        {
            let Component::Normal(component) = component else {
                unreachable!("transaction path components were validated")
            };
            parent.push(component);
            match fs::symlink_metadata(&parent) {
                Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                    bail!(
                        "source transaction parent is not a regular directory: {}",
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
        let target = self.root.join(relative);
        match fs::symlink_metadata(&target) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => bail!(
                "source transaction target is not a regular file: {}",
                target.display()
            ),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("inspecting {}", target.display()));
            }
        }
        Ok(target)
    }

    pub(super) fn track_item(&mut self, planned: &Planned, raw: &RawItem) -> Result<()> {
        let directory = Path::new(&planned.dir);
        self.track_relative(directory.join(format!("{}.md", planned.stem)))?;
        if planned.html.is_some() {
            self.track_relative(directory.join(format!("{}.html", planned.stem)))?;
        }
        if raw.preview.is_some()
            && let Some(preview) = &planned.front.preview
        {
            self.track_relative(directory.join(&preview.file))?;
        }
        for image in &raw.images {
            for companion in image.files(&planned.stem) {
                self.track_relative(directory.join(companion.metadata.file))?;
            }
        }
        Ok(())
    }

    pub(super) fn track_seen(&mut self, slug: &str) -> Result<()> {
        self.track_relative(Path::new("sources").join(slug).join("seen.txt"))
    }

    pub(super) fn track_state(&mut self, slug: &str) -> Result<()> {
        self.track_relative(Path::new("sources").join(slug).join("state.toml"))
    }

    pub(super) fn commit(mut self) {
        self.finished = true;
    }

    pub(super) fn rollback(&mut self) -> Result<()> {
        let mut first_error = None;
        for snapshot in self.snapshots.iter().rev() {
            let restored = (|| -> Result<()> {
                let relative = snapshot
                    .target
                    .strip_prefix(&self.root)
                    .context("transaction target escaped its store root")?;
                let target = self.checked_target(relative)?;
                match &snapshot.backup {
                    Some(backup) => {
                        match fs::symlink_metadata(&target) {
                            Ok(metadata) if metadata.file_type().is_file() => {
                                fs::remove_file(&target)
                                    .with_context(|| format!("removing {}", target.display()))?;
                            }
                            Ok(_) => bail!(
                                "refusing to replace non-file transaction target {}",
                                target.display()
                            ),
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                            Err(error) => {
                                return Err(error)
                                    .with_context(|| format!("inspecting {}", target.display()));
                            }
                        }
                        if let Some(parent) = target.parent() {
                            fs::create_dir_all(parent)
                                .with_context(|| format!("recreating {}", parent.display()))?;
                        }
                        fs::copy(backup, &target)
                            .with_context(|| format!("restoring {}", target.display()))?;
                    }
                    None => match fs::remove_file(&target) {
                        Ok(()) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => {
                            return Err(error).with_context(|| {
                                format!("removing new file {}", target.display())
                            });
                        }
                    },
                }
                Ok(())
            })();
            if let Err(error) = restored {
                log::error!("source transaction rollback: {error:#}");
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        self.finished = true;
        first_error.map_or(Ok(()), Err)
    }
}

impl Drop for SourceTransaction {
    fn drop(&mut self) {
        if !self.finished
            && let Err(error) = self.rollback()
        {
            log::error!("source transaction rollback during drop: {error:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{options, source};
    use super::super::{ArticleFailures, FetchOneContext, Options, StatePolicy, fetch_one};
    use super::*;
    use crate::config::{ContentMode, Engine};
    use crate::http;
    use crate::store::Store;
    use httpmock::prelude::*;
    use std::sync::Arc;
    use url::Url;

    #[tokio::test]
    async fn failed_source_rolls_back_earlier_items_and_seen_state() {
        crate::http::install_crypto_provider();
        let server = MockServer::start_async().await;
        let feed = server
            .mock_async(|when, then| {
                when.method(GET).path("/feed.json");
                then.status(200)
                    .header("content-type", "application/feed+json")
                    .json_body(serde_json::json!({
                        "version": "https://jsonfeed.org/version/1.1",
                        "title": "Transaction test",
                        "items": [
                            {
                                "id": "second",
                                "title": "Second",
                                "url": server.url("/second"),
                                "date_published": "2026-09-02T08:00:00Z",
                                "content_html": "<p>Second body.</p>"
                            },
                            {
                                "id": "first",
                                "title": "First",
                                "url": server.url("/first"),
                                "date_published": "2026-08-31T08:00:00Z",
                                "content_html": "<p>First body.</p>"
                            }
                        ]
                    }));
            })
            .await;
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(root.path()));
        let invalid = root.path().join("items/blog/2026/09");
        fs::create_dir_all(invalid.parent().unwrap()).unwrap();
        fs::write(&invalid, "not a directory").unwrap();
        let cache = tempfile::tempdir().unwrap();
        let client = http::Client::new(&crate::config::FetchConfig {
            retries: 0,
            ..Default::default()
        })
        .unwrap();
        let mut configured = source();
        configured.content = ContentMode::Light;
        configured.html = false;
        configured.previews = false;
        configured.images = false;
        configured.engine = Engine::Feed {
            url: Url::parse(&server.url("/feed.json")).unwrap(),
        };

        let test_options = Options {
            article_concurrency: 1,
            ..options()
        };
        let failures = ArticleFailures::default();
        let error = match fetch_one(
            &configured,
            FetchOneContext {
                store: &store,
                store_root: root.path(),
                client: &client,
                cache_dir: cache.path(),
                options: &test_options,
                article_failures: &failures,
                state_policy: StatePolicy::PersistentBranch,
            },
        )
        .await
        {
            Ok(_) => panic!("the invalid second target should fail the source"),
            Err(error) => error,
        };

        assert!(
            format!("{error:#}").contains("source transaction parent is not a regular directory"),
            "{error:#}"
        );
        assert!(
            !root
                .path()
                .join("items/blog/2026/08/2026-08-31-first.md")
                .exists()
        );
        assert!(!root.path().join("sources/blog/seen.txt").exists());
        assert!(invalid.is_file());
        assert!(test_options.archived_links.state().committed.is_empty());
        assert!(test_options.archived_links.state().reserved.is_empty());
        configured.slug = "working".into();
        let retry = fetch_one(
            &configured,
            FetchOneContext {
                store: &store,
                store_root: root.path(),
                client: &client,
                cache_dir: cache.path(),
                options: &test_options,
                article_failures: &failures,
                state_policy: StatePolicy::PersistentBranch,
            },
        )
        .await
        .unwrap();
        assert_eq!(retry.added, 2);
        assert_eq!(store.items().unwrap().len(), 2);
        feed.assert_calls_async(2).await;
    }
}
