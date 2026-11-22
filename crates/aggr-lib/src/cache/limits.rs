//! Byte limits for disposable caches. Archives never enter this module.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use anyhow::{Context as _, Result, ensure};
use serde::Serialize;

use super::Namespace;

#[derive(Debug, Default, Serialize)]
pub struct Usage {
    pub before_bytes: u64,
    pub after_bytes: u64,
    pub removed_files: usize,
}

#[derive(Debug, Default, Serialize)]
pub struct LimitReport {
    pub media: Usage,
    pub responses: Usage,
}

impl std::fmt::Display for LimitReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "media {} bytes, responses {} bytes; removed {} disposable files",
            self.media.after_bytes,
            self.responses.after_bytes,
            self.media.removed_files + self.responses.removed_files
        )
    }
}

const MAX_RECENT_ENTRIES: usize = 65_536;
const MAX_RESPONSE_ENTRIES: usize = 65_536;
const RESPONSE_NAMESPACES: [Namespace; 9] = [
    Namespace::Articles,
    Namespace::Discussions,
    Namespace::ValidatedImages,
    Namespace::ImageFailures,
    Namespace::CaptureRetries,
    Namespace::RecordingDuration,
    Namespace::FeedParsing,
    Namespace::Budget,
    Namespace::PreparedMarkdown,
];
static RECENT: OnceLock<Mutex<BTreeMap<PathBuf, SystemTime>>> = OnceLock::new();

/// Usage is process-local: a hit neither rewrites a cached byte nor changes a file timestamp.
/// The next maintenance pass protects these entries before considering older inactive files.
pub(crate) fn mark_used(path: &Path) {
    let mut recent = RECENT
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    recent.insert(path.to_path_buf(), SystemTime::now());
    if recent.len() > MAX_RECENT_ENTRIES
        && let Some(oldest) = recent
            .iter()
            .min_by_key(|(_, time)| **time)
            .map(|(path, _)| path.clone())
    {
        recent.remove(&oldest);
    }
}

fn take_recent(root: &Path) -> BTreeSet<PathBuf> {
    let mut recent = RECENT
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let paths: BTreeSet<_> = recent
        .keys()
        .filter(|path| path.starts_with(root))
        .cloned()
        .collect();
    recent.retain(|path, _| !paths.contains(path));
    paths
}

/// Enforce configured caps after workers finish. Failure only concerns disposable state; callers
/// may warn and retain an otherwise successful sync/build. Publication images have their own
/// allowance; page responses and small derived state share one byte and file-count allowance.
/// Only registered namespaces are considered; symlinked roots and entries are refused.
pub fn enforce_limits(root: &Path, limits: &crate::config::CacheConfig) -> Result<LimitReport> {
    ensure!(
        !std::fs::symlink_metadata(root)?.file_type().is_symlink(),
        "cache root is a symlink"
    );
    Ok(LimitReport {
        media: enforce_media(
            &Namespace::DeploymentMedia.dir(root),
            limits.media_max_bytes,
            crate::site::compressed_media::implementation_key(),
        )?,
        responses: enforce_responses(
            &RESPONSE_NAMESPACES.map(|namespace| namespace.dir(root)),
            limits.response_max_bytes,
            MAX_RESPONSE_ENTRIES,
        )?,
    })
}

#[derive(Debug)]
struct File {
    path: PathBuf,
    bytes: u64,
    modified: SystemTime,
}

fn files(root: &Path) -> Result<Vec<File>> {
    let metadata = match std::fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("inspecting {}", root.display())),
    };
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "cache namespace {} is not an ordinary directory",
        root.display()
    );
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(root).follow_links(false).min_depth(1) {
        let entry = entry.with_context(|| format!("walking {}", root.display()))?;
        ensure!(
            !entry.file_type().is_symlink(),
            "cache entry {} is a symlink",
            entry.path().display()
        );
        if entry.file_type().is_file() {
            let metadata = entry.metadata()?;
            files.push(File {
                path: entry.into_path(),
                bytes: metadata.len(),
                modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            });
        }
    }
    Ok(files)
}

fn remove(file: &File, usage: &mut Usage) -> Result<()> {
    match std::fs::remove_file(&file.path) {
        Ok(()) => {
            usage.after_bytes = usage.after_bytes.saturating_sub(file.bytes);
            usage.removed_files += 1;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            usage.after_bytes = usage.after_bytes.saturating_sub(file.bytes);
        }
        Err(error) => {
            return Err(error).with_context(|| format!("evicting {}", file.path.display()));
        }
    }
    Ok(())
}

