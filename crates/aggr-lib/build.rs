use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

fn run(command: &mut Command, operation: &str) {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("{operation}: {error}; run `make setup` first"));
    assert!(
        output.status.success(),
        "{operation}: {}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn files(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(root).expect("reading UI build output") {
        let path = entry.expect("reading UI output entry").path();
        if path.is_dir() {
            paths.extend(files(&path));
        } else {
            paths.push(path);
        }
    }
    paths.sort();
    paths
}

fn main() {
    let manifest =
        PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let root = if manifest.join("web").is_dir() {
        manifest.clone()
    } else {
        manifest.join("../..")
    };
    let web = root.join("web");
    let output = PathBuf::from(std::env::var_os("OUT_DIR").expect("build output directory"));
    let ui = output.join("ui");
    let workspace = web.join("build.ts").is_file();
    let packaged = manifest.join(".generated/publish");
    let resources = [
        ("config.default.toml", "config.default.toml"),
        ("examples/starter.toml", "starter.toml"),
        ("examples/aggr.toml", "example.toml"),
        (
            "docs/aggr-instance.schema.json",
            "aggr-instance.schema.json",
        ),
        ("Cargo.lock", "Cargo.lock"),
    ];
    for (source, name) in resources {
        let source = if workspace {
            root.join(source)
        } else {
            packaged.join(name)
        };
        println!("cargo:rerun-if-changed={}", source.display());
        fs::copy(&source, output.join(name)).unwrap_or_else(|error| {
            panic!(
                "embedding {}: {error}; run `make package` before packaging",
                source.display()
            )
        });
    }
    if workspace {
        println!("cargo:rustc-env=AGGR_SOURCE_ROOT={}", root.display());
        for input in [
            "web/src",
            "web/public",
            "web/build.ts",
            "web/build",
            "web/vite.config.ts",
            "web/tsconfig.json",
            "web/svelte.config.js",
            "web/package.json",
            "web/package-lock.json",
            "crates/aggr-ui-model/src",
            "mise.toml",
        ] {
            println!("cargo:rerun-if-changed={}", root.join(input).display());
        }
        println!("cargo:rerun-if-env-changed=NODE_ENV");

        let generated = web.join(".generated");
        fs::create_dir_all(&generated).expect("creating generated UI directory");
        let guard = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(generated.join("build.lock"))
            .expect("opening UI build lock");
        guard.lock().expect("locking UI generation");

        run(
            Command::new("node")
                .arg("build/dependencies.mjs")
                .current_dir(&web),
            "preparing locked frontend dependencies",
        );
        aggr_ui_model::export_types(generated.join("types")).expect("exporting page model types");
        run(
            Command::new("node")
                .arg("build.ts")
                .current_dir(&web)
                .env("AGGR_UI_OUT_DIR", &ui),
            "building the Svelte reader",
        );
    } else {
        let source = packaged.join("ui");
        println!("cargo:rerun-if-changed={}", source.display());
        assert!(
            source.join("assets.json").is_file(),
            "packaged reader assets missing; run `make package` before publishing"
        );
        if ui.exists() {
            fs::remove_dir_all(&ui).expect("clearing previous packaged reader assets");
        }
        for path in files(&source) {
            let destination = ui.join(path.strip_prefix(&source).expect("packaged UI path"));
            fs::create_dir_all(destination.parent().expect("asset parent"))
                .expect("creating packaged UI directory");
            fs::copy(path, destination).expect("copying packaged reader assets");
        }
    }

    let mut fingerprint = Sha256::new();
    let mut embedded = String::from("pub static UI_FILES: &[(&str, &[u8])] = &[\n");
    for path in files(&ui) {
        let name = path
            .strip_prefix(&ui)
            .expect("UI output path")
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = fs::read(&path).expect("reading generated UI asset");
        fingerprint.update((name.len() as u64).to_le_bytes());
        fingerprint.update(name.as_bytes());
        fingerprint.update((bytes.len() as u64).to_le_bytes());
        fingerprint.update(&bytes);
        embedded.push_str(&format!("({name:?}, include_bytes!({:?})),\n", path));
    }
    embedded.push_str("];");
    fs::write(output.join("ui.rs"), embedded).expect("writing embedded UI table");
    println!(
        "cargo:rustc-env=AGGR_UI_HASH={}",
        fingerprint
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
}
