//! One rendered HTML page: its metadata (titles, descriptions, schema.org graph) and the template
//! context every theme sees. `Pages` holds what all pages of one build share so list pages and
//! standalone documents render through the same path.

use std::path::Path;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;

use super::context::{BuildCtx, CategoryCtx, ItemCtx, PageCtx, SiteCtx, SourceCtx};
use super::render::Renderer;
use super::{outputs, paginate, relative_root, write};
use crate::content;

/// Archive-wide template values prepared once per build. MiniJinja values retain their prepared
/// representation across renders, so custom article templates still receive the entire archive
/// without serializing it again for every page.
#[derive(Serialize)]
pub(super) struct SharedCtx {
    site: minijinja::Value,
    build: minijinja::Value,
    sources: minijinja::Value,
    categories: minijinja::Value,
    tags: minijinja::Value,
}

impl SharedCtx {
    pub(super) fn new(
        site: &SiteCtx,
        build: &BuildCtx,
        sources: &[SourceCtx],
        categories: &[CategoryCtx],
        tags: &[CategoryCtx],
    ) -> Self {
        Self {
            site: minijinja::Value::from_serialize(site),
            build: minijinja::Value::from_serialize(build),
            sources: minijinja::Value::from_serialize(sources),
            categories: minijinja::Value::from_serialize(categories),
            tags: minijinja::Value::from_serialize(tags),
        }
    }
}

#[derive(Serialize)]
struct Ctx<'a> {
    #[serde(flatten)]
    shared: &'a SharedCtx,
    page: PageCtx,
    items: minijinja::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    item: Option<&'a ItemCtx>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<&'a SourceCtx>,
    #[serde(skip_serializing_if = "Option::is_none")]
    category: Option<&'a CategoryCtx>,
    #[serde(skip_serializing_if = "Option::is_none")]
    schema: Option<serde_json::Value>,
}

/// What every page of one build shares: the site, the renderer, the prepared template values and
/// the whole archive that standalone documents expose as `items`.
pub(super) struct Pages<'a> {
    site: &'a SiteCtx,
    renderer: &'a Renderer,
    shared: SharedCtx,
    archive: &'a [ItemCtx],
    archive_value: minijinja::Value,
    per_page: usize,
}

/// A paginated collection: the river, or one source, category or tag archive.
pub(super) struct ListPage<'a> {
    pub kind: &'a str,
    pub title: &'a str,
    pub prefix: &'a str,
    pub list: &'a [ItemCtx],
    pub source: Option<&'a SourceCtx>,
    pub category: Option<&'a CategoryCtx>,
}

/// A standalone document: directory, utility and article pages, the offline shell and the
/// manifest. `page_items` narrows the `items` a template sees; the default is the whole archive.
pub(super) struct SimplePage<'a> {
    pub kind: &'a str,
    pub title: &'a str,
    pub path: &'a str,
    pub template: &'a str,
    pub item: Option<&'a ItemCtx>,
    pub page_items: Option<&'a [ItemCtx]>,
}

impl<'a> SimplePage<'a> {
    pub(super) fn new(kind: &'a str, title: &'a str, path: &'a str, template: &'a str) -> Self {
        Self {
            kind,
            title,
            path,
            template,
            item: None,
            page_items: None,
        }
    }
}

impl<'a> Pages<'a> {
    pub(super) fn new(
        site: &'a SiteCtx,
        renderer: &'a Renderer,
        shared: SharedCtx,
        archive: &'a [ItemCtx],
        per_page: usize,
    ) -> Self {
        Self {
            site,
            renderer,
            shared,
            archive,
            archive_value: minijinja::Value::from_serialize(archive),
            per_page,
        }
    }