fn enforce_media(root: &Path, limit: u64, generation: &str) -> Result<Usage> {
    let recent = take_recent(root);
    let mut groups = BTreeMap::<PathBuf, Vec<File>>::new();
    let mut usage = Usage::default();
    for file in files(root)? {
        usage.before_bytes = usage.before_bytes.saturating_add(file.bytes);
        groups
            .entry(file.path.with_extension(""))
            .or_default()
            .push(file);
    }
    usage.after_bytes = usage.before_bytes;
    let mut retained = Vec::new();
    for group in groups.into_values() {
        let receipt = group.iter().find(|file| {
            file.path
                .extension()
                .is_some_and(|extension| extension == "json")
        });
        let current = receipt
            .and_then(|file| {
                (file.bytes <= 16 * 1024)
                    .then(|| std::fs::read(&file.path).ok())
                    .flatten()
            })
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|value| {
                value
                    .get("record")?
                    .get("implementation")?
                    .as_str()
                    .map(str::to_owned)
            })
            .is_some_and(|implementation| implementation == generation);
        if !current {
            for file in &group {
                remove(file, &mut usage)?;
            }
        } else {
            retained.push(group);
        }
    }
    retained.sort_by_key(|group| {
        (
            group.iter().any(|file| recent.contains(&file.path)),
            group
                .iter()
                .map(|file| file.modified)
                .max()
                .unwrap_or(SystemTime::UNIX_EPOCH),
            group.first().map(|file| file.path.clone()),
        )
    });
    for group in retained {
        if usage.after_bytes <= limit {
            break;
        }
        for file in &group {
            remove(file, &mut usage)?;
        }
    }
    Ok(usage)
}

