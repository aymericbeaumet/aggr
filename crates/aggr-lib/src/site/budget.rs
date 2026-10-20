//! Deterministic media admission and bounded attempts to fit a complete static reader.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use chrono::{DateTime, Duration, Utc};

pub(super) fn full_quality(published: DateTime<Utc>, now: DateTime<Utc>, days: u32) -> bool {
    now.signed_duration_since(published) <= Duration::days(i64::from(days))
}

pub(super) struct MediaBudget {
    limit: u64,
    pub used: u64,
    pub omitted: usize,
    paths: BTreeMap<String, u64>,
}

impl MediaBudget {
    pub fn new(limit: u64) -> Self {
        Self {
            limit,
            used: 0,
            omitted: 0,
            paths: BTreeMap::new(),
        }
    }

    pub fn allowance(&self) -> u64 {
        self.limit
    }

    pub fn fits(&self, parts: &[(String, u64)]) -> bool {
        let additional: BTreeMap<_, _> = parts
            .iter()
            .filter(|(path, _)| !self.paths.contains_key(path))
            .map(|(path, bytes)| (path, *bytes))
            .collect();
        additional
            .values()
            .fold(0u64, |total, bytes| total.saturating_add(*bytes))
            <= self.limit.saturating_sub(self.used)
    }

    /// Admit whole image families so a published srcset and its master always exist together.
    /// Shared content-addressed paths consume space once, even across articles.
    pub fn admit(&mut self, parts: impl IntoIterator<Item = (String, u64)>) -> bool {
        let additional: BTreeMap<_, _> = parts
            .into_iter()
            .filter(|(path, _)| !self.paths.contains_key(path))
            .collect();
        let bytes = additional
            .values()
            .fold(0u64, |total, bytes| total.saturating_add(*bytes));
        if bytes > self.limit.saturating_sub(self.used) {
            self.omitted += 1;
            return false;
        }
        self.used += bytes;
        self.paths.extend(additional);
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Attempt {
    Media { allowance: u64, adjustments: u8 },
    TextOnly,
}

impl Attempt {
    pub fn new(limit: u64) -> Self {
        Self::Media {
            allowance: limit,
            adjustments: 2,
        }
    }

    /// Start from what a previous build measured. It keeps its full set of adjustments, so a
    /// remembered answer that no longer fits is corrected exactly as a first guess would be.
    pub fn resuming(allowance: u64) -> Self {
        Self::Media {
            allowance,
            adjustments: 2,
        }
    }

    pub fn allowance(self) -> u64 {
        match self {
            Self::Media { allowance, .. } => allowance,
            Self::TextOnly => 0,
        }
    }

    /// Exact measured overflow reserves space for text, manifests and reader assets on retry.
    /// A final media-free attempt distinguishes insufficient capacity from optional media cost.
    pub fn next(self, excess: u64, admitted: u64) -> Option<Self> {
        match self {
            Self::Media { adjustments, .. } if adjustments > 0 && admitted > excess => {
                Some(Self::Media {
                    allowance: admitted - excess,
                    adjustments: adjustments - 1,
                })
            }
            Self::Media { .. } if admitted > 0 => Some(Self::TextOnly),
            _ => None,
        }
    }
}

pub(super) fn output_bytes(root: &Path) -> Result<u64> {
    let mut bytes = 0u64;
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry.with_context(|| format!("measuring build output {}", root.display()))?;
        if entry.file_type().is_dir() {
            continue;
        }
        if !entry.file_type().is_file() {
            bail!(
                "non-regular file in generated site: {}",
                entry.path().display()
            );
        }
        bytes = bytes
            .checked_add(entry.metadata()?.len())
            .context("generated site size overflow")?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_admission_counts_shared_paths_and_rejects_whole_families() {
        let mut budget = MediaBudget::new(10);
        assert!(budget.admit([("master".into(), 6), ("small".into(), 2)]));
        assert!(budget.admit([("master".into(), 6), ("other".into(), 2)]));
        assert_eq!(budget.used, 10);
        assert!(budget.admit([("master".into(), 6)]));
        assert!(!budget.admit([("master".into(), 6), ("large".into(), 1)]));
        assert_eq!(budget.used, 10);
        assert_eq!(budget.omitted, 1);
    }

    #[test]
    fn oversized_new_media_does_not_prevent_smaller_later_media() {
        let mut budget = MediaBudget::new(10);
        assert!(!budget.admit([("oversized".into(), 11)]));
        assert!(budget.admit([("fits".into(), 10)]));
        assert!(!budget.admit([("older".into(), 1)]));
    }

    #[test]
    fn attempts_reduce_measured_overflow_and_end_with_text() {
        let first = Attempt::new(100);
        let second = first.next(20, 90).unwrap();
        assert_eq!(second.allowance(), 70);
        let third = second.next(1, 70).unwrap();
        assert_eq!(third.allowance(), 69);
        assert_eq!(third.next(1, 69), Some(Attempt::TextOnly));
        assert_eq!(Attempt::TextOnly.next(1, 0), None);
        assert_eq!(first.next(100, 90), Some(Attempt::TextOnly));
        assert_eq!(first.next(1, 0), None);
    }

    #[test]
    fn full_quality_includes_the_boundary_and_future_dates() {
        let now = DateTime::parse_from_rfc3339("2026-09-20T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(full_quality(now - Duration::days(30), now, 30));
        assert!(!full_quality(
            now - Duration::days(30) - Duration::seconds(1),
            now,
            30
        ));
        assert!(full_quality(now + Duration::days(1), now, 30));
        assert!(full_quality(DateTime::<Utc>::MIN_UTC, now, u32::MAX));
    }
}
