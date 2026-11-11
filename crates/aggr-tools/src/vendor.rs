use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Component, Path};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256, Sha512};

pub async fn run(args: Vec<String>) -> Result<()> {
    ensure!(
        args.len() == 1,
        "Usage: vendor <verify|record|patch|update>"
    );
    let root = aggr_ui_build::vendor_root();
    match args[0].as_str() {
        "verify" => {
            aggr_ui_build::verify_vendor()?;
            println!("Vendored compiler and runtime sources verified.");
        }
        "record" => record(&root)?,
        "patch" => update(&root, true).await?,
        "update" => update(&root, false).await?,
        other => {
            anyhow::bail!(
                "Unknown vendor operation {other}; expected verify, record, patch or update"
            )
        }
    }
    Ok(())
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SourcePatch {
    path: String,
    create: bool,
    upstream_sha256: String,
    sha256: String,
    diff: String,
}

impl SourcePatch {
    fn new(path: String, original: &str, patched: &str) -> Self {
        Self {
            path,
            create: false,
            upstream_sha256: hex::encode(Sha256::digest(original.as_bytes())),
            sha256: hex::encode(Sha256::digest(patched.as_bytes())),
            diff: diffy::create_patch(original, patched).to_string(),
        }
    }

    fn apply(&self, original: &str) -> Result<String> {
        let path = Path::new(&self.path);
        ensure!(
            path.starts_with("packages")
                && !self.path.contains('\\')
                && path
                    .components()
                    .all(|part| matches!(part, Component::Normal(_))),
            "Unsafe vendor patch path {}",
            self.path
        );
        ensure!(
            hex::encode(Sha256::digest(original.as_bytes())) == self.upstream_sha256,
            "Upstream source changed for {}; review its security patch before updating",
            self.path
        );
        let patch = diffy::Patch::from_str(&self.diff)
            .map_err(|error| anyhow::anyhow!("Invalid patch for {}: {error}", self.path))?;
        let patched = diffy::apply(original, &patch)
            .map_err(|error| anyhow::anyhow!("Cannot patch {}: {error}", self.path))?;
        ensure!(
            hex::encode(Sha256::digest(patched.as_bytes())) == self.sha256,
            "Patched source checksum mismatch for {}",
            self.path
        );
        Ok(patched)
    }
}

fn patch_sources(root: &Path, stage: &Path, record_patches: bool) -> Result<()> {
    let receipt = root.join("patches.json");
    let patches: Vec<SourcePatch> = if record_patches {
        let mut patches = Vec::new();
        // Deletions need an explicit policy rather than silently restoring removed files.
        for entry in walkdir::WalkDir::new(stage.join("packages")).sort_by_file_name() {
            let entry = entry?;
            if !entry.file_type().is_file() {
                continue;
            }
            let relative = entry.path().strip_prefix(stage)?;
            ensure!(
                root.join(relative).is_file(),
                "Missing local source {}",
                relative.display()
            );
        }
        // Compare verified originals with reviewed local sources, including added helpers.
        for entry in walkdir::WalkDir::new(root.join("packages")).sort_by_file_name() {
            let entry = entry?;
            ensure!(
                !entry.file_type().is_symlink(),
                "Vendored sources must not contain symlinks"
            );
            if !entry.file_type().is_file() {
                continue;
            }
            let relative = entry.path().strip_prefix(root)?;
            let upstream = stage.join(relative);
            let create = !upstream.exists();
            let original = if create {
                Vec::new()
            } else {
                std::fs::read(upstream)?
            };
            let patched = std::fs::read(entry.path())?;
            if create || original != patched {
                let mut patch = SourcePatch::new(
                    relative.to_string_lossy().replace('\\', "/"),
                    std::str::from_utf8(&original)?,
                    std::str::from_utf8(&patched)?,
                );
                patch.create = create;
                patches.push(patch);
            }
        }
        patches
    } else if receipt.exists() {
        serde_json::from_slice(&std::fs::read(&receipt)?)?
    } else {
        Vec::new()
    };
    let mut seen = std::collections::BTreeSet::new();
    for patch in &patches {
        ensure!(
            seen.insert(&patch.path),
            "Duplicate vendor patch {}",
            patch.path
        );
        // Validate before joining the path to the staging root or writing any source.
        let path = Path::new(&patch.path);
        ensure!(
            path.starts_with("packages")
                && !patch.path.contains('\\')
                && path
                    .components()
                    .all(|part| matches!(part, Component::Normal(_))),
            "Unsafe vendor patch path {}",
            patch.path
        );
        let destination = stage.join(path);
        let original = if patch.create {
            ensure!(
                !destination.exists(),
                "Added vendor source now exists upstream: {}",
                patch.path
            );
            String::new()
        } else {
            std::fs::read_to_string(&destination)?
        };
        let patched = patch.apply(&original)?;
        std::fs::create_dir_all(destination.parent().context("Vendor patch has no parent")?)?;
        std::fs::write(&destination, patched)?;
    }
    if !patches.is_empty() || receipt.exists() {
        std::fs::write(
            stage.join("patches.json"),
            serde_json::to_vec_pretty(&patches)?,
        )?;
    }
    println!("Applied {} reviewed vendor source patches.", patches.len());
    Ok(())
}

fn record(root: &Path) -> Result<()> {
    let mut inventory = BTreeMap::new();
    for entry in walkdir::WalkDir::new(root).sort_by_file_name() {
        let entry = entry.context("Reading vendored frontend sources")?;
        ensure!(
            !entry.file_type().is_symlink(),
            "Vendored sources must not contain symlinks"
        );
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(root)?
            .to_string_lossy()
            .replace('\\', "/");
        if relative != "integrity.json" {
            inventory.insert(
                relative,
                hex::encode(Sha256::digest(std::fs::read(entry.path())?)),
            );
        }
    }
    let mut receipt = tempfile::NamedTempFile::new_in(root)?;
    serde_json::to_writer_pretty(&mut receipt, &inventory)?;
    receipt.write_all(b"\n")?;
    receipt
        .persist(root.join("integrity.json"))
        .map_err(|error| error.error)?;
    println!("Recorded {} reviewed vendored files.", inventory.len());
    Ok(())
}

fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
        && !matches!(value, "." | "..")
}

