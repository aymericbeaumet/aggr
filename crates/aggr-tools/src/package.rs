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

pub async fn run(args: Vec<String>) -> Result<()> {
    let root = crate::root();
    let destination = root.join("crates/aggr-lib/.generated/publish");
    let parent = destination.parent().context("Locating packaged inputs")?;
    std::fs::create_dir_all(parent)?;
    let stage = tempfile::Builder::new()
        .prefix(".publish-")
        .tempdir_in(parent)?;
    aggr_ui_model::export_types(root.join("web/.generated/types"))?;
    let stage_path = stage.path().to_path_buf();
    let build_root = root.clone();
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
        Command::new("cargo")
            .current_dir(root)
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
