use std::{
    collections::BTreeMap,
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use anyhow::{Context as _, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{BundleOptions, BundleOutput, bundle, vendor_root};

/// A complete immutable generation, ready for embedding or serving.
#[derive(Debug, Clone)]
pub struct BuildResult {
    pub directory: PathBuf,
    pub hash: String,
    pub cache_hit: bool,
}

#[derive(Serialize, Deserialize)]
struct Pointer {
    directory: String,
}

#[derive(Serialize, Deserialize)]
struct BundleCache {
    recipe: String,
    modules: BTreeMap<PathBuf, String>,
    output: BundleOutput,
}

fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(root).sort_by_file_name() {
        let entry = entry.with_context(|| format!("walking {}", root.display()))?;
        ensure!(
            !entry.file_type().is_symlink(),
            "UI input must not be a symlink: {}",
            entry.path().display()
        );
        if entry.file_type().is_file() {
            files.push(entry.into_path());
        }
    }
    files.sort_by(|left, right| left.as_os_str().cmp(right.as_os_str()));
    Ok(files)
}

fn inventory_hash(root: &Path) -> Result<String> {
    let mut digest = Sha256::new();
    for file in files(root)? {
        let path = file
            .strip_prefix(root)?
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = fs::read(&file)?;
        digest.update((path.len() as u64).to_le_bytes());
        digest.update(path.as_bytes());
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(&bytes);
    }
    Ok(hex::encode(digest.finalize()))
}

fn recipe() -> &'static str {
    static RECIPE: OnceLock<String> = OnceLock::new();
    RECIPE.get_or_init(|| {
        let mut digest = Sha256::new();
        for source in [
            include_str!("build.rs"),
            include_str!("bundle.rs"),
            include_str!("compiler.rs"),
            include_str!("../Cargo.toml"),
        ] {
            digest.update(source.as_bytes());
        }
        // Vendored inputs are immutable package sources, rather than ambient npm installations.
        if let Ok(value) = inventory_hash(&vendor_root()) {
            digest.update(value.as_bytes());
        }
        hex::encode(digest.finalize())
    })
}

fn source_hash(root: &Path) -> Result<String> {
    let mut digest = Sha256::new();
    digest.update(recipe().as_bytes());
    for relative in ["web/src", "web/public", "web/.generated/types"] {
        let directory = root.join(relative);
        ensure!(
            directory.is_dir(),
            "missing UI input {}; export the page model types first",
            directory.display()
        );
        for path in files(&directory)? {
            let name = path
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/");
            if name.ends_with(".test.ts") || name.ends_with(".dom.test.ts") {
                continue;
            }
            digest.update((name.len() as u64).to_le_bytes());
            digest.update(name.as_bytes());
            let bytes = fs::read(path)?;
            digest.update((bytes.len() as u64).to_le_bytes());
            digest.update(bytes);
        }
    }
    Ok(hex::encode(digest.finalize()))
}

fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().context("output has no parent")?;
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(&serde_json::to_vec(value)?)?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn cached_bundle(root: &Path, name: &str, options: BundleOptions) -> Result<BundleOutput> {
    let path = root
        .join("web/.generated/native-bundles")
        .join(format!("{name}.json"));
    let mut digest = Sha256::new();
    digest.update(format!("{}:{options:?}", recipe()).as_bytes());
    // New files can change extension or directory resolution without changing an imported file.
    for relative in ["web/src", "web/.generated/types"] {
        let directory = root.join(relative);
        if directory.is_dir() {
            for path in files(&directory)? {
                let name = path.strip_prefix(root)?.to_string_lossy();
                digest.update((name.len() as u64).to_le_bytes());
                digest.update(name.as_bytes());
            }
        }
    }
    let key = hex::encode(digest.finalize());
    if let Ok(bytes) = fs::read(&path)
        && let Ok(cached) = serde_json::from_slice::<BundleCache>(&bytes)
        && cached.recipe == key
        && !cached.modules.is_empty()
        && cached
            .modules
            .iter()
            .all(|(path, expected)| fs::read(path).is_ok_and(|bytes| hash(&bytes) == *expected))
    {
        return Ok(cached.output);
    }
    let before = if root.join("web/src").is_dir() {
        Some(source_hash(root)?)
    } else {
        None
    };
    let output = bundle(root, options)?;
    if let Some(before) = before {
        // Never stamp old bundle bytes with source hashes read after a concurrent edit.
        if source_hash(root)? != before {
            return Ok(output);
        }
    }
    let modules = output
        .modules
        .iter()
        .map(|path| Ok((path.clone(), hash(&fs::read(path)?))))
        .collect::<Result<BTreeMap<_, _>>>()?;
    atomic_json(
        &path,
        &BundleCache {
            recipe: key,
            modules,
            output: output.clone(),
        },
    )?;
    Ok(output)
}

