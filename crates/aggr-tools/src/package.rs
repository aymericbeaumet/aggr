use std::io::Seek as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, ensure};

use crate::distribution::without_credentials;

pub fn setup() -> Result<()> {
    let config: toml::Value =
        toml::from_str(&std::fs::read_to_string(crate::root().join("mise.toml"))?)?;
    let tools = config["tools"]
        .as_table()
        .context("mise.toml must declare project tools")?;
    let os = std::env::consts::OS;
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        value => value,
    };
    let platform = format!("{os}/{arch}");
    let selected = tools
        .iter()
        .filter(|(_, tool)| {
            tool.get("os")
                .and_then(toml::Value::as_array)
                .is_none_or(|platforms| {
                    platforms.iter().any(|value| {
                        value.as_str() == Some(os) || value.as_str() == Some(platform.as_str())
                    })
                })
        })
        .map(|(name, _)| name.as_str());
    crate::command(without_credentials(
        Command::new("mise")
            .current_dir(crate::root())
            .arg("install")
            .arg("--locked")
            .args(selected),
    ))?;
    crate::command(without_credentials(
        Command::new("cargo")
            .current_dir(crate::root())
            .args(["fetch", "--locked"]),
    ))
}

/// Cargo checks explicitly included files even when Git ignores them. In a clean release,
/// build the generated inputs alongside an immutable export, outside the Git working tree.
pub(crate) struct PackageSource {
    checkout: PathBuf,
    export: Option<tempfile::TempDir>,
    target: PathBuf,
}

