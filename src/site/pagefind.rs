//! A purpose-built Pagefind index. Only titles and cleaned article prose affect ranking. Display
//! metadata travels in Pagefind's result chunks with zero runtime weight, avoiding a second,
//! whole-corpus request before the first result can render.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context as _, Result};
use pagefind::api::PagefindIndex;
use serde::Serialize;

use super::context::ItemCtx;

#[derive(Debug, Clone)]
pub struct SearchDocument {
    pub url: String,
    pub content: String,
    pub meta: BTreeMap<String, String>,
    pub filters: BTreeMap<String, Vec<String>>,
    pub sort: BTreeMap<String, String>,
}

#[derive(Serialize)]
struct SearchDisplay<'a> {
    original: &'a str,
    domain: &'a str,
    source_slug: &'a str,
    excerpt: &'a str,
    discussions: &'a [super::context::DiscussionLinkCtx],
}

impl SearchDocument {
    pub fn new(item: &ItemCtx, markdown: &str) -> Self {
        let mut meta = BTreeMap::new();
        meta.insert("title".into(), item.title.clone());
        meta.insert("date".into(), item.date.to_rfc3339());
        meta.insert(
            "aggr_display".into(),
            serde_json::to_vec(&SearchDisplay {
                original: &item.link,
                domain: &item.domain,
                source_slug: &item.source,
                excerpt: &item.excerpt,
                discussions: &item.discussions,
            })
            .map(hex::encode)
            .unwrap_or_default(),
        );

        let mut filters = BTreeMap::new();
        if let Some(category) = &item.category {
            filters.insert(
                "category".into(),
                vec![super::context::category_slug(category)],
            );
        }
        if !item.labels.is_empty() {
            let mut labels: Vec<_> = item
                .labels
                .iter()
                .map(|label| super::context::category_slug(label))
                .filter(|label| !label.is_empty())
                .collect();
            labels.sort();
            labels.dedup();
            filters.insert("tag".into(), labels);
        }

        let mut sort = BTreeMap::new();
        sort.insert("date".into(), item.date.to_rfc3339());

        Self {
            // Relative records are independently portable; both Pagefind's default API and
            // aggr's UI can mount the generated directory beneath any path.
            url: item.url.trim_start_matches('/').to_string(),
            // Markdown syntax, link targets, and raw HTML are intentionally absent from the
            // digest handed to Pagefind. Its generated snippet is always human prose.
            content: crate::content::html_to_text(&crate::content::render_markdown(markdown)),
            meta,
            filters,
            sort,
        }
    }
}

/// Pagefind's API is async and uses blocking workers internally. Site generation is deliberately
/// synchronous, so isolate its small runtime on a thread; this also works when `aggr build` is
/// already running inside the CLI's multithreaded Tokio runtime.
pub fn build(out: &Path, documents: &[SearchDocument], language: &str) -> Result<()> {
    let site = out.to_path_buf();
    let documents = documents.to_vec();
    let language = language.to_string();
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
                        language.clone(),
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

    use crate::site::context::DiscussionLinkCtx;

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
        assert_eq!(document.url, "items/blog/post/");
        assert!(document.content.contains("Human prose about Ferris"));
        assert!(document.content.contains("useful label"));
        assert!(!document.content.contains("https://"));
        assert_eq!(document.meta.len(), 3);
        assert_eq!(document.meta["title"], "Useful result");
        let display: serde_json::Value = serde_json::from_slice(
            &hex::decode(&document.meta["aggr_display"]).expect("hex display metadata"),
        )
        .expect("JSON display metadata");
        assert_eq!(display["original"], item().link);
        assert_eq!(display["domain"], "secret.example");
        assert_eq!(display["source_slug"], "blog");
        assert_eq!(display["excerpt"], "A concise fallback");
        assert_eq!(display["discussions"][0]["name"], "hackernews");
        assert_eq!(document.filters["category"], ["engineering"]);
        assert_eq!(document.filters["tag"], ["rust"]);
    }

    #[test]
    fn writes_pagefind_without_a_whole_corpus_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let document = SearchDocument::new(&item(), "Only this prose is searchable.");
        build(dir.path(), &[document], "fr").unwrap();
        assert!(dir.path().join("pagefind/pagefind.js").is_file());
        assert!(!dir.path().join("search-meta.json").exists());
    }
}
