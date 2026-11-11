//! Immutable views of completed source transactions for development publication.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};

pub(in crate::commands) struct ArchiveSnapshot {
    directory: Arc<tempfile::TempDir>,
    root: PathBuf,
    scratch: PathBuf,
}

impl ArchiveSnapshot {
    /// Capture before synchronization starts, or after all source transactions have finished.
    pub(in crate::commands) fn capture(data: &Path, scratch: &Path) -> Result<Self> {
        let data = directory(data, false)?;
        if std::path::absolute(scratch)?.starts_with(&data) {
            bail!("dev snapshot scratch and archive directories must not overlap");
        }
        let scratch = directory(scratch, true)?;
        if data.starts_with(&scratch) || scratch.starts_with(&data) {
            bail!("dev snapshot scratch and archive directories must not overlap");
        }
        let snapshot = Self::empty(scratch)?;
        copy_tree(&data, snapshot.root())?;
        Ok(snapshot)
    }

    /// Only the completed source is read from the live archive; every other source remains at
    /// its previous stable version, regardless of concurrent fetches or later rollbacks.
    pub(in crate::commands) fn completed(&self, data: &Path, slug: &str) -> Result<Self> {
        let mut components = Path::new(slug).components();
        if !matches!(components.next(), Some(Component::Normal(_)))
            || components.next().is_some()
            || slug.contains(['/', '\\'])
            || slug == ".git"
        {
            bail!("unsafe dev snapshot source slug {slug:?}");
        }
        let data = directory(data, false)?;
        if data.starts_with(&self.scratch) || self.scratch.starts_with(&data) {
            bail!("dev snapshot scratch and archive directories must not overlap");
        }
        let snapshot = Self::empty(self.scratch.clone())?;
        copy_tree(self.root(), snapshot.root())?;
        for family in ["items", "sources"] {
            let relative = Path::new(family).join(slug);
            let target = snapshot.root().join(&relative);
            if target.exists() {
                fs::remove_dir_all(&target)
                    .with_context(|| format!("replacing completed source {}", target.display()))?;
            }
            let source = data.join(relative);
            match fs::symlink_metadata(&source) {
                Ok(_) => {
                    directory(&source, false)?;
                    copy_tree(&source, &target)?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).with_context(|| format!("inspecting {}", source.display()));
                }
            }
        }
        Ok(snapshot)
    }

    pub(in crate::commands) fn root(&self) -> &Path {
        &self.root
    }

    pub(in crate::commands) fn directory(&self) -> Arc<tempfile::TempDir> {
        Arc::clone(&self.directory)
    }

    fn empty(scratch: PathBuf) -> Result<Self> {
        let directory = Arc::new(
            tempfile::Builder::new()
                .prefix("archive-")
                .tempdir_in(&scratch)
                .context("creating immutable dev archive snapshot")?,
        );
        Ok(Self {
            root: directory.path().join("data"),
            directory,
            scratch,
        })
    }
}

/// Resolve only real directories, checking ancestors before creating a scratch directory.
fn directory(path: &Path, create: bool) -> Result<PathBuf> {
    let mut current = PathBuf::new();
    for component in std::path::absolute(path)?.components() {
        if matches!(component, Component::ParentDir) || component.as_os_str() == ".git" {
            bail!("unsafe dev snapshot directory {}", path.display());
        }
        current.push(component);
        // A bare Windows drive prefix is not a directory; inspect after adding the root.
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                bail!(
                    "dev snapshot path is not a regular directory: {}",
                    current.display()
                );
            }
            Ok(_) => {}
            Err(error) if create && error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current)
                    .with_context(|| format!("creating {}", current.display()))?;
            }
            Err(error) => {
                return Err(error).with_context(|| format!("inspecting {}", current.display()));
            }
        }
    }
    current
        .canonicalize()
        .with_context(|| format!("resolving {}", path.display()))
}

fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir_all(target)
        .with_context(|| format!("creating dev snapshot directory {}", target.display()))?;
    for entry in
        fs::read_dir(source).with_context(|| format!("reading dev archive {}", source.display()))?
    {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            bail!("dev archive contains a symlink: {}", entry.path().display());
        }
        if entry.file_name() == ".git" {
            continue;
        }
        let destination = target.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &destination)?;
        } else if kind.is_file() {
            // Store updates replace files atomically. Linking keeps captured media cheap while
            // preserving the old bytes when a later transaction replaces or removes its path.
            if fs::hard_link(entry.path(), &destination).is_err() {
                fs::copy(entry.path(), &destination).with_context(|| {
                    format!("copying dev archive file {}", entry.path().display())
                })?;
            }
        } else {
            bail!(
                "dev archive contains a special file: {}",
                entry.path().display()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn snapshot_directory_validates_rooted_windows_paths_after_the_volume_prefix() {
        let temp = tempfile::tempdir().unwrap();
        let canonical = temp.path().canonicalize().unwrap();
        assert_eq!(directory(temp.path(), false).unwrap(), canonical);
        assert_eq!(directory(&canonical, false).unwrap(), canonical);
        let scratch = canonical.join("nested").join("scratch");
        assert_eq!(directory(&scratch, true).unwrap(), scratch);
    }

    fn write(root: &Path, name: &str, bytes: &[u8]) {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        crate::cache::write(&path, bytes).unwrap();
    }

    #[test]
    fn completed_sources_exclude_other_inflight_changes_and_keep_old_views() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let data = root.join("live");
        write(&data, "items/fast/post.md", b"original fast");
        write(&data, "items/slow/post.md", b"original slow");
        write(&data, "sources/fast/state.toml", b"original state");
        write(&data, "status.toml", b"stable status");
        let initial = ArchiveSnapshot::capture(&data, &root.join("scratch")).unwrap();

        write(&data, "items/fast/post.md", b"committed fast");
        write(&data, "sources/fast/state.toml", b"committed state");
        write(&data, "items/slow/post.md", b"uncommitted slow");
        write(&data, "items/slow/new.md", b"will roll back");
        write(&data, "status.toml", b"not published yet");
        let completed = initial.completed(&data, "fast").unwrap();
        assert_eq!(
            fs::read(completed.root().join("items/fast/post.md")).unwrap(),
            b"committed fast"
        );
        assert_eq!(
            fs::read(completed.root().join("sources/fast/state.toml")).unwrap(),
            b"committed state"
        );
        assert_eq!(
            fs::read(completed.root().join("items/slow/post.md")).unwrap(),
            b"original slow"
        );
        assert!(!completed.root().join("items/slow/new.md").exists());
        assert_eq!(
            fs::read(completed.root().join("status.toml")).unwrap(),
            b"stable status"
        );
        assert_eq!(
            fs::read(initial.root().join("items/fast/post.md")).unwrap(),
            b"original fast"
        );

        write(&data, "items/slow/post.md", b"original slow");
        fs::remove_file(data.join("items/slow/new.md")).unwrap();
        let final_view = completed.completed(&data, "slow").unwrap();
        assert!(!final_view.root().join("items/slow/new.md").exists());
    }

    #[test]
    fn completed_source_deletions_preserve_media_and_snapshot_lifetimes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let data = root.join("live");
        write(&data, "items/feed/post.md", b"article");
        write(&data, "items/feed/post.image.webp", b"old image");
        write(&data, "sources/feed/state.toml", b"old state");
        let initial = ArchiveSnapshot::capture(&data, &root.join("scratch")).unwrap();
        write(&data, "items/feed/post.image.webp", b"replacement image");
        fs::remove_file(data.join("items/feed/post.md")).unwrap();
        fs::remove_dir_all(data.join("sources/feed")).unwrap();
        let completed = initial.completed(&data, "feed").unwrap();
        assert!(!completed.root().join("items/feed/post.md").exists());
        assert!(!completed.root().join("sources/feed").exists());
        assert_eq!(
            fs::read(initial.root().join("items/feed/post.image.webp")).unwrap(),
            b"old image"
        );
        assert_eq!(
            fs::read(completed.root().join("items/feed/post.image.webp")).unwrap(),
            b"replacement image"
        );
        let root = initial.root().to_owned();
        let owner = initial.directory();
        drop(initial);
        assert!(root.exists());
        drop(owner);
        assert!(!root.exists());
    }

    #[test]
    fn snapshot_paths_exclude_git_and_reject_traversal_and_overlap() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let data = root.join("live");
        write(&data, "items/feed/post.md", b"article");
        write(&data, ".git/config", b"not archive data");
        let snapshot = ArchiveSnapshot::capture(&data, &root.join("scratch")).unwrap();
        assert!(!snapshot.root().join(".git").exists());
        for slug in [
            "",
            ".",
            "..",
            "../feed",
            "feed/other",
            "/feed",
            "feed\\other",
            ".git",
        ] {
            assert!(snapshot.completed(&data, slug).is_err(), "{slug:?}");
        }
        assert!(ArchiveSnapshot::capture(&data, &data.join("scratch")).is_err());
        assert!(ArchiveSnapshot::capture(&data.join(".git"), &root.join("other")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_files_directories_and_ancestors_are_rejected() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let data = root.join("live");
        write(&data, "items/feed/post.md", b"article");
        let snapshot = ArchiveSnapshot::capture(&data, &root.join("scratch")).unwrap();
        symlink(&root, data.join("items/feed/outside")).unwrap();
        assert!(snapshot.completed(&data, "feed").is_err());
        assert!(ArchiveSnapshot::capture(&data, &root.join("other")).is_err());
        symlink(&data, root.join("alias")).unwrap();
        assert!(snapshot.completed(&root.join("alias"), "feed").is_err());
        assert!(ArchiveSnapshot::capture(&data, &root.join("alias/scratch")).is_err());
    }
}
