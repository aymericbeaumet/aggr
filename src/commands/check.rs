//! `aggr check`: the configuration parsed (or we would not be here), every source probed once,
//! one line per source. Non-zero exit when any probe fails.

use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use super::Project;
use crate::config::{Engine, Source};
use crate::git;
use crate::http::{self, Request, Response};
use crate::sources::{feed, html};

pub async fn run(project: &Project) -> Result<()> {
    println!(
        "config: {} ({} source(s), theme {:?}, timezone {})",
        project.config_path.display(),
        project.sources.len(),
        project.config.site.theme,
        project.config.site.timezone
    );
    if let Some(repository) = project.config.repository() {
        println!("repository: {repository}");
    }
    if let Some(digest) = &project.config.digest {
        println!(
            "digest: daily at {} ({})",
            digest.at.format("%H:%M"),
            project.config.site.timezone
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
            let request = Request {
                headers: &source.headers,
                ..Request::get(url)
            };
            let body = match client.get(request).await? {
                Response::Ok(body) => body,
                Response::NotModified => bail!("unexpected 304 without validators"),
            };
            let parsed = feed::parse(&body.bytes, &body.final_url)?;
            Ok(format!(
                "{} entr{}{}",
                parsed.entries.len(),
                if parsed.entries.len() == 1 {
                    "y"
                } else {
                    "ies"
                },
                parsed
                    .title
                    .as_ref()
                    .map(|t| format!("  \"{}\"", t.content))
                    .unwrap_or_default()
            ))
        }
        Engine::Html { url, fields } => {
            let request = Request {
                headers: &source.headers,
                ..Request::get(url)
            };
            let body = match client.get(request).await? {
                Response::Ok(body) => body,
                Response::NotModified => bail!("unexpected 304 without validators"),
            };
            let (meta, items) = html::extract(
                &String::from_utf8_lossy(&body.bytes),
                &body.final_url,
                fields,
            )?;
            let dated = items.iter().filter(|item| item.published.is_some()).count();
            Ok(format!(
                "{} item(s), {dated} dated{}{}",
                items.len(),
                meta.title
                    .as_ref()
                    .map(|t| format!("  \"{t}\""))
                    .unwrap_or_default(),
                items
                    .first()
                    .map(|item| format!("  first: {:?}", item.title))
                    .unwrap_or_default()
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
