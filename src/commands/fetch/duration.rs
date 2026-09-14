use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

use crate::config::{Engine, Source};
use crate::store::SourceState;

const PARSER_VERSION: &[u8] = b"itunes-duration-seconds-v1\n";

pub(super) struct Receipt {
    path: PathBuf,
}

/// A parser change needs one unconditional response, without changing archived no-op state.
pub(super) fn prepare(
    source: &Source,
    state: &mut SourceState,
    cache_dir: &Path,
) -> Result<Option<Receipt>> {
    if !matches!(source.engine, Engine::Feed { .. }) {
        return Ok(None);
    }
    let receipt = Receipt {
        path: cache_dir.join("feed-parsing").join(format!(
            "{}.receipt",
            crate::model::sha1_hex(source.identity.as_bytes())
        )),
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
        if metadata.len() != PARSER_VERSION.len() as u64 {
            return Ok(false);
        }
        Ok(fs::read(&self.path).context("reading feed parser receipt")? == PARSER_VERSION)
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
            .write_all(PARSER_VERSION)
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
