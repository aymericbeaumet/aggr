use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Component, Path};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use base64::Engine as _;
use serde_json::{Value, json};
use sha2::{Digest, Sha256, Sha512};

pub async fn run(args: Vec<String>) -> Result<()> {
    ensure!(args.len() == 1, "Usage: vendor <verify|record|update>");
    let root = aggr_ui_build::vendor_root();
    match args[0].as_str() {
        "verify" => {
            aggr_ui_build::verify_vendor()?;
            println!("Vendored compiler and runtime sources verified.");
        }
        "record" => record(&root)?,
        "update" => update(&root).await?,
        other => {
            anyhow::bail!("Unknown vendor operation {other}; expected verify, record or update")
        }
    }
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

async fn update(root: &Path) -> Result<()> {
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
