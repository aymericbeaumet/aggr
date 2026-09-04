//! Static site generation: the data tree + config + git facts in, a directory of files out.
//! The planning half (`plan`) is pure; `build` does the IO.

pub mod context;
pub mod outputs;
mod pagefind;
mod related;
pub mod render;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use sha1::{Digest as _, Sha1};

use crate::config::{Config, SiteConfig, Source};
use crate::content;
use crate::model::Item;
use crate::store::{Status, Store};
use context::{
    ArticleLinkCtx, BuildCtx, CategoryCtx, GitHubLinks, ItemCtx, ItemOptions, PageCtx,
    PaginatorCtx, SiteCtx, SourceCtx, SourceErrorCtx,
};
use render::{Layers, Renderer};

const EXCERPT_CHARS: usize = 240;
const MARKER: &str = ".aggr-site";

/// Facts about the build that do not come from the data tree.
pub struct BuildInfo {
    pub out: PathBuf,
    pub base_url: Option<String>,
    pub config_sha: Option<String>,
    /// Repository-relative path of the root config file.
    pub config_path: Option<String>,
    pub data_sha: Option<String>,
    pub generation: String,
    pub now: DateTime<Utc>,
    /// Production build: absolute URLs from `base_url`, CNAME for custom domains.
    pub release: bool,
    pub discussions: crate::discussions::ResolutionSet,
    /// Search-index cache shared across template-only rebuilds.
    pub pagefind_cache: Option<PathBuf>,
}

/// Which items belong on the recent home feed. `stubbed` is retained for configuration and
/// planning compatibility; archive builds render every retained item as a full page.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Window {
    pub rendered: Vec<usize>,
    pub stubbed: Vec<usize>,
}

