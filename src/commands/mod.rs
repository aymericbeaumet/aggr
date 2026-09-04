//! One file per subcommand. `Project` is what they all start from: the parsed config, the
//! resolved sources, and the repository holding `aggr.toml`.

pub mod build;
pub mod check;
pub mod dev;
pub mod fetch;
pub mod init;
pub mod server;
pub mod sync;

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use clap::CommandFactory as _;

use crate::cli::{Cli, Command};
use crate::config::{Config, Source};
use crate::git::{Repo, Worktree};
use crate::store::Store;

pub async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Init(args) => init::run(&cli.config, &args),
        Command::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "aggr", &mut std::io::stdout());
            Ok(())
        }
        Command::Sync(args) => sync::run(&Project::load(&cli.config)?, &args).await,
        Command::Build(args) => build::sync_and_run(&Project::load(&cli.config)?, &args).await,
        Command::Dev(args) => dev::run(&Project::load(&cli.config)?, &args).await,
        Command::Check => check::run(&Project::load(&cli.config)?).await,
    }
}

pub struct Project {
    pub config: Config,
    pub sources: Vec<Source>,
    pub config_path: PathBuf,
    /// Directory holding `aggr.toml`; themes and `templates/` are resolved against it.
    pub root: PathBuf,
    pub repo: Repo,
}

impl Project {
    pub fn load(config_path: &Path) -> Result<Self> {
        let config = Config::load(config_path)?;
        let sources = config.sources()?;
        let config_path = config_path
            .canonicalize()
            .with_context(|| format!("resolving {}", config_path.display()))?;
        let root = config_path
            .parent()
            .context("config file has a parent directory")?
            .to_path_buf();
        let repo = Repo::discover(&root)?;
        Ok(Self {
            config,
            sources,
            config_path,
            root,
            repo,
        })
    }

    /// The data branch checked out at `[store] dir`, created on first use.
    pub fn worktree(&self) -> Result<Worktree> {
        let worktree = self
            .repo
            .ensure_worktree(&self.config.store.branch, &self.config.store.dir)?;
        Store::open(worktree.dir()).bootstrap()?;
        Ok(worktree)
    }

    /// Repository-local cache for build/sync. It is never shared with `aggr dev`.
    pub fn build_cache_dir(&self) -> Result<PathBuf> {
        let dir = crate::cache::build(self.repo.root());
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        self.repo.exclude(&dir)?;
        Ok(dir)
    }

    /// OS-standard, project-isolated cache for dev. It never touches this repository's worktree.
    pub fn dev_cache_dir(&self) -> Result<PathBuf> {
        let dir = crate::cache::dev(&self.config_path)?;
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(dir)
    }

    /// The `main` commit the config was read from, for the `Aggr-Config` trailer and the footer.
    pub fn config_sha(&self) -> Option<String> {
        self.repo
            .head_matches_files(&self.config.loaded_files)
            .ok()
            .filter(|matches| *matches)
            .and_then(|_| self.repo.head_sha().ok().flatten())
    }

    pub fn config_repo_path(&self) -> Option<String> {
        self.repo
            .relative_path(&self.config_path)
            .map(|path| path.to_string_lossy().replace('\\', "/"))
    }
}
