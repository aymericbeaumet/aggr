//! Output directory preparation: a previous build is wiped only when it is clearly ours.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

/// Marker file every build writes at its root so the next build may clear the directory.
pub(super) const MARKER: &str = ".aggr-site";

/// Wipe a previous build, refusing to touch a directory we did not create.
pub(crate) fn prepare_out_dir(out: &Path) -> Result<()> {
    let resolved = validate_replaceable_output(out)?;
    if resolved.exists() {
        std::fs::remove_dir_all(&resolved)
            .with_context(|| format!("clearing {}", resolved.display()))?;
    }
    std::fs::create_dir_all(&resolved).with_context(|| format!("creating {}", resolved.display()))
}

/// Promote a verified staged tree and retain the previous tree until replacement succeeds.
pub(crate) fn promote(staging: &Path, out: &Path) -> Result<()> {
    let out = validate_replaceable_output(out)?;
    let parent = out.parent().context("output parent")?;
    std::fs::create_dir_all(parent)?;
    let transaction = tempfile::Builder::new()
        .prefix(".aggr-publish-")
        .tempdir_in(parent)?;
    let next = transaction.path().join("next");
    // A cache can be on another filesystem. Copy only when rename cannot cross that boundary.
    if let Err(error) = std::fs::rename(staging, &next) {
        if error.kind() != std::io::ErrorKind::CrossesDevices {
            return Err(error.into());
        }
        for entry in walkdir::WalkDir::new(staging).follow_links(false) {
            let entry = entry?;
            let target = next.join(entry.path().strip_prefix(staging)?);
            if entry.file_type().is_dir() {
                std::fs::create_dir_all(target)?;
            } else if entry.file_type().is_file() {
                std::fs::copy(entry.path(), target)?;
            } else {
                bail!("staged output contains a non-file entry");
            }
        }
    }
    let previous = transaction.path().join("previous");
    if out.exists() {
        std::fs::rename(&out, &previous)?;
    }
    if let Err(error) = std::fs::rename(&next, &out) {
        if previous.exists() {
            std::fs::rename(&previous, &out).context("restoring previous output")?;
        }
        return Err(error).context("promoting generated output");
    }
    Ok(())
}

fn validate_replaceable_output(out: &Path) -> Result<PathBuf> {
    if out
        .components()
        .any(|component| component == std::path::Component::ParentDir)
    {
        bail!(
            "refusing output path with parent traversal: {}",
            out.display()
        );
    }
    if std::fs::symlink_metadata(out).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        bail!("refusing symlink output {}", out.display());
    }
    let absolute = resolve_output_path(out)?;
    if absolute.parent().is_none()
        || absolute.components().any(|component| {
            component
                .as_os_str()
                .to_str()
                .is_some_and(|name| name.eq_ignore_ascii_case(".git"))
        })
    {
        bail!("refusing protected output path {}", out.display());
    }

    let metadata = match std::fs::symlink_metadata(&absolute) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(absolute),
        Err(error) => return Err(error).with_context(|| format!("inspecting {}", out.display())),
    };
    if !metadata.is_dir() {
        bail!("refusing to clear {}: expected a directory", out.display());
    }
    if std::fs::read_dir(&absolute)?.next().is_none() {
        return Ok(absolute);
    }

    let marker = std::fs::symlink_metadata(absolute.join(MARKER)).map_err(|error| {
        anyhow::anyhow!(
            "refusing to clear {}: not an aggr output directory (no {MARKER} marker): {error}",
            out.display()
        )
    })?;
    if marker.file_type().is_symlink() || !marker.is_file() {
        bail!("refusing invalid output marker in {}", out.display());
    }
    for entry in walkdir::WalkDir::new(&absolute).follow_links(false) {
        let entry = entry?;
        if entry.file_type().is_symlink() {
            bail!(
                "refusing output containing symlink {}",
                entry.path().display()
            );
        }
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.eq_ignore_ascii_case(".git"))
        {
            bail!(
                "refusing generated output containing Git metadata: {}",
                entry.path().display()
            );
        }
    }
    Ok(absolute)
}

fn resolve_output_path(path: &Path) -> Result<PathBuf> {
    let mut existing = std::path::absolute(path)?;
    let mut missing = Vec::new();
    while !existing.exists() {
        missing.push(
            existing
                .file_name()
                .context("output path has an existing ancestor")?
                .to_os_string(),
        );
        existing.pop();
    }
    let mut resolved = existing
        .canonicalize()
        .with_context(|| format!("resolving output ancestor {}", existing.display()))?;
    for component in missing.into_iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_to_clear_foreign_directories() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("precious"), "x").unwrap();
        assert!(prepare_out_dir(dir.path()).is_err());
        std::fs::write(dir.path().join(MARKER), "").unwrap();
        prepare_out_dir(dir.path()).unwrap();
        assert!(!dir.path().join("precious").exists());
    }

    #[test]
    fn refuses_git_metadata_inside_owned_output() {
        for name in [".git", ".GIT"] {
            let root = tempfile::tempdir().unwrap();
            let out = root.path().join("site");
            std::fs::create_dir_all(out.join("nested").join(name)).unwrap();
            std::fs::write(out.join(MARKER), "1").unwrap();
            std::fs::write(out.join("nested").join(name).join("proof"), "keep").unwrap();
            let error = prepare_out_dir(&out).unwrap_err();
            assert!(format!("{error:#}").contains("Git metadata"));
            assert!(out.join("nested").join(name).join("proof").exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinks_inside_owned_output() {
        let root = tempfile::tempdir().unwrap();
        let out = root.path().join("site");
        let outside = root.path().join("outside");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(out.join(MARKER), "1").unwrap();
        std::fs::write(outside.join("proof"), "keep").unwrap();
        std::os::unix::fs::symlink(&outside, out.join("linked")).unwrap();

        let error = prepare_out_dir(&out).unwrap_err();
        assert!(format!("{error:#}").contains("symlink"));
        assert_eq!(
            std::fs::read_to_string(outside.join("proof")).unwrap(),
            "keep"
        );
    }
}