/// Split items (sorted newest first, hidden already removed) into the recent feed and its tail.
pub fn window(
    dates: &[DateTime<Utc>],
    now: DateTime<Utc>,
    max_items: usize,
    max_age_days: u32,
    max_stubs: usize,
) -> Window {
    let cutoff = now - Duration::days(i64::from(max_age_days));
    let mut out = Window::default();
    for (index, date) in dates.iter().enumerate() {
        if out.rendered.len() < max_items && *date >= cutoff {
            out.rendered.push(index);
        } else if out.stubbed.len() < max_stubs {
            out.stubbed.push(index);
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pager {
    pub path: String,
    pub range: std::ops::Range<usize>,
    pub context: PaginatorCtx,
}

fn navigation_path(path: &str) -> String {
    if path.is_empty() {
        "./".to_string()
    } else {
        path.to_string()
    }
}

fn canonical_url(site: &SiteCtx, path: &str) -> Option<String> {
    site.base_url
        .as_ref()
        .map(|root| format!("{root}{}", path.trim_start_matches('/')))
}

/// Zola-style pagers for `total` entries: the first lives at `prefix`, then `page/N/`.
pub fn paginate(prefix: &str, total: usize, per_page: usize) -> Vec<Pager> {
    assert!(per_page > 0, "paginate_by must be positive");
    let number_pagers = total.div_ceil(per_page).max(1);
    let paths: Vec<_> = (1..=number_pagers)
        .map(|current_index| {
            if current_index == 1 {
                prefix.to_string()
            } else {
                format!("{prefix}page/{current_index}/")
            }
        })
        .collect();
    let first = navigation_path(&paths[0]);
    let last = navigation_path(paths.last().expect("at least one pager"));
    paths
        .iter()
        .enumerate()
        .map(|(index, path)| {
            let current_index = index + 1;
            let start = index * per_page;
            Pager {
                path: path.clone(),
                range: start..(start + per_page).min(total),
                context: PaginatorCtx {
                    paginate_by: per_page,
                    base_url: format!("{prefix}page/"),
                    number_pagers,
                    first: first.clone(),
                    last: last.clone(),
                    previous: index
                        .checked_sub(1)
                        .map(|previous| navigation_path(&paths[previous])),
                    next: paths.get(index + 1).map(|next| navigation_path(next)),
                    current_index,
                    total_items: total,
                    offset: start,
                },
            }
        })
        .collect()
}

/// Everything the service worker fetches at install, as site paths under `base`: shells, list
/// indexes, the assets needed to render them, the Pagefind bundle, and newest offline items.
pub fn precache_paths(
    base: &str,
    lists: impl IntoIterator<Item = String>,
    assets: &[String],
    item_urls: impl IntoIterator<Item = String>,
    offline_items: usize,
) -> Vec<String> {
    const SHELLS: [&str; 7] = [
        "",
        "sources/",
        "search/",
        "preferences/",
        "404.html",
        "offline.html",
        "manifest.webmanifest",
    ];
    let mut seen = BTreeSet::new();
    SHELLS
        .iter()
        .map(|path| path.to_string())
        .chain(lists)
        .chain(
            assets
                .iter()
                .filter(|name| {
                    name.ends_with(".css") || name.ends_with(".js") || name.starts_with("favicon-")
                })
                .map(|name| format!("assets/{name}")),
        )
        .chain(item_urls.into_iter().take(offline_items))
        .map(|path| format!("{base}{path}"))
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

/// Names the precache from durable build inputs. A no-op rebuild therefore reuses its cache,
/// while a changed worker refreshes the same keys during installation.
pub fn cache_version(build: &BuildCtx) -> String {
    fn short(sha: Option<&str>) -> &str {
        sha.map_or("local", |sha| &sha[..sha.len().min(12)])
    }
    format!(
        "{}-{}-{}-{}",
        build.version,
        short(build.data_sha.as_deref()),
        short(build.config_sha.as_deref()),
        short(Some(&build.generation))
    )
}

/// Fingerprint of data that can affect rendered output, including age bands and the home-feed
/// cutoff. It changes only at a semantic boundary, so repeated builds stay instant while an
/// unchanged repository can never keep a stale 24-hour marker or expired river entry.
pub fn render_generation(items: &[Item], site: &SiteConfig, now: DateTime<Utc>) -> String {
    fn field(hash: &mut Sha1, bytes: &[u8]) {
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    let cutoff = now - Duration::days(i64::from(site.max_age_days));
    let mut ordered: Vec<_> = items.iter().filter(|item| !item.front.hidden).collect();
    ordered.sort_by(|a, b| a.path.cmp(&b.path));
    let mut hash = Sha1::new();
    for item in ordered {
        field(&mut hash, item.path.as_bytes());
        field(
            &mut hash,
            &serde_json::to_vec(&item.front).unwrap_or_default(),
        );
        field(&mut hash, item.body.as_bytes());
        field(
            &mut hash,
            context::age_band(now, item.created_at()).as_bytes(),
        );
        field(&mut hash, &[u8::from(item.created_at() >= cutoff)]);
    }
    hex::encode(hash.finalize())
}

/// `https://user.github.io/repo/` → `/repo/`; anything unparsable → `/`.
pub fn base_path(base_url: Option<&str>) -> String {
    let path = base_url
        .and_then(|url| url::Url::parse(url).ok())
        .map(|url| url.path().to_string())
        .unwrap_or_default();
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        "/".into()
    } else {
        format!("/{trimmed}/")
    }
}

/// Relative reference from a generated page back to the output root. Directory routes end in
/// `/`; standalone root files such as `offline.html` do not add a level.
pub fn relative_root(path: &str) -> String {
    let depth = path
        .trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .count()
        .saturating_sub(usize::from(!path.is_empty() && !path.ends_with('/')));
    if depth == 0 {
        "./".into()
    } else {
        "../".repeat(depth)
    }
}

#[derive(Serialize)]
struct Ctx<'a> {
    site: &'a SiteCtx,
    build: &'a BuildCtx,
    page: PageCtx,
    items: &'a [ItemCtx],
    sources: &'a [SourceCtx],
    categories: &'a [CategoryCtx],
    tags: &'a [CategoryCtx],
    #[serde(skip_serializing_if = "Option::is_none")]
    item: Option<&'a ItemCtx>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<&'a SourceCtx>,
    #[serde(skip_serializing_if = "Option::is_none")]
    category: Option<&'a CategoryCtx>,
    #[serde(skip_serializing_if = "Option::is_none")]
    html: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    schema: Option<serde_json::Value>,
}

/// Schema.org metadata is publication metadata, so it is emitted only when the build has a
/// stable public URL. Internal navigation stays relative and the resulting tree remains movable.
fn structured_data(
    site: &SiteCtx,
    page: &PageCtx,
    items: &[ItemCtx],
    item: Option<&ItemCtx>,
) -> Option<serde_json::Value> {
    let site_url = site.base_url.as_deref()?;
    let page_url = page.canonical_url.as_deref()?;
    let website_id = format!("{site_url}#website");
    let website = serde_json::json!({
        "@type": "WebSite",
        "@id": website_id,
        "url": site_url,
        "name": site.title,
        "description": site.description,
        "inLanguage": site.language,
        "potentialAction": {
            "@type": "SearchAction",
            "target": {
                "@type": "EntryPoint",
                "urlTemplate": format!("{site_url}search/?q={{search_term_string}}")
            },
            "query-input": "required name=search_term_string"
        }
    });

    let page_node = if let Some(item) = item {
        let article_id = format!("{page_url}#article");
        let authors: Vec<_> = item
            .authors
            .iter()
            .map(|name| serde_json::json!({"@type": "Person", "name": name}))
            .collect();
        let mut article = serde_json::json!({
            "@type": "BlogPosting",
            "@id": article_id,
            "url": page_url,
            "headline": item.title,
            "description": item.excerpt,
            "inLanguage": site.language,
            "datePublished": item.published.unwrap_or(item.date).to_rfc3339(),
            "dateModified": item.updated.unwrap_or(item.date).to_rfc3339(),
            "mainEntityOfPage": {"@id": format!("{page_url}#webpage")},
            "isBasedOn": item.link,
            "keywords": item.labels,
        });
        if let Some(category) = &item.category {
            article["articleSection"] = serde_json::Value::String(category.clone());
        }
        if !authors.is_empty() {
            article["author"] = serde_json::Value::Array(authors);
        }
        serde_json::json!({
            "@type": "WebPage",
            "@id": format!("{page_url}#webpage"),
            "url": page_url,
            "name": page.title,
            "description": item.excerpt,
            "inLanguage": site.language,
            "isPartOf": {"@id": website_id},
            "mainEntity": article,
        })
    } else {
        let page_type = match page.kind.as_str() {
            "search" => "SearchResultsPage",
            "river" | "source" | "category" | "tag" | "sources" | "categories" | "tags" => {
                "CollectionPage"
            }
            _ => "WebPage",
        };
        let mut node = serde_json::json!({
            "@type": page_type,
            "@id": format!("{page_url}#webpage"),
            "url": page_url,
            "name": page.title,
            "description": site.description,
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
                        "dateCreated": item.date.to_rfc3339(),
                        "item": {
                            "@type": "BlogPosting",
                            "@id": format!("{url}#article"),
                            "url": url,
                            "headline": item.title,
                            "isBasedOn": item.link,
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
        node
    };

    Some(serde_json::json!({
        "@context": "https://schema.org",
        "@graph": [website, page_node],
    }))
}

/// What `sw.js` sees: the cache name and the install-time fetch list.
#[derive(Serialize)]
struct SwCtx<'a> {
    site: &'a SiteCtx,
    build: &'a BuildCtx,
    version: String,
    precache: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub pages: usize,
    pub items: usize,
    pub stubs: usize,
}

pub fn build(
    config: &Config,
    sources: &[Source],
    store: &Store,
    project_root: &Path,
    info: &BuildInfo,
) -> Result<Summary> {
    let out = &info.out;
    prepare_out_dir(out)?;
    let config_name = info
        .config_path
        .as_deref()
        .and_then(|path| Path::new(path).file_name())
        .unwrap_or_else(|| std::ffi::OsStr::new("aggr.toml"));
    let config_source = project_root.join(config_name);
    if config_source.is_file() {
        write(&out.join("aggr.toml"), &std::fs::read(&config_source)?)?;
    }

    let base = base_path(info.base_url.as_deref());
    let repository = config.repository();
    let description = format!("Latest entries from {}.", config.site.title);
    let mut site = SiteCtx {
        title: config.site.title.clone(),
        description,
        language: config.site.language.clone(),
        base_path: base.clone(),
        base_url: info.base_url.clone().map(|url| ensure_trailing_slash(&url)),
        repository: repository.clone(),
        data_branch: config.store.branch.clone(),
        pwa: config.site.pwa,
        config_url: repository.as_deref().and_then(|repository| {
            info.config_sha.as_deref().map(|sha| {
                format!(
                    "https://raw.githubusercontent.com/{repository}/{sha}/{}",
                    info.config_path.as_deref().unwrap_or("aggr.toml")
                )
            })
        }),
        has_categories: false,
        discussions: config
            .networks
            .iter()
            .map(|d| context::DiscussionLinkCtx {
                name: context::compact_name(&d.name),
                url: d.url.clone(),
                found: false,
                score: None,
            })
            .collect(),
        entry_shortcuts: Vec::new(),
        params: config.site.params.clone(),
    };
    let build_ctx = BuildCtx {
        time: info.now,
        version: env!("CARGO_PKG_VERSION").to_string(),
        config_sha: info.config_sha.clone(),
        data_sha: info.data_sha.clone(),
        generation: info.generation.clone(),
        release: info.release,
    };
    let links = repository.as_deref().map(|repository| GitHubLinks {
        repository,
        branch: &config.store.branch,
        data_sha: info.data_sha.as_deref(),
    });

    let renderer = Renderer::new(theme_layers(config, project_root)?, &base)?;

    // Sources: config order, enriched with stored state and counts.
    let status = store.status()?;
    let mut all_items = store.items()?;
    all_items.retain(|item| !item.front.hidden);
    for item in &mut all_items {
        item.body =
            content::strip_leading_metadata(&item.body, item.front.published, &item.front.source);
    }
    all_items.sort_by(|a, b| {
        b.created_at()
            .cmp(&a.created_at())
            .then_with(|| b.path.cmp(&a.path))
    });
    let source_ctxs = source_contexts(sources, store, &status, &all_items)?;
    let source_by_slug: BTreeMap<&str, &SourceCtx> =
        source_ctxs.iter().map(|s| (s.slug.as_str(), s)).collect();

    let dates: Vec<_> = all_items.iter().map(Item::created_at).collect();
    let window = window(
        &dates,
        info.now,
        config.site.max_items,
        config.site.max_age_days,
        config.site.max_stubs,
    );

    // The bounded window controls only the river. Source/category/tag pages, search, and clean
    // article pages are archives over the retained database; `[store]` retention is the explicit
    // knob for bounding those. This keeps old sources browsable without making the home feed stale.
    let mut archive_items = Vec::with_capacity(all_items.len());
    for item in &all_items {
        let (source_name, category) = source_by_slug
            .get(item.front.source.as_str())
            .map(|s| (s.name.as_str(), s.category.as_deref()))
            .unwrap_or((item.front.source.as_str(), None));
        let excerpt = item
            .front
            .summary
            .clone()
            .map(|s| content::excerpt(&s, EXCERPT_CHARS))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| content::excerpt(&item.body, EXCERPT_CHARS));
        archive_items.push(ItemCtx::from_item(
            item,
            ItemOptions {
                source_name,
                category,
                links: links.as_ref(),
                excerpt,
                discussions: &config.networks,
                resolutions: &info.discussions,
                now: info.now,
            },
        ));
    }

    let recommendations = related::resolve(&archive_items);
    let article_links: Vec<_> = archive_items.iter().map(ArticleLinkCtx::from).collect();
    for (item, recommendation) in archive_items.iter_mut().zip(recommendations) {
        item.previous_article = recommendation
            .previous
            .map(|index| article_links[index].clone());
        item.next_article = recommendation
            .next
            .map(|index| article_links[index].clone());
        item.recommended_articles = recommendation
            .articles
            .into_iter()
            .map(|index| article_links[index].clone())
            .collect();
    }

    let river_items: Vec<ItemCtx> = window
        .rendered
        .iter()
        .map(|&index| archive_items[index].clone())
        .collect();
    site.entry_shortcuts = river_items
        .iter()
        .take(9)
        .map(|item| item.url.clone())
        .collect();
    let stored_by_path: BTreeMap<&str, &Item> = all_items
        .iter()
        .map(|item| (item.path.as_str(), item))
        .collect();

    let source_members = source_members(&archive_items);
    let TaxonomyIndex {
        terms: categories,
        members: category_members,
    } = taxonomy_index(&archive_items, Taxonomy::Categories);
    site.has_categories = !categories.is_empty();
    let TaxonomyIndex {
        terms: tags,
        members: tag_members,
    } = taxonomy_index(&archive_items, Taxonomy::Tags);
    let mut pages = 0;
    let per_page = config.site.items_per_page;
    let mut sitemap_urls: Vec<outputs::SitemapUrl> = Vec::new();
    {
        let mut write_list = |kind: &str,
                              title: &str,
                              prefix: &str,
                              list: &[ItemCtx],
                              source: Option<&SourceCtx>,
                              category: Option<&CategoryCtx>|
         -> Result<()> {
            let pagination = paginate(prefix, list.len(), per_page);
            for pager in &pagination {
                let page = PageCtx {
                    kind: kind.to_string(),
                    title: title.to_string(),
                    path: pager.path.clone(),
                    root: relative_root(&pager.path),
                    canonical_url: canonical_url(&site, &pager.path),
                    feed_path: Some(prefix.to_string()),
                    paginator: Some(pager.context.clone()),
                };
                let page_items = &list[pager.range.clone()];
                if let Some(url) = &page.canonical_url {
                    sitemap_urls.push(outputs::SitemapUrl::new(
                        url,
                        page_items
                            .iter()
                            .map(|item| item.updated.unwrap_or(item.date))
                            .max(),
                    ));
                }
                let schema = structured_data(&site, &page, page_items, None);
                let html = renderer.render(
                    "index.html",
                    Ctx {
                        site: &site,
                        build: &build_ctx,
                        page,
                        items: page_items,
                        sources: &source_ctxs,
                        categories: &categories,
                        tags: &tags,
                        item: None,
                        source,
                        category,
                        html: None,
                        schema,
                    },
                )?;
                write(&out.join(&pager.path).join("index.html"), html.as_bytes())?;
                pages += 1;
            }
            Ok(())
        };

        write_list("river", &config.site.title, "", &river_items, None, None)?;
        for source in &source_ctxs {
            let list = indexed_items(
                &archive_items,
                source_members.get(&source.slug).map(Vec::as_slice),
            );
            write_list(
                "source",
                &source.name,
                &source.page,
                &list,
                Some(source),
                None,
            )?;
            let feed_items = feed_items(&list, &stored_by_path, per_page);
            write_collection_feeds(
                out,
                &site,
                &build_ctx,
                &source.name,
                &source.page,
                &feed_items,
            )?;
        }
        for category in &categories {
            let list = indexed_items(
                &archive_items,
                category_members.get(&category.slug).map(Vec::as_slice),
            );
            write_list(
                "category",
                &category.name,
                &category.page,
                &list,
                None,
                Some(category),
            )?;
            let feed_items = feed_items(&list, &stored_by_path, per_page);
            write_collection_feeds(
                out,
                &site,
                &build_ctx,
                &category.name,
                &category.page,
                &feed_items,
            )?;
        }
        for tag in &tags {
            let list = indexed_items(
                &archive_items,
                tag_members.get(&tag.slug).map(Vec::as_slice),
            );
            write_list("tag", &tag.name, &tag.page, &list, None, Some(tag))?;
            let feed_items = feed_items(&list, &stored_by_path, per_page);
            write_collection_feeds(out, &site, &build_ctx, &tag.name, &tag.page, &feed_items)?;
        }
    }

    let archive_updated = archive_items
        .iter()
        .map(|item| item.updated.unwrap_or(item.date))
        .max();
    let taxonomy_indexes = ["sources/", "tags/"]
        .into_iter()
        .chain(site.has_categories.then_some("categories/"));
    for path in taxonomy_indexes {
        if let Some(url) = canonical_url(&site, path) {
            sitemap_urls.push(outputs::SitemapUrl::new(url, archive_updated));
        }
    }

    let simple = |kind: &str,
                  title: &str,
                  path: &str,
                  template: &str,
                  item: Option<&ItemCtx>,
                  html: Option<&str>,
                  page_items: Option<&[ItemCtx]>|
     -> Result<String> {
        let page = PageCtx {
            kind: kind.to_string(),
            title: title.to_string(),
            path: path.to_string(),
            root: relative_root(path),
            canonical_url: canonical_url(&site, path),
            feed_path: matches!(kind, "sources" | "categories" | "tags").then(|| path.to_string()),
            paginator: None,
        };
        let items = page_items.unwrap_or(&archive_items);
        let schema = structured_data(&site, &page, items, item);
        renderer.render(
            template,
            Ctx {
                site: &site,
                build: &build_ctx,
                page,
                items,
                sources: &source_ctxs,
                categories: &categories,
                tags: &tags,
                item,
                source: None,
                category: None,
                html,
                schema,
            },
        )
    };

    write(
        &out.join("sources/index.html"),
        simple(
            "sources",
            "Sources",
            "sources/",
            "sources.html",
            None,
            None,
            None,
        )?
        .as_bytes(),
    )?;
    if site.has_categories {
        write(
            &out.join("categories/index.html"),
            simple(
                "categories",
                "Categories",
                "categories/",
                "taxonomy.html",
                None,
                None,
                None,
            )?
            .as_bytes(),
        )?;
        pages += 1;
    }
    write(
        &out.join("tags/index.html"),
        simple("tags", "Tags", "tags/", "taxonomy.html", None, None, None)?.as_bytes(),
    )?;
    write(
        &out.join("search/index.html"),
        simple(
            "search",
            "Search",
            "search/",
            "shell.html",
            None,
            None,
            None,
        )?
        .as_bytes(),
    )?;
    write(
        &out.join("preferences/index.html"),
        simple(
            "preferences",
            "Preferences",
            "preferences/",
            "preferences.html",
            None,
            None,
            None,
        )?
        .as_bytes(),
    )?;
    write(
        &out.join("404.html"),
        simple("404", "Not found", "", "404.html", None, None, None)?.as_bytes(),
    )?;
    pages += 5;

    for (ctx, item) in archive_items.iter().zip(&all_items) {
        let mut ctx = ctx.clone();
        ctx.body_html = Some(content::render_markdown(&item.body));
        let dir = out.join(&ctx.url);
        let representation = out.join(ctx.url.trim_end_matches('/'));
        write(
            &dir.join("index.html"),
            simple(
                "item",
                &ctx.title,
                &ctx.url,
                "item.html",
                Some(&ctx),
                None,
                None,
            )?
            .as_bytes(),
        )?;
        let markdown = crate::store::frontmatter::render(&item.front, &item.body)?;
        write(&representation.with_extension("md"), markdown.as_bytes())?;
        write(
            &representation.with_extension("txt"),
            outputs::text_item(&ctx).as_bytes(),
        )?;
        write(
            &representation.with_extension("rst"),
            outputs::rst_item(&ctx).as_bytes(),
        )?;
        write(
            &representation.with_extension("json"),
            outputs::item_json(&site, &ctx, &item.body)?.as_bytes(),
        )?;
        if let Some(url) = canonical_url(&site, &ctx.url) {
            sitemap_urls.push(outputs::SitemapUrl::new(
                url,
                Some(ctx.updated.unwrap_or(ctx.date)),
            ));
        }
    }

    let search_documents: Vec<_> = archive_items
        .iter()
        .zip(&all_items)
        .map(|(item, stored)| pagefind::SearchDocument::new(item, &stored.body))
        .collect();

    let stubs = 0;

    let root_feed_items = feed_items(&river_items, &stored_by_path, per_page);
    let archive_feed_items = feed_items(&archive_items, &stored_by_path, per_page);
    let atom = outputs::atom_feed(&site, &build_ctx, &root_feed_items);
    write(&out.join("feed.xml"), atom.as_bytes())?;
    write(&out.join("atom.xml"), atom.as_bytes())?;
    write(
        &out.join("rss.xml"),
        outputs::rss_collection(&site, &build_ctx, &site.title, "", &root_feed_items).as_bytes(),
    )?;
    write(
        &out.join("feed.json"),
        outputs::json_collection(&site, &site.title, "", &root_feed_items)?.as_bytes(),
    )?;
    write_collection_feeds(
        out,
        &site,
        &build_ctx,
        &format!("{} sources", site.title),
        "sources/",
        &archive_feed_items,
    )?;
    if site.has_categories {
        write_collection_feeds(
            out,
            &site,
            &build_ctx,
            &format!("{} categories", site.title),
            "categories/",
            &archive_feed_items,
        )?;
    }
    write_collection_feeds(
        out,
        &site,
        &build_ctx,
        &format!("{} tags", site.title),
        "tags/",
        &archive_feed_items,
    )?;
    if let Some(root) = site.base_url.as_deref() {
        let default_search_description = format!("Search {}", site.title);
        let search_description = if site.description.is_empty() {
            &default_search_description
        } else {
            &site.description
        };
        let search_url = format!("{root}search/?q={{searchTerms}}");
        let opensearch_url = format!("{root}opensearch.xml");
        let opensearch = outputs::opensearch_description(&outputs::OpenSearchDescription {
            short_name: &site.title,
            description: search_description,
            search_url: &search_url,
            self_url: Some(&opensearch_url),
        });
        write(&out.join("opensearch.xml"), opensearch.as_bytes())?;

        sitemap_urls.sort_by(|left, right| left.loc.cmp(&right.loc));
        sitemap_urls.dedup_by(|left, right| {
            if left.loc != right.loc {
                return false;
            }
            right.lastmod = left.lastmod.max(right.lastmod);
            true
        });
        let sitemap_url = format!("{root}sitemap.xml");
        let sitemap = outputs::sitemap(
            &sitemap_urls,
            &sitemap_url,
            outputs::SitemapLimits::default(),
        )?;
        write(&out.join("sitemap.xml"), sitemap.root_xml().as_bytes())?;
        for chunk in sitemap.chunks() {
            write(&out.join(&chunk.name), chunk.xml.as_bytes())?;
        }
        if site.base_path == "/" {
            write(
                &out.join("robots.txt"),
                format!("User-agent: *\nAllow: /\nSitemap: {sitemap_url}\n").as_bytes(),
            )?;
        }
    }
    write(&out.join(".nojekyll"), b"")?;
    write(&out.join(MARKER), env!("CARGO_PKG_VERSION").as_bytes())?;
    if let Some(domain) = cname(info) {
        write(&out.join("CNAME"), domain.as_bytes())?;
    }
    let assets = renderer.write_static(out)?;

    pagefind::build_cached(
        out,
        &search_documents,
        &site.language,
        info.pagefind_cache.as_deref(),
    )?;

    if config.site.pwa {
        let offline_items = &archive_items[..archive_items.len().min(config.site.offline_items)];
        write(
            &out.join("offline.html"),
            simple(
                "offline",
                "Offline",
                "offline.html",
                "offline.html",
                None,
                None,
                Some(offline_items),
            )?
            .as_bytes(),
        )?;
        write(
            &out.join("manifest.webmanifest"),
            simple(
                "manifest",
                &config.site.title,
                "manifest.webmanifest",
                "manifest.webmanifest",
                None,
                None,
                None,
            )?
            .as_bytes(),
        )?;
        let scoped_lists = source_ctxs
            .iter()
            .map(|s| s.page.clone())
            .chain(categories.iter().map(|c| c.page.clone()))
            .chain(tags.iter().map(|tag| tag.page.clone()))
            .take(config.site.offline_items.clamp(32, 256));
        let taxonomy_lists = ["tags/".to_string()]
            .into_iter()
            .chain(site.has_categories.then(|| "categories/".to_string()));
        let lists = taxonomy_lists
            .chain(scoped_lists)
            .chain(pagefind_precache_paths(out)?);
        let sw = renderer.render(
            "sw.js",
            SwCtx {
                site: &site,
                build: &build_ctx,
                version: cache_version(&build_ctx),
                precache: precache_paths(
                    "",
                    lists,
                    &assets,
                    archive_items.iter().map(|i| i.url.clone()),
                    config.site.offline_items,
                ),
            },
        )?;
        write(&out.join("sw.js"), sw.as_bytes())?;
        pages += 1;
    }

    Ok(Summary {
        pages,
        items: archive_items.len(),
        stubs,
    })
}

/// Template/static lookup order: the project's own `templates/`+`static/`, then the configured
/// theme directory, then the embedded default theme.
pub fn theme_layers(config: &Config, project_root: &Path) -> Result<Layers> {
    let mut layers = Layers::default();
    if project_root.join("templates").is_dir() || project_root.join("static").is_dir() {
        layers.dirs.push(project_root.to_path_buf());
    }
    match config.site.theme.as_str() {
        "default" => {
            // A development binary reads the shipped theme from its source tree so `aggr dev`
            // can rebuild template/CSS/JS edits without recompiling Rust. Release binaries stay
            // fully embedded and have no dependency on the build machine.
            #[cfg(debug_assertions)]
            {
                let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("themes/default");
                if source.is_dir() && !source.starts_with(project_root) {
                    layers.dirs.push(source);
                }
            }
        }
        theme => {
            let dir = project_root.join(theme);
            if !dir.is_dir() {
                bail!("theme {theme:?} is not a directory (git themes arrive in a later release)");
            }
            layers.dirs.push(dir);
        }
    }
    Ok(layers)
}

fn source_contexts(
    sources: &[Source],
    store: &Store,
    status: &Status,
    items: &[Item],
) -> Result<Vec<SourceCtx>> {
    let mut counts: BTreeMap<&str, (usize, Option<DateTime<Utc>>)> = BTreeMap::new();
    for item in items {
        let entry = counts.entry(item.front.source.as_str()).or_default();
        entry.0 += 1;
        entry.1 = entry.1.max(Some(item.created_at()));
    }
    let mut contexts = sources
        .iter()
        .map(|source| {
            let state = store.source_state(&source.slug)?;
            let (count, latest) = counts
                .get(source.slug.as_str())
                .copied()
                .unwrap_or_default();
            let name = source
                .name
                .clone()
                .or_else(|| state.title.clone())
                .unwrap_or_else(|| source.slug.clone());
            Ok(SourceCtx {
                page: format!("sources/{}/", source.slug),
                slug: source.slug.clone(),
                name,
                url: source.public_url.clone(),
                site_url: state.site_url.clone(),
                category: source.category.clone(),
                engine: source.engine.name().to_string(),
                count,
                latest,
                error: status.errors.get(&source.slug).map(|error| SourceErrorCtx {
                    message: error.message.clone(),
                    since: error.since,
                }),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    contexts.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.slug.cmp(&b.slug))
    });
    Ok(contexts)
}

#[derive(Clone, Copy)]
enum Taxonomy {
    Categories,
    Tags,
}

struct TaxonomyIndex {
    terms: Vec<CategoryCtx>,
    members: BTreeMap<String, Vec<usize>>,
}

fn source_members(items: &[ItemCtx]) -> BTreeMap<String, Vec<usize>> {
    let mut members: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, item) in items.iter().enumerate() {
        members.entry(item.source.clone()).or_default().push(index);
    }
    members
}

fn taxonomy_index(items: &[ItemCtx], taxonomy: Taxonomy) -> TaxonomyIndex {
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    let mut members: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, item) in items.iter().enumerate() {
        let terms_for_item: Vec<&str> = match taxonomy {
            Taxonomy::Categories => item.category.as_deref().into_iter().collect(),
            Taxonomy::Tags => item.labels.iter().map(String::as_str).collect(),
        };
        let mut seen = std::collections::BTreeSet::new();
        for name in terms_for_item {
            let slug = context::category_slug(name);
            if slug.is_empty() || !seen.insert(slug.clone()) {
                continue;
            }
            names
                .entry(slug.clone())
                .or_insert_with(|| name.to_string());
            members.entry(slug).or_default().push(index);
        }
    }
    let root = match taxonomy {
        Taxonomy::Categories => "categories",
        Taxonomy::Tags => "tags",
    };
    let mut terms: Vec<_> = names
        .into_iter()
        .map(|(slug, name)| CategoryCtx {
            page: format!("{root}/{slug}/"),
            count: members.get(&slug).map_or(0, Vec::len),
            latest: members
                .get(&slug)
                .into_iter()
                .flatten()
                .map(|&index| items[index].date)
                .max(),
            name,
            slug,
        })
        .collect();
    terms.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.slug.cmp(&b.slug))
    });
    TaxonomyIndex { terms, members }
}

