//! A purpose-built Pagefind index. Titles, exact/normalized original URLs, and cleaned article
//! prose are searchable. Display metadata travels in result chunks with zero runtime weight,
//! avoiding a second whole-corpus request before the first result can render.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context as _, Result};
use pagefind::api::PagefindIndex;
use serde::Serialize;
use sha1::{Digest as _, Sha1};
use sha2::{Digest as _, Sha256};

use super::context::ItemCtx;
use crate::cache::Namespace;

const CACHE_KEY: &str = ".aggr-pagefind-key";

#[derive(Debug, Clone, Serialize)]
pub struct SearchDocument {
    pub url: String,
    pub content: String,
    pub meta: BTreeMap<String, String>,
    pub filters: BTreeMap<String, Vec<String>>,
    pub sort: BTreeMap<String, String>,
    pub facet_labels: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchFacet {
    pub value: String,
    pub label: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize)]
struct IndexFile {
    pub url: String,
    pub size: u64,
    pub digest: String,
}

/// What the reader needs to reach the index: its immutable base and the facet vocabulary.
#[derive(Debug, Clone, Serialize)]
pub struct SearchCatalogue {
    pub version: String,
    pub base: String,
    pub docs: usize,
    pub facets: BTreeMap<String, Vec<SearchFacet>>,
}

#[derive(Serialize)]
struct SearchDisplay<'a> {
    #[serde(flatten)]
    metadata: super::display::Metadata,
    excerpt: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    preview: Option<&'a super::context::PreviewCtx>,
}

