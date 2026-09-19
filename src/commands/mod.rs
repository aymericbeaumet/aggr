//! One file per subcommand. `Project` is what they all start from: the parsed config, the
//! resolved sources, and the repository holding `aggr.toml`.

pub mod build;
pub mod check;
pub mod clean;
pub mod dev;
pub mod fetch;
pub mod init;
pub mod lock;
pub mod server;
pub mod sync;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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
        Command::Sync(args) => {
            if args.clean {
                clean::run(&cli.config, &crate::cli::CleanArgs::default())?;
            }
            let project = Project::load(&cli.config).await?;
            let _guard = project.lock("sync")?;
            sync::run(&project, &args).await.map(|_| ())
        }
        Command::Build(args) => {
            if args.clean {
                clean::run(
                    &cli.config,
                    &crate::cli::CleanArgs {
                        out: args.out.clone(),
                        dry_run: false,
                    },
                )?;
            }
            let project = if args.data_ref.is_some() {
                Project::load_offline(&cli.config).await?
            } else {
                Project::load(&cli.config).await?
            };
            let _guard = project.lock("build")?;
            build::sync_and_run(&project, &args).await
        }
        Command::Dev(args) => dev::run(&cli.config, &args).await,
        Command::Clean(args) => clean::run(&cli.config, &args),
        Command::Check => check::run(&Project::load(&cli.config).await?).await,
    }
}

pub struct Project {
    pub config: Config,
    pub sources: Vec<Source>,
    pub config_path: PathBuf,
    /// Directory holding `aggr.toml`; themes and `templates/` are resolved against it.
    pub root: PathBuf,
    pub repo: Repo,
    /// The data branch checkout, established once per process by [`Project::worktree`].
    worktree: OnceLock<Worktree>,
}

impl Project {
    pub async fn load(config_path: &Path) -> Result<Self> {
        let config = Config::load(config_path).await?;
        Self::from_config(config_path, config)
    }

    async fn load_offline(config_path: &Path) -> Result<Self> {
        let config = Config::load_offline(config_path).await?;
        Self::from_config(config_path, config)
    }

    fn from_config(config_path: &Path, config: Config) -> Result<Self> {
        let sources = config.sources()?;
        let config_path = config_path
            .canonicalize()
            .with_context(|| format!("resolving {}", config_path.display()))?;
        let root = config_path
            .parent()
            .context("config file has a parent directory")?
            .to_path_buf();
        let repo = Repo::discover(&root)?;
        let project = Self {
            config,
            sources,
            config_path,
            root,
            repo,
            worktree: OnceLock::new(),
        };
        project.validate_layout()?;
        Ok(project)
    }

    /// Establish that all configured persistent paths are disjoint before a command writes.
    pub(super) fn validate_layout(&self) -> Result<()> {
        clean::validate_project_layout(self)
    }

    /// Serialise the commands that mutate this repository's `.aggr/` state for as long as the
    /// returned guard lives. Two runs racing to create, prune or rebase the shared data checkout
    /// would corrupt it; the second one fails at once instead. Called only after the layout was
    /// validated, so a rejected configuration still leaves no `.aggr/` behind.
    pub(super) fn lock(&self, subject: &str) -> Result<lock::Guard> {
        let dir = self.repo.root().join(".aggr");
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        // The namespace as a whole is aggr's, not only the archive and cache directories below
        // it that register themselves: the lock file must never show up in `git status` either.
        self.repo.exclude(&dir)?;
        lock::Guard::acquire(&self.lock_path(), subject, self.repo.root())
    }

    /// [`Project::lock`] for a command that only reports: it still refuses to run beside a
    /// mutating command, but creates neither the lock file nor `.aggr/` when there is none.
    pub(super) fn inspect_lock(&self, subject: &str) -> Result<Option<lock::Guard>> {
        lock::Guard::inspect(&self.lock_path(), subject, self.repo.root())
    }

    fn lock_path(&self) -> PathBuf {
        self.repo.root().join(".aggr").join("aggr.lock")
    }

    /// The data branch checked out at `[store] dir`, created on first use. The fetch, prune, and
    /// rebase that establish it run once per process: build's own sync already brought the branch
    /// up to date, and the commands hold the only reference to the checkout in between.
    pub fn worktree(&self) -> Result<&Worktree> {
        if let Some(worktree) = self.worktree.get() {
            return Ok(worktree);
        }
        // Fetch/build can write through nested cache paths after opening the archive. Reject a
        // redirected cache tree before bootstrap makes any persistent project change.
        clean::validate_owned_cache_dir(
            &crate::cache::build(self.repo.root()),
            "repository build cache",
        )?;
        let worktree = self
            .repo
            .ensure_worktree(&self.config.store.branch, &self.config.store.dir)?;
        Store::open(worktree.dir()).bootstrap()?;
        Ok(self.worktree.get_or_init(|| worktree))
    }

    /// Repository-local cache for build/sync. It is never shared with `aggr dev`.
    pub fn build_cache_dir(&self) -> Result<PathBuf> {
        let dir = crate::cache::build(self.repo.root());
        clean::validate_owned_cache_dir(&dir, "repository build cache")?;
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        self.repo.exclude(&dir)?;
        Ok(dir)
    }

    /// OS-standard, project-isolated cache for dev. It never touches this repository's worktree.
    pub fn dev_cache_dir(&self) -> Result<PathBuf> {
        let requested = crate::cache::dev(&self.config_path)?;
        let dir = clean::validate_dev_cache(self, &requested)?;
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
