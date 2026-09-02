//! `aggr fetch`: every selected source in parallel, each writing only its own directory of the
//! data worktree. The store's dedupe keys decide what is new; nothing here touches git.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use chrono::{DateTime, Utc};
use futures_util::{StreamExt as _, stream};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use super::Project;
use crate::cli::FetchArgs;
use crate::config::{ContentMode, Source, StoreConfig};
use crate::content;
use crate::git::Worktree;
use crate::http;
use crate::model::{
    ContentKind, FrontMatter, RawItem, dedupe_keys, file_stem, item_dir, unique_stem,
};
use crate::sources::{self, Fetch};
use crate::store::{NewItem, Outcome, Store, retention};

pub struct Report {
    pub sources: Vec<SourceReport>,
    pub status_changed: bool,
    /// Items deleted by `[store]` retention.
    pub removed: usize,
}

pub struct SourceReport {
    pub slug: String,
    pub outcome: Outcome,
    pub added: usize,
    pub unchanged: bool,
}

impl Report {
    pub fn added(&self) -> usize {
        self.sources.iter().map(|s| s.added).sum()
    }

    pub fn ok(&self) -> usize {
        self.sources
            .iter()
            .filter(|s| s.outcome == Outcome::Ok)
            .count()
    }

    pub fn errors(&self) -> usize {
        self.sources.len() - self.ok()
    }

    pub fn all_failed(&self) -> bool {
        !self.sources.is_empty() && self.ok() == 0
    }
}

#[derive(Clone)]
struct Options {
    dry_run: bool,
    refresh: bool,
    html: bool,
    html_max_bytes: usize,
    article_concurrency: usize,
    now: DateTime<Utc>,
}

pub async fn run(project: &Project, worktree: &Worktree, args: &FetchArgs) -> Result<Report> {
    let cache_dir = project.cache_dir()?;
    run_with_cache(project, worktree, args, &cache_dir).await
}

pub async fn run_with_cache(
    project: &Project,
    worktree: &Worktree,
    args: &FetchArgs,
    cache_dir: &Path,
) -> Result<Report> {
    let selected: Vec<Source> = project.sources.clone();
    let store = Arc::new(Store::open(worktree.dir()));
    let client = Arc::new(http::Client::new(&project.config.fetch)?);
    let options = Options {
        dry_run: args.dry_run,
        refresh: args.refresh,
        html: project.config.store.html,
        html_max_bytes: project.config.store.html_max_bytes,
        article_concurrency: project.config.fetch.article_concurrency,
        now: Utc::now(),
    };
    let limit = Arc::new(Semaphore::new(project.config.fetch.concurrency));

    let mut tasks = JoinSet::new();
    for source in selected {
        let (store, client, cache_dir, options, limit) = (
            store.clone(),
            client.clone(),
            cache_dir.to_path_buf(),
            options.clone(),
            limit.clone(),
        );
        tasks.spawn(async move {
            let _permit = limit.acquire_owned().await;
            let result = fetch_one(&source, &store, &client, &cache_dir, &options).await;
            (source.slug, result)
        });
    }

    let mut reports = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        let (slug, result) = joined.context("a fetch task panicked")?;
        let report = match result {
            Ok(report) => report,
            Err(err) => {
                log::warn!("{slug}: {err:#}");
                SourceReport {
                    slug,
                    outcome: Outcome::Error(format!("{err:#}")),
                    added: 0,
                    unchanged: false,
                }
            }
        };
        reports.push(report);
    }
    let order: BTreeMap<&str, usize> = project
        .sources
        .iter()
        .enumerate()
        .map(|(i, s)| (s.slug.as_str(), i))
        .collect();
    reports.sort_by_key(|report| order.get(report.slug.as_str()).copied());

    let outcomes: BTreeMap<String, Outcome> = reports
        .iter()
        .map(|r| (r.slug.clone(), r.outcome.clone()))
        .collect();
    let known: BTreeSet<String> = project.sources.iter().map(|s| s.slug.clone()).collect();
    let mut status = store.status()?;
    let status_changed = status.apply(&outcomes, &known, options.now);
    if status_changed && !args.dry_run {
        store.write_status(&status)?;
    }

    for report in &reports {
        match &report.outcome {
            Outcome::Error(message) => println!("{}: error: {message}", report.slug),
            Outcome::Ok if report.unchanged => println!("{}: unchanged", report.slug),
            Outcome::Ok => println!("{}: +{}", report.slug, report.added),
        }
    }

    let removed = if args.dry_run {
        0
    } else {
        apply_retention(&store, &project.config.store, options.now)?
    };
    if removed > 0 {
        println!("retention: -{removed}");
    }
    Ok(Report {
        sources: reports,
        status_changed,
        removed,
    })
}

