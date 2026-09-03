//! A purpose-built Pagefind index. Only titles and cleaned article prose are searchable; URLs,
//! timestamps, taxonomy, and discussion links live in a display-only sidecar so they cannot
//! distort relevance or excerpts.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context as _, Result};
use pagefind::api::PagefindIndex;
use serde::Serialize;

use super::context::{DiscussionLinkCtx, ItemCtx};

#[derive(Debug, Clone)]
pub struct SearchDocument {
    pub url: String,
    pub content: String,
    pub meta: BTreeMap<String, String>,
    pub filters: BTreeMap<String, Vec<String>>,
    pub sort: BTreeMap<String, String>,
    pub display: SearchDisplay,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchDisplay {
    pub original: String,
    pub domain: String,
    pub source_slug: String,
    pub excerpt: String,
    pub discussions: Vec<DiscussionLinkCtx>,
}

impl SearchDocument {
    pub fn new(item: &ItemCtx, markdown: &str) -> Self {
        let mut meta = BTreeMap::new();
        meta.insert("title".into(), item.title.clone());
        meta.insert("date".into(), item.date.to_rfc3339());

        let mut filters = BTreeMap::new();
        if let Some(category) = &item.category {
            filters.insert("category".into(), vec![category.clone()]);
        }
        if !item.labels.is_empty() {
            filters.insert("tag".into(), item.labels.clone());
        }

        let mut sort = BTreeMap::new();
        sort.insert("date".into(), item.date.to_rfc3339());

        Self {
            url: format!("/{}", item.url.trim_start_matches('/')),
            // Markdown syntax, link targets, and raw HTML are intentionally absent from the
            // digest handed to Pagefind. Its generated snippet is always human prose.
            content: crate::content::html_to_text(&crate::content::render_markdown(markdown)),
            meta,
            filters,
            sort,
            display: SearchDisplay {
                original: item.link.clone(),
                domain: item.domain.clone(),
                source_slug: item.source.clone(),
                excerpt: item.excerpt.clone(),
                discussions: item.discussions.clone(),
            },
        }
    }
}

/// Pagefind's API is async and uses blocking workers internally. Site generation is deliberately
/// synchronous, so isolate its small runtime on a thread; this also works when `aggr build` is
/// already running inside the CLI's multithreaded Tokio runtime.
pub fn build(out: &Path, documents: &[SearchDocument]) -> Result<()> {
    let site = out.to_path_buf();
    let documents = documents.to_vec();
    std::thread::spawn(move || -> Result<()> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("starting Pagefind runtime")?;
        runtime.block_on(async move {
            let mut index = PagefindIndex::new(None).context("configuring Pagefind")?;
            for document in &documents {
                index
                    .add_custom_record(
                        document.url.clone(),
                        document.content.clone(),
                        "en".into(),
                        Some(document.meta.clone()),
                        Some(document.filters.clone()),
                        Some(document.sort.clone()),
                    )
                    .await
                    .with_context(|| format!("indexing {}", document.url))?;
            }
            index
                .write_files(Some(site.join("pagefind").to_string_lossy().into_owned()))
                .await
                .context("writing Pagefind index")?;

            let display: BTreeMap<_, _> = documents
                .iter()
                .map(|document| {
                    (
                        document.url.trim_start_matches('/').to_string(),
                        &document.display,
                    )
                })
                .collect();
            std::fs::write(
                site.join("search-meta.json"),
                serde_json::to_vec(&display).context("serializing search display metadata")?,
            )
            .context("writing search-meta.json")?;
            Ok(())
        })
    })
    .join()
    .map_err(|_| anyhow::anyhow!("Pagefind indexing thread panicked"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone as _, Utc};

    fn item() -> ItemCtx {
        ItemCtx {
            path: "items/blog/2026/09/post".into(),
            url: "items/blog/post/".into(),
            title: "Useful result".into(),
            link: "https://secret.example/path?token=noise".into(),
            domain: "secret.example".into(),
            source: "blog".into(),
            source_name: "Blog".into(),
            category: Some("engineering".into()),
            date: Utc.with_ymd_and_hms(2026, 9, 3, 10, 0, 0).unwrap(),
            age_band: "h1",
            published: None,
            updated: None,
            first_seen: Utc.with_ymd_and_hms(2026, 9, 3, 10, 0, 0).unwrap(),
            authors: vec!["Person".into()],
            labels: vec!["rust".into()],
            discussions: vec![DiscussionLinkCtx {
                name: "hackernews".into(),
                url: "https://hn.example/search".into(),
                found: false,
                score: None,
            }],
            summary: Some("A concise fallback".into()),
            excerpt: "A concise fallback".into(),
            content: crate::model::ContentKind::Extracted,
            extra: BTreeMap::new(),
            permalink: None,
            raw_url: None,
            history_url: None,
            edit_url: None,
            body_html: None,
        }
    }

    #[test]
    fn searchable_record_excludes_urls_and_display_metadata() {
        let document = SearchDocument::new(
            &item(),
            "Human prose about Ferris. [useful label](https://noise.example/hidden)",
        );
        assert!(document.content.contains("Human prose about Ferris"));
        assert!(document.content.contains("useful label"));
        assert!(!document.content.contains("https://"));
        assert_eq!(document.meta.len(), 2);
        assert_eq!(document.meta["title"], "Useful result");
        assert_eq!(document.filters["category"], ["engineering"]);
        assert_eq!(document.filters["tag"], ["rust"]);
        assert_eq!(
            document.display.original,
            "https://secret.example/path?token=noise"
        );
    }

    #[test]
    fn writes_pagefind_and_a_display_only_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let document = SearchDocument::new(&item(), "Only this prose is searchable.");
        build(dir.path(), &[document]).unwrap();
        assert!(dir.path().join("pagefind/pagefind.js").is_file());
        let sidecar: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("search-meta.json")).unwrap())
                .unwrap();
        assert_eq!(
            sidecar["items/blog/post/"]["original"],
            "https://secret.example/path?token=noise"
        );
    }
}
