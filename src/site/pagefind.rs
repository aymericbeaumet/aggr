//! A purpose-built Pagefind index. Only titles and cleaned article prose affect ranking. Display
//! metadata travels in Pagefind's result chunks with zero runtime weight, avoiding a second,
//! whole-corpus request before the first result can render.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context as _, Result};
use pagefind::api::PagefindIndex;
use serde::Serialize;
use sha1::{Digest as _, Sha1};

use super::context::ItemCtx;

const CACHE_NAMESPACE: &str = "pagefind-v1";
const CACHE_KEY: &str = ".aggr-pagefind-key";

#[derive(Debug, Clone, Serialize)]
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
        meta.insert(
            "source".into(),
            format!("{} {} {}", item.source_name, item.source, item.domain),
        );
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
#[cfg(test)]
fn build(out: &Path, documents: &[SearchDocument], language: &str) -> Result<()> {
    build_cached(out, documents, language, None)
}

/// Restore an identical index even when page templates changed. Search content and display
/// metadata are the complete input, so this cache remains independent from rendered HTML.
pub fn build_cached(
    out: &Path,
    documents: &[SearchDocument],
    language: &str,
    cache_root: Option<&Path>,
) -> Result<()> {
    let fingerprint = fingerprint(documents, language)?;
    if let Some(cache_root) = cache_root
        && restore(cache_root, &fingerprint, out)?
    {
        log::debug!("restored Pagefind index from cache");
        return Ok(());
    }
    build_uncached(out, documents, language)?;
    if let Some(cache_root) = cache_root {
        store(cache_root, &fingerprint, out)?;
    }
    Ok(())
}

fn build_uncached(out: &Path, documents: &[SearchDocument], language: &str) -> Result<()> {
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

fn fingerprint(documents: &[SearchDocument], language: &str) -> Result<String> {
    let mut hash = Sha1::new();
    hash.update(b"aggr-pagefind-v1\0");
    hash.update(include_str!("pagefind.rs").as_bytes());
    hash.update([0]);
    hash.update(language.as_bytes());
    for document in documents {
        hash.update([0xff]);
        hash.update(serde_json::to_vec(document).context("fingerprinting Pagefind record")?);
    }
    Ok(hex::encode(hash.finalize()))
}

fn restore(cache_root: &Path, fingerprint: &str, out: &Path) -> Result<bool> {
    let cached = cache_root.join(CACHE_NAMESPACE);
    if std::fs::read_to_string(cached.join(CACHE_KEY))
        .ok()
        .is_none_or(|key| key != fingerprint)
        || !cached.join("site/pagefind.js").is_file()
    {
        return Ok(false);
    }
    let destination = out.join("pagefind");
    if destination.exists() {
        std::fs::remove_dir_all(&destination)
            .with_context(|| format!("clearing {}", destination.display()))?;
    }
    crate::cache::copy_tree(&cached.join("site"), &destination)?;
    Ok(true)
}

fn store(cache_root: &Path, fingerprint: &str, out: &Path) -> Result<()> {
    std::fs::create_dir_all(cache_root)
        .with_context(|| format!("creating {}", cache_root.display()))?;
    let scratch = tempfile::Builder::new()
        .prefix("pagefind-")
        .tempdir_in(cache_root)
        .context("creating Pagefind cache staging directory")?;
    let staged = scratch.path().join(CACHE_NAMESPACE);
    crate::cache::copy_tree(&out.join("pagefind"), &staged.join("site"))?;
    crate::cache::write(&staged.join(CACHE_KEY), fingerprint.as_bytes())?;

    let current = cache_root.join(CACHE_NAMESPACE);
    let previous = cache_root.join(format!(".{CACHE_NAMESPACE}.previous"));
    if previous.exists() {
        std::fs::remove_dir_all(&previous)
            .with_context(|| format!("clearing {}", previous.display()))?;
    }
    if current.exists() {
        std::fs::rename(&current, &previous)
            .with_context(|| format!("moving {} aside", current.display()))?;
    }
    if let Err(error) = std::fs::rename(&staged, &current) {
        if previous.exists() {
            let _ = std::fs::rename(&previous, &current);
        }
        return Err(error).with_context(|| format!("caching Pagefind at {}", current.display()));
    }
    if previous.exists() {
        std::fs::remove_dir_all(&previous)
            .with_context(|| format!("clearing {}", previous.display()))?;
    }
    Ok(())
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
            previous_article: None,
            next_article: None,
            recommended_articles: Vec::new(),
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
        assert_eq!(document.meta.len(), 4);
        assert_eq!(document.meta["title"], "Useful result");
        assert_eq!(document.meta["source"], "Blog blog secret.example");
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

    #[test]
    fn restores_an_unchanged_index_independently() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        let first = dir.path().join("first");
        let second = dir.path().join("second");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        let document = SearchDocument::new(&item(), "Only this prose is searchable.");

        build_cached(&first, std::slice::from_ref(&document), "en", Some(&cache)).unwrap();
        std::fs::write(cache.join(CACHE_NAMESPACE).join("site/proof"), "cached").unwrap();
        build_cached(&second, &[document], "en", Some(&cache)).unwrap();

        assert!(second.join("pagefind/pagefind.js").is_file());
        assert_eq!(
            std::fs::read_to_string(second.join("pagefind/proof")).unwrap(),
            "cached"
        );
    }
}
