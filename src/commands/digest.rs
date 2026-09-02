//! `aggr digest`: once a day, after the configured local time, open a GitHub issue listing what
//! arrived since the previous digest and pin a `refs/aggr/digest/<date>` ref on the data commit.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use anyhow::{Context as _, Result, bail};
use chrono::{NaiveDate, Utc};
use serde::Serialize;

use super::Project;
use crate::cli::DigestArgs;
use crate::digest::{self, DigestCtx, Group};
use crate::github::{GitHub, NewIssue};
use crate::http;
use crate::site::{self, context::GitHubLinks, render::Renderer};
use crate::store::Store;

#[derive(Serialize)]
struct Ctx {
    digest: DigestCtx,
    groups: Vec<Group>,
    site_title: String,
}

pub async fn run(project: &Project, args: &DigestArgs) -> Result<()> {
    let Some(config) = &project.config.digest else {
        println!("digest: disabled (no [digest] section)");
        return Ok(());
    };
    let worktree = project.worktree()?;
    let Some(head) = worktree.head_sha()? else {
        bail!("the data branch has no commits yet; run `aggr sync` first");
    };
    let now = Utc::now();
    let tz = config.timezone;

    let posted: BTreeMap<NaiveDate, String> = worktree
        .list_refs(&format!("{}*", digest::REF_PREFIX))?
        .into_iter()
        .filter_map(|(name, sha)| digest::ref_date(&name).map(|date| (date, sha)))
        .collect();
    let dates: BTreeSet<NaiveDate> = posted.keys().copied().collect();
    let date = if args.force {
        now.with_timezone(&tz).date_naive()
    } else {
        match digest::due(now, tz, config.at, &dates) {
            Some(date) => date,
            None => {
                println!(
                    "digest: not due (daily after {} {tz}{})",
                    config.at.format("%H:%M"),
                    dates
                        .last()
                        .map(|d| format!(", last posted {d}"))
                        .unwrap_or_default()
                );
                return Ok(());
            }
        }
    };
    let number = posted.keys().filter(|d| **d < date).count() + 1;
    let previous = posted
        .iter()
        .rfind(|(d, _)| **d < date)
        .map(|(_, sha)| sha.clone());

    let store = Store::open(worktree.dir());
    let items = store.items()?;
    let (added, since) = match previous {
        Some(sha) if worktree.ensure_commit(&sha)? => {
            let files: HashSet<String> = worktree
                .added_files(&sha, &head, "items/")?
                .into_iter()
                .collect();
            (Some(files), worktree.commit_time(&sha)?)
        }
        _ => (None, digest::first_digest_since(now)),
    };
    let selected = digest::select(items, added.as_ref(), since);
    if selected.is_empty() && config.skip_empty && !args.force {
        println!("digest: nothing new since {since}, skipping today");
        return Ok(());
    }

    let site_url = super::build::non_empty(args.base_url.as_deref())
        .map(str::to_string)
        .or_else(|| project.config.site.url.as_ref().map(|url| url.to_string()))
        .map(|url| {
            if url.ends_with('/') {
                url
            } else {
                format!("{url}/")
            }
        });
    let repository = project.config.repository();
    let links = repository.as_deref().map(|repository| GitHubLinks {
        repository,
        branch: &project.config.store.branch,
        data_sha: Some(&head),
    });
    let names: BTreeMap<String, String> = project
        .sources
        .iter()
        .map(|source| {
            let name = source
                .name
                .clone()
                .or_else(|| store.source_state(&source.slug).ok()?.title)
                .unwrap_or_else(|| source.slug.clone());
            (source.slug.clone(), name)
        })
        .collect();
    let (groups, omitted) = digest::group(
        &selected,
        &project.sources,
        &names,
        links.as_ref(),
        site_url.as_deref(),
        config.max_items,
    );
    let title = digest::title(
        &config.title,
        number,
        date,
        selected.len(),
        &project.config.site.title,
    );
    let ctx = Ctx {
        digest: DigestCtx {
            number,
            date: date.format("%Y-%m-%d").to_string(),
            count: selected.len(),
            omitted,
            since,
            site_url,
            repository: repository.clone(),
            data_sha: Some(head.clone()),
        },
        groups,
        site_title: project.config.site.title.clone(),
    };
    let renderer = Renderer::new(site::theme_layers(&project.config, &project.root)?, "/")?;
    let body = renderer.render("digest.md", &ctx)?;

    if args.dry_run {
        println!("{title}\n\n{body}");
        return Ok(());
    }

    let Some(repository) = repository else {
        bail!("digest needs a repository: set `[site] repository` or run inside GitHub Actions");
    };
    let client = http::Client::new(&project.config.fetch)?;
    let Some(github) = GitHub::from_env(&repository, client.raw().clone()) else {
        bail!("digest needs GITHUB_TOKEN (or GH_TOKEN) to open issues on {repository}");
    };

    let assignees = if config.assignees.is_empty() {
        github.default_assignee().await?.into_iter().collect()
    } else {
        config.assignees.clone()
    };
    github
        .ensure_label(
            &config.label,
            digest::LABEL_COLOR,
            digest::LABEL_DESCRIPTION,
        )
        .await?;
    let previous_issues = if config.close_previous {
        github.open_issues(&config.label).await?
    } else {
        Vec::new()
    };
    let issue = github
        .create_issue(&NewIssue {
            title: &title,
            body: &body,
            labels: std::slice::from_ref(&config.label),
            assignees: &assignees,
        })
        .await
        .context("creating the digest issue")?;
    for old in previous_issues
        .iter()
        .filter(|old| old.number != issue.number)
    {
        match github.close_issue(old.number).await {
            Ok(()) => println!("closed #{} {}", old.number, old.title),
            Err(err) => log::warn!("closing #{}: {err:#}", old.number),
        }
    }
    worktree.update_ref(&digest::ref_name(date), &head)?;
    println!(
        "digest #{number}: {} ({} item(s))",
        issue.html_url,
        selected.len()
    );
    Ok(())
}