    /// Render every pager of a list to `<out>/<pager>/index.html`, record each page in the
    /// sitemap, and return how many pages were written.
    pub(super) fn write_list(
        &self,
        out: &Path,
        page: ListPage<'_>,
        sitemap_urls: &mut Vec<outputs::SitemapUrl>,
    ) -> Result<usize> {
        let ListPage {
            kind,
            title,
            prefix,
            list,
            source,
            category,
        } = page;
        let site = self.site;
        let pagination = paginate(prefix, list.len(), self.per_page);
        for pager in &pagination {
            let page_number = pager.context.current_index;
            let page = PageCtx {
                kind: kind.to_string(),
                title: title.to_string(),
                document_title: document_title(&site.title, title, kind, page_number),
                description: page_description(site, title, kind, page_number),
                indexable: site.indexing,
                path: pager.path.clone(),
                root: relative_root(&pager.path),
                canonical_url: site.absolute(&pager.path),
                feed_path: Some(prefix.to_string()),
                feed_title: Some(title.to_string()),
                paginator: Some(pager.context.clone()),
            };
            let page_items = &list[pager.range.clone()];
            if let Some(url) = &page.canonical_url {
                sitemap_urls.push(outputs::SitemapUrl::new(
                    url,
                    page_items.iter().map(archive_modified_at).max(),
                ));
            }
            let schema = structured_data(site, &page, page_items, None);
            let html = self.renderer.render(
                "index.html",
                Ctx {
                    shared: &self.shared,
                    page,
                    items: minijinja::Value::from_serialize(page_items),
                    item: None,
                    source,
                    category,
                    schema,
                },
            )?;
            write(&out.join(&pager.path).join("index.html"), html.as_bytes())?;
        }
        Ok(pagination.len())
    }

    /// Render one standalone document.
    pub(super) fn simple(&self, page: SimplePage<'_>) -> Result<String> {
        let SimplePage {
            kind,
            title,
            path,
            template,
            item,
            page_items,
        } = page;
        let site = self.site;
        let page_number = 1;
        let utility_fallback = matches!(kind, "404" | "offline");
        // Error, offline and manifest documents are not reading pages; everything else
        // advertises the root feeds so an article page is enough to subscribe from.
        let advertises_feeds = !matches!(kind, "404" | "offline" | "manifest");
        let page = PageCtx {
            kind: kind.to_string(),
            title: title.to_string(),
            document_title: document_title(&site.title, title, kind, page_number),
            description: item.map_or_else(
                || page_description(site, title, kind, page_number),
                |item| item_description(site, item),
            ),
            indexable: site.indexing && !matches!(kind, "preferences" | "404" | "offline"),
            path: path.to_string(),
            // These documents may be served for an arbitrarily deep failed navigation. An
            // explicit scoped root keeps every asset and menu link inside the installed app.
            root: if utility_fallback {
                site.base_path.clone()
            } else {
                relative_root(path)
            },
            canonical_url: (!utility_fallback).then(|| site.absolute(path)).flatten(),
            feed_path: advertises_feeds.then(String::new),
            feed_title: advertises_feeds.then(|| site.title.clone()),
            paginator: None,
        };
        let items = page_items.unwrap_or(self.archive);
        let schema = (!utility_fallback)
            .then(|| structured_data(site, &page, items, item))
            .flatten();
        self.renderer.render(
            template,
            Ctx {
                shared: &self.shared,
                page,
                items: page_items
                    .map(minijinja::Value::from_serialize)
                    .unwrap_or_else(|| self.archive_value.clone()),
                item,
                source: None,
                category: None,
                schema,
            },
        )
    }
}

pub(super) fn default_site_description(title: &str) -> String {
    format!(
        "Browse {title}, an independent, searchable archive of readable snapshots from followed feeds, preserved in Git with aggr."
    )
}

