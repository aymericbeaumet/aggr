//! `aggr sync`: fetch, commit with trailers, push, move `refs/aggr/last-good`. What CI runs.

use std::path::Path;

use anyhow::{Result, bail};

use super::Project;
use super::fetch::{self, Report};
use crate::cli::FetchArgs;
use crate::git::{CommitMessage, PushOutcome};
use crate::store::Outcome;

pub const LAST_GOOD: &str = "refs/aggr/last-good";

pub async fn run(project: &Project, args: &FetchArgs) -> Result<()> {
    let worktree = project.worktree()?;
    let first = worktree.head_sha()?.is_none();
    let report = fetch::run(project, &worktree, args).await?;

    if args.dry_run {
        println!("dry run: {} new item(s), nothing committed", report.added());
        return finish(&report);
    }

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
        if report.errors() == 0 {
            worktree.update_ref(LAST_GOOD, &head)?;
        }
        println!("{head}");
    }
    finish(&report)
}

/// The same required synchronization stage for dev, redirected to its private cache and stopped
/// before git commit/push. Keeping this here prevents build and dev from growing different fetch
/// semantics over time.
pub async fn run_dev(
    project: &Project,
    worktree: &crate::git::Worktree,
    args: &FetchArgs,
    cache: &Path,
) -> Result<Report> {
    let report =
        fetch::run_with_cache(project, worktree, args, cache, fetch::StatePolicy::DevCache).await?;
    finish(&report)?;
    Ok(report)
}

fn finish(report: &Report) -> Result<()> {
    if report.all_failed() {
        bail!("every source failed");
    }
    Ok(())
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
        }
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