/// Drop what `[store] max_age_days` / `max_items` exclude. A no-op unless one of them is set.
fn apply_retention(store: &Store, config: &StoreConfig, now: DateTime<Utc>) -> Result<usize> {
    let limits = retention::Limits {
        max_age_days: config.max_age_days,
        max_items: config.max_items,
    };
    if limits.is_unbounded() {
        return Ok(0);
    }
    let drop = retention::plan(&store.items()?, limits, now);
    for path in &drop {
        store.remove_item(path)?;
    }
    Ok(drop.len())
}

async fn fetch_one(
    source: &Source,
    store: &Store,
    client: &http::Client,
    cache_dir: &Path,
    options: &Options,
) -> Result<SourceReport> {
    let slug = &source.slug;
    let state = store.source_state(slug)?;
    let ctx = sources::Context {
        client,
        state: &state,
        cache_dir,
    };
    let fetched = sources::fetch(source, &ctx).await?;

    let mut next_state = state.clone();
    if let Some(url) = source.engine.url() {
        next_state.url = url.to_string();
    }
    let report = match fetched {
        Fetch::Unchanged { validators } => {
            validators.apply(&mut next_state);
            SourceReport {
                slug: slug.clone(),
                outcome: Outcome::Ok,
                added: 0,
                unchanged: true,
            }
        }
        Fetch::Changed {
            validators,
            meta,
            items,
        } => {
            validators.apply(&mut next_state);
            next_state.title = meta.title.or(next_state.title);
            next_state.site_url = meta.site_url.or(next_state.site_url);

            let seen = store.seen(slug)?;
            let mut new_keys: Vec<String> = Vec::new();
            let mut prospective = seen.clone();
            let mut taken: HashSet<(String, String)> = HashSet::new();
            let mut added = 0;
            // Feeds list newest first; preserve oldest-first writes while original pages download
            // concurrently. This keeps deterministic suffixes without making heavy mode serial.
            let mut candidates = Vec::new();
            for raw in items.iter().rev() {
                let keys = dedupe_keys(raw);
                let known = keys.iter().any(|key| prospective.contains(key));
                if known && !options.refresh {
                    continue;
                }
                if !known {
                    prospective.extend(keys.iter().cloned());
                }
                candidates.push((raw.clone(), keys, known));
            }
            let enriched = stream::iter(candidates)
                .map(|(raw, keys, known)| async move {
                    let (raw, kind) = heavy_content(&raw, source, client).await;
                    (raw, kind, keys, known)
                })
                .buffered(options.article_concurrency)
                .collect::<Vec<_>>()
                .await;
            for (raw, content_kind, keys, known) in enriched {
                let mut planned = plan(&raw, source, options, content_kind);
                if !options.refresh {
                    let dir = planned.dir.clone();
                    planned.stem = unique_stem(&planned.stem, |stem| {
                        store.stem_exists(&dir, stem)
                            || taken.contains(&(dir.clone(), stem.to_string()))
                    });
                }
                if planned.html.is_some() {
                    planned.front.html = Some(format!("{}.html", planned.stem));
                }
                if !options.dry_run {
                    store.write_item(NewItem {
                        dir: &planned.dir,
                        stem: &planned.stem,
                        front: &planned.front,
                        body: &planned.body,
                        html: planned.html.as_deref(),
                    })?;
                }
                taken.insert((planned.dir, planned.stem));
                if !known {
                    new_keys.extend(keys);
                }
                added += 1;
            }
            if !options.dry_run && !new_keys.is_empty() {
                store.append_seen(slug, &new_keys, options.now)?;
            }
            SourceReport {
                slug: slug.clone(),
                outcome: Outcome::Ok,
                added,
                unchanged: false,
            }
        }
    };
    if !options.dry_run {
        store.write_source_state(slug, &next_state)?;
    }
    Ok(report)
}

