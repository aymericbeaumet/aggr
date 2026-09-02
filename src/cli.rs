use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

const ABOUT: &str = "Git-native feed reader: aggr.toml in, static site out, history on a branch.";

const LONG_ABOUT: &str = "\
aggr reads `aggr.toml`, fetches items from feeds and other sources, stores every item as a
Markdown file (plus its raw HTML) on an append-only git branch, and renders a static site.

Typical loop:

    aggr init --github   # write aggr.toml and the GitHub workflow
    aggr build           # sync, then render the site into _site/
    aggr dev             # sync + build + live-reloading server

Everything the site shows comes from the data branch, so every item has a permanent URL on
GitHub and a run that finds nothing new leaves no trace.";

#[derive(Debug, Parser)]
#[command(name = "aggr", version, about = ABOUT, long_about = LONG_ABOUT)]
pub struct Cli {
    /// Path to the configuration file.
    #[arg(long, global = true, default_value = crate::config::DEFAULT_FILE, env = "AGGR_CONFIG")]
    pub config: PathBuf,

    /// Increase log verbosity (-v: info, -vv: debug, -vvv: trace).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Write a commented aggr.toml (and optionally the GitHub workflow) in the current directory.
    Init(InitArgs),
    /// Fetch sources and write new items into the data worktree without committing.
    Fetch(FetchArgs),
    /// Fetch, commit to the data branch, and push. This is what CI runs.
    Sync(FetchArgs),
    /// Sync, then render the static site into the output directory.
    Build(BuildArgs),
    /// Sync, build, then serve with fast rebuilds and live reload.
    Dev(DevArgs),
    /// Post the daily digest issue on GitHub when one is due.
    Digest(DigestArgs),
    /// Validate the configuration and probe every source.
    Check,
    /// Generate shell completions.
    Completions {
        /// Shell to generate completions for.
        shell: clap_complete::Shell,
    },
}

#[derive(Debug, Args, Default)]
pub struct InitArgs {
    /// Also write .github/workflows/aggr.yml.
    #[arg(long)]
    pub github: bool,
    /// Write every option with its default value instead of the minimal file.
    #[arg(long)]
    pub defaults: bool,
    /// Overwrite existing files.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args, Default, Clone)]
pub struct FetchArgs {
    /// Only fetch these sources (slugs). Repeatable.
    #[arg(long = "source", value_name = "SLUG")]
    pub sources: Vec<String>,
    /// Fetch and report, but do not write anything.
    #[arg(long)]
    pub dry_run: bool,
    /// Rewrite items that already exist (hand edits are lost).
    #[arg(long)]
    pub refresh: bool,
}

#[derive(Debug, Args, Default)]
pub struct BuildArgs {
    /// Output directory.
    #[arg(long, value_name = "DIR")]
    pub out: Option<PathBuf>,
    /// Absolute URL the site will be served from (e.g. https://user.github.io/repo/).
    #[arg(long, value_name = "URL", env = "AGGR_BASE_URL")]
    pub base_url: Option<String>,
    /// Render from this git ref of the data branch instead of the worktree.
    #[arg(long, value_name = "REF")]
    pub data_ref: Option<String>,
    /// Production build: absolute URLs from [site] url (or --base-url) and a CNAME for custom
    /// domains. Without it the site is built to be served from `/`.
    #[arg(long)]
    pub release: bool,
}

#[derive(Debug, Args, Default)]
pub struct DevArgs {
    #[command(flatten)]
    pub build: BuildArgs,
    #[command(flatten)]
    pub fetch: FetchArgs,
    /// Port to listen on.
    #[arg(long, short, default_value_t = 7319)]
    pub port: u16,
}

#[derive(Debug, Args, Default)]
pub struct DigestArgs {
    /// Absolute URL of the site, for the "read" links (defaults to [site] url).
    #[arg(long, value_name = "URL", env = "AGGR_BASE_URL")]
    pub base_url: Option<String>,
    /// Print the issue instead of posting it.
    #[arg(long)]
    pub dry_run: bool,
    /// Post even if today's digest exists or the scheduled time has not come.
    #[arg(long)]
    pub force: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_has_local_defaults() {
        let dev = Cli::try_parse_from(["aggr", "dev", "--source", "rust", "--release"]).unwrap();
        let Command::Dev(dev) = dev.command else {
            panic!("expected dev");
        };
        assert_eq!(dev.port, 7319);
        assert_eq!(dev.fetch.sources, ["rust"]);
        assert!(dev.build.release);
    }
}