impl PackageSource {
    pub(crate) fn new(
        checkout: &Path,
        args: &[String],
        metadata: &serde_json::Value,
    ) -> Result<Self> {
        let target = metadata["target_directory"]
            .as_str()
            .context("Cargo metadata has no target directory")?
            .into();
        let export = if args.iter().any(|arg| arg == "--allow-dirty") {
            None
        } else {
            Some(clean_export(checkout)?)
        };
        Ok(Self {
            checkout: checkout.into(),
            export,
            target,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        self.export
            .as_ref()
            .map_or(self.checkout.as_path(), |export| export.path())
    }

    pub(crate) fn cargo(&self) -> Command {
        let mut command = Command::new("cargo");
        command
            .current_dir(self.path())
            .env("CARGO_TARGET_DIR", &self.target);
        command
    }
}

pub(crate) fn workspace_metadata(checkout: &Path) -> Result<serde_json::Value> {
    let metadata = without_credentials(Command::new("cargo").current_dir(checkout).args([
        "metadata",
        "--no-deps",
        "--format-version",
        "1",
        "--locked",
    ]))
    .output()
    .context("Locating Cargo target directory")?;
    ensure!(
        metadata.status.success(),
        "Cargo metadata failed: {}",
        String::from_utf8_lossy(&metadata.stderr)
    );
    serde_json::from_slice(&metadata.stdout).context("Reading Cargo metadata JSON")
}

fn clean_export(checkout: &Path) -> Result<tempfile::TempDir> {
    let git = || {
        let mut command = Command::new("git");
        command.current_dir(checkout);
        command
    };
    let status = without_credentials(git().args([
        "status",
        "--porcelain=v1",
        "--untracked-files=all",
        "--ignore-submodules=none",
    ]))
    .output()
    .context("Checking release source")?;
    ensure!(
        status.status.success(),
        "Checking release source: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    ensure!(
        status.stdout.is_empty(),
        "Release source has uncommitted changes; commit them or explicitly pass --allow-dirty:\n{}",
        String::from_utf8_lossy(&status.stdout)
    );

    let export = tempfile::Builder::new().prefix("aggr-package-").tempdir()?;
    let mut archive = tempfile::tempfile()?;
    crate::command(without_credentials(
        git()
            .args(["archive", "--format=tar", "HEAD"])
            .stdout(archive.try_clone()?),
    ))?;
    archive.rewind()?;
    tar::Archive::new(archive)
        .unpack(export.path())
        .context("Exporting committed release source")?;
    Ok(export)
}

pub async fn run(args: Vec<String>) -> Result<()> {
    let checkout = crate::root();
    let source = PackageSource::new(&checkout, &args, &workspace_metadata(&checkout)?)?;
    verify(&source, &args).await
}

pub(crate) async fn verify(source: &PackageSource, args: &[String]) -> Result<()> {
    let root = source.path();
    let destination = root.join("crates/aggr-lib/.generated/publish");
    let parent = destination.parent().context("Locating packaged inputs")?;
    std::fs::create_dir_all(parent)?;
    let stage = tempfile::Builder::new()
        .prefix(".publish-")
        .tempdir_in(parent)?;
    aggr_ui_model::export_types(root.join("web/.generated/types"))?;
    let stage_path = stage.path().to_path_buf();
    let build_root = root.to_owned();
    let built = tokio::task::spawn_blocking(move || {
        aggr_ui_build::build(&build_root, &stage_path.join("bundles"))
    })
    .await??;
    std::fs::rename(&built.directory, stage.path().join("ui"))?;
    std::fs::remove_dir_all(stage.path().join("bundles"))?;
    for (source, name) in [
        ("config.default.toml", "config.default.toml"),
        ("examples/starter.toml", "starter.toml"),
        ("examples/aggr.toml", "example.toml"),
        (
            "docs/aggr-instance.schema.json",
            "aggr-instance.schema.json",
        ),
        ("Cargo.lock", "Cargo.lock"),
    ] {
        std::fs::copy(root.join(source), stage.path().join(name))
            .with_context(|| format!("Packaging {source}"))?;
    }
    promote(stage.path(), &destination)?;
    println!(
        "Prepared self-contained library inputs in {}",
        destination.display()
    );
    crate::command(without_credentials(
        source
            .cargo()
            .args([
                "package",
                "--locked",
                "-p",
                "aggr-ui-model",
                "-p",
                "aggr-ui-build",
                "-p",
                "aggr-lib",
                "-p",
                "aggr",
            ])
            .args(args),
    ))
}

fn promote(stage: &Path, destination: &Path) -> Result<()> {
    let previous = destination.with_extension("previous");
    if previous.exists() {
        std::fs::remove_dir_all(&previous).context("Removing previous packaged inputs")?;
    }
    let replaced = destination.exists();
    if replaced {
        std::fs::rename(destination, &previous).context("Retaining previous packaged inputs")?;
    }
    if let Err(error) = std::fs::rename(stage, destination) {
        if replaced {
            std::fs::rename(&previous, destination)
                .context("Restoring previous packaged inputs")?;
        }
        return Err(error).context("Publishing packaged inputs");
    }
    if replaced {
        std::fs::remove_dir_all(previous)?;
    }
    Ok(())
}

pub fn msrv(args: Vec<String>) -> Result<()> {
    let manifest: toml::Value =
        toml::from_str(&std::fs::read_to_string(crate::root().join("Cargo.toml"))?)?;
    let version = manifest["package"]["rust-version"]
        .as_str()
        .context("Cargo.toml must declare the minimum Rust version")?;
    let tool = format!("rust@{version}");
    crate::command(without_credentials(
        Command::new("mise")
            .current_dir(crate::root())
            .args(["install", &tool]),
    ))?;
    crate::command(without_credentials(
        Command::new("mise")
            .current_dir(crate::root())
            .args(["exec", &tool, "--", "cargo", "build", "--locked"])
            .args(args),
    ))
}

pub fn verify_release(args: Vec<String>) -> Result<()> {
    ensure!(args.len() <= 1, "Usage: verify-release [BINARY]");
    let filename = if cfg!(windows) { "aggr.exe" } else { "aggr" };
    let binary = args.first().map_or_else(
        || crate::root().join("target/release").join(filename),
        PathBuf::from,
    );
    for args in [&["--help"][..], &["dev", "--help"][..]] {
        let output = without_credentials(Command::new(&binary).args(args))
            .output()
            .with_context(|| format!("Starting {}", binary.display()))?;
        ensure!(
            output.status.success(),
            "Release help failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let help = String::from_utf8(output.stdout)?;
        if args[0] == "dev" {
            for flag in ["--no-watch", "--demo"] {
                ensure!(help.contains(flag), "Release dev help is missing {flag}");
            }
        } else {
            ensure!(
                help.lines()
                    .any(|line| line.split_whitespace().next() == Some("dev")),
                "Release CLI has no dev command"
            );
        }
    }
    println!("Release exposes native development, isolated demo, and publication preview.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_packages_include_generated_assets_and_reject_uncommitted_sources() {
        let fixture = tempfile::tempdir().unwrap();
        let checkout = fixture.path().join("checkout");
        std::fs::create_dir_all(checkout.join("src")).unwrap();
        std::fs::write(checkout.join("src/lib.rs"), "pub const VERSION: u8 = 1;").unwrap();
        std::fs::write(
            checkout.join("Cargo.toml"),
            r#"[package]
name = "release-fixture"
version = "1.0.0"
edition = "2024"
include = ["src/**", ".generated/**"]
"#,
        )
        .unwrap();
        std::fs::write(checkout.join(".gitignore"), "/.generated\n").unwrap();
        for args in [
            vec!["init", "--template="],
            vec!["add", "."],
            vec![
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.org",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=disabled-hooks",
                "commit",
                "-m",
                "fixture",
            ],
        ] {
            crate::command(Command::new("git").current_dir(&checkout).args(args)).unwrap();
        }
        let target = fixture.path().join("target");
        let metadata = serde_json::json!({"target_directory": target});
        std::fs::create_dir(checkout.join(".generated")).unwrap();
        std::fs::write(checkout.join(".generated/asset"), "stale").unwrap();

        let source = PackageSource::new(&checkout, &[], &metadata).unwrap();
        assert!(!source.path().join(".generated/asset").exists());
        std::fs::create_dir(source.path().join(".generated")).unwrap();
        std::fs::write(source.path().join(".generated/asset"), "fresh").unwrap();
        crate::command(without_credentials(source.cargo().args([
            "package",
            "--offline",
            "--no-verify",
        ])))
        .unwrap();
        let file = std::fs::File::open(target.join("package/release-fixture-1.0.0.crate")).unwrap();
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
        let mut asset = archive
            .entries()
            .unwrap()
            .map(Result::unwrap)
            .find(|file| file.path().unwrap().ends_with(".generated/asset"))
            .unwrap();
        assert_eq!(std::io::read_to_string(&mut asset).unwrap(), "fresh");

        for path in ["src/lib.rs", "notes.txt"] {
            std::fs::write(checkout.join(path), "local edit").unwrap();
            assert!(
                clean_export(&checkout)
                    .unwrap_err()
                    .to_string()
                    .contains(path)
            );
            if path == "src/lib.rs" {
                crate::command(
                    Command::new("git")
                        .current_dir(&checkout)
                        .args(["add", path]),
                )
                .unwrap();
                assert!(
                    clean_export(&checkout)
                        .unwrap_err()
                        .to_string()
                        .contains(path)
                );
                std::fs::write(checkout.join(path), "pub const VERSION: u8 = 1;").unwrap();
                crate::command(
                    Command::new("git")
                        .current_dir(&checkout)
                        .args(["add", path]),
                )
                .unwrap();
            }
        }
        let dirty = PackageSource::new(&checkout, &["--allow-dirty".into()], &metadata).unwrap();
        assert_eq!(dirty.path(), checkout);
        assert_eq!(
            std::fs::read_to_string(dirty.path().join("notes.txt")).unwrap(),
            "local edit"
        );
    }

    #[test]
    fn package_promotion_keeps_a_complete_generation_and_recovers_after_failure() {
        let root = tempfile::tempdir().unwrap();
        let previous = root.path().join("publish");
        std::fs::create_dir(&previous).unwrap();
        std::fs::write(previous.join("receipt"), "old").unwrap();
        assert!(promote(&root.path().join("missing"), &previous).is_err());
        assert_eq!(
            std::fs::read_to_string(previous.join("receipt")).unwrap(),
            "old"
        );
        let next = root.path().join("stage");
        std::fs::create_dir(&next).unwrap();
        std::fs::write(next.join("receipt"), "new").unwrap();
        promote(&next, &previous).unwrap();
        assert_eq!(
            std::fs::read_to_string(previous.join("receipt")).unwrap(),
            "new"
        );
        assert!(!previous.with_extension("previous").exists());
    }
}
