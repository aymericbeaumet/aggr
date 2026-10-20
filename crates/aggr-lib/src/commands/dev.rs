//! `aggr dev`: immediately serve, then sync/build and live-reload without touching the repository.

use std::path::Path;

use anyhow::{Context as _, Result, ensure};

use super::lock::DevLock;
use super::{Project, server};
use crate::cli::DevArgs;
use crate::store::Store;

pub(super) mod snapshot;

const INTERNAL_WORKER: &str = "AGGR_DEV_INTERNAL_WORKER";

pub(super) fn supervised() -> bool {
    std::env::var_os("AGGR_DEV_SUPERVISED").as_deref() == Some(std::ffi::OsStr::new("1"))
}

pub(super) fn restarted() -> bool {
    supervised()
        && std::env::var_os("AGGR_DEV_RESTART").as_deref() == Some(std::ffi::OsStr::new("1"))
}

pub(super) fn worker_ready(origin: &str, base: &str) -> Result<()> {
    if supervised() {
        use std::io::Write as _;
        writeln!(
            std::io::stdout().lock(),
            "\u{1e}aggr-ready {}",
            serde_json::json!({ "origin": origin, "base": base }),
        )
        .context("reporting development worker readiness")?;
    }
    Ok(())
}

pub(super) async fn supervisor_shutdown() -> std::io::Result<()> {
    if !supervised() {
        return std::future::pending().await;
    }
    let (sender, receiver) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("aggr-dev-control".into())
        .spawn(move || {
            let mut command = String::new();
            let result = std::io::stdin().read_line(&mut command).map(|_| ());
            let _ = sender.send(result);
        })?;
    receiver
        .await
        .map_err(|error| std::io::Error::other(error.to_string()))?
}

pub async fn run(config_path: &Path, args: &DevArgs, verbose: u8) -> Result<()> {
    let config_path = if args.demo {
        demo_config()?
    } else {
        config_path.to_path_buf()
    };
    if args.native
        || source_checkout().is_none()
        || std::env::var_os(INTERNAL_WORKER).as_deref() == Some(std::ffi::OsStr::new("1"))
    {
        return run_worker(&config_path, args).await;
    }
    let (project, _) = prepare(&config_path, args.clean).await?;
    let cache =
        super::clean::validate_dev_cache(&project, &crate::cache::dev(&project.config_path)?)?;
    drop(DevLock::inspect(&cache, &project.config_path)?);
    launch(&project.config_path, args, verbose)
}

pub(crate) fn source_checkout() -> Option<std::path::PathBuf> {
    option_env!("AGGR_SOURCE_ROOT")
        .map(std::path::PathBuf::from)
        .filter(|root| root.join("web/dev/start.mjs").is_file())
}

