//! `aggr init`: the two files a repository needs.

use std::path::Path;

use anyhow::{Context as _, Result, bail};

use crate::cli::InitArgs;
use crate::config;

pub const MINIMAL_CONFIG: &str = "\
# aggr.toml — only what differs from the defaults needs to be here.
# `aggr init --defaults` writes every option with its default value and a comment.

[site]
title = \"My reads\"
# url = \"https://reads.example.com\"   # custom domain: also writes the CNAME file

[digest]
at = \"08:00\"                          # daily GitHub issue, in [site] timezone

[[sources]]
url = \"https://blog.rust-lang.org/feed.xml\"
category = \"rust\"

[[sources]]
url = \"https://hnrss.org/frontpage?points=100\"
name = \"Hacker News\"
";

pub const WORKFLOW_PATH: &str = ".github/workflows/aggr.yml";

pub const WORKFLOW: &str = "\
name: aggr
on:
  schedule: [{ cron: \"*/30 * * * *\" }]
  push: { branches: [main], paths: [\"*.toml\", themes/**, templates/**, static/**] }
  workflow_dispatch:
permissions: { contents: write, pages: write, id-token: write, issues: write }
jobs:
  aggr:
    uses: aymericbeaumet/aggr/.github/workflows/aggr.yml@v1
";

pub fn run(config_path: &Path, args: &InitArgs) -> Result<()> {
    let config_text = if args.defaults {
        config::DEFAULTS
    } else {
        MINIMAL_CONFIG
    };
    write(config_path, config_text, args.force)?;
    if args.github {
        let workflow = config_path
            .parent()
            .filter(|dir| !dir.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .join(WORKFLOW_PATH);
        write(&workflow, WORKFLOW, args.force)?;
        println!("next: commit both files and push; the workflow builds the site on every run");
    } else {
        println!("next: `aggr init --github` for the workflow, or `aggr sync && aggr serve`");
    }
    Ok(())
}

fn write(path: &Path, content: &str, force: bool) -> Result<()> {
    if path.exists() && !force {
        bail!("{} exists; pass --force to overwrite", path.display());
    }
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
        assert!(
            WORKFLOW.lines().count() <= 10,
            "the workflow must stay tiny"
        );
        assert!(WORKFLOW.contains("issues: write"));
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