fn package_name(value: &str) -> bool {
    if let Some(scoped) = value.strip_prefix('@') {
        scoped
            .split_once('/')
            .is_some_and(|(scope, name)| valid_component(scope) && valid_component(name))
    } else {
        valid_component(value)
    }
}

fn verify_archive(bytes: &[u8], integrity: &str) -> Result<()> {
    let digest = integrity
        .split_ascii_whitespace()
        .find_map(|value| value.strip_prefix("sha512-"))
        .context("Registry must provide SHA-512 integrity")?;
    let expected = base64::engine::general_purpose::STANDARD.decode(digest)?;
    ensure!(
        expected.as_slice() == Sha512::digest(bytes).as_slice(),
        "Registry archive integrity mismatch"
    );
    Ok(())
}

fn verify_registry_pin(
    package: &Value,
    local_version: Option<&str>,
    integrity: &str,
) -> Result<()> {
    if local_version.is_some() && local_version == package["version"].as_str() {
        let recorded = package["source"]["integrity"]
            .as_str()
            .context("Pinned dependency has no recorded archive integrity")?;
        ensure!(
            recorded == integrity,
            "Registry archive integrity changed for {}@{}; refusing to replace the pinned source",
            package["name"],
            package["version"]
        );
    }
    Ok(())
}

fn retain(name: &str, path: &Path) -> bool {
    if path.starts_with("bin") {
        return false;
    }
    if name != "typescript" {
        return true;
    }
    let value = path.to_string_lossy();
    value == "package.json"
        || value == "lib/typescript.js"
        || value.ends_with(".d.ts")
        || value.to_ascii_lowercase().contains("license")
        || value.to_ascii_lowercase().contains("notice")
}