fn indexed_items(items: &[ItemCtx], indices: Option<&[usize]>) -> Vec<ItemCtx> {
    indices
        .into_iter()
        .flatten()
        .map(|&index| items[index].clone())
        .collect()
}

/// Prepare a bounded feed once, then hand the same identity/order/content to every serializer.
/// This mirrors Zola's feed-scope pipeline and keeps Atom, RSS, and JSON Feed in lockstep.
fn feed_items(
    items: &[ItemCtx],
    stored_by_path: &BTreeMap<&str, &Item>,
    limit: usize,
) -> Vec<ItemCtx> {
    items
        .iter()
        .take(limit)
        .cloned()
        .map(|mut item| {
            item.body_html = stored_by_path
                .get(item.path.as_str())
                .map(|stored| content::render_markdown(&stored.body));
            item
        })
        .collect()
}

fn write_collection_feeds(
    out: &Path,
    site: &SiteCtx,
    build: &BuildCtx,
    title: &str,
    path: &str,
    items: &[ItemCtx],
) -> Result<()> {
    let dir = out.join(path);
    write(
        &dir.join("atom.xml"),
        outputs::atom_collection(site, build, title, path, items).as_bytes(),
    )?;
    write(
        &dir.join("rss.xml"),
        outputs::rss_collection(site, build, title, path, items).as_bytes(),
    )?;
    write(
        &dir.join("feed.json"),
        outputs::json_collection(site, title, path, items)?.as_bytes(),
    )
}