async fn heavy_content(
    raw: &RawItem,
    source: &Source,
    client: &http::Client,
) -> (RawItem, ContentKind) {
    let fallback = if raw.content_html.is_some() {
        ContentKind::Feed
    } else {
        ContentKind::None
    };
    if source.content == ContentMode::Light {
        return (raw.clone(), fallback);
    }
    let Ok(url) = url::Url::parse(&raw.link) else {
        return (raw.clone(), fallback);
    };
    let result = async {
        let response = client
            .get(http::Request {
                url: &url,
                headers: &source.headers,
                etag: None,
                last_modified: None,
            })
            .await?;
        let http::Response::Ok(body) = response else {
            anyhow::bail!("original page unexpectedly returned not modified");
        };
        let page = String::from_utf8_lossy(&body.bytes);
        content::extract_article(&page, &body.final_url)
    }
    .await;
    match result {
        Ok(html) => {
            let mut enriched = raw.clone();
            enriched.content_html = Some(html);
            (enriched, ContentKind::Extracted)
        }
        Err(err) => {
            log::warn!(
                "{}: heavy content fallback for {}: {err:#}",
                source.slug,
                raw.link
            );
            (raw.clone(), fallback)
        }
    }
}

/// An item's files, decided before anything is written.
struct Planned {
    dir: String,
    stem: String,
    front: FrontMatter,
    body: String,
    html: Option<String>,
}