fn unpack(name: &str, bytes: &[u8], out: &Path) -> Result<()> {
    let decoder = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(decoder);
    let mut total = 0_u64;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let source = entry.path()?.into_owned();
        ensure!(
            source
                .components()
                .all(|part| matches!(part, Component::Normal(_))),
            "Unsafe dependency archive path {}",
            source.display()
        );
        let package_root = if name.starts_with("@types/") {
            name.trim_start_matches("@types/")
        } else {
            "package"
        };
        let relative = source
            .strip_prefix(package_root)
            .context("Dependency archive must have its expected package root")?;
        let kind = entry.header().entry_type();
        ensure!(
            kind.is_file() || kind.is_dir(),
            "Dependency archive contains an unsupported link or special file"
        );
        if kind.is_dir() || relative.as_os_str().is_empty() || !retain(name, relative) {
            continue;
        }
        total = total
            .checked_add(entry.size())
            .context("Dependency archive is too large")?;
        ensure!(
            total <= 128 * 1024 * 1024,
            "Dependency archive expands beyond 128 MiB"
        );
        let destination = out.join(relative);
        std::fs::create_dir_all(
            destination
                .parent()
                .context("Dependency file has no parent")?,
        )?;
        let mut file = std::fs::File::create(&destination)?;
        std::io::copy(&mut entry, &mut file)?;
    }
    Ok(())
}

