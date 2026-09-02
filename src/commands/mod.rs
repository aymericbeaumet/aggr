//! One file per subcommand. `Project` is what they all start from: the parsed config, the
//! resolved sources, and the repository holding `aggr.toml`.

pub mod build;
pub mod check;
pub mod digest;
pub mod fetch;
pub mod init;
pub mod serve;
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
        Command::Fetch(args) => {
            let project = Project::load(&cli.config)?;
            let worktree = project.worktree()?;
            fetch::run(&project, &worktree, &args).await.map(|_| ())
        }
        Command::Sync(args) => sync::run(&Project::load(&cli.config)?, &args).await,
        Command::Build(args) => build::run(&Project::load(&cli.config)?, &args).map(|_| ()),
        Command::Serve(args) => serve::run(&Project::load(&cli.config)?, &args).await,
        Command::Digest(args) => digest::run(&Project::load(&cli.config)?, &args).await,
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

    /// Scratch space next to the data worktree, never committed anywhere.
    pub fn cache_dir(&self) -> Result<PathBuf> {
        let dir = self.repo.root().join(".aggr").join("cache");
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        self.repo.exclude(&dir)?;
        Ok(dir)
    }

    /// Sources selected on the command line, or all of them.
    pub fn select<'a>(&'a self, slugs: &[String]) -> Result<Vec<&'a Source>> {
        if slugs.is_empty() {
            return Ok(self.sources.iter().collect());
        }
        slugs
            .iter()
            .map(|slug| {
                self.sources
                    .iter()
                    .find(|source| source.slug == *slug)
                    .with_context(|| format!("no source with slug {slug:?}"))
            })
            .collect()
    }

    /// The `main` commit the config was read from, for the `Aggr-Config` trailer and the footer.
    pub fn config_sha(&self) -> Option<String> {
        self.repo.head_sha().ok().flatten()
    }
}
