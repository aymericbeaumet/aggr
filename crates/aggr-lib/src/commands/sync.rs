//! `aggr sync`: fetch, commit with trailers, push, move `refs/aggr/last-good`. What CI runs.

use std::path::Path;

use anyhow::{Result, bail};

use super::Project;
use super::fetch::{self, Report};
use crate::cli::FetchArgs;
use crate::cli::SyncArgs;
use crate::discussions::ResolutionSet;
use crate::git::{CommitMessage, PushOutcome};
use crate::store::Outcome;

pub const LAST_GOOD: &str = "refs/aggr/last-good";

/// Returns the discussion matches resolved for the synced data when that stage ran and
/// succeeded, so a build following the sync does not have to resolve them a second time.
/// `--clean` is handled by the dispatcher before the project is loaded.
pub async fn run(project: &Project, args: &SyncArgs) -> Result<Option<ResolutionSet>> {
    let result = run_sync(project, args).await;
    if !args.fetch.dry_run {
        trim_cache(project);
    }
    result
}

async fn run_sync(project: &Project, args: &SyncArgs) -> Result<Option<ResolutionSet>> {
    let worktree = project.worktree()?;
    if !args.fetch.dry_run {
        sweep_cache(project);
    }
    let first = worktree.head_sha()?.is_none();
    let report = fetch::run(project, worktree, &args.fetch).await?;

    let mut discussions = None;
    if !args.fetch.dry_run
        && !args.fetch.backfill_media
        && crate::discussions::any_provider(&project.config.defaults.discussions, &project.sources)
    {
        let cache = project.build_cache_dir()?;
        let store = crate::store::Store::open(worktree.dir());
        match super::build::resolve_discussions(project, &store, &cache, chrono::Utc::now()).await {
            Ok(resolved) => discussions = Some(resolved),
            // Conversation links are enrichment. A provider outage must never prevent source
            // data from being committed, and the next sync will retry from the same cache state.
            Err(err) => log::warn!("discussion matching: {err:#}"),
        }
    }

    if args.fetch.dry_run {
        println!("dry run: {} new item(s), nothing committed", report.added());
        finish(&report)?;
        return Ok(discussions);
    }
    if args.fetch_only {
        super::offline::capture(project, worktree.dir(), discussions.as_ref()).await?;
        println!(
            "fetch only: {} new item(s), nothing committed or pushed",
            report.added()
        );
        finish(&report)?;
        return Ok(discussions);
    }

    super::offline::capture(project, worktree.dir(), discussions.as_ref()).await?;

    let message = commit_message(
        &report,
        first,
        env!("CARGO_PKG_VERSION"),
        project.config_sha().as_deref(),
    );
    match worktree.commit(&message)? {
        Some(sha) => println!(
            "committed {}: {}",
            &sha[..7.min(sha.len())],
            message.subject
        ),
        None => println!("nothing new"),
    }
    match worktree.push()? {
        PushOutcome::Pushed => println!("pushed {}", worktree.branch()),
        PushOutcome::UpToDate => {}
        PushOutcome::NoRemote => log::info!("no origin remote; skipping push"),
    }
    if let Some(head) = worktree.head_sha()? {
        if report.errors() == 0
            && !args.fetch.backfill_media
            && let Err(err) = worktree.update_ref(LAST_GOOD, &head)
        {
            log::warn!(
                "data saved successfully; recovery pointer {LAST_GOOD} could not be updated: {err}. Continuing with the current data snapshot"
            );
            log::debug!("recovery pointer update: {err:#}");
        }
        println!("{head}");
    }
    finish(&report)?;
    Ok(discussions)
}

fn trim_cache(project: &Project) {
    if let Err(err) = project
        .build_cache_dir()
        .and_then(|cache| crate::cache::enforce_limits(&cache, &project.config.cache))
    {
        log::warn!("cache limit cleanup skipped: {err:#}");
    }
}

/// The same required synchronization stage for dev, redirected to its private cache and stopped
/// before git commit/push. Keeping this here prevents build and dev from growing different fetch
/// semantics over time.
pub(super) async fn run_dev(
    project: &Project,
    worktree: &crate::git::Worktree,
    args: &FetchArgs,
    cache: &Path,
    ownership: Option<std::sync::Arc<super::lock::DevLock>>,
) -> Result<Report> {
    run_dev_observed(project, worktree, args, cache, None, ownership).await
}