async fn update(root: &Path, record_patches: bool) -> Result<()> {
    let mut packages: Vec<Value> =
        serde_json::from_slice(&std::fs::read(root.join("packages.json"))?)?;
    let stage = tempfile::Builder::new()
        .prefix(".vendor-")
        .tempdir_in(root.parent().context("Vendor directory has no parent")?)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .user_agent(concat!("aggr-tools/", env!("CARGO_PKG_VERSION")))
        .build()?;
    for package in &mut packages {
        let name = package["name"]
            .as_str()
            .context("Vendored package has no name")?
            .to_owned();
        let version = package["version"]
            .as_str()
            .context("Vendored package has no version")?
            .to_owned();
        ensure!(
            package_name(&name) && valid_component(&version),
            "Unsafe package name or version"
        );
        let local_manifest = root.join("packages").join(&name).join("package.json");
        let local: Option<Value> = match std::fs::read(&local_manifest) {
            Ok(bytes) => Some(serde_json::from_slice(&bytes)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let local_version = local.as_ref().and_then(|value| value["version"].as_str());
        if record_patches {
            ensure!(
                local_version == Some(version.as_str()),
                "Local {name} version differs from its manifest; rebase patches before recording"
            );
        }
        let response = client
            .get(format!("https://registry.npmjs.org/{name}/{version}"))
            .send()
            .await?
            .error_for_status()?;
        let metadata: Value = response.json().await?;
        ensure!(
            metadata["name"] == name && metadata["version"] == version,
            "Registry returned a different package version"
        );
        let url = reqwest::Url::parse(
            metadata["dist"]["tarball"]
                .as_str()
                .context("Package has no source archive")?,
        )?;
        ensure!(
            url.scheme() == "https"
                && url.host_str() == Some("registry.npmjs.org")
                && url.username().is_empty(),
            "Unexpected registry archive origin"
        );
        let integrity = metadata["dist"]["integrity"]
            .as_str()
            .context("Package has no integrity checksum")?;
        verify_registry_pin(package, local_version, integrity)?;
        let mut response = client.get(url.clone()).send().await?.error_for_status()?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= 32 * 1024 * 1024,
                "Dependency archive exceeds 32 MiB"
            );
            bytes.extend_from_slice(&chunk);
        }
        verify_archive(&bytes, integrity)?;
        let relative = format!("packages/{name}");
        unpack(&name, &bytes, &stage.path().join(&relative))?;
        package["path"] = relative.into();
        package["source"] = json!({"url": url.as_str(), "integrity": integrity});
        println!("Verified {name}@{version}");
    }
    patch_sources(root, stage.path(), record_patches)?;
    std::fs::write(
        stage.path().join("packages.json"),
        serde_json::to_vec_pretty(&packages)?,
    )?;
    let mut notices = String::from("Vendored compiler and frontend library sources.\n\n");
    for package in &packages {
        notices.push_str(&format!(
            "{} {} — {}\n",
            package["name"].as_str().unwrap_or_default(),
            package["version"].as_str().unwrap_or_default(),
            package["license"].as_str().unwrap_or_default()
        ));
    }
    notices.push_str("\nEach package retains its upstream license and notice files.\n");
    if stage.path().join("patches.json").exists() {
        notices.push_str("Local security fixes are recorded in patches.json. Package versions and source checksums identify the original upstream archives.\n");
    }
    std::fs::write(stage.path().join("NOTICE"), notices)?;
    record(stage.path())?;
    let previous = root.with_extension("previous");
    ensure!(
        !previous.exists(),
        "Previous vendor update is still present; inspect {}",
        previous.display()
    );
    std::fs::rename(root, &previous)?;
    if let Err(error) = std::fs::rename(stage.path(), root) {
        std::fs::rename(&previous, root).context("Restoring previous vendored sources")?;
        return Err(error).context("Promoting vendored sources");
    }
    std::fs::remove_dir_all(previous)?;
    aggr_ui_build::verify_vendor()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_registry_archives_cannot_change_without_a_version_upgrade() {
        let mut package = json!({"name":"example", "version":"1.0.0",
            "source":{"integrity":"sha512-original"}});
        verify_registry_pin(&package, Some("1.0.0"), "sha512-original").unwrap();
        assert!(verify_registry_pin(&package, Some("1.0.0"), "sha512-changed").is_err());
        package["version"] = "2.0.0".into();
        verify_registry_pin(&package, Some("1.0.0"), "sha512-upgraded").unwrap();
        verify_registry_pin(&package, None, "sha512-new-package").unwrap();
    }

    #[test]
    fn vendor_patches_require_exact_upstream_and_result() {
        let original = "const input = unsafe();\n";
        let fixed = "const input = bounded();\n";
        let mut patch = SourcePatch::new("packages/example/index.js".into(), original, fixed);
        assert_eq!(patch.apply(original).unwrap(), fixed);
        assert!(patch.apply("const input = changed();\n").is_err());
        patch.sha256 = "invalid".into();
        assert!(patch.apply(original).is_err());
    }

    #[test]
    fn vendor_patch_paths_cannot_escape_package_sources() {
        for path in [
            "../outside",
            "/tmp/outside",
            "packages/../outside",
            "packages\\escape",
            "NOTICE",
        ] {
            let patch = SourcePatch::new(path.into(), "old", "new");
            assert!(patch.apply("old").is_err(), "accepted {path}");
        }
    }

    #[test]
    fn vendor_patches_preserve_unicode_and_missing_final_newline() {
        let original = "const label = 'élève';";
        let fixed = "const label = 'élève 🦀';";
        let patch = SourcePatch::new("packages/example/index.js".into(), original, fixed);
        assert_eq!(patch.apply(original).unwrap(), fixed);
    }

    #[test]
    fn vendor_patches_reproduce_added_helpers_and_reject_upstream_drift() {
        let local = tempfile::tempdir().unwrap();
        let stage = tempfile::tempdir().unwrap();
        for root in [local.path(), stage.path()] {
            std::fs::create_dir_all(root.join("packages/example")).unwrap();
        }
        let path = "packages/example/index.js";
        std::fs::write(stage.path().join(path), "old\n").unwrap();
        std::fs::write(local.path().join(path), "fixed\n").unwrap();
        std::fs::write(local.path().join("packages/example/helper.js"), "helper\n").unwrap();
        patch_sources(local.path(), stage.path(), true).unwrap();
        assert_eq!(std::fs::read(stage.path().join(path)).unwrap(), b"fixed\n");
        assert_eq!(
            std::fs::read(stage.path().join("packages/example/helper.js")).unwrap(),
            b"helper\n"
        );
        std::fs::copy(
            stage.path().join("patches.json"),
            local.path().join("patches.json"),
        )
        .unwrap();
        std::fs::remove_file(stage.path().join("packages/example/helper.js")).unwrap();
        std::fs::write(stage.path().join(path), "old\n").unwrap();
        patch_sources(local.path(), stage.path(), false).unwrap();
        assert_eq!(std::fs::read(stage.path().join(path)).unwrap(), b"fixed\n");
        assert!(patch_sources(local.path(), stage.path(), false).is_err());
    }

    #[test]
    fn registry_checksums_reject_changed_archive_bytes() {
        let digest = format!(
            "sha512-{}",
            base64::engine::general_purpose::STANDARD.encode(Sha512::digest(b"archive"))
        );
        verify_archive(b"archive", &digest).unwrap();
        assert!(verify_archive(b"modified", &digest).is_err());
        assert!(verify_archive(b"archive", "sha1-old").is_err());
    }

    #[test]
    fn package_names_cannot_escape_the_vendor_directory() {
        for name in ["svelte", "@jridgewell/trace-mapping"] {
            assert!(package_name(name));
        }
        for name in [
            "..",
            "@scope/../escape",
            "/tmp",
            "@scope/",
            "name?token=value",
        ] {
            assert!(!package_name(name));
        }
    }

    #[test]
    fn vendoring_keeps_compiler_libraries_and_not_javascript_host_launchers() {
        assert!(retain("typescript", Path::new("lib/typescript.js")));
        assert!(retain("typescript", Path::new("lib/typescript.d.ts")));
        assert!(retain("typescript", Path::new("LICENSE.txt")));
        assert!(!retain("typescript", Path::new("lib/_tsc.js")));
        assert!(!retain("typescript", Path::new("bin/tsserver")));
        assert!(!retain("acorn", Path::new("bin/acorn")));
    }

    #[test]
    fn definitely_typed_archives_use_their_named_root() {
        // @types/estree publishes estree/, unlike ordinary npm package/ archives.
        let mut compressed =
            flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        {
            let mut archive = tar::Builder::new(&mut compressed);
            let mut header = tar::Header::new_gnu();
            header.set_path("estree/index.d.ts").unwrap();
            header.set_size(4);
            header.set_mode(0o644);
            header.set_cksum();
            archive.append(&header, &b"type"[..]).unwrap();
            archive.finish().unwrap();
        }
        let bytes = compressed.finish().unwrap();
        let root = tempfile::tempdir().unwrap();
        unpack("@types/estree", &bytes, root.path()).unwrap();
        assert_eq!(
            std::fs::read(root.path().join("index.d.ts")).unwrap(),
            b"type"
        );
        assert!(unpack("svelte", &bytes, root.path()).is_err());
    }

    #[test]
    fn vendor_archives_cannot_create_symlinks() {
        let mut compressed =
            flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        {
            let mut archive = tar::Builder::new(&mut compressed);
            let mut header = tar::Header::new_gnu();
            header.set_path("package/escape").unwrap();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_link_name("/tmp/escape").unwrap();
            header.set_size(0);
            header.set_cksum();
            archive.append(&header, std::io::empty()).unwrap();
            archive.finish().unwrap();
        }
        let bytes = compressed.finish().unwrap();
        let root = tempfile::tempdir().unwrap();
        assert!(unpack("svelte", &bytes, root.path()).is_err());
        assert!(!root.path().join("escape").exists());
    }
}