fn demo_config() -> Result<std::path::PathBuf> {
    let base = match std::env::var_os("AGGR_CACHE_DIR") {
        Some(path) => std::path::PathBuf::from(path),
        None => directories::ProjectDirs::from("", "", "aggr")
            .context("finding the demo cache directory")?
            .cache_dir()
            .to_path_buf(),
    };
    let root = base.join("try-v1");
    super::clean::validate_owned_cache_dir(&root, "demo project")?;
    std::fs::create_dir_all(&root).context("creating isolated demo project")?;
    let _initializing = super::lock::Guard::acquire(&root.join("init.lock"), "demo init", &root)?;
    if !root.join(".git").is_dir() {
        let output = std::process::Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .arg(&root)
            .output()
            .context("initializing isolated demo repository")?;
        ensure!(
            output.status.success(),
            "initializing demo repository: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let config = root.join("aggr.toml");
    if !config.exists() {
        std::fs::write(&config, super::init::MINIMAL_CONFIG)
            .context("writing demo configuration")?;
    }
    println!(
        "trying the reader in {}; your current directory is unchanged",
        root.display()
    );
    Ok(config)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Context {
    repository: std::path::PathBuf,
    cwd: std::path::PathBuf,
    executable: std::path::PathBuf,
    worker_args: Vec<String>,
    vite_origin: String,
    strict_port: bool,
}

fn launch(config_path: &Path, args: &DevArgs, verbose: u8) -> Result<()> {
    let repository = source_checkout()
        .context("contributor source checkout is unavailable; use `aggr dev --native`")?;
    let script = repository.join("web/dev/start.mjs");
    let dependencies = std::process::Command::new("mise")
        .args(["exec", "--", "node"])
        .arg(repository.join("web/build/dependencies.mjs"))
        .current_dir(&repository)
        .status()
        .context("preparing frontend dependencies; run `make setup` first")?;
    ensure!(
        dependencies.success(),
        "frontend dependency repair failed; run `make setup`"
    );
    ensure!(
        script.is_file()
            && repository
                .join("web/node_modules/vite/package.json")
                .is_file(),
        "contributor development needs the source checkout and Node packages; run `make setup` in {}",
        repository.display()
    );
    let config = config_path
        .canonicalize()
        .with_context(|| format!("resolving {}", config_path.display()))?;
    let context = Context {
        repository,
        cwd: std::env::current_dir().context("resolving development directory")?,
        executable: std::env::current_exe().context("resolving development executable")?,
        worker_args: worker_arguments(&config, args, verbose)?,
        vite_origin: if std::env::var_os("AGGR_VITE_URL").is_some() {
            server::vite_origin()?
        } else {
            format!("http://127.0.0.1:{}", args.port)
        },
        strict_port: std::env::var_os("AGGR_VITE_URL").is_some(),
    };
    let status = std::process::Command::new("mise")
        .args(["exec", "--", "node"])
        .arg(script)
        .current_dir(&context.repository)
        .env("AGGR_DEV_CONTEXT", serde_json::to_string(&context)?)
        .status()
        .context("starting Node development tools; run `make setup` first")?;
    ensure!(status.success(), "development tools exited with {status}");
    Ok(())
}

fn worker_arguments(config: &Path, args: &DevArgs, verbose: u8) -> Result<Vec<String>> {
    let mut arguments = vec![
        "--config".into(),
        config
            .to_str()
            .context("development config path must be UTF-8")?
            .into(),
        "dev".into(),
        "--port".into(),
        args.port.to_string(),
    ];
    if verbose > 0 {
        arguments.push(format!("-{}", "v".repeat(usize::from(verbose))));
    }
    for (enabled, flag) in [
        (args.clean, "--clean"),
        (args.release, "--release"),
        (args.fetch.dry_run, "--dry-run"),
        (args.fetch.refresh, "--refresh"),
        (args.fetch.reprocess, "--reprocess"),
        (args.fetch.backfill_media, "--backfill-media"),
    ] {
        if enabled {
            arguments.push(flag.into());
        }
    }
    if let Some(base_url) = &args.base_url {
        arguments.extend(["--base-url".into(), base_url.clone()]);
    }
    Ok(arguments)
}

async fn prepare(config_path: &Path, clean: bool) -> Result<(Project, Option<super::clean::Plan>)> {
    let project = if clean {
        super::clean::load_project(config_path)?
    } else {
        Project::load_offline(config_path).await?
    };
    if clean {
        project.validate_layout()?;
    }
    let cleanup = clean
        .then(|| super::clean::Plan::new(&project, None))
        .transpose()?;
    Ok((project, cleanup))
}

async fn run_worker(config_path: &Path, args: &DevArgs) -> Result<()> {
    let (project, cleanup) = prepare(config_path, args.clean).await?;
    let persistent = project.dev_cache_dir()?;
    let ownership = std::sync::Arc::new(DevLock::acquire(&persistent, &project.config_path)?);
    if let Some(cleanup) = cleanup {
        cleanup.execute(false)?;
    }
    let project = if args.clean {
        Project::load_offline(config_path).await?
    } else {
        project
    };
    let data = persistent.join("data");
    let cache = persistent.join("fetch");
    let site = persistent.join("site");
    std::fs::create_dir_all(&data).context("creating cached dev store")?;
    std::fs::create_dir_all(&cache).context("creating cached dev fetch state")?;
    Store::open(&data).bootstrap()?;
    let scratch = dev_scratch(&persistent)?;
    let staging = scratch.join("site");
    server::run_with_reload(project, args, data, cache, site, staging, ownership).await
}

fn dev_scratch(cache: &Path) -> Result<std::path::PathBuf> {
    let root = cache.join("tmp");
    super::clean::validate_owned_cache_dir(&root, "dev scratch directory")?;
    if root.exists() {
        std::fs::remove_dir_all(&root).context("clearing stale dev scratch directory")?;
    }
    std::fs::create_dir_all(&root).context("creating dev scratch directory")?;
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn clean_preparation_does_not_require_source_credentials() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q", "-b", "main"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
        let config = root.join("aggr.toml");
        std::fs::write(
            &config,
            "[[sources]]\nurl = 'https://example.com/${AGGR_TEST_UNSET_DEV_CLEAN_TOKEN}/feed.xml'\n",
        )
        .unwrap();
        assert!(prepare(&config, false).await.is_err());
        let (_, cleanup) = prepare(&config, true).await.unwrap();
        assert!(cleanup.is_some());
        assert!(!root.join(".aggr").exists());
    }

    #[test]
    fn launcher_forwards_typed_flags_and_shell_characters_as_arguments() {
        use clap::Parser as _;
        let config = Path::new("reader with spaces; $(touch nope).toml");
        let args = DevArgs {
            native: false,
            demo: false,
            clean: true,
            release: true,
            port: 8123,
            base_url: Some("https://example.com/reads/?literal=$(touch nope)&x=1".into()),
            fetch: crate::cli::FetchArgs {
                refresh: true,
                ..Default::default()
            },
        };
        let arguments = worker_arguments(config, &args, 2).unwrap();
        let mut command = vec!["aggr".to_string()];
        command.extend(arguments);
        let parsed = crate::cli::Cli::try_parse_from(command).unwrap();
        assert_eq!(parsed.config, config);
        assert_eq!(parsed.verbose, 2);
        let crate::cli::Command::Dev(parsed) = parsed.command else {
            panic!("expected dev")
        };
        assert_eq!(parsed.port, args.port);
        assert_eq!(parsed.base_url, args.base_url);
        assert!(parsed.clean && parsed.release && parsed.fetch.refresh);
    }

    #[test]
    fn scratch_workspace_shares_the_dev_cache_filesystem() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().canonicalize().unwrap().join("dev-v1/project");
        std::fs::create_dir_all(cache.join("tmp")).unwrap();
        std::fs::write(cache.join("tmp/stale"), "old").unwrap();
        let scratch = dev_scratch(&cache).unwrap();
        assert_eq!(scratch, cache.join("tmp"));
        assert!(!scratch.join("stale").exists());
    }
}