fn enforce_responses(roots: &[PathBuf], limit: u64, max_entries: usize) -> Result<Usage> {
    let mut recent = BTreeSet::new();
    let mut files = Vec::new();
    for root in roots {
        recent.extend(take_recent(root));
        files.extend(self::files(root)?);
    }
    let mut remaining = files.len();
    let before_bytes = files.iter().map(|file| file.bytes).sum();
    let mut usage = Usage {
        before_bytes,
        after_bytes: before_bytes,
        removed_files: 0,
    };
    files.sort_by_key(|file| {
        (
            recent.contains(&file.path),
            file.modified,
            file.path.clone(),
        )
    });
    for file in files {
        if usage.after_bytes <= limit && remaining <= max_entries {
            break;
        }
        remove(&file, &mut usage)?;
        remaining = remaining.saturating_sub(1);
    }
    Ok(usage)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(root: &Path, name: &str, content: &[u8]) -> PathBuf {
        let path = root.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }

    fn receipt(root: &Path, key: &str, generation: &str, bytes: usize) -> Vec<PathBuf> {
        vec![
            put(
                root,
                &format!("{key}.json"),
                format!("{{\"record\":{{\"implementation\":\"{generation}\"}}}}").as_bytes(),
            ),
            put(root, &format!("{key}.image"), &vec![42; bytes]),
        ]
    }

    #[test]
    fn caps_evict_inactive_groups_before_images_used_in_this_run() {
        let temp = tempfile::tempdir().unwrap();
        let media = Namespace::DeploymentMedia.dir(temp.path());
        let active = receipt(&media, "active", "current", 40);
        let inactive = receipt(&media, "inactive", "current", 40);
        mark_used(&active[0]);
        let size = active
            .iter()
            .map(|path| path.metadata().unwrap().len())
            .sum();
        let report = enforce_media(&media, size, "current").unwrap();
        assert_eq!(report.after_bytes, size);
        assert!(active.iter().all(|path| path.exists()));
        assert!(inactive.iter().all(|path| !path.exists()));
    }

    #[test]
    fn obsolete_media_is_removed_even_below_the_cap() {
        let temp = tempfile::tempdir().unwrap();
        let media = Namespace::DeploymentMedia.dir(temp.path());
        let old = receipt(&media, "old", "previous", 40);
        let live = receipt(&media, "live", "current", 40);
        let report = enforce_media(&media, u64::MAX, "current").unwrap();
        assert_eq!(report.removed_files, 2);
        assert!(old.iter().all(|path| !path.exists()));
        assert!(live.iter().all(|path| path.exists()));
    }

    #[test]
    fn response_budget_covers_lookup_and_validation_state_without_touching_archive_or_outputs() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("cache");
        let namespaces = [
            Namespace::Articles,
            Namespace::Discussions,
            Namespace::ValidatedImages,
            Namespace::ImageFailures,
            Namespace::CaptureRetries,
            Namespace::RecordingDuration,
            Namespace::FeedParsing,
            Namespace::Budget,
            Namespace::PreparedMarkdown,
        ];
        let state: Vec<_> = namespaces
            .into_iter()
            .map(|namespace| put(&namespace.dir(&root), "old-state", &[1; 10]))
            .collect();
        let active = &state[1];
        mark_used(active);
        let protected = [
            put(temp.path(), "data/items/master.png", &[2; 40]),
            put(&Namespace::Render.dir(&root), "manifest.toml", &[3; 40]),
            put(
                &Namespace::Pagefind.dir(&root),
                "site/pagefind.js",
                &[4; 40],
            ),
        ];
        let report = enforce_limits(
            &root,
            &crate::config::CacheConfig {
                media_max_bytes: 100,
                response_max_bytes: 20,
            },
        )
        .unwrap();
        assert_eq!(report.responses.before_bytes, 90);
        assert_eq!(report.responses.after_bytes, 20);
        assert_eq!(report.responses.removed_files, 7);
        assert!(
            active.exists(),
            "recently used lookup state is preferred to inactive responses"
        );
        assert!(
            protected
                .iter()
                .all(|path| path.metadata().unwrap().len() == 40)
        );
    }

    #[test]
    fn response_namespaces_cover_every_cache_except_single_generation_output_and_media() {
        let mut classified = BTreeSet::from([
            Namespace::Render,
            Namespace::Pagefind,
            Namespace::DeploymentMedia,
        ]);
        classified.extend(RESPONSE_NAMESPACES);
        assert_eq!(classified, Namespace::ALL.into_iter().collect());
    }

    #[test]
    fn entry_count_bounds_small_file_overhead_even_below_the_byte_limit() {
        let temp = tempfile::tempdir().unwrap();
        let articles = Namespace::Articles.dir(temp.path());
        let lookups = Namespace::Discussions.dir(temp.path());
        let old = put(&articles, "old", b"");
        let later = put(&articles, "later", b"");
        let active = put(&lookups, "active", b"");
        mark_used(&active);
        let report = enforce_responses(&[articles, lookups], 100, 2).unwrap();
        assert_eq!(report.before_bytes, 0);
        assert_eq!(report.after_bytes, 0);
        assert_eq!(report.removed_files, 1);
        assert!(active.exists());
        assert_eq!([old, later].iter().filter(|path| path.exists()).count(), 1);
    }

    #[test]
    fn response_cap_is_exact_and_does_not_touch_adjacent_archive() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("cache");
        let responses = Namespace::Articles.dir(&root);
        let inactive = put(&responses, "bodies/old.html", &[1; 20]);
        let active = put(&responses, "bodies/live.html", &[2; 15]);
        let archive = put(temp.path(), "data/items/master.png", &[3; 30]);
        mark_used(&active);
        let report =
            enforce_responses(std::slice::from_ref(&responses), 15, MAX_RESPONSE_ENTRIES).unwrap();
        assert_eq!(report.after_bytes, 15);
        assert!(!inactive.exists());
        assert!(active.exists());
        assert_eq!(std::fs::read(archive).unwrap(), [3; 30]);
        assert_eq!(
            enforce_responses(&[responses], 15, MAX_RESPONSE_ENTRIES)
                .unwrap()
                .removed_files,
            0
        );
    }

    #[cfg(unix)]
    #[test]
    fn redirected_cache_namespaces_are_never_pruned() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("archive");
        let original = put(&archive, "master.png", &[3; 30]);
        let root = temp.path().join("cache");
        std::fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(&archive, Namespace::Articles.dir(&root)).unwrap();
        assert!(
            enforce_responses(&[Namespace::Articles.dir(&root)], 1, MAX_RESPONSE_ENTRIES).is_err()
        );
        assert!(original.exists());
    }

    #[cfg(unix)]
    #[test]
    fn redirected_lookup_namespaces_abort_before_evicting_any_response() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("archive");
        let original = put(&archive, "master.png", &[3; 30]);
        let root = temp.path().join("cache");
        let response = put(&Namespace::Articles.dir(&root), "response", &[1; 10]);
        std::os::unix::fs::symlink(&archive, Namespace::Discussions.dir(&root)).unwrap();
        assert!(
            enforce_limits(
                &root,
                &crate::config::CacheConfig {
                    media_max_bytes: 100,
                    response_max_bytes: 1,
                }
            )
            .is_err()
        );
        assert_eq!(std::fs::read(original).unwrap(), [3; 30]);
        assert_eq!(std::fs::read(response).unwrap(), [1; 10]);
    }
}