fn document_title(site_title: &str, page_title: &str, kind: &str, page_number: usize) -> String {
    let page_title = if matches!(kind, "item" | "source") {
        page_title.to_string()
    } else {
        page_title.to_lowercase()
    };
    match (kind, page_number) {
        ("river", 1) => format!("{site_title} | aggr"),
        ("river", page) => format!("{site_title} — page {page} | aggr"),
        (_, 1) => format!("{page_title} | {site_title} | aggr"),
        (_, page) => format!("{page_title} — page {page} | {site_title} | aggr"),
    }
}

fn item_description(site: &SiteCtx, item: &ItemCtx) -> String {
    let source = if item.domain.is_empty() {
        item.source_name.as_str()
    } else {
        item.domain.as_str()
    };
    let context = format!(
        "Archived readable snapshot of {} from {source}, first captured {} and preserved by {}.",
        item.title,
        item.first_seen.format("%Y-%m-%d"),
        site.title
    );
    if item.excerpt.is_empty() {
        context
    } else {
        content::excerpt(&format!("{context} {}", item.excerpt), 220)
    }
}

/// Sitemap freshness belongs to the local archive page. A newly captured old article must not
/// look stale merely because its upstream publication date is old.
pub(super) fn archive_modified_at(item: &ItemCtx) -> DateTime<Utc> {
    item.published
        .into_iter()
        .chain(item.updated)
        .chain(std::iter::once(item.first_seen))
        .chain(item.replicated_at)
        .max()
        .unwrap_or(item.first_seen)
}

fn page_description(site: &SiteCtx, page_title: &str, kind: &str, page_number: usize) -> String {
    let description = match kind {
        "river" => site.description.clone(),
        "source" => format!(
            "Browse retained readable snapshots from {page_title} in {}, with original URLs and capture dates.",
            site.title
        ),
        "category" => format!(
            "Browse retained readable snapshots in the {page_title} category of {}.",
            site.title
        ),
        "tag" => format!(
            "Browse retained readable snapshots tagged {page_title} in {}.",
            site.title
        ),
        "browse" => format!(
            "Browse the sources, categories, and tags collected in {}, with links to every retained archive.",
            site.title
        ),
        _ => site.description.clone(),
    };
    if page_number > 1 {
        format!("{} Page {page_number}.", description.trim_end_matches('.'))
    } else {
        description
    }
}