fn write_hashed(
    out: &Path,
    logical: &str,
    bytes: &[u8],
    paths: &mut BTreeMap<String, String>,
) -> Result<String> {
    let path = Path::new(logical);
    let stem = path
        .file_stem()
        .context("asset has no name")?
        .to_string_lossy();
    let extension = path
        .extension()
        .map(|value| format!(".{}", value.to_string_lossy()))
        .unwrap_or_default();
    let filename = format!("{stem}-{}{extension}", &hash(bytes)[..12]);
    let physical = path
        .with_file_name(filename)
        .to_string_lossy()
        .replace('\\', "/");
    let destination = out.join(&physical);
    fs::create_dir_all(destination.parent().context("asset has no parent")?)?;
    fs::write(destination, bytes)?;
    paths.insert(logical.into(), physical.clone());
    Ok(physical)
}

fn prune(directory: &Path, limit: usize, preserve: Option<&str>, directories: bool) -> Result<()> {
    if !directory.is_dir() {
        return Ok(());
    }
    let mut entries = fs::read_dir(directory)?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let metadata = entry.metadata().ok()?;
            let digest = name.strip_suffix(".json").unwrap_or(&name);
            (metadata.is_dir() == directories
                && digest.len() == 64
                && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .then(|| (metadata.modified().ok(), name, entry.path()))
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    let mut retained = usize::from(preserve.is_some());
    for (_, name, path) in entries {
        if preserve == Some(name.as_str()) {
            continue;
        }
        if retained < limit {
            retained += 1;
            continue;
        }
        if directories {
            fs::remove_dir_all(path)?;
        } else {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn compile_generation(root: &Path, out: &Path) -> Result<()> {
    let mut paths = BTreeMap::new();
    let options = |entry: &str, server, global: Option<&str>| BundleOptions {
        entry: root.join("web/src").join(entry),
        server,
        global_name: global.map(str::to_owned),
        minify: true,
        ..Default::default()
    };
    let mut browser_options = options("main.ts", false, None);
    browser_options.code_splitting = true;
    let browser = cached_bundle(root, "browser", browser_options)?;
    for (name, bytes) in &browser.assets {
        let logical = format!("assets/app/{name}");
        let destination = out.join(&logical);
        fs::create_dir_all(
            destination
                .parent()
                .context("browser asset has no parent")?,
        )?;
        fs::write(destination, bytes)?;
        paths.insert(logical.clone(), logical);
    }
    let app = format!("assets/app/{}", browser.entry_file);
    let imports: Vec<_> = browser
        .imports
        .iter()
        .map(|name| format!("assets/app/{name}"))
        .collect();
    let css = browser
        .css
        .as_ref()
        .map(|css| write_hashed(out, "assets/app/app.css", css.as_bytes(), &mut paths))
        .transpose()?;
    let bootstrap = cached_bundle(
        root,
        "bootstrap",
        options("bootstrap.ts", false, Some("AggrBootstrap")),
    )?;
    let bootstrap = write_hashed(
        out,
        "assets/app/bootstrap.js",
        bootstrap.code.as_bytes(),
        &mut paths,
    )?;
    let worker = cached_bundle(
        root,
        "worker",
        options("sw/main.ts", false, Some("AggrWorker")),
    )?;
    let worker = write_hashed(
        out,
        "assets/app/worker.js",
        worker.code.as_bytes(),
        &mut paths,
    )?;
    let renderer = cached_bundle(
        root,
        "renderer",
        options("embedded.ts", true, Some("AggrRenderer")),
    )?;
    fs::write(out.join("renderer.js"), renderer.code)?;
    for path in files(&root.join("web/public"))? {
        let logical = path
            .strip_prefix(root.join("web/public"))?
            .to_string_lossy()
            .replace('\\', "/");
        write_hashed(out, &logical, &fs::read(path)?, &mut paths)?;
    }
    let mut notices = fs::read_to_string(vendor_root().join("NOTICE"))?;
    for path in files(&vendor_root())? {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        if name.starts_with("license") || name == "copying" || name == "thirdpartynoticetext.txt" {
            notices.push_str(&format!(
                "\n\n{}\n",
                path.strip_prefix(vendor_root())?.display()
            ));
            notices.push_str(&fs::read_to_string(path)?);
        }
    }
    write_hashed(
        out,
        "assets/third-party-notices.txt",
        notices.as_bytes(),
        &mut paths,
    )?;
    fs::write(
        out.join("assets.json"),
        serde_json::to_vec(&serde_json::json!({
            "contract": inventory_hash(&root.join("web/.generated/types"))?,
            "app": app, "css": css, "imports": imports, "bootstrap": bootstrap, "worker": worker, "files": paths,
        }))?,
    )?;
    Ok(())
}

/// Compile a repository's reader into immutable, content-addressed generations.
///
/// `root` contains `web/src`, `web/public` and exported `web/.generated/types`.
/// The returned directory contains `assets.json`, `renderer.js` and public assets.
/// `out/current.json` changes atomically after a complete successful build. Failed or
/// superseded builds leave the previous generation available. No external process runs.
pub fn build(root: &Path, out: &Path) -> Result<BuildResult> {
    crate::verify_vendor()?;
    let root = root.canonicalize().context("locating UI repository")?;
    fs::create_dir_all(out)?;
    let out = out.canonicalize()?;
    ensure!(
        !root.starts_with(&out),
        "UI output must not contain the source repository"
    );
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(out.join("build.lock"))?;
    lock.lock().context("locking native reader generation")?;
    loop {
        let input = source_hash(&root)?;
        let receipt = out.join(".sources").join(format!("{input}.json"));
        if let Ok(bytes) = fs::read(&receipt)
            && let Ok(pointer) = serde_json::from_slice::<Pointer>(&bytes)
            && pointer.directory.len() == 64
            && pointer
                .directory
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            let directory = out.join(&pointer.directory);
            if inventory_hash(&directory).is_ok_and(|hash| hash == pointer.directory) {
                atomic_json(&out.join("current.json"), &pointer)?;
                return Ok(BuildResult {
                    directory,
                    hash: pointer.directory,
                    cache_hit: true,
                });
            }
        }
        let staging = tempfile::Builder::new()
            .prefix(".staging-")
            .tempdir_in(&out)?;
        compile_generation(&root, staging.path()).context("compiling native reader generation")?;
        if source_hash(&root)? != input {
            continue;
        }
        let hash = inventory_hash(staging.path())?;
        let directory = out.join(&hash);
        if directory.exists() && inventory_hash(&directory).is_ok_and(|value| value != hash) {
            fs::remove_dir_all(&directory)?;
        }
        if !directory.exists() {
            fs::rename(staging.path(), &directory)?;
        }
        let pointer = Pointer {
            directory: hash.clone(),
        };
        atomic_json(&receipt, &pointer)?;
        atomic_json(&out.join("current.json"), &pointer)?;
        // Consumers retain generation bytes, so old disk generations are disposable.
        prune(&out, 4, Some(&hash), true)?;
        prune(&out.join(".sources"), 64, None, false)?;
        prune(
            &root.join("web/.generated/native-transforms"),
            2048,
            None,
            false,
        )?;
        return Ok(BuildResult {
            directory,
            hash,
            cache_hit: false,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_fingerprint_matches_sorted_embedded_asset_inventory() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("assets")).unwrap();
        fs::write(root.path().join("assets/app.js"), "app").unwrap();
        fs::write(root.path().join("assets.json"), "manifest").unwrap();
        let mut embedded = Sha256::new();
        for (name, bytes) in [("assets.json", "manifest"), ("assets/app.js", "app")] {
            embedded.update((name.len() as u64).to_le_bytes());
            embedded.update(name.as_bytes());
            embedded.update((bytes.len() as u64).to_le_bytes());
            embedded.update(bytes.as_bytes());
        }
        assert_eq!(
            inventory_hash(root.path()).unwrap(),
            hex::encode(embedded.finalize())
        );
    }

    #[test]
    fn unchanged_modules_are_reused_and_source_changes_invalidate_bundles() {
        let root = tempfile::tempdir().unwrap();
        let entry = root.path().join("entry.ts");
        fs::write(&entry, "export const value: number = 1;").unwrap();
        let options = || BundleOptions {
            entry: entry.clone(),
            ..Default::default()
        };
        let first = cached_bundle(root.path(), "test", options()).unwrap();
        fs::write(&entry, "export const value: number = 2;").unwrap();
        let second = cached_bundle(root.path(), "test", options()).unwrap();
        assert_ne!(first.code, second.code);
        assert_eq!(
            second.code,
            cached_bundle(root.path(), "test", options()).unwrap().code
        );
    }

    #[test]
    fn previously_tree_shaken_modules_can_gain_side_effects() {
        let root = tempfile::tempdir().unwrap();
        let entry = root.path().join("entry.ts");
        let unused = root.path().join("unused.ts");
        fs::write(&entry, "import './unused.ts'; export const value = 1;").unwrap();
        fs::write(&unused, "export const ignored = 2;").unwrap();
        let options = || BundleOptions {
            entry: entry.clone(),
            ..Default::default()
        };
        let first = cached_bundle(root.path(), "side-effects", options()).unwrap();
        fs::write(
            &unused,
            "console.log('new-side-effect'); export const ignored = 2;",
        )
        .unwrap();
        let second = cached_bundle(root.path(), "side-effects", options()).unwrap();
        assert!(!first.code.contains("new-side-effect"));
        assert!(second.code.contains("new-side-effect"));
    }

    #[test]
    fn added_modules_can_change_extension_resolution() {
        let root = tempfile::tempdir().unwrap();
        for directory in ["web/src", "web/public", "web/.generated/types"] {
            fs::create_dir_all(root.path().join(directory)).unwrap();
        }
        let entry = root.path().join("web/src/main.ts");
        fs::write(&entry, "export { value } from './value';").unwrap();
        fs::write(
            root.path().join("web/src/value.js"),
            "export const value = 'javascript';",
        )
        .unwrap();
        let options = || BundleOptions {
            entry: entry.clone(),
            ..Default::default()
        };
        let first = cached_bundle(root.path(), "resolution", options()).unwrap();
        fs::write(
            root.path().join("web/src/value.ts"),
            "export const value = 'typescript';",
        )
        .unwrap();
        let second = cached_bundle(root.path(), "resolution", options()).unwrap();
        assert!(first.code.contains("javascript"));
        assert!(second.code.contains("typescript"), "{}", second.code);
    }

    #[test]
    fn actual_reader_build_is_complete_cached_and_recoverable() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        if !root.join("web/.generated/types").is_dir() {
            return;
        }
        let out = tempfile::tempdir().unwrap();
        let first = build(&root, out.path()).unwrap();
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(first.directory.join("assets.json")).unwrap())
                .unwrap();
        assert_eq!(
            manifest["contract"],
            inventory_hash(&root.join("web/.generated/types")).unwrap()
        );
        for path in manifest["files"].as_object().unwrap().values() {
            assert!(first.directory.join(path.as_str().unwrap()).is_file());
        }
        assert!(first.directory.join("renderer.js").is_file());
        let second = build(&root, out.path()).unwrap();
        assert!(second.cache_hit);
        assert_eq!(first.hash, second.hash);
        fs::write(second.directory.join("renderer.js"), "corrupt").unwrap();
        let repaired = build(&root, out.path()).unwrap();
        assert!(!repaired.cache_hit);
        assert_eq!(repaired.hash, first.hash);
    }
}

#[cfg(test)]
mod generation_tests {
    use super::*;

    #[test]
    fn failed_compilation_keeps_the_complete_previous_generation() {
        let repository = tempfile::tempdir().unwrap();
        let root = repository.path();
        for directory in ["web/src/sw", "web/public", "web/.generated/types"] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        for file in ["main.ts", "bootstrap.ts", "sw/main.ts", "embedded.ts"] {
            fs::write(root.join("web/src").join(file), "globalThis.fixture = 1;").unwrap();
        }
        let out = tempfile::tempdir().unwrap();
        let first = build(root, out.path()).unwrap();
        let contract = |directory: &Path| -> String {
            let manifest: serde_json::Value =
                serde_json::from_slice(&fs::read(directory.join("assets.json")).unwrap()).unwrap();
            manifest["contract"].as_str().unwrap().into()
        };
        fs::write(
            root.join("web/.generated/types/Page.ts"),
            "export type Page = 'new';",
        )
        .unwrap();
        let changed = build(root, out.path()).unwrap();
        assert_ne!(contract(&first.directory), contract(&changed.directory));
        assert!(first.directory.join("assets.json").is_file());
        let pointer = fs::read(out.path().join("current.json")).unwrap();
        fs::write(root.join("web/src/main.ts"), "export const broken = ;").unwrap();
        assert!(build(root, out.path()).is_err());
        assert_eq!(fs::read(out.path().join("current.json")).unwrap(), pointer);
        assert!(first.directory.join("assets.json").is_file());
    }
}
