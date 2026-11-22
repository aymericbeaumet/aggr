use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};

/// Exact inventory of the checked-in compiler and runtime sources, excluding the receipt itself.
pub fn vendor_inventory(root: &Path) -> Result<BTreeMap<String, String>> {
    let mut inventory = BTreeMap::new();
    for entry in walkdir::WalkDir::new(root).sort_by_file_name() {
        let entry = entry.context("Reading vendored frontend sources")?;
        ensure!(
            !entry.file_type().is_symlink(),
            "Vendored sources must not contain symlinks: {}",
            entry.path().display()
        );
        if entry.file_type().is_file() {
            let relative = entry
                .path()
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/");
            if relative == "integrity.json" {
                continue;
            }
            let bytes = std::fs::read(entry.path())?;
            inventory.insert(relative, hex::encode(Sha256::digest(&bytes)));
        }
    }
    Ok(inventory)
}

/// Verify local dependencies without invoking a package manager or accessing the network.
pub fn verify_vendor() -> Result<()> {
    static VERIFIED: OnceLock<std::result::Result<(), String>> = OnceLock::new();
    VERIFIED
        .get_or_init(|| verify_current_sources().map_err(|error| format!("{error:#}")))
        .as_ref()
        .copied()
        .map_err(|message| anyhow::anyhow!("{message}"))
}

fn verify_current_sources() -> Result<()> {
    let root = crate::vendor_root();
    let expected: BTreeMap<String, String> = serde_json::from_slice(&std::fs::read(root.join("integrity.json"))
        .context("Missing vendored dependency receipt; run mise run vendor -- record after reviewing source updates")?)?;
    let actual = vendor_inventory(&root)?;
    ensure!(!expected.is_empty(), "Vendored dependency receipt is empty");
    for (path, digest) in &expected {
        ensure!(
            actual.get(path) == Some(digest),
            "Vendored dependency changed or disappeared: {path}; review it before recording a new receipt"
        );
    }
    ensure!(
        actual.len() == expected.len(),
        "Vendored dependencies contain unrecorded files; review them before recording a new receipt"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_hashes_source_bytes_and_includes_new_files() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("compiler.js"), "first").unwrap();
        let first = vendor_inventory(root.path()).unwrap();
        std::fs::write(root.path().join("compiler.js"), "second").unwrap();
        assert_ne!(vendor_inventory(root.path()).unwrap(), first);
        std::fs::write(root.path().join("integrity.json"), "receipt").unwrap();
        assert_eq!(vendor_inventory(root.path()).unwrap().len(), 1);
        std::fs::write(root.path().join("extra.js"), "extra").unwrap();
        assert_eq!(vendor_inventory(root.path()).unwrap().len(), 2);
    }
}
