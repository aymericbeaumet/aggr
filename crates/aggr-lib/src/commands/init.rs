//! `aggr init`: the two files a repository needs.

use std::path::{Component, Path, PathBuf};

use anyhow::{Context as _, Result, bail};

use crate::cli::InitArgs;
use crate::config;
use crate::git::Repo;

pub const MINIMAL_CONFIG: &str = include_str!(concat!(env!("OUT_DIR"), "/starter.toml"));

pub const WORKFLOW_PATH: &str = ".github/workflows/aggr.yml";

pub const WORKFLOW: &str = "\
name: aggr
on:
  schedule: [{ cron: \"7,37 * * * *\" }]
  push: { paths: [\"*.toml\", \"**/*.toml\"] }
  workflow_dispatch:
permissions: { contents: write, pages: write, id-token: write }
jobs:
  aggr:
    if: github.event_name != 'push' || github.ref_name == github.event.repository.default_branch
    uses: aymericbeaumet/aggr/.github/workflows/aggr.yml@v2
";

pub fn run(config_path: &Path, args: &InitArgs) -> Result<()> {
    let config_text = if args.defaults {
        config::DEFAULTS
    } else {
        MINIMAL_CONFIG
    };
    let workflow = args.github.then(|| workflow(config_path)).transpose()?;
    preflight(config_path, args.force)?;
    if let Some((path, _)) = &workflow {
        preflight(path, args.force)?;
    }
    write(config_path, config_text)?;
    if let Some((path, content)) = workflow {
        write(&path, &content)?;
        println!(
            "next: edit your sources, enable GitHub Actions and Pages (Source: GitHub Actions), then commit both files and push"
        );
    } else {
        println!(
            "next: edit your sources, then run `aggr dev` to preview; `aggr build` saves the archive and writes _site/ (run `git init` first if needed)"
        );
    }
    Ok(())
}

fn workflow(config_path: &Path) -> Result<(PathBuf, String)> {
    let absolute = absolute_target(config_path)?;
    let parent = absolute.parent().context("config path has no parent")?;
    let existing = parent
        .ancestors()
        .find(|path| path.is_dir())
        .context("config path has no existing parent directory")?;
    let (root, relative) = match Repo::discover(existing) {
        Ok(repo) => {
            let relative = repo
                .relative_path(&absolute)
                .context("GitHub workflow configuration must be inside the repository")?;
            (repo.root().to_path_buf(), relative)
        }
        Err(_) => {
            let cwd = std::env::current_dir()?.canonicalize()?;
            let root = if absolute.starts_with(&cwd) {
                cwd
            } else {
                parent.to_path_buf()
            };
            let relative = absolute.strip_prefix(&root)?.to_path_buf();
            (root, relative)
        }
    };
    let path = root.join(WORKFLOW_PATH);
    if absolute == absolute_target(&path)? {
        bail!("configuration and GitHub workflow must use different paths");
    }
    let content = if relative == Path::new(config::DEFAULT_FILE) {
        WORKFLOW.to_string()
    } else {
        let name = relative
            .to_str()
            .context("GitHub workflow configuration path must be UTF-8")?
            .replace('\\', "/");
        format!(
            "{WORKFLOW}    with: {{ config: {} }}\n",
            serde_json::to_string(&name)?
        )
    };
    Ok((path, content))
}

fn absolute_target(path: &Path) -> Result<PathBuf> {
    let mut absolute = PathBuf::new();
    for component in std::path::absolute(path)?.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                absolute.pop();
            }
            other => absolute.push(other),
        }
    }
    let existing = absolute
        .ancestors()
        .find(|path| path.exists())
        .context("path has no existing ancestor")?;
    Ok(existing
        .canonicalize()?
        .join(absolute.strip_prefix(existing)?))
}

fn preflight(path: &Path, force: bool) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !force {
                bail!("{} exists; pass --force to overwrite", path.display());
            }
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                bail!(
                    "refusing to overwrite {}: expected a regular file",
                    path.display()
                );
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).with_context(|| format!("checking {}", path.display())),
    }
    for parent in path
        .ancestors()
        .skip(1)
        .filter(|path| !path.as_os_str().is_empty())
    {
        match std::fs::metadata(parent) {
            Ok(metadata) if !metadata.is_dir() => bail!("{} is not a directory", parent.display()),
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("checking {}", parent.display()));
            }
        }
    }
    Ok(())
}

fn write(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, content).with_context(|| format!("writing {}", path.display()))?;
    println!("wrote {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_snippets_are_valid() {
        crate::config::Config::parse(MINIMAL_CONFIG).unwrap();
        let workflow: serde_json::Value = serde_yaml_ng::from_str(WORKFLOW).unwrap();
        let push = &workflow["on"]["push"];
        assert!(
            push.get("branches").is_none(),
            "the default branch is resolved from the event instead of being hardcoded"
        );
        assert_eq!(push["paths"], serde_json::json!(["*.toml", "**/*.toml"]));
        assert_eq!(
            workflow["jobs"]["aggr"]["if"],
            "github.event_name != 'push' || github.ref_name == github.event.repository.default_branch"
        );
        assert_eq!(workflow["on"]["schedule"][0]["cron"], "7,37 * * * *");
        assert!(workflow["on"].get("workflow_dispatch").is_some());
        assert_eq!(
            workflow["jobs"]["aggr"]["uses"],
            "aymericbeaumet/aggr/.github/workflows/aggr.yml@v2"
        );
        assert!(
            WORKFLOW.lines().count() <= 10,
            "the workflow must stay tiny"
        );
    }

    #[test]
    fn starter_uses_the_per_feed_retention_and_capture_defaults() {
        let config = crate::config::Config::parse(MINIMAL_CONFIG).unwrap();
        assert_eq!(config.sources.len(), 2);
        assert_eq!(config.defaults.limits.max_items, 250);
        assert_eq!(config.defaults.limits.max_age_days, 730);
        assert_eq!(config.defaults.limits.max_bytes, 1_000_000_000);
        assert_eq!(config.defaults.media, crate::config::MediaPolicy::Remote);
        assert_eq!(config.defaults.content, crate::config::ContentMode::Light);
        assert_eq!(config.site.build_max_bytes, 1_000_000_000);
    }

    #[test]
    fn writes_config_and_workflow_once() {
        let tmp = tempfile::tempdir().unwrap();
        let config = tmp.path().join("aggr.toml");
        run(
            &config,
            &InitArgs {
                github: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(std::fs::read_to_string(&config).unwrap(), MINIMAL_CONFIG);
        assert!(tmp.path().join(WORKFLOW_PATH).exists());
        assert!(run(&config, &InitArgs::default()).is_err());
        run(
            &config,
            &InitArgs {
                defaults: true,
                force: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(std::fs::read_to_string(&config).unwrap(), config::DEFAULTS);
    }
}
