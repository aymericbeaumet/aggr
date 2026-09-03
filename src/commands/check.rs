//! `aggr check`: the configuration parsed (or we would not be here), every source probed once,
//! one line per source. Non-zero exit when any probe fails.

use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use super::Project;
use crate::config::{Engine, Source};
use crate::git;
use crate::http;
use crate::sources::{self, Fetch, feed};
use crate::store::SourceState;

pub async fn run(project: &Project) -> Result<()> {
    println!(
        "config: {} ({} source(s), theme {:?})",
        project.config_path.display(),
        project.sources.len(),
        project.config.site.theme
    );
    if let Some(repository) = project.config.repository() {
        println!("repository: {repository}");
    }
    if let Some(digest) = &project.config.digest {
        println!(
            "digest: daily at {} ({})",
            digest.at.format("%H:%M"),
            digest.timezone
        );
    }

    let client = Arc::new(http::Client::new(&project.config.fetch)?);
    let limit = Arc::new(Semaphore::new(project.config.fetch.concurrency));
    let mut tasks = JoinSet::new();
    for (index, source) in project.sources.iter().cloned().enumerate() {
        let (client, limit) = (client.clone(), limit.clone());
        tasks.spawn(async move {
            let _permit = limit.acquire_owned().await;
            let outcome = probe(&source, &client).await;
            (index, source, outcome)
        });
    }
    let mut results = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        results.push(joined.context("a probe task panicked")?);
    }
    results.sort_by_key(|(index, ..)| *index);

    let width = results
        .iter()
        .map(|(_, s, _)| s.slug.len())
        .max()
        .unwrap_or(0);
    let mut failures = 0;
    for (_, source, outcome) in &results {
        let engine = source.engine.name();
        let url = source
            .engine
            .url()
            .map(|u| u.to_string())
            .unwrap_or_default();
        match outcome {
            Ok(detail) => println!("ok     {:<width$}  {engine}  {url}  {detail}", source.slug),
            Err(err) => {
                failures += 1;
                println!("error  {:<width$}  {engine}  {url}  {err:#}", source.slug);
            }
        }
    }
    if failures > 0 {
        bail!("{failures} of {} source(s) failed", results.len());
    }
    Ok(())
}

async fn probe(source: &Source, client: &http::Client) -> Result<String> {
    match &source.engine {
        Engine::Feed { url } => {
            let state = SourceState::default();
            let cache = tempfile::tempdir()?;
            let ctx = sources::Context {
                client,
                state: &state,
                cache_dir: cache.path(),
            };
            let (meta, items, resolved) = match feed::fetch(url, source, &ctx).await? {
                Fetch::Changed {
                    validators,
                    meta,
                    items,
                } => (meta, items, validators.resolved_url),
                Fetch::Unchanged { .. } => bail!("unexpected unchanged source without validators"),
            };
            Ok(format!(
                "{} item(s){}{}",
                items.len(),
                meta.title
                    .map(|title| format!("  \"{title}\""))
                    .unwrap_or_default(),
                resolved
                    .filter(|resolved| resolved != url.as_str())
                    .map(|resolved| format!("  discovered {resolved}"))
                    .unwrap_or_default(),
            ))
        }
        Engine::Aggr { url, branch, .. } => {
            let (remote, wanted) = (url.to_string(), branch.clone());
            let tip = tokio::task::spawn_blocking(move || git::remote_tip(&remote, &wanted))
                .await
                .context("git task")??;
            match tip {
                Some(sha) => Ok(format!("branch at {}", &sha[..7.min(sha.len())])),
                None => bail!("no {branch:?} branch"),
            }
        }
    }
}
