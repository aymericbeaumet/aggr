use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

const ABOUT: &str =
    "Git-native feed reader and readable web archive: aggr.toml in, static site out.";

const LONG_ABOUT: &str = "\
aggr reads `aggr.toml`, fetches items from feeds and other sources, stores every item as a
Markdown file (plus stripped source HTML when available) on an append-only git branch, and
renders a searchable static archive.

Typical loop:

    aggr init --github   # write aggr.toml and the GitHub workflow
    aggr build           # sync, then render the site into _site/
    aggr dev             # local resync + build + live-reloading server

Everything the site shows comes from the data branch, so a pinned data commit can be rendered
without refetching its sources. The storage engine uses ordinary Git; GitHub is the turnkey host,
not a requirement. A run that finds nothing new leaves no trace.";

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
    /// Fetch, commit to the data branch, and push. This is what CI runs.
    Sync(SyncArgs),
    /// Sync and render, or render a pinned data ref without syncing.
    Build(BuildArgs),
    /// Sync and build in a disposable workspace, then serve with live reload.
    Dev(DevArgs),
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
    /// Fetch and report, but do not write anything.
    #[arg(long)]
    pub dry_run: bool,
    /// Rewrite items that already exist (hand edits are lost).
    #[arg(long)]
    pub refresh: bool,
}

#[derive(Debug, Args, Default, Clone)]
pub struct SyncArgs {
    #[command(flatten)]
    pub fetch: FetchArgs,
    /// Write fetched data locally, but do not commit or push it.
    #[arg(long)]
    pub fetch_only: bool,
}

#[derive(Debug, Args, Default, Clone)]
pub struct BuildArgs {
    /// Output directory.
    #[arg(long, value_name = "DIR")]
    pub out: Option<PathBuf>,
    /// Absolute URL the site will be served from (e.g. https://user.github.io/repo/).
    #[arg(long, value_name = "URL", env = "AGGR_BASE_URL")]
    pub base_url: Option<String>,
    /// Render this data-branch ref without syncing, committing, or pushing.
    #[arg(long, value_name = "REF")]
    pub data_ref: Option<String>,
    /// Production build: absolute URLs from [site] url (or --base-url) and a CNAME for custom
    /// domains. Without it the site is built to be served from `/`.
    #[arg(long)]
    pub release: bool,
}

#[derive(Debug, Args, Default)]
pub struct DevArgs {
    /// Absolute URL used to exercise a release build locally (defaults to [site] url).
    #[arg(long, value_name = "URL", env = "AGGR_BASE_URL")]
    pub base_url: Option<String>,
    /// Build exactly as production while retaining dev's isolated cache and live reload.
    #[arg(long)]
    pub release: bool,
    #[command(flatten)]
    pub fetch: FetchArgs,
    /// Port to listen on.
    #[arg(long, short, default_value_t = 7319)]
    pub port: u16,
}

impl DevArgs {
    pub(crate) fn build_args(&self) -> BuildArgs {
        BuildArgs {
            out: None,
            base_url: self.base_url.clone(),
            data_ref: None,
            release: self.release,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_has_local_defaults() {
        let dev = Cli::try_parse_from(["aggr", "dev", "--release"]).unwrap();
        let Command::Dev(dev) = dev.command else {
            panic!("expected dev");
        };
        assert_eq!(dev.port, 7319);
        assert!(dev.release);
        assert!(Cli::try_parse_from(["aggr", "dev", "--out", "_site"]).is_err());
        assert!(Cli::try_parse_from(["aggr", "dev", "--data-ref", "HEAD"]).is_err());

        let after_subcommand =
            Cli::try_parse_from(["aggr", "dev", "--config", "/tmp/reader/aggr.toml"]).unwrap();
        assert_eq!(
            after_subcommand.config,
            PathBuf::from("/tmp/reader/aggr.toml")
        );
    }

    #[test]
    fn fetch_is_a_sync_mode_instead_of_a_command() {
        let cli = Cli::try_parse_from(["aggr", "sync", "--fetch-only", "--refresh"]).unwrap();
        let Command::Sync(sync) = cli.command else {
            panic!("expected sync");
        };
        assert!(sync.fetch_only);
        assert!(sync.fetch.refresh);
        assert!(Cli::try_parse_from(["aggr", "fetch"]).is_err());
    }
}