/// Wipe a previous build, refusing to touch a directory we did not create.
pub(crate) fn prepare_out_dir(out: &Path) -> Result<()> {
    if out.exists() {
        let ours = out.join(MARKER).exists() || std::fs::read_dir(out)?.next().is_none();
        if !ours {
            bail!(
                "refusing to clear {}: not an aggr output directory (no {MARKER} marker)",
                out.display()
            );
        }
        std::fs::remove_dir_all(out).with_context(|| format!("clearing {}", out.display()))?;
    }
    std::fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))
}

/// The host `CNAME` should carry: a release build on a domain that is not GitHub's own.
pub fn cname(info: &BuildInfo) -> Option<String> {
    if !info.release {
        return None;
    }
    let url = url::Url::parse(info.base_url.as_deref()?).ok()?;
    let host = url.host_str()?;
    (!host.ends_with(".github.io") && host != "localhost").then(|| host.to_string())
}

fn ensure_trailing_slash(url: &str) -> String {
    if url.ends_with('/') {
        url.to_string()
    } else {
        format!("{url}/")
    }
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

fn files_under(root: &Path, directory: &str) -> Result<Vec<String>> {
    let base = root.join(directory);
    let mut paths = Vec::new();
    for entry in walkdir::WalkDir::new(&base) {
        let entry = entry.with_context(|| format!("reading {}", base.display()))?;
        if entry.file_type().is_file() {
            paths.push(
                entry
                    .path()
                    .strip_prefix(root)?
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    paths.sort();
    Ok(paths)
}

/// Keep install cost bounded on very large archives. Small Pagefind indexes remain completely
/// searchable offline; once the index exceeds the budget, only its loader/runtime is installed
/// and searches visited online naturally populate the scoped runtime cache.
fn pagefind_precache_paths(root: &Path) -> Result<Vec<String>> {
    const MAX_BYTES: u64 = 8 * 1024 * 1024;
    const MAX_FILES: usize = 512;

    let files = files_under(root, "pagefind")?;
    let total_bytes = files.iter().try_fold(0_u64, |total, file| {
        std::fs::metadata(root.join(file))
            .map(|metadata| total.saturating_add(metadata.len()))
            .with_context(|| format!("reading metadata for {file}"))
    })?;
    if files.len() <= MAX_FILES && total_bytes <= MAX_BYTES {
        return Ok(files);
    }
    Ok(files
        .into_iter()
        .filter(|file| {
            let name = Path::new(file)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            name == "pagefind.js" || name == "pagefind-entry.json" || name.ends_with(".wasm")
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn day(d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, d, 0, 0, 0).unwrap()
    }

    #[test]
    fn window_respects_count_age_and_stub_cap() {
        let dates = vec![day(10), day(9), day(8), day(1)];
        let w = window(&dates, day(10), 2, 30, 1);
        assert_eq!(
            w,
            Window {
                rendered: vec![0, 1],
                stubbed: vec![2]
            }
        );
        let w = window(&dates, day(10), 10, 5, 10);
        assert_eq!(
            w,
            Window {
                rendered: vec![0, 1, 2],
                stubbed: vec![3]
            }
        );
    }

    #[test]
    fn pagination_paths() {
        let pages = paginate("", 0, 10);
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].path, "");
        assert_eq!(pages[0].range, 0..0);
        assert_eq!(pages[0].context.first, "./");
        assert_eq!(pages[0].context.last, "./");
        assert_eq!(pages[0].context.total_items, 0);
        let pages = paginate("sources/x/", 25, 10);
        assert_eq!(
            pages.iter().map(|p| p.path.as_str()).collect::<Vec<_>>(),
            ["sources/x/", "sources/x/page/2/", "sources/x/page/3/"]
        );
        assert_eq!(pages[2].range, 20..25);
    }

    #[test]
    fn paginator_context_uses_stable_edge_and_adjacent_routes() {
        let pages = paginate("", 35, 10);
        let first = &pages[0].context;
        assert_eq!(first.current_index, 1);
        assert_eq!(first.number_pagers, 4);
        assert_eq!(first.paginate_by, 10);
        assert_eq!(first.total_items, 35);
        assert_eq!(first.first, "./");
        assert_eq!(first.last, "page/4/");
        assert_eq!(first.previous, None);
        assert_eq!(first.next.as_deref(), Some("page/2/"));

        let middle = &pages[2].context;
        assert_eq!(middle.first, "./");
        assert_eq!(middle.last, "page/4/");
        assert_eq!(middle.previous.as_deref(), Some("page/2/"));
        assert_eq!(middle.next.as_deref(), Some("page/4/"));
        assert_eq!(middle.offset, 20);

        let last = &pages[3].context;
        assert_eq!(last.previous.as_deref(), Some("page/3/"));
        assert_eq!(last.next, None);

        let nested = paginate("sources/x/", 11, 10);
        assert_eq!(nested[1].context.first, "sources/x/");
        assert_eq!(nested[1].context.previous.as_deref(), Some("sources/x/"));
        assert_eq!(nested[1].context.last, "sources/x/page/2/");
    }

    #[test]
    fn categories_without_rendered_items_are_omitted() {
        assert!(taxonomy_index(&[], Taxonomy::Categories).terms.is_empty());
    }

    #[test]
    fn base_paths() {
        assert_eq!(base_path(None), "/");
        assert_eq!(base_path(Some("https://u.github.io/")), "/");
        assert_eq!(base_path(Some("https://u.github.io/repo")), "/repo/");
        assert_eq!(base_path(Some("https://u.github.io/a/b/")), "/a/b/");
    }

    #[test]
    fn relative_roots_cover_files_and_nested_routes() {
        assert_eq!(relative_root(""), "./");
        assert_eq!(relative_root("404.html"), "./");
        assert_eq!(relative_root("search/"), "../");
        assert_eq!(relative_root("categories/rust/"), "../../");
        assert_eq!(relative_root("sources/rust/page/2/"), "../../../../");
    }

    #[test]
    fn refuses_to_clear_foreign_directories() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("precious"), "x").unwrap();
        assert!(prepare_out_dir(dir.path()).is_err());
        std::fs::write(dir.path().join(MARKER), "").unwrap();
        prepare_out_dir(dir.path()).unwrap();
        assert!(!dir.path().join("precious").exists());
    }

    #[test]
    fn precache_lists_shells_lists_assets_and_the_newest_items() {
        let paths = precache_paths(
            "/repo/",
            ["sources/a/".to_string(), "sources/a/".to_string()],
            &["style.css".to_string()],
            ["items/a/1/".to_string(), "items/a/2/".to_string()],
            1,
        );
        assert_eq!(paths[0], "/repo/");
        assert!(paths.contains(&"/repo/offline.html".to_string()));
        assert!(paths.contains(&"/repo/preferences/".to_string()));
        assert!(!paths.contains(&"/repo/settings/".to_string()));
        assert!(paths.contains(&"/repo/sources/a/".to_string()));
        assert!(paths.contains(&"/repo/assets/style.css".to_string()));
        assert!(paths.contains(&"/repo/items/a/1/".to_string()));
        assert!(!paths.contains(&"/repo/items/a/2/".to_string()));
        assert_eq!(
            paths
                .iter()
                .filter(|path| *path == "/repo/sources/a/")
                .count(),
            1
        );
    }

    #[test]
    fn oversized_pagefind_precache_keeps_only_its_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let pagefind = dir.path().join("pagefind");
        std::fs::create_dir_all(&pagefind).unwrap();
        std::fs::write(pagefind.join("pagefind.js"), "runtime").unwrap();
        std::fs::write(pagefind.join("pagefind-entry.json"), "{}").unwrap();
        std::fs::write(pagefind.join("pagefind.wasm"), "wasm").unwrap();
        for index in 0..510 {
            std::fs::write(pagefind.join(format!("fragment-{index}.pf_fragment")), "x").unwrap();
        }

        assert_eq!(
            pagefind_precache_paths(dir.path()).unwrap(),
            [
                "pagefind/pagefind-entry.json",
                "pagefind/pagefind.js",
                "pagefind/pagefind.wasm",
            ]
        );
    }

    #[test]
    fn cache_version_changes_with_content_but_not_rebuild_time() {
        let build = BuildCtx {
            time: day(2),
            version: "0".into(),
            config_sha: Some("c".repeat(40)),
            data_sha: None,
            generation: "g".repeat(40),
            release: false,
        };
        assert_eq!(
            cache_version(&build),
            format!("0-local-{}-{}", "c".repeat(12), "g".repeat(12))
        );
        let later = BuildCtx {
            data_sha: Some("d".repeat(40)),
            time: day(3),
            ..build.clone()
        };
        assert_ne!(cache_version(&later), cache_version(&build));
        let rebuilt = BuildCtx {
            time: day(3),
            ..build.clone()
        };
        assert_eq!(cache_version(&rebuilt), cache_version(&build));
    }

    #[test]
    fn render_generation_changes_only_at_visible_time_boundaries() {
        let at = day(3);
        let item = |path: &str, published| Item {
            path: path.into(),
            front: crate::model::FrontMatter {
                title: path.into(),
                link: format!("https://example.com/{path}"),
                source: "example".into(),
                published: Some(published),
                first_seen: published,
                ..Default::default()
            },
            body: "body".into(),
        };
        let site = SiteConfig {
            max_age_days: 2,
            ..Default::default()
        };

        let stable = item("stable", at - Duration::hours(30));
        assert_eq!(
            render_generation(std::slice::from_ref(&stable), &site, at),
            render_generation(&[stable], &site, at + Duration::minutes(10))
        );

        let age_boundary = item("age", at - Duration::minutes(59));
        assert_ne!(
            render_generation(std::slice::from_ref(&age_boundary), &site, at),
            render_generation(&[age_boundary], &site, at + Duration::minutes(2))
        );

        let cutoff = item("cutoff", at - Duration::hours(47) - Duration::minutes(59));
        assert_ne!(
            render_generation(std::slice::from_ref(&cutoff), &site, at),
            render_generation(&[cutoff], &site, at + Duration::minutes(2))
        );
    }

    /// A store with `count` items of one source, newest last, and a matching config.
    fn fixture(root: &Path, count: usize, extra: &str) -> (Config, Vec<Source>, Store) {
        use crate::model::{FrontMatter, file_stem, item_dir};
        use crate::store::NewItem;

        let config = Config::parse(&format!(
            "[site]\ntitle = \"Demo <site>\"\n{extra}\n[[sources]]\nslug = \"blog\"\nurl = \"https://blog.example/feed\"\n"
        ))
        .unwrap();
        let sources = config.resolve_sources(&|_| None).unwrap();
        let store = Store::open(root.join("data"));
        for i in 0..count {
            let date = day(1 + i as u32);
            let front = FrontMatter {
                title: format!("Post {i}"),
                link: format!("https://blog.example/{i}"),
                source: "blog".into(),
                published: Some(date),
                first_seen: date,
                ..Default::default()
            };
            store
                .write_item(NewItem {
                    dir: &item_dir("blog", date),
                    stem: &file_stem(date, &front.title),
                    front: &front,
                    body: "Hello",
                    html: None,
                })
                .unwrap();
        }
        (config, sources, store)
    }

    fn info(out: PathBuf) -> BuildInfo {
        BuildInfo {
            out,
            base_url: Some("https://u.github.io/repo/".into()),
            config_sha: Some("c".repeat(40)),
            config_path: Some("aggr.toml".into()),
            data_sha: Some("d".repeat(40)),
            generation: "fixture".into(),
            now: day(20),
            release: false,
            discussions: crate::discussions::ResolutionSet::default(),
            pagefind_cache: None,
        }
    }

    #[test]
    fn old_items_stay_browsable_in_source_archives() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "max_age_days = 5\npwa = false\n");
        let out = dir.path().join("out");
        let summary = build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        assert_eq!(summary.items, 1);
        assert_eq!(summary.stubs, 0);

        let feed = std::fs::read_to_string(out.join("index.html")).unwrap();
        assert!(
            !feed.contains("Post 0"),
            "old items should not crowd the recent feed"
        );
        let source = std::fs::read_to_string(out.join("sources/blog/index.html")).unwrap();
        assert!(source.contains("Post 0"), "{source}");
        assert!(
            out.join("items/blog/2026-09-01-post-0/index.html")
                .is_file()
        );
    }

    #[test]
    fn item_pages_show_three_unlabelled_article_suggestions() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 6, "max_age_days = 30\npwa = false\n");
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();

        let newest =
            std::fs::read_to_string(out.join("items/blog/2026-09-06-post-5/index.html")).unwrap();
        assert!(
            newest.contains("data-next-url=\"items/blog/2026-09-05-post-4/\""),
            "{newest}"
        );
        assert!(!newest.contains("data-previous-url="), "{newest}");
        assert_eq!(newest.matches("class=\"article-more-link\"").count(), 3);
        assert!(!newest.contains("article-more-label"), "{newest}");

        let second =
            std::fs::read_to_string(out.join("items/blog/2026-09-05-post-4/index.html")).unwrap();
        assert!(second.contains("data-previous-url=\"items/blog/2026-09-06-post-5/\""));
        assert!(second.contains("data-next-url=\"items/blog/2026-09-04-post-3/\""));
        assert_eq!(second.matches("class=\"article-more-link\"").count(), 3);

        for day in 1..=6 {
            let page = std::fs::read_to_string(out.join(format!(
                "items/blog/2026-09-{day:02}-post-{}/index.html",
                day - 1
            )))
            .unwrap();
            assert_eq!(
                page.matches("class=\"article-more-link\"").count(),
                3,
                "day {day} should always have three suggestions"
            );
        }
    }

    #[test]
    fn build_cleans_leading_publication_dates_from_existing_items() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "max_age_days = 30\npwa = false\n");
        let mut item = store.items().unwrap().remove(0);
        item.body = "1st September 2026\n\nActual opening.\n".into();
        let stem = item.path.rsplit('/').next().unwrap();
        let dir_path = item.path.rsplit_once('/').unwrap().0;
        store
            .write_item(crate::store::NewItem {
                dir: dir_path,
                stem,
                front: &item.front,
                body: &item.body,
                html: None,
            })
            .unwrap();
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();

        let html =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0/index.html")).unwrap();
        assert!(!html.contains("1st September 2026"), "{html}");
        assert!(html.contains("Actual opening."), "{html}");
        let markdown =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0.md")).unwrap();
        assert!(!markdown.contains("1st September 2026"), "{markdown}");
    }

    #[test]
    fn middle_pages_link_to_adjacent_and_edge_pages() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 5, "items_per_page = 1\npwa = false\n");
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();

        let middle = std::fs::read_to_string(out.join("page/3/index.html")).unwrap();
        assert!(middle.contains("href=\"./\">⇤ first</a>"), "{middle}");
        assert!(middle.contains("href=\"page/2/\">← newer</a>"), "{middle}");
        assert!(middle.contains("href=\"page/4/\">older →</a>"), "{middle}");
        assert!(middle.contains("href=\"page/5/\">last ⇥</a>"), "{middle}");
        assert!(
            middle.contains("<link rel=\"canonical\" href=\"https://u.github.io/repo/page/3/\">"),
            "{middle}"
        );
        assert!(
            middle.contains("<link rel=\"first\" href=\"./\">"),
            "{middle}"
        );
        assert!(
            middle.contains("<link rel=\"prev\" href=\"page/2/\">"),
            "{middle}"
        );
        assert!(
            middle.contains("<link rel=\"next\" href=\"page/4/\">"),
            "{middle}"
        );
        assert!(
            middle.contains("<link rel=\"last\" href=\"page/5/\">"),
            "{middle}"
        );

        let second = std::fs::read_to_string(out.join("page/2/index.html")).unwrap();
        assert!(second.contains("href=\"./\">⇤ first</a>"), "{second}");
        assert!(second.contains("href=\"./\">← newer</a>"), "{second}");
    }

    #[test]
    fn pwa_outputs_cover_the_shells_and_the_newest_items() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 3, "offline_items = 2\n");
        let out = dir.path().join("out");
        let summary = build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        assert_eq!(summary.items, 3);
        // River, the source page, five fixed pages, and the offline page. No category output is
        // emitted when no retained item has a category.
        assert_eq!(summary.pages, 1 + 1 + 5 + 1);
        assert!(!out.join("categories").exists());
        let home = std::fs::read_to_string(out.join("index.html")).unwrap();
        assert!(!home.contains("href=\"categories/\""), "{home}");
        assert!(!home.contains("<dd>Categories</dd>"), "{home}");
        let search = std::fs::read_to_string(out.join("search/index.html")).unwrap();
        assert!(!search.contains("id=\"category-filter\""), "{search}");

        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(out.join("manifest.webmanifest")).unwrap())
                .unwrap();
        assert_eq!(manifest["name"], "aggr");
        assert_eq!(manifest["short_name"], "aggr");
        assert_eq!(manifest["description"], "Latest entries from Demo <site>.");
        assert_eq!(manifest["start_url"], "./");
        assert_eq!(manifest["scope"], "./");
        assert_eq!(manifest["display"], "standalone");
        assert_eq!(manifest["icons"].as_array().unwrap().len(), 4);
        assert!(
            manifest["icons"][0]["src"]
                .as_str()
                .unwrap()
                .starts_with("assets/icon-192-")
        );
        for icon in [
            "icon-192-",
            "icon-512-",
            "icon-maskable-512-",
            "apple-touch-icon-",
        ] {
            let path = std::fs::read_dir(out.join("assets"))
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| {
                    path.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with(icon)
                })
                .unwrap();
            let png = std::fs::read(path).unwrap();
            assert!(png.starts_with(b"\x89PNG"), "{icon} is not a PNG");
        }

        let sw = std::fs::read_to_string(out.join("sw.js")).unwrap();
        let version = format!(
            "{}-{}-{}-{}",
            env!("CARGO_PKG_VERSION"),
            "d".repeat(12),
            "c".repeat(12),
            "fixture"
        );
        assert!(sw.contains(&format!("var VERSION = {version:?};")), "{sw}");
        assert!(sw.contains("new URL(\"./\", self.registration.scope)"));
        assert!(sw.contains("\"assets/style-"));
        assert!(sw.contains("\"pagefind/pagefind.js\""));
        assert!(!out.join("search-meta.json").exists());
        assert!(
            !sw.contains("assets/logo-"),
            "the multi-megabyte source icon is not precached"
        );
        assert!(sw.contains("navigationPreload.enable"));
        assert!(sw.contains("x-requested-with"));
        assert!(sw.contains("caches.match(request, { ignoreSearch: true })"));
        assert!(sw.contains("\"offline.html\""));
        assert!(sw.contains("\"sources/blog/\""));
        // Newest two of three: posts 2 and 1, not 0.
        assert!(sw.contains("\"items/blog/2026-09-03-post-2/\""));
        assert!(sw.contains("\"items/blog/2026-09-02-post-1/\""));
        assert!(!sw.contains("post-0/\""));
        assert_eq!(sw.matches("\"items/").count(), 2);

        let offline = std::fs::read_to_string(out.join("offline.html")).unwrap();
        assert!(offline.contains("data-shell=\"offline\""));
        assert!(offline.contains("Post 2"));
        assert!(offline.contains("Post 1"));
        assert!(!offline.contains("Post 0"));
        assert!(offline.contains("<link rel=\"manifest\" href=\"manifest.webmanifest\">"));
        assert!(offline.contains("pwa: true"));
        let river = std::fs::read_to_string(out.join("index.html")).unwrap();
        assert!(
            river.contains("<title>aggr | demo &lt;site&gt;</title>"),
            "{river}"
        );
        assert!(
            river.contains(
                "<meta name=\"description\" content=\"Latest entries from Demo &lt;site&gt;.\">"
            ),
            "{river}"
        );
        assert!(river.contains("type=\"application/ld+json\""), "{river}");
        assert!(river.contains("https://schema.org"), "{river}");
        assert!(river.contains("rel=\"search\""), "{river}");
        assert!(river.contains("rel=\"apple-touch-icon\""));
        assert!(river.contains("name=\"theme-color\""));
        assert!(river.contains("href=\"sources/\""), "{river}");
        assert!(river.contains("href=\"preferences/\""), "{river}");
        assert!(river.contains("aggr.toml ↗</a>"), "{river}");
        assert!(out.join("preferences/index.html").is_file());
        assert!(!out.join("settings/index.html").exists());
        assert!(out.join("pagefind/pagefind.js").is_file());
        assert!(out.join("sources/blog/atom.xml").is_file());
        assert!(out.join("sources/blog/rss.xml").is_file());
        assert!(out.join("sources/blog/feed.json").is_file());
        let atom = std::fs::read_to_string(out.join("sources/blog/atom.xml")).unwrap();
        assert!(atom.contains("xml:lang=\"en\""), "{atom}");
        assert!(atom.contains("<id>urn:aggr:item:"), "{atom}");
        assert!(atom.contains("<link rel=\"via\""), "{atom}");
        assert!(atom.contains("<content type=\"html\">"), "{atom}");
        let opensearch = std::fs::read_to_string(out.join("opensearch.xml")).unwrap();
        assert!(
            opensearch.contains("https://u.github.io/repo/search/?q={searchTerms}"),
            "{opensearch}"
        );
        let sitemap = std::fs::read_to_string(out.join("sitemap.xml")).unwrap();
        assert!(
            sitemap.contains("https://u.github.io/repo/items/blog/2026-09-03-post-2/"),
            "{sitemap}"
        );
        assert!(
            !out.join("robots.txt").exists(),
            "a robots file under a project subpath cannot govern the origin"
        );
        let item =
            std::fs::read_to_string(out.join("items/blog/2026-09-03-post-2/index.html")).unwrap();
        assert!(
            item.contains("<link rel=\"canonical\" href=\"https://u.github.io/repo/items/blog/2026-09-03-post-2/\">"),
            "{item}"
        );
        assert!(
            item.contains("<link rel=\"via\" href=\"https://blog.example/2\">"),
            "{item}"
        );
        assert!(item.contains("isBasedOn"), "{item}");
    }

    #[test]
    fn origin_root_build_advertises_its_sitemap_in_robots() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "pwa = false\n");
        let out = dir.path().join("out");
        let mut build_info = info(out.clone());
        build_info.base_url = Some("https://reads.example/".into());
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();

        let robots = std::fs::read_to_string(out.join("robots.txt")).unwrap();
        assert_eq!(
            robots,
            "User-agent: *\nAllow: /\nSitemap: https://reads.example/sitemap.xml\n"
        );
    }

    #[test]
    fn pwa_off_writes_no_manifest_worker_or_offline_page() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "pwa = false\n");
        let out = dir.path().join("out");
        let summary = build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        assert_eq!(summary.pages, 1 + 1 + 5);
        for name in ["manifest.webmanifest", "sw.js", "offline.html"] {
            assert!(!out.join(name).exists(), "{name} was written");
        }
        let river = std::fs::read_to_string(out.join("index.html")).unwrap();
        assert!(!river.contains("rel=\"manifest\""));
        assert!(river.contains("pwa: false"));
    }
}