/// Schema.org metadata describes the independent archive page and its upstream provenance, so it
/// is emitted only when the build has a stable public URL. Internal navigation stays portable.
fn structured_data(
    site: &SiteCtx,
    page: &PageCtx,
    items: &[ItemCtx],
    item: Option<&ItemCtx>,
) -> Option<serde_json::Value> {
    let site_url = site.base_url.as_deref()?;
    let page_url = page.canonical_url.as_deref()?;
    let website_id = format!("{site_url}#website");
    let descriptor_url = format!("{site_url}aggr.json");
    let config_url = site
        .config_url
        .clone()
        .unwrap_or_else(|| format!("{site_url}aggr.toml"));
    let identity_id = site
        .identity
        .as_ref()
        .map(|_| format!("{site_url}#identity"));
    let mut website = serde_json::json!({
        "@type": "WebSite",
        "@id": website_id,
        "additionalType": outputs::AGGR_INSTANCE_TYPE,
        "url": site_url,
        "name": site.title,
        "description": site.description,
        "inLanguage": site.language,
        "isPartOf": {"@id": outputs::AGGR_NETWORK},
        "isBasedOn": config_url,
        "subjectOf": {
            "@type": "DigitalDocument",
            "url": descriptor_url,
            "encodingFormat": "application/json"
        },
        "potentialAction": {
            "@type": "SearchAction",
            "target": {
                "@type": "EntryPoint",
                "urlTemplate": format!("{site_url}?q={{search_term_string}}")
            },
            "query-input": "required name=search_term_string"
        }
    });
    if let Some(identity_id) = &identity_id {
        website["creator"] = serde_json::json!({"@id": identity_id});
        website["publisher"] = serde_json::json!({"@id": identity_id});
    }

    let (page_node, original_node) = if let Some(item) = item {
        let snapshot_id = format!("{page_url}#webpage");
        let authors: Vec<_> = item
            .authors
            .iter()
            .map(|name| serde_json::json!({"@type": "Person", "name": name}))
            .collect();
        let mut original = serde_json::json!({
            "@type": "CreativeWork",
            "@id": item.link,
            "url": item.link,
            "headline": item.title,
            "description": item.excerpt,
            "inLanguage": site.language,
            "archivedAt": {"@id": snapshot_id},
            "keywords": item.labels,
        });
        if item.word_count > 0 {
            original["wordCount"] = serde_json::json!(item.word_count);
            original["timeRequired"] =
                serde_json::Value::String(format!("PT{}M", item.reading_minutes));
        }
        if let Some(published) = item.published {
            original["datePublished"] = serde_json::Value::String(published.to_rfc3339());
        }
        if let Some(updated) = item.updated {
            original["dateModified"] = serde_json::Value::String(updated.to_rfc3339());
        }
        if let Some(category) = &item.category {
            original["genre"] = serde_json::Value::String(category.clone());
        }
        if !authors.is_empty() {
            original["author"] = serde_json::Value::Array(authors);
        }
        if let Some(preview) = &item.preview {
            original["image"] = serde_json::json!({
                "@type": "ImageObject",
                "url": format!("{site_url}{}", preview.url),
                "width": preview.width,
                "height": preview.height,
                "caption": preview.alt,
            });
        }
        (
            serde_json::json!({
                "@type": ["WebPage", "ArchiveComponent"],
                "@id": snapshot_id,
                "url": page_url,
                "name": format!("Archived snapshot: {}", item.title),
                "description": page.description,
                "dateCreated": item.replicated_at.unwrap_or(item.first_seen).to_rfc3339(),
                "temporalCoverage": item.first_seen.to_rfc3339(),
                "inLanguage": site.language,
                "isPartOf": {"@id": website_id},
                "isBasedOn": {"@id": item.link},
                "mainEntity": {"@id": item.link},
            }),
            Some(original),
        )
    } else {
        let page_type = match page.kind.as_str() {
            "river" | "source" | "category" | "tag" | "browse" => "CollectionPage",
            _ => "WebPage",
        };
        let mut node = serde_json::json!({
            "@type": page_type,
            "@id": format!("{page_url}#webpage"),
            "url": page_url,
            "name": page.title,
            "description": page.description,
            "inLanguage": site.language,
            "isPartOf": {"@id": website_id},
        });
        if page.paginator.is_some() {
            let offset = page
                .paginator
                .as_ref()
                .map_or(0, |paginator| paginator.offset);
            let elements: Vec<_> = items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    let url = format!("{site_url}{}", item.url);
                    serde_json::json!({
                        "@type": "DataFeedItem",
                        "position": offset + index + 1,
                        "dateCreated": item.replicated_at.unwrap_or(item.first_seen).to_rfc3339(),
                        "item": {
                            "@type": ["WebPage", "ArchiveComponent"],
                            "@id": format!("{url}#webpage"),
                            "url": url,
                            "name": format!("Archived snapshot: {}", item.title),
                            "dateCreated": item.replicated_at.unwrap_or(item.first_seen).to_rfc3339(),
                            "temporalCoverage": item.first_seen.to_rfc3339(),
                            "isBasedOn": {"@id": item.link},
                        }
                    })
                })
                .collect();
            node["mainEntity"] = serde_json::json!({
                "@type": "DataFeed",
                "@id": format!("{page_url}#feed"),
                "name": page.title,
                "dataFeedElement": elements,
            });
        }
        (node, None)
    };

    let network = serde_json::json!({
        "@type": "CreativeWorkSeries",
        "@id": outputs::AGGR_NETWORK,
        "name": "aggr network",
        "url": outputs::AGGR_REPOSITORY,
    });

    let identity = site
        .identity
        .as_ref()
        .zip(identity_id)
        .map(|(identity, id)| {
            let mut node = serde_json::json!({
                "@type": identity.kind,
                "@id": id,
                "name": identity.name,
            });
            if let Some(url) = &identity.url {
                node["url"] = serde_json::Value::String(url.clone());
            }
            if !identity.same_as.is_empty() {
                node["sameAs"] = serde_json::json!(identity.same_as);
            }
            node
        });

    let mut graph = vec![network];
    if let Some(identity) = identity {
        graph.push(identity);
    }
    graph.push(website);
    graph.push(page_node);
    if let Some(original_node) = original_node {
        graph.push(original_node);
    }

    Some(serde_json::json!({
        "@context": "https://schema.org",
        "@graph": graph,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Item;
    use crate::site::context::{self, ArticlePreviewCtx, ItemOptions};
    use crate::site::render::Layers;
    use chrono::TimeZone;

    fn day(d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, d, 0, 0, 0).unwrap()
    }

    #[test]
    fn prepared_context_preserves_custom_theme_archive_access_without_reserializing() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Counted<'a>(&'a AtomicUsize);
        impl Serialize for Counted<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                self.0.fetch_add(1, Ordering::Relaxed);
                serde_json::json!({"title":"A <story>","preview":{"width":320}})
                    .serialize(serializer)
            }
        }
        let visits = AtomicUsize::new(0);
        let items = minijinja::Value::from_serialize([Counted(&visits), Counted(&visits)]);
        let shared = SharedCtx {
            site: minijinja::context!(title => "Reader"),
            build: minijinja::context!(version => "1"),
            sources: minijinja::Value::from_serialize([serde_json::json!({"name":"Publisher"})]),
            categories: minijinja::Value::from_serialize([serde_json::json!({"name":"news"})]),
            tags: minijinja::Value::from_serialize(["rust"]),
        };
        let mut env = minijinja::Environment::new();
        env.add_template("custom.html", "{{ site.title }} {{ build.version }} {{ sources[0].name }} {{ categories[0].name }} {{ tags[0] }} {{ items|length }} {{ items[0].title }} {{ items[1].preview.width }} {{ page.title }} {{ item is defined }}").unwrap();
        for _ in 0..20 {
            let page = PageCtx {
                kind: "item".into(),
                title: "Page".into(),
                document_title: "Page".into(),
                description: String::new(),
                indexable: true,
                path: "items/test/".into(),
                root: "../../".into(),
                canonical_url: None,
                feed_path: None,
                feed_title: None,
                paginator: None,
            };
            let ctx = Ctx {
                shared: &shared,
                page,
                items: items.clone(),
                item: None,
                source: None,
                category: None,
                schema: None,
            };
            assert_eq!(
                env.get_template("custom.html")
                    .unwrap()
                    .render(ctx)
                    .unwrap(),
                "Reader 1 Publisher news rust 2 A &lt;story&gt; 320 Page False"
            );
        }
        assert_eq!(visits.load(Ordering::Relaxed), 2);
    }

    #[test]
    #[ignore = "manual template preparation performance comparison"]
    fn benchmark_prepared_template_context() {
        let records: Vec<_> = (0..1000)
            .map(|index| {
                serde_json::json!({
                    "title":format!("Article {index}"), "source":"publisher", "date":"2026-09-01",
                    "url":format!("items/publisher/article-{index}/"), "labels":["reading","rust"],
                    "preview":{"url":"assets/preview.jpg","width":320,"height":180},
                    "next_article":{"title":"Next article","url":"items/next/"},
                })
            })
            .collect();
        #[derive(Serialize)]
        struct Raw<'a> {
            items: &'a [serde_json::Value],
        }
        #[derive(Serialize)]
        struct Prepared {
            items: minijinja::Value,
        }
        let mut env = minijinja::Environment::new();
        env.add_template(
            "item.html",
            "{{ items|length }} {{ items[0].title }} {{ items[999].title }}",
        )
        .unwrap();
        let template = env.get_template("item.html").unwrap();
        let started = std::time::Instant::now();
        for _ in 0..1000 {
            assert_eq!(
                template.render(Raw { items: &records }).unwrap(),
                "1000 Article 0 Article 999"
            );
        }
        let repeated = started.elapsed();
        let started = std::time::Instant::now();
        let prepared = Prepared {
            items: minijinja::Value::from_serialize(&records),
        };
        for _ in 0..1000 {
            assert_eq!(
                template.render(&prepared).unwrap(),
                "1000 Article 0 Article 999"
            );
        }
        println!(
            "1000 pages × 1000 archive entries: repeated={repeated:?}, prepared={:?}",
            started.elapsed()
        );
    }

    #[test]
    fn document_titles_keep_page_site_and_aggr_identity_consistent() {
        for (kind, page, title, expected) in [
            ("river", 1, "Reader", "Reader | aggr"),
            ("river", 2, "Reader", "Reader — page 2 | aggr"),
            ("search", 1, "Search", "search | Reader | aggr"),
            ("browse", 1, "Browse", "browse | Reader | aggr"),
            (
                "preferences",
                1,
                "Preferences",
                "preferences | Reader | aggr",
            ),
            ("source", 3, "Example", "Example — page 3 | Reader | aggr"),
            ("category", 1, "Engineering", "engineering | Reader | aggr"),
            ("tag", 1, "Rust", "rust | Reader | aggr"),
            ("item", 1, "Article Title", "Article Title | Reader | aggr"),
            ("404", 1, "Not found", "not found | Reader | aggr"),
            ("offline", 1, "Offline", "offline | Reader | aggr"),
        ] {
            assert_eq!(
                document_title("Reader", title, kind, page),
                expected,
                "{kind}"
            );
        }
    }

    #[test]
    fn social_cards_prefer_the_article_lead_image() {
        let now = day(20);
        let item = Item {
            path: "items/blog/2026/09/2026-09-02-lead".into(),
            front: crate::model::FrontMatter {
                title: "Lead".into(),
                link: "https://blog.example/lead".into(),
                source: "blog".into(),
                first_seen: now,
                ..Default::default()
            },
            body: "Body".into(),
        };
        let mut ctx = ItemCtx::from_item(
            &item,
            ItemOptions {
                reading_metrics: content::reading_metrics(&item.body),
                source_name: "Blog",
                category: None,
                links: None,
                excerpt: String::new(),
                discussions: &[],
                resolutions: &crate::discussions::ResolutionSet::default(),
                now,
            },
        );
        ctx.body_html = Some("<p>Body</p>".into());
        let placeholder =
            crate::media::placeholder::from_image(&image::DynamicImage::new_rgb8(4, 4)).unwrap();
        ctx.preview = Some(context::PreviewCtx {
            url: "assets/previews/small.jpg".into(),
            width: 320,
            height: 180,
            alt: Some("A small card".into()),
            color: Some("#285a8c".into()),
            placeholder: placeholder.clone(),
        });
        let lead = ArticlePreviewCtx {
            url: "assets/images/lead.jpg".into(),
            width: 1200,
            height: 630,
            alt: None,
            srcset: "assets/images/lead.jpg 1200w".into(),
            color: "#285a8c".into(),
            placeholder,
        };
        let render = |item: &ItemCtx, base_url: Option<&str>| {
            let site = SiteCtx {
                title: "Reader".into(),
                description: String::new(),
                identity: None,
                language: "en-GB".into(),
                og_locale: context::og_locale("en-GB"),
                base_path: "/".into(),
                base_url: base_url.map(str::to_string),
                indexing: true,
                repository: None,
                data_branch: "aggr".into(),
                network_url: outputs::AGGR_NETWORK,
                instance_type_url: outputs::AGGR_INSTANCE_TYPE,
                pwa: false,
                preferences: serde_json::json!({}),
                preference_schema: crate::config::preferences::ReaderPreferences::default()
                    .schema(),
                config_page_url: None,
                config_url: None,
                has_categories: false,
                discussions: Vec::new(),
                entry_shortcuts: Vec::new(),
                params: toml::Table::new(),
            };
            let build = BuildCtx {
                time: now,
                version: "1".into(),
                app_version: "app".into(),
                content_version: "content".into(),
                config_sha: None,
                data_sha: None,
                generation: "g".into(),
                release: false,
            };
            let shared = SharedCtx {
                site: minijinja::Value::from_serialize(&site),
                build: minijinja::Value::from_serialize(&build),
                sources: minijinja::Value::from_serialize(Vec::<SourceCtx>::new()),
                categories: minijinja::Value::from_serialize(Vec::<CategoryCtx>::new()),
                tags: minijinja::Value::from_serialize(Vec::<CategoryCtx>::new()),
            };
            let page = PageCtx {
                kind: "item".into(),
                title: item.title.clone(),
                document_title: item.title.clone(),
                description: String::new(),
                indexable: true,
                path: item.url.clone(),
                root: "../../".into(),
                canonical_url: None,
                feed_path: Some(String::new()),
                feed_title: Some(site.title.clone()),
                paginator: None,
            };
            let html = Renderer::new(Layers::default())
                .unwrap()
                .render(
                    "item.html",
                    Ctx {
                        shared: &shared,
                        page,
                        items: minijinja::Value::from_serialize(Vec::<ItemCtx>::new()),
                        item: Some(item),
                        source: None,
                        category: None,
                        schema: None,
                    },
                )
                .unwrap();
            scraper::Html::parse_document(&html)
        };
        let meta = |document: &scraper::Html, selector: &str| {
            document
                .select(&scraper::Selector::parse(selector).unwrap())
                .next()
                .and_then(|meta| meta.value().attr("content").map(str::to_string))
        };

        let card = render(&ctx, Some("https://reader.test/"));
        assert_eq!(
            meta(&card, "meta[property=\"og:image\"]").as_deref(),
            Some("https://reader.test/assets/previews/small.jpg")
        );
        assert_eq!(
            meta(&card, "meta[property=\"og:image:width\"]").as_deref(),
            Some("320")
        );
        assert_eq!(
            meta(&card, "meta[name=\"twitter:card\"]").as_deref(),
            Some("summary")
        );
        assert_eq!(
            meta(&card, "meta[property=\"og:locale\"]").as_deref(),
            Some("en_GB")
        );

        ctx.article_preview = Some(lead);
        let large = render(&ctx, Some("https://reader.test/"));
        assert_eq!(
            meta(&large, "meta[property=\"og:image\"]").as_deref(),
            Some("https://reader.test/assets/images/lead.jpg")
        );
        assert_eq!(
            meta(&large, "meta[property=\"og:image:width\"]").as_deref(),
            Some("1200")
        );
        assert_eq!(
            meta(&large, "meta[property=\"og:image:height\"]").as_deref(),
            Some("630")
        );
        assert_eq!(
            meta(&large, "meta[property=\"og:image:alt\"]").as_deref(),
            Some("A small card")
        );
        assert_eq!(
            meta(&large, "meta[name=\"twitter:image\"]").as_deref(),
            Some("https://reader.test/assets/images/lead.jpg")
        );
        assert_eq!(
            meta(&large, "meta[name=\"twitter:card\"]").as_deref(),
            Some("summary_large_image")
        );

        let portable = render(&ctx, None);
        assert_eq!(meta(&portable, "meta[property=\"og:image\"]"), None);
        assert_eq!(
            meta(&portable, "meta[name=\"twitter:card\"]").as_deref(),
            Some("summary"),
            "a large card needs an absolute image URL"
        );
    }
}