fn plan(raw: &RawItem, source: &Source, options: &Options, content_kind: ContentKind) -> Planned {
    // A future-dated entry would otherwise land in a directory that does not exist yet.
    let published = raw.published.map(|date| date.min(options.now));
    let date = published.unwrap_or(options.now);
    let base = url::Url::parse(&raw.link).ok();
    let body = match &raw.content_html {
        Some(html) => content::to_markdown(html, base.as_ref()),
        None => raw.summary.clone().unwrap_or_default(),
    };
    let (html, truncated) = match &raw.content_html {
        Some(html) if options.html && source.html => {
            let (stored, truncated) = content::storage_html(html, options.html_max_bytes);
            (Some(stored), truncated)
        }
        _ => (None, false),
    };
    let front = FrontMatter {
        title: raw.title.clone(),
        link: raw.link.clone(),
        source: source.slug.clone(),
        published,
        updated: raw.updated.map(|date| date.min(options.now)),
        first_seen: options.now,
        authors: raw.authors.clone(),
        labels: source
            .labels
            .iter()
            .chain(&raw.labels)
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect(),
        summary: raw.summary.clone().filter(|s| !s.trim().is_empty()),
        content: content_kind,
        html: None,
        html_truncated: truncated,
        extra: raw.extra.clone(),
        hidden: false,
    };
    Planned {
        dir: item_dir(&source.slug, date),
        stem: file_stem(date, &raw.title),
        front,
        body,
        html,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use httpmock::prelude::*;
    use url::Url;

    fn source() -> Source {
        Source {
            slug: "blog".into(),
            name: None,
            category: None,
            labels: vec![],
            headers: vec![],
            html: true,
            content: ContentMode::Heavy,
            engine: crate::config::Engine::Feed {
                url: Url::parse("https://blog.example/feed").unwrap(),
            },
        }
    }

    fn options() -> Options {
        Options {
            dry_run: false,
            refresh: false,
            html: true,
            html_max_bytes: 1000,
            article_concurrency: 4,
            now: Utc.with_ymd_and_hms(2026, 9, 2, 12, 0, 0).unwrap(),
        }
    }

    #[test]
    fn plans_paths_from_the_published_date_and_clamps_the_future() {
        let raw = RawItem {
            title: "Hello, World!".into(),
            link: "https://blog.example/hello".into(),
            published: Some(Utc.with_ymd_and_hms(2026, 8, 30, 1, 0, 0).unwrap()),
            content_html: Some("<p>Hi <script>x()</script><b>there</b></p>".into()),
            ..Default::default()
        };
        let planned = plan(&raw, &source(), &options(), ContentKind::Feed);
        assert_eq!(planned.dir, "items/blog/2026/08");
        assert_eq!(planned.stem, "2026-08-30-hello-world");
        assert_eq!(planned.front.content, ContentKind::Feed);
        assert_eq!(planned.front.first_seen, options().now);
        assert!(planned.body.contains("**there**"), "{}", planned.body);
        assert!(!planned.body.contains("script"));
        let html = planned.html.unwrap();
        assert!(!html.contains("<script>"), "{html}");

        let future = RawItem {
            published: Some(Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap()),
            ..raw
        };
        let planned = plan(&future, &source(), &options(), ContentKind::None);
        assert_eq!(planned.dir, "items/blog/2026/09");
        assert_eq!(planned.front.published, Some(options().now));
    }

    #[test]
    fn titles_only_entries_keep_the_summary_as_body_and_no_html() {
        let raw = RawItem {
            title: "T".into(),
            link: "https://x/".into(),
            summary: Some("Just a summary".into()),
            ..Default::default()
        };
        let planned = plan(&raw, &source(), &options(), ContentKind::None);
        assert_eq!(planned.body, "Just a summary");
        assert_eq!(planned.front.content, ContentKind::None);
        assert!(planned.html.is_none());
        assert_eq!(planned.dir, "items/blog/2026/09");

        let no_html = Source {
            html: false,
            ..source()
        };
        let with_content = RawItem {
            content_html: Some("<p>x</p>".into()),
            ..raw
        };
        assert!(
            plan(&with_content, &no_html, &options(), ContentKind::Feed)
                .html
                .is_none()
        );
    }

    #[tokio::test]
    async fn heavy_downloads_the_article_while_light_keeps_feed_content() {
        let server = MockServer::start_async().await;
        let article = server
            .mock_async(|when, then| {
                when.method(GET).path("/post");
                then.status(200).body(
                    "<html><title>Post</title><article><h1>Post</h1><p>The complete original article has substantially more useful text than its feed excerpt.</p><p>This second paragraph makes it readable.</p></article></html>",
                );
            })
            .await;
        let raw = RawItem {
            title: "Post".into(),
            link: server.url("/post"),
            content_html: Some("<p>short feed excerpt</p>".into()),
            ..Default::default()
        };
        let client = http::Client::new(&crate::config::FetchConfig {
            retries: 0,
            ..Default::default()
        })
        .unwrap();

        let (heavy, kind) = heavy_content(&raw, &source(), &client).await;
        assert_eq!(kind, ContentKind::Extracted);
        assert!(
            heavy
                .content_html
                .unwrap()
                .contains("complete original article")
        );
        article.assert_calls_async(1).await;

        let light = Source {
            content: ContentMode::Light,
            ..source()
        };
        let (unchanged, kind) = heavy_content(&raw, &light, &client).await;
        assert_eq!(kind, ContentKind::Feed);
        assert_eq!(unchanged.content_html, raw.content_html);
        article.assert_calls_async(1).await;
    }
}