pub(super) async fn run_dev_with_progress(
    project: &Project,
    worktree: &crate::git::Worktree,
    args: &FetchArgs,
    cache: &Path,
    progress: tokio::sync::mpsc::Sender<fetch::Progress>,
    ownership: Option<std::sync::Arc<super::lock::DevLock>>,
) -> Result<Report> {
    run_dev_observed(project, worktree, args, cache, Some(progress), ownership).await
}

async fn run_dev_observed(
    project: &Project,
    worktree: &crate::git::Worktree,
    args: &FetchArgs,
    cache: &Path,
    progress: Option<tokio::sync::mpsc::Sender<fetch::Progress>>,
    ownership: Option<std::sync::Arc<super::lock::DevLock>>,
) -> Result<Report> {
    let report = fetch::run_with_cache_progress(
        project,
        worktree,
        args,
        cache,
        fetch::StatePolicy::DevCache,
        progress,
        ownership,
    )
    .await;
    if !args.dry_run
        && let Err(err) = crate::cache::enforce_limits(cache, &project.config.cache)
    {
        log::warn!("cache limit cleanup skipped: {err:#}");
    }
    let report = report?;
    finish(&report)?;
    Ok(report)
}

fn finish(report: &Report) -> Result<()> {
    if report.all_failed() {
        bail!("every source failed");
    }
    Ok(())
}

/// Drop cache files nothing reads any more before the run adds new ones. Housekeeping only: it is
/// throttled to one pass a day inside the cache, a failure is reported, and the sync proceeds on
/// the cache as it is.
fn sweep_cache(project: &Project) {
    let swept = project.build_cache_dir().and_then(|cache| {
        crate::cache::sweep(&cache, Some(crate::media::image_failure_generation()))
    });
    match swept {
        Ok(report) if report.throttled => {}
        Ok(report) => log::debug!("cache sweep: {report}"),
        Err(err) => log::warn!("cache sweep skipped: {err:#}"),
    }
}

