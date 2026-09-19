//! Shared integration-test helpers: one git and environment isolation policy for every target.
//!
//! Each target uses a subset of these helpers and clippy runs with `-D warnings`, so unused items
//! are allowed here rather than in every consumer.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, bail};
use assert_cmd::prelude::*;

/// Variables a test never inherits from the developer's shell or from CI: they would change the
/// binary's identity, config path, cache location or base URL behind the test's back.
pub const SCRUBBED_ENV: &[&str] = &[
    "GITHUB_ACTIONS",
    "GITHUB_REPOSITORY",
    "GITHUB_TOKEN",
    "GH_TOKEN",
    "AGGR_BASE_URL",
    "AGGR_CONFIG",
    "AGGR_CACHE_DIR",
    "AGGR_CLEAN_UNSET",
];

/// The git environment every test process runs with: no global or system configuration (hooks,
/// signing, line endings), a fixed identity, and no credential prompts.
pub fn git_env() -> &'static [(&'static str, &'static str)] {
    &[
        ("GIT_CONFIG_GLOBAL", "/dev/null"),
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_AUTHOR_NAME", "Test"),
        ("GIT_AUTHOR_EMAIL", "test@example.com"),
        ("GIT_COMMITTER_NAME", "Test"),
        ("GIT_COMMITTER_EMAIL", "test@example.com"),
        ("GIT_TERMINAL_PROMPT", "0"),
    ]
}

/// Applies [`git_env`] and removes [`SCRUBBED_ENV`] from `command`.
pub fn isolate(command: &mut Command) -> &mut Command {
    for (key, value) in git_env() {
        command.env(key, value);
    }
    for key in SCRUBBED_ENV {
        command.env_remove(key);
    }
    command
}

/// Runs `git args` in `dir` under [`git_env`], never signing commits or converting line endings,
/// and returns its stdout.
pub fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let mut command = Command::new("git");
    command
        .current_dir(dir)
        .args(["-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"])
        .args(args);
    for (key, value) in git_env() {
        command.env(key, value);
    }
    let output = command
        .output()
        .with_context(|| format!("git {args:?} in {}", dir.display()))?;
    if !output.status.success() {
        bail!("git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    }
    String::from_utf8(output.stdout).with_context(|| format!("git {args:?} stdout"))
}

/// The `aggr` binary under test, run from `dir` with the isolated environment. Tests that need a
/// variable from [`SCRUBBED_ENV`] set it explicitly afterwards.
pub fn aggr_command(dir: &Path) -> Command {
    let mut command = Command::cargo_bin("aggr").expect("the aggr binary is built for tests");
    command.current_dir(dir);
    isolate(&mut command);
    command
}

/// Creates a bare `origin.git` whose default branch is `main` and a `clone` checkout of it next
/// to it, both under `tmp`. Returns `(origin, clone)`.
pub fn bare_origin_with_clone(tmp: &Path) -> Result<(PathBuf, PathBuf)> {
    let origin = tmp.join("origin.git");
    let clone = tmp.join("clone");
    git(tmp, &["init", "-q", "--bare", "-b", "main", "origin.git"])?;
    let origin_arg = origin
        .to_str()
        .context("temporary origin path is not UTF-8")?;
    let clone_arg = clone
        .to_str()
        .context("temporary clone path is not UTF-8")?;
    git(tmp, &["clone", "-q", origin_arg, clone_arg])?;
    Ok((origin, clone))
}
