use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

use crate::cache::Namespace;
use crate::config::Source;
use crate::store::SourceState;

const PARSER_VERSION: &[u8] = b"source-selection-v3:light-feed-only\n";

pub(super) struct Receipt {
    path: PathBuf,
    version: Vec<u8>,
}

/// Parser or policy changes need one unconditional enumeration, without rewriting archive state.
pub(super) fn prepare(
    source: &Source,
    state: &mut SourceState,
    cache_dir: &Path,
) -> Result<Option<Receipt>> {
    let mut version = PARSER_VERSION.to_vec();
    version.extend(crate::store::evictions::source_policy(source)?.as_bytes());
    let receipt = Receipt {
        path: Namespace::FeedParsing.dir(cache_dir).join(format!(
            "{}.receipt",
            crate::model::sha1_hex(source.identity.as_bytes())
        )),
        version,
    };
    if receipt.current()? {
        return Ok(None);
    }
    state.etag = None;
    state.last_modified = None;
    state.body_hash = None;
    Ok(Some(receipt))
}

impl Receipt {
    fn current(&self) -> Result<bool> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error).context("inspecting feed parser receipt"),
        };
        if !metadata.file_type().is_file() {
            bail!(
                "feed parser receipt is not a regular file: {}",
                self.path.display()
            );
        }
        if metadata.len() != self.version.len() as u64 {
            return Ok(false);
        }
        Ok(fs::read(&self.path).context("reading source parser receipt")? == self.version)
    }

    /// Called only after the source transaction succeeds; failed sources must retry parsing.
    pub(super) fn commit(self) -> Result<()> {
        if self.current()? {
            return Ok(());
        }
        let directory = self
            .path
            .parent()
            .context("feed parser receipt has no directory")?;
        fs::create_dir_all(directory).context("creating feed parser receipt directory")?;
        let mut temporary = tempfile::NamedTempFile::new_in(directory)
            .context("creating temporary feed parser receipt")?;
        temporary
            .write_all(&self.version)
            .context("writing feed parser receipt")?;
        temporary
            .persist(&self.path)
            .context("publishing feed parser receipt")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_sources_retry_and_successful_sources_resume_validators_without_state_rewrites() {
        let directory = tempfile::tempdir().unwrap();
        let source = super::super::tests::source();
        let original = SourceState {
            identity: source.identity.clone(),
            resolved_url: Some("https://example.com/resolved.xml".into()),
            title: Some("Publisher".into()),
            etag: Some("unchanged".into()),
            last_modified: Some("Mon, 01 Sep 2026 00:00:00 GMT".into()),
            body_hash: Some("original body".into()),
            ..Default::default()
        };
        let mut first = original.clone();
        let failed = prepare(&source, &mut first, directory.path())
            .unwrap()
            .unwrap();
        assert_eq!(first.resolved_url, original.resolved_url);
        assert_eq!(first.title, original.title);
        assert_eq!(first.identity, original.identity);
        assert!(first.etag.is_none() && first.last_modified.is_none() && first.body_hash.is_none());
        assert!(!failed.path.exists());
        drop(failed);
        let receipt = prepare(&source, &mut original.clone(), directory.path())
            .unwrap()
            .unwrap();
        let path = receipt.path.clone();
        receipt.commit().unwrap();
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        let mut next = original.clone();
        assert!(
            prepare(&source, &mut next, directory.path())
                .unwrap()
                .is_none()
        );
        assert_eq!(next, original);
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
        let mut changed_source = source.clone();
        changed_source.identity = "changed configuration".into();
        assert!(
            prepare(&changed_source, &mut next, directory.path())
                .unwrap()
                .is_some()
        );
        fs::write(&path, "old parser").unwrap();
        assert!(
            prepare(&source, &mut original.clone(), directory.path())
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn content_mode_changes_revalidate_sources_without_discarding_endpoint_identity() {
        let directory = tempfile::tempdir().unwrap();
        let mut source = super::super::tests::source();
        let original = SourceState {
            identity: source.identity.clone(),
            resolved_url: Some("https://example.com/news".into()),
            etag: Some("same-page".into()),
            body_hash: Some("same-body".into()),
            ..Default::default()
        };
        prepare(&source, &mut original.clone(), directory.path())
            .unwrap()
            .unwrap()
            .commit()
            .unwrap();
        source.content = crate::config::ContentMode::Light;
        let mut next = original.clone();
        let changed = prepare(&source, &mut next, directory.path())
            .unwrap()
            .expect("a cached HTML response must be reconsidered under feed-only mode");
        assert!(next.etag.is_none() && next.body_hash.is_none());
        assert_eq!(next.resolved_url, original.resolved_url);
        changed.commit().unwrap();
        assert!(
            prepare(&source, &mut next, directory.path())
                .unwrap()
                .is_none()
        );

        source.content = crate::config::ContentMode::Heavy;
        assert!(
            prepare(&source, &mut next, directory.path())
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn limits_and_media_changes_revalidate_unchanged_git_mirrors() {
        let directory = tempfile::tempdir().unwrap();
        let mut source = super::super::tests::source();
        source.engine = crate::config::Engine::Aggr {
            url: "https://example.com/reader.git".parse().unwrap(),
            branch: "aggr".into(),
            sources: Vec::new(),
        };
        let mut state = SourceState {
            identity: source.identity.clone(),
            body_hash: Some("unchanged git commit".into()),
            ..Default::default()
        };
        for iteration in 0..3 {
            if iteration == 1 {
                source.limits.max_items += 1;
            }
            if iteration == 2 {
                source.images = crate::config::ImagePolicy::Remote;
            }
            let receipt = prepare(&source, &mut state, directory.path())
                .unwrap()
                .expect("effective source options must reselect the same upstream commit");
            assert!(state.body_hash.is_none());
            receipt.commit().unwrap();
            state.body_hash = Some("unchanged git commit".into());
            assert!(
                prepare(&source, &mut state, directory.path())
                    .unwrap()
                    .is_none()
            );
            assert_eq!(state.body_hash.as_deref(), Some("unchanged git commit"));
        }
    }

    #[test]
    fn parser_receipts_do_not_hide_filesystem_errors() {
        let directory = tempfile::tempdir().unwrap();
        let source = super::super::tests::source();
        let receipt = prepare(&source, &mut SourceState::default(), directory.path())
            .unwrap()
            .unwrap();
        fs::create_dir_all(&receipt.path).unwrap();
        assert!(prepare(&source, &mut SourceState::default(), directory.path()).is_err());
        assert!(receipt.commit().is_err());
    }
}