/// Subject says what changed, body lists sources, trailers make runs greppable.
pub fn commit_message(
    report: &Report,
    first: bool,
    version: &str,
    config_sha: Option<&str>,
) -> CommitMessage {
    let added = report.added();
    let subject = if first {
        "aggr: init".to_string()
    } else if added > 0 {
        format!("aggr: +{added} item{}", if added == 1 { "" } else { "s" })
    } else if report.reprocessed > 0 {
        format!("aggr: reprocess {} items", report.reprocessed)
    } else if report.status_changed {
        "aggr: status".to_string()
    } else {
        "aggr: update".to_string()
    };
    let mut body: Vec<String> = report
        .sources
        .iter()
        .filter(|s| s.added > 0 || !matches!(s.outcome, Outcome::Ok))
        .map(|s| match &s.outcome {
            Outcome::Ok => format!("{}: +{}", s.slug, s.added),
            Outcome::Error(message) => format!(
                "{}: error: {}",
                s.slug,
                message.lines().next().unwrap_or_default()
            ),
        })
        .collect();
    if report.reprocessed > 0 {
        body.push(format!(
            "reprocess: {} stored article bodies",
            report.reprocessed
        ));
    }
    if report.removed > 0 {
        body.push(format!("retention: -{}", report.removed));
    }
    let mut trailers = vec![("Aggr-Version".to_string(), version.to_string())];
    if let Some(sha) = config_sha {
        trailers.push(("Aggr-Config".to_string(), sha.to_string()));
    }
    trailers.push((
        "Aggr-Sources".to_string(),
        format!("{} ok, {} error", report.ok(), report.errors()),
    ));
    CommitMessage {
        subject,
        body,
        trailers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::fetch::SourceReport;

    fn report(sources: Vec<(&str, Outcome, usize)>, status_changed: bool) -> Report {
        Report {
            sources: sources
                .into_iter()
                .map(|(slug, outcome, added)| SourceReport {
                    slug: slug.into(),
                    outcome,
                    added,
                    unchanged: added == 0,
                })
                .collect(),
            status_changed,
            removed: 0,
            reprocessed: 0,
        }
    }

    #[tokio::test]
    async fn fetch_only_and_failed_syncs_enforce_cache_limits_but_dry_runs_do_not() {
        crate::http::install_crypto_provider();
        let origin = httpmock::MockServer::start_async().await;
        origin
            .mock_async(|_, then| {
                then.status(404);
            })
            .await;
        for (dry_run, failing) in [(false, false), (false, true), (true, false)] {
            let root = tempfile::tempdir().unwrap();
            assert!(
                std::process::Command::new("git")
                    .args(["init", "--quiet"])
                    .arg(root.path())
                    .status()
                    .unwrap()
                    .success()
            );
            let config = root.path().join("aggr.toml");
            let sources = if failing {
                format!("[[sources]]\nurl='{}/feed.xml'\n", origin.base_url())
            } else {
                String::new()
            };
            std::fs::write(&config, format!(
                "[defaults]\ndiscussions=[]\n[fetch]\nretries=0\n[cache]\nresponse_max_bytes=1\n{sources}"
            )).unwrap();
            let project = Project::load(&config).await.unwrap();
            let cache = project.build_cache_dir().unwrap();
            let entry = crate::cache::Namespace::Articles
                .dir(&cache)
                .join("old-entry");
            crate::cache::write(&entry, b"disposable response cache").unwrap();
            let args = SyncArgs {
                fetch: crate::cli::FetchArgs {
                    dry_run,
                    ..Default::default()
                },
                fetch_only: true,
                ..Default::default()
            };
            let result = run(&project, &args).await;
            if failing {
                assert!(format!("{:#}", result.unwrap_err()).contains("every source failed"));
            } else {
                result.unwrap();
            }
            assert_eq!(
                entry.exists(),
                dry_run,
                "dry_run={dry_run}, failing={failing}"
            );
        }
    }

    #[test]
    fn reprocessed_bodies_are_reported_without_masking_failed_sources() {
        let mut r = report(
            vec![("current", Outcome::Error("offline".into()), 0)],
            false,
        );
        r.reprocessed = 3;
        assert_eq!(r.added(), 0);
        assert_eq!(r.ok(), 0);
        assert_eq!(r.errors(), 1);
        assert!(r.all_failed());
        assert!(finish(&r).is_err());
        let message = commit_message(&r, false, "1", None);
        assert_eq!(message.subject, "aggr: reprocess 3 items");
        assert_eq!(
            message.body,
            [
                "current: error: offline",
                "reprocess: 3 stored article bodies"
            ]
        );
    }

    #[test]
    fn subjects_follow_what_changed() {
        let r = report(
            vec![
                ("a", Outcome::Ok, 3),
                ("b", Outcome::Ok, 0),
                ("c", Outcome::Error("boom\nmore".into()), 0),
            ],
            true,
        );
        let m = commit_message(&r, false, "1.2.3", Some("abc"));
        assert_eq!(m.subject, "aggr: +3 items");
        assert_eq!(m.body, ["a: +3", "c: error: boom"]);
        assert_eq!(
            m.render(),
            "aggr: +3 items\n\na: +3\nc: error: boom\n\nAggr-Version: 1.2.3\nAggr-Config: abc\nAggr-Sources: 2 ok, 1 error\n"
        );

        let one = report(vec![("a", Outcome::Ok, 1)], false);
        assert_eq!(
            commit_message(&one, false, "1", None).subject,
            "aggr: +1 item"
        );
        assert_eq!(commit_message(&one, true, "1", None).subject, "aggr: init");
        let status = report(vec![("a", Outcome::Error("x".into()), 0)], true);
        assert_eq!(
            commit_message(&status, false, "1", None).subject,
            "aggr: status"
        );
        let none = report(vec![("a", Outcome::Ok, 0)], false);
        let m = commit_message(&none, false, "1", None);
        assert_eq!(m.subject, "aggr: update");
        assert!(m.body.is_empty());
        assert_eq!(m.trailers.len(), 2);
    }
}