impl SearchDocument {
    pub fn new(item: &ItemCtx, prose: &str) -> Self {
        let mut meta = BTreeMap::new();
        meta.insert("title".into(), item.title.clone());
        meta.insert(
            "source".into(),
            format!("{} {} {}", item.source_name, item.source, item.domain),
        );
        meta.insert("date".into(), item.date.to_rfc3339());
        // Pagefind tokenizes every metadata value, including fields ranked at zero. Keep display
        // data opaque so provider names, JSON keys and preview URLs cannot become search terms.
        meta.insert(
            "aggr_display".into(),
            serde_json::to_vec(&SearchDisplay {
                metadata: super::display::Metadata::from(item),
                excerpt: &item.excerpt,
                preview: item.preview.as_ref(),
            })
            .map(hex::encode)
            .unwrap_or_default(),
        );

        let mut filters = BTreeMap::new();
        let day = item.date.format("%Y-%m-%d").to_string();
        filters.insert(
            "source".into(),
            item.source_memberships
                .iter()
                .map(|source| source.slug.clone())
                .collect(),
        );
        filters.insert("type".into(), vec![item.item_type.as_str().into()]);
        filters.insert("published-day".into(), vec![day.clone()]);
        let mut facet_labels = BTreeMap::from([
            (
                "source".into(),
                item.source_memberships
                    .iter()
                    .map(|source| (source.slug.clone(), source.name.clone()))
                    .collect(),
            ),
            ("published-day".into(), BTreeMap::from([(day.clone(), day)])),
        ]);
        if let Some(category) = &item.category {
            facet_labels.insert(
                "category".into(),
                BTreeMap::from([(super::context::category_slug(category), category.clone())]),
            );
            filters.insert(
                "category".into(),
                vec![super::context::category_slug(category)],
            );
        }
        if !item.labels.is_empty() {
            facet_labels.insert(
                "tag".into(),
                item.labels
                    .iter()
                    .map(|label| (super::context::category_slug(label), label.clone()))
                    .collect(),
            );
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

        let normalized = crate::model::normalize_link(&item.link);
        let lookup = if normalized == item.link {
            format!("Archived original URL: {}", item.link)
        } else {
            format!(
                "Archived original URL: {}\nNormalized URL alias: {normalized}",
                item.link
            )
        };

        Self {
            // Relative records are independently portable; both Pagefind's default API and
            // aggr's UI can mount the generated directory beneath any path.
            url: item.url.trim_start_matches('/').to_string(),
            // Embedded link targets and raw HTML stay out of the digest. The one intentional URL
            // is upstream identity, allowing a pasted article URL to find its local snapshot.
            content: format!("{lookup}\n\n{prose}"),
            meta,
            filters,
            sort,
            facet_labels,
        }
    }
}

/// Pagefind's API is async and uses blocking workers internally. Site generation is deliberately
/// synchronous, so isolate its small runtime on a thread; this also works when `aggr build` is
/// already running inside the CLI's multithreaded Tokio runtime.
#[cfg(test)]
fn build(out: &Path, documents: &[SearchDocument], language: &str) -> Result<SearchCatalogue> {
    build_cached(out, documents, language, None)
}

/// Restore an identical index even when page templates changed. Search content and display
/// metadata are the complete input, so this cache remains independent from rendered HTML.
pub fn build_cached(
    out: &Path,
    documents: &[SearchDocument],
    language: &str,
    cache_root: Option<&Path>,
) -> Result<SearchCatalogue> {
    let fingerprint = fingerprint(documents, language)?;
    if let Some(cache_root) = cache_root
        && restore(cache_root, &fingerprint, out)?
    {
        log::debug!("restored Pagefind index from cache");
        return publish(out, documents);
    }
    build_uncached(out, documents, language)?;
    if let Some(cache_root) = cache_root {
        store(cache_root, &fingerprint, out)?;
    }
    publish(out, documents)
}

fn publish(out: &Path, documents: &[SearchDocument]) -> Result<SearchCatalogue> {
    let mut facets: BTreeMap<String, BTreeMap<String, SearchFacet>> = BTreeMap::new();
    for document in documents {
        for (kind, values) in &document.filters {
            for value in values {
                let entry = facets
                    .entry(kind.clone())
                    .or_default()
                    .entry(value.clone())
                    .or_insert_with(|| SearchFacet {
                        value: value.clone(),
                        label: document
                            .facet_labels
                            .get(kind)
                            .and_then(|labels| labels.get(value))
                            .cloned()
                            .unwrap_or_else(|| value.clone()),
                        count: 0,
                    });
                entry.count += 1;
            }
        }
    }
    let facets: BTreeMap<_, Vec<_>> = facets
        .into_iter()
        .map(|(kind, values)| (kind, values.into_values().collect()))
        .collect();
    let root = out.join("pagefind");
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(&root).sort_by_file_name() {
        let entry = entry.context("enumerating search index")?;
        if !entry.file_type().is_file() {
            continue;
        }
        let bytes = std::fs::read(entry.path()).context("reading search index resource")?;
        files.push(IndexFile {
            url: entry
                .path()
                .strip_prefix(&root)?
                .to_string_lossy()
                .replace('\\', "/"),
            size: bytes.len() as u64,
            digest: hex::encode(Sha256::digest(&bytes)),
        });
    }
    let version = hex::encode(Sha256::digest(serde_json::to_vec(&(
        &files,
        &facets,
        documents.len(),
    ))?));
    let base = format!("pagefind/{version}/");
    let staging = tempfile::Builder::new()
        .prefix(".pagefind-publish-")
        .tempdir_in(out)
        .context("staging versioned search publication")?;
    let staged_index = staging.path().join("index");
    std::fs::rename(&root, &staged_index)
        .context("moving search index into publication staging")?;
    std::fs::create_dir_all(&root).context("creating versioned search directory")?;
    std::fs::rename(&staged_index, out.join(&base))
        .context("publishing immutable search resources")?;
    let catalogue = SearchCatalogue {
        version,
        base,
        docs: documents.len(),
        facets,
    };
    // Network-first at the site root, so a cached page always discovers the live index version
    // rather than trusting one baked into its HTML.
    crate::cache::write(
        &out.join("search-catalog.json"),
        &serde_json::to_vec(&catalogue)?,
    )?;
    Ok(catalogue)
}

fn build_uncached(out: &Path, documents: &[SearchDocument], language: &str) -> Result<()> {
    let site = out.to_path_buf();
    let output = site.join("pagefind");
    if output.exists() {
        std::fs::remove_dir_all(&output).context("clearing previous search output")?;
    }
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
    let cached = Namespace::Pagefind.dir(cache_root);
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
    let staged = Namespace::Pagefind.dir(scratch.path());
    crate::cache::copy_tree(&out.join("pagefind"), &staged.join("site"))?;
    crate::cache::write(&staged.join(CACHE_KEY), fingerprint.as_bytes())?;

    let current = Namespace::Pagefind.dir(cache_root);
    let previous = cache_root.join(format!(".{}.previous", Namespace::Pagefind.dir_name()));
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
            publisher_source: "blog".into(),
            source_memberships: vec![crate::site::context::SourceMembershipCtx {
                query_value: "blog".into(),
                slug: "blog".into(),
                name: "Blog".into(),
                display: "secret.example".into(),
            }],
            source_name: "Blog".into(),
            source_display: "secret.example".into(),
            source_title: "Blog".into(),
            source_url: "https://secret.example/".into(),
            feed_display: "secret.example".into(),
            is_aggregated: false,
            is_youtube: false,
            language: None,
            category: Some("engineering".into()),
            date: Utc.with_ymd_and_hms(2026, 9, 3, 10, 0, 0).unwrap(),
            age_band: "h1",
            published: None,
            updated: None,
            first_seen: Utc.with_ymd_and_hms(2026, 9, 3, 10, 0, 0).unwrap(),
            replicated_at: None,
            authors: vec!["Person".into()],
            labels: vec!["rust".into()],
            resources: Vec::new(),
            discussions: vec![DiscussionLinkCtx {
                name: "hackernews".into(),
                url: "https://news.ycombinator.com/item?id=42".into(),
                shortcut: None,
                found: true,
                score: Some(12),
            }],
            summary: Some("A concise fallback".into()),
            excerpt: "A concise fallback".into(),
            content: crate::model::ContentKind::Extracted,
            word_count: 0,
            reading_minutes: 0,
            preview: None,
            article_preview: None,
            video: None,
            document: None,
            interactive: None,
            native_media: None,
            item_type: crate::site::item_type::ItemType::Article,
            extra: BTreeMap::new(),
            metadata: super::super::display::Metadata::default(),
            permalink: None,
            raw_url: None,
            history_url: None,
            edit_url: None,
            previous_article: None,
            next_article: None,
            recommended_articles: Vec::new(),
            body_html: None,
            has_margin_notes: false,
        }
    }

    fn document(item: &ItemCtx, markdown: &str) -> SearchDocument {
        SearchDocument::new(
            item,
            crate::content::PreparedMarkdown::new(markdown).plain_text(),
        )
    }

    #[test]
    fn display_queries_are_readable_while_index_filters_keep_stable_source_ids() {
        let mut item = item();
        item.source_memberships[0].query_value = "Publisher display".into();
        let document = SearchDocument::new(&item, "Article text");
        assert_eq!(document.filters["source"], vec!["blog"]);
        let opaque = hex::decode(&document.meta["aggr_display"]).unwrap();
        let display: serde_json::Value = serde_json::from_slice(&opaque).unwrap();
        assert_eq!(display["source_slug"], "blog");
        assert_eq!(display["source_query"], "Publisher display");
    }

    #[test]
    fn searchable_record_indexes_original_url_but_excludes_embedded_link_targets() {
        let document = document(
            &item(),
            "Human prose about Ferris. [useful label](https://noise.example/hidden)",
        );
        assert_eq!(document.url, "items/blog/post/");
        assert!(document.content.contains("Human prose about Ferris"));
        assert!(document.content.contains("useful label"));
        assert!(document.content.contains(&item().link));
        assert!(
            document
                .content
                .contains("https://secret.example/path?token=noise")
        );
        assert!(!document.content.contains("https://noise.example/hidden"));
        assert_eq!(document.meta.len(), 4);
        assert_eq!(document.meta["title"], "Useful result");
        assert_eq!(document.meta["source"], "Blog blog secret.example");
        let display: serde_json::Value = serde_json::from_slice(
            &hex::decode(&document.meta["aggr_display"]).expect("hex display metadata"),
        )
        .expect("JSON display metadata");
        assert_eq!(display["original"], item().link);
        assert_eq!(display["source_display"], "secret.example");
        assert!(display.get("domain").is_none());
        assert!(display.get("source_url").is_none());
        assert!(display.get("feed_display").is_none());
        assert!(display.get("is_aggregated").is_none());
        assert_eq!(display["source_slug"], "blog");
        assert_eq!(display["excerpt"], "A concise fallback");
        assert_eq!(display["discussions"][0]["name"], "hackernews");
        assert_eq!(
            display["discussions"][0]["url"],
            "https://news.ycombinator.com/item?id=42"
        );
        assert_eq!(display["discussions"][0]["score"], 12);
        assert_eq!(document.filters["type"], ["article"]);
        assert_eq!(document.filters["category"], ["engineering"]);
        assert_eq!(document.filters["tag"], ["rust"]);
        assert_eq!(document.filters["source"], ["blog"]);
        assert_eq!(document.filters["published-day"], ["2026-09-03"]);
    }

    #[test]
    fn static_and_search_media_metadata_share_duration_and_unknown_state() {
        let renderer =
            crate::site::render::Renderer::new(crate::site::render::Layers::default()).unwrap();
        for (kind, action) in [
            (super::super::item_type::ItemType::Podcast, "listen"),
            (super::super::item_type::ItemType::Audio, "listen"),
            (super::super::item_type::ItemType::Video, "watch"),
        ] {
            for seconds in [None, Some(3601_u64)] {
                let mut item = item();
                item.item_type = kind;
                item.word_count = 450;
                item.reading_minutes = 2;
                if let Some(seconds) = seconds {
                    item.extra.insert("duration_seconds".into(), seconds.into());
                }
                item.metadata = super::super::display::Metadata::from(&item);
                let document = document(&item, "Transcript prose");
                let display: serde_json::Value =
                    serde_json::from_slice(&hex::decode(&document.meta["aggr_display"]).unwrap())
                        .unwrap();
                assert_eq!(
                    display["consumption"],
                    serde_json::to_value(&item.metadata.consumption).unwrap()
                );
                let rendered = renderer
                    .render("_metadata.html", minijinja::context! { item => item })
                    .unwrap();
                assert!(rendered.contains(&format!("data-consumption=\"{action}\"")));
                assert!(!rendered.contains("min read"));
                assert!(!rendered.contains("450 words"));
                if seconds.is_some() {
                    assert!(
                        rendered.contains(&format!("datetime=\"PT3601S\">61 min {action}</time>"))
                    );
                } else {
                    assert!(!rendered.contains("min listen") && !rendered.contains("min watch"));
                    assert!(rendered.contains(if action == "listen" {
                        ">Listen</span>"
                    } else {
                        ">Watch</span>"
                    }));
                }
            }
        }
    }

    #[test]
    fn static_and_search_share_escaped_metadata_with_optional_counts() {
        let mut item = item();
        item.source_display = "publisher<&>".into();
        item.source_title = "Publisher \"quoted\"".into();
        item.is_aggregated = true;
        item.feed_display = "feed.example/news".into();
        item.source_memberships
            .push(crate::site::context::SourceMembershipCtx {
                query_value: "feed".into(),
                slug: "feed".into(),
                name: "The feed".into(),
                display: "feed.example/news".into(),
            });
        item.extra.insert("points".into(), 0.into());
        item.extra.insert("num_comments".into(), "12".into());
        item.extra.insert(
            "comments_url".into(),
            "https://example.com/comments?a=1&b=2".into(),
        );
        item.metadata = super::super::display::Metadata::from(&item);
        let document = document(&item, "prose");
        let display: serde_json::Value =
            serde_json::from_slice(&hex::decode(&document.meta["aggr_display"]).unwrap()).unwrap();
        let metadata = serde_json::to_value(&item.metadata).unwrap();
        for (key, value) in metadata.as_object().unwrap() {
            assert_eq!(&display[key], value, "shared field {key}");
        }
        assert_eq!(display["points"], 0);
        assert_eq!(display["comments"]["count"], 12);
        assert!(!document.meta["aggr_display"].contains("publisher"));
        assert!(!document.content.contains("comments?a="));

        let renderer =
            crate::site::render::Renderer::new(crate::site::render::Layers::default()).unwrap();
        let rendered = renderer
            .render("_metadata.html", minijinja::context! { item => item })
            .unwrap();
        assert!(rendered.contains("publisher&lt;&amp;&gt;"));
        assert!(rendered.contains("Publisher &quot;quoted&quot;"));
        assert!(rendered.contains("</a> <em>via <a class=\"source-feed\""));
        assert!(rendered.contains("title=\"The feed\">feed.example/news</a></em>"));
        assert!(rendered.contains("0 points</span>"));
        assert!(rendered.contains("12 comments</a>"));
        assert!(rendered.contains("a=1&amp;b=2"));
        assert!(rendered.contains("matching discussion found, score 12"));
        assert!(!rendered.contains(" · "));
        assert!(!rendered.contains("rust</a>"));
    }

    #[test]
    fn invalid_or_missing_optional_metadata_does_not_create_links_or_counts() {
        let mut item = item();
        item.discussions[0].url = "javascript:alert(1)".into();
        item.extra.insert("points".into(), (-1).into());
        item.extra.insert("num_comments".into(), 10.into());
        item.extra
            .insert("comments_url".into(), "javascript:alert(1)".into());
        let metadata = serde_json::to_value(super::super::display::Metadata::from(&item)).unwrap();
        assert!(metadata.get("points").is_none());
        assert!(metadata.get("comments").is_none());
        assert_eq!(metadata["discussions"], serde_json::json!([]));

        item.extra
            .insert("comments_url".into(), "https://example.com/comments".into());
        item.extra.insert("num_comments".into(), "unknown".into());
        item.updated = Some(item.date);
        let metadata = serde_json::to_value(super::super::display::Metadata::from(&item)).unwrap();
        assert!(metadata["comments"].get("count").is_none());
        assert!(metadata.get("updated").is_none());
    }

    #[test]
    fn search_display_does_not_invent_unavailable_discussions() {
        let mut item = item();
        item.discussions[0].found = false;
        let document = document(&item, "Searchable prose.");
        let display: serde_json::Value = serde_json::from_slice(
            &hex::decode(&document.meta["aggr_display"]).expect("hex display metadata"),
        )
        .expect("JSON display metadata");

        assert_eq!(display["discussions"], serde_json::json!([]));
    }

    #[test]
    fn search_display_retains_aggregate_feed_identity() {
        let mut item = item();
        item.is_aggregated = true;
        item.feed_display = "hnrss.org/frontpage".into();
        let document = document(&item, "Searchable prose.");
        let display: serde_json::Value =
            serde_json::from_slice(&hex::decode(&document.meta["aggr_display"]).unwrap()).unwrap();

        assert_eq!(display["is_aggregated"], true);
        assert_eq!(display["feed_display"], "hnrss.org/frontpage");
        assert_eq!(display["source_display"], "secret.example");
    }

    #[test]
    fn search_display_precomputes_category_archive_link() {
        let mut item = item();
        let display = |item: &ItemCtx| {
            let document = document(item, "Searchable prose.");
            serde_json::from_slice::<serde_json::Value>(
                &hex::decode(&document.meta["aggr_display"]).unwrap(),
            )
            .unwrap()
        };
        item.category = Some("rust & friends".into());
        assert_eq!(
            display(&item)["category"],
            serde_json::json!({
                "name": "rust & friends", "slug": "rust-friends"
            })
        );
        item.category = None;
        assert!(display(&item).get("category").is_none());
    }

    #[test]
    fn search_display_includes_update_date_only_when_available() {
        let mut item = item();
        let display = |item: &ItemCtx| {
            let document = document(item, "Searchable prose.");
            serde_json::from_slice::<serde_json::Value>(
                &hex::decode(&document.meta["aggr_display"]).unwrap(),
            )
            .unwrap()
        };
        assert!(display(&item).get("updated").is_none());
        item.updated = Some(Utc.with_ymd_and_hms(2026, 9, 4, 12, 30, 0).unwrap());
        assert_eq!(display(&item)["updated"], "2026-09-04T12:30:00+00:00");
    }

    #[test]
    fn writes_pagefind_without_a_whole_corpus_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let document = document(&item(), "Only this prose is searchable.");
        let manifest = build(dir.path(), &[document], "fr").unwrap();
        assert!(
            dir.path()
                .join(&manifest.base)
                .join("pagefind.js")
                .is_file()
        );
        assert!(!dir.path().join("pagefind/pagefind.js").exists());
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
        let document = document(&item(), "Only this prose is searchable.");

        build_cached(&first, std::slice::from_ref(&document), "en", Some(&cache)).unwrap();
        std::fs::write(Namespace::Pagefind.dir(&cache).join("site/proof"), "cached").unwrap();
        let manifest = build_cached(&second, &[document], "en", Some(&cache)).unwrap();

        assert!(second.join(&manifest.base).join("pagefind.js").is_file());
        assert!(!second.join("pagefind/pagefind.js").exists());
        assert_eq!(
            std::fs::read_to_string(second.join(&manifest.base).join("proof")).unwrap(),
            "cached"
        );
    }

    #[test]
    fn catalogue_versions_the_index_and_counts_facets_without_article_bodies() {
        let dir = tempfile::tempdir().unwrap();
        let stage = |loader: &str| {
            let root = dir.path().join("pagefind");
            if root.exists() {
                std::fs::remove_dir_all(&root).unwrap();
            }
            std::fs::create_dir_all(root.join("fragment")).unwrap();
            std::fs::write(root.join("pagefind.js"), loader).unwrap();
            std::fs::write(root.join("fragment/one.pf_fragment"), "fragment").unwrap();
        };
        stage("loader");
        let mut second = item();
        second.labels.push("Rust & friends".into());
        second.item_type = crate::site::item_type::ItemType::Podcast;
        let documents = [
            document(&item(), "Secret article body"),
            document(&second, "Other article body"),
        ];
        let catalogue = publish(dir.path(), &documents).unwrap();
        assert_eq!(catalogue.docs, 2);
        assert_eq!(catalogue.facets["source"][0].value, "blog");
        assert_eq!(catalogue.facets["source"][0].label, "Blog");
        assert_eq!(catalogue.facets["source"][0].count, 2);
        assert_eq!(catalogue.facets["type"][0].value, "article");
        assert_eq!(catalogue.facets["type"][0].count, 1);
        assert_eq!(catalogue.facets["type"][1].value, "podcast");
        assert_eq!(catalogue.facets["type"][1].count, 1);
        assert!(
            catalogue.facets["tag"]
                .iter()
                .any(|facet| facet.value == "rust-friends"
                    && facet.label == "Rust & friends"
                    && facet.count == 1)
        );

        // The catalogue is vocabulary only: it must never carry article text.
        let json = std::fs::read_to_string(dir.path().join("search-catalog.json")).unwrap();
        assert!(!json.contains("Secret article body"));
        let published: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(published.as_object().unwrap().len(), 4);
        for field in ["version", "base", "docs", "facets"] {
            assert!(published.get(field).is_some(), "{field}");
        }
        // The offline verification manifest went with the offline archive.
        assert!(!dir.path().join("search-manifest.json").exists());

        // The index is republished under an immutable directory, with no unversioned alias.
        assert!(!dir.path().join("pagefind/pagefind.js").exists());
        assert!(!dir.path().join("pagefind/fragment").exists());
        assert_eq!(
            std::fs::read_dir(dir.path().join("pagefind"))
                .unwrap()
                .count(),
            1
        );

        stage("loader");
        let identical = publish(dir.path(), &documents).unwrap();
        assert_eq!(catalogue.version, identical.version);
        stage("new loader");
        let changed = publish(dir.path(), &documents).unwrap();
        assert_ne!(catalogue.version, changed.version);
        assert_eq!(
            std::fs::read_to_string(dir.path().join(&changed.base).join("pagefind.js")).unwrap(),
            "new loader"
        );
        assert!(!dir.path().join("pagefind/pagefind.js").exists());
    }
}
