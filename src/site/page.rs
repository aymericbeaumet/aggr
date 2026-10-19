//! Prepare complete page models and document metadata from the local archive.

use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, Utc};

use super::client::{
    self, ClientDirectory, ClientDirectoryEntry, ClientDirectoryGroup, ClientHead, ClientLink,
    ClientMeta, ClientSourceError, ClientView, RenderRequest,
};
use super::context::{BuildCtx, CategoryCtx, ItemCtx, PageCtx, SiteCtx, SourceCtx};
use super::render::Renderer;
use super::{PreparedSite, outputs, paginate, relative_root, write};
use crate::content;

/// Shared prepared values; page requests only carry the directory groups they display.
#[derive(Clone)]
pub(super) struct SharedCtx {
    sources: Vec<SourceCtx>,
    categories: Vec<CategoryCtx>,
    tags: Vec<CategoryCtx>,
    pub(super) client: client::Shared,
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
            sources: sources.to_vec(),
            categories: categories.to_vec(),
            tags: tags.to_vec(),
            client: client::Shared::new(site, build),
        }
    }
}

pub(super) struct Pages<'a> {
    site: &'a SiteCtx,
    renderer: &'a Renderer,
    shared: SharedCtx,
    archive: &'a [ItemCtx],
    per_page: usize,
}

pub(super) struct ListPage<'a> {
    pub kind: &'a str,
    pub title: &'a str,
    pub prefix: &'a str,
    pub list: &'a [ItemCtx],
    pub source: Option<&'a SourceCtx>,
    pub category: Option<&'a CategoryCtx>,
}

#[derive(Clone, Copy)]
pub(super) struct SimplePage<'a> {
    pub kind: &'a str,
    pub title: &'a str,
    pub path: &'a str,
    pub item: Option<&'a ItemCtx>,
    pub page_items: Option<&'a [ItemCtx]>,
}

impl<'a> SimplePage<'a> {
    pub(super) const fn new(kind: &'a str, title: &'a str, path: &'a str) -> Self {
        Self {
            kind,
            title,
            path,
            item: None,
            page_items: None,
        }
    }
}

/// Fixed HTML routes and whether each belongs in the sitemap.
pub(super) const FIXED_PAGES: [(SimplePage<'static>, bool); 6] = [
    (SimplePage::new("browse", "Browse", "browse/"), true),
    (SimplePage::new("sources", "Sources", "sources/"), true),
    (
        SimplePage::new("categories", "Categories", "categories/"),
        true,
    ),
    (SimplePage::new("tags", "Tags", "tags/"), true),
    (
        SimplePage::new("preferences", "Preferences", "preferences/"),
        false,
    ),
    (SimplePage::new("404", "Not found", "404.html"), false),
];

impl SimplePage<'_> {
    pub(super) fn output_path(&self) -> PathBuf {
        if self.path.ends_with('/') {
            Path::new(self.path).join("index.html")
        } else {
            PathBuf::from(self.path)
        }
    }
}

/// The collection inventory shared by publication and request-driven development. Indices keep
/// it independent of rendered media, so preparing routes never materializes article companions.
pub(super) struct Collection {
    pub(super) indices: Vec<usize>,
    pub(super) prefix: String,
    pub(super) title: String,
    kind: &'static str,
    source: Option<usize>,
    category: Option<(bool, usize)>,
}

impl PreparedSite {
    pub(super) fn collections(&self) -> Vec<Collection> {
        let articles: std::collections::BTreeMap<_, _> = self
            .archive_items
            .iter()
            .enumerate()
            .map(|(i, item)| (item.url.as_str(), i))
            .collect();
        let mut collections = vec![Collection {
            kind: "river",
            title: self.site.title.clone(),
            prefix: String::new(),
            indices: self
                .river_items
                .iter()
                .filter_map(|item| articles.get(item.url.as_str()).copied())
                .collect(),
            source: None,
            category: None,
        }];
        for (i, source) in self.source_ctxs.iter().enumerate() {
            collections.push(Collection {
                kind: "source",
                title: source.name.clone(),
                prefix: source.page.clone(),
                indices: self
                    .source_members
                    .get(&source.slug)
                    .cloned()
                    .unwrap_or_default(),
                source: Some(i),
                category: None,
            });
        }
        for (is_tag, terms, members) in [
            (false, &self.categories, &self.category_members),
            (true, &self.tags, &self.tag_members),
        ] {
            for (i, term) in terms.iter().enumerate() {
                collections.push(Collection {
                    kind: if is_tag { "tag" } else { "category" },
                    title: term.name.clone(),
                    prefix: term.page.clone(),
                    indices: members.get(&term.slug).cloned().unwrap_or_default(),
                    source: None,
                    category: Some((is_tag, i)),
                });
            }
        }
        collections
    }
}

impl Collection {
    pub(super) fn page<'a>(
        &'a self,
        items: &'a [ItemCtx],
        sources: &'a [SourceCtx],
        categories: &'a [CategoryCtx],
        tags: &'a [CategoryCtx],
    ) -> ListPage<'a> {
        ListPage {
            kind: self.kind,
            title: &self.title,
            prefix: &self.prefix,
            list: items,
            source: self.source.map(|i| &sources[i]),
            category: self
                .category
                .map(|(is_tag, i)| if is_tag { &tags[i] } else { &categories[i] }),
        }
    }
}

impl<'a> Pages<'a> {
    pub(super) fn new(
        site: &'a SiteCtx,
        renderer: &'a Renderer,
        mut shared: SharedCtx,
        archive: &'a [ItemCtx],
        per_page: usize,
    ) -> Self {
        shared.client.site.assets = renderer.client_assets();
        Self {
            site,
            renderer,
            shared,
            archive,
            per_page,
        }
    }

    pub(super) fn write_list(
        &self,
        out: &Path,
        page: ListPage<'_>,
        sitemap_urls: &mut Vec<outputs::SitemapUrl>,
    ) -> Result<usize> {
        let pagination = paginate(page.prefix, page.list.len(), self.per_page);
        for pager in &pagination {
            let html = self.list(&page, pager)?;
            if let Some(url) = self.site.absolute(&pager.path) {
                sitemap_urls.push(outputs::SitemapUrl::new(
                    url,
                    page.list[pager.range.clone()]
                        .iter()
                        .map(archive_modified_at)
                        .max(),
                ));
            }
            write(&out.join(&pager.path).join("index.html"), html.as_bytes())?;
        }
        Ok(pagination.len())
    }

    pub(super) fn list_page(&self, list: &ListPage<'_>, pager: &super::Pager) -> PageCtx {
        let site = self.site;
        let page_number = pager.context.current_index;
        PageCtx {
            kind: list.kind.to_string(),
            title: list.title.to_string(),
            document_title: document_title(&site.title, list.title, list.kind, page_number),
            description: page_description(site, list.title, list.kind, page_number),
            indexable: site.indexing,
            path: pager.path.clone(),
            root: relative_root(&pager.path),
            canonical_url: site.absolute(&pager.path),
            feed_path: Some(list.prefix.to_string()),
            feed_title: Some(list.title.to_string()),
            paginator: Some(pager.context.clone()),
        }
    }

    pub(super) fn list_request(&self, list: &ListPage<'_>, pager: &super::Pager) -> RenderRequest {
        let page = self.list_page(list, pager);
        let items = &list.list[pager.range.clone()];
        let mut model =
            self.shared
                .client
                .list(&page, &pager.context, items, list.source, list.category);
        model.head = self.head(&page, None, structured_data(self.site, &page, items, None));
        RenderRequest {
            page: model,
            body_html: String::new(),
        }
    }

    pub(super) fn list(&self, list: &ListPage<'_>, pager: &super::Pager) -> Result<String> {
        self.renderer.render_page(&self.list_request(list, pager))
    }

    pub(super) fn simple_page(&self, page: &SimplePage<'_>) -> PageCtx {
        let SimplePage {
            kind,
            title,
            path,
            item,
            ..
        } = *page;
        let site = self.site;
        let utility_fallback = matches!(kind, "404" | "offline");
        let advertises_feeds = !matches!(kind, "404" | "offline");
        PageCtx {
            kind: kind.to_string(),
            title: title.to_string(),
            document_title: document_title(&site.title, title, kind, 1),
            description: item.map_or_else(
                || page_description(site, title, kind, 1),
                |item| item_description(site, item),
            ),
            indexable: site.indexing && !matches!(kind, "preferences" | "404" | "offline"),
            path: path.to_string(),
            root: if utility_fallback {
                site.base_path.clone()
            } else {
                relative_root(path)
            },
            canonical_url: (!utility_fallback).then(|| site.absolute(path)).flatten(),
            feed_path: advertises_feeds.then(String::new),
            feed_title: advertises_feeds.then(|| site.title.clone()),
            paginator: None,
        }
    }

    pub(super) fn simple_request(&self, simple: SimplePage<'_>) -> RenderRequest {
        let page = self.simple_page(&simple);
        let items = simple.page_items.unwrap_or(self.archive);
        let schema = (!matches!(simple.kind, "404" | "offline"))
            .then(|| structured_data(self.site, &page, items, simple.item))
            .flatten();
        let mut model = self.shared.client.simple(&page, simple.item, self.site);
        if matches!(simple.kind, "browse" | "sources" | "categories" | "tags") {
            model.page = ClientView::Directory(Box::new(self.directory(simple.kind)));
        }
        model.head = self.head(&page, simple.item, schema);
        let body_html = simple
            .item
            .and_then(|item| item.body_html.as_deref())
            .map(|html| content::rebase_site_paths(html, &page.root))
            .unwrap_or_default();
        RenderRequest {
            page: model,
            body_html,
        }
    }

    pub(super) fn simple(&self, page: SimplePage<'_>) -> Result<String> {
        self.renderer.render_page(&self.simple_request(page))
    }

    fn directory(&self, kind: &str) -> ClientDirectory {
        let mut groups = Vec::new();
        for (directory_kind, title, categories) in [
            ("categories", "Categories", &self.shared.categories),
            ("sources", "Sources", &self.shared.categories),
            ("tags", "Tags", &self.shared.tags),
        ] {
            if kind != "browse" && kind != directory_kind {
                continue;
            }
            let entries = if directory_kind == "sources" {
                self.shared
                    .sources
                    .iter()
                    .filter(|source| source.listed)
                    .map(|source| ClientDirectoryEntry {
                        name: source.name.clone(),
                        url: format!(
                            "?{}",
                            url::form_urlencoded::Serializer::new(String::new())
                                .append_pair(
                                    "q",
                                    &format!(
                                        "source:{}",
                                        serde_json::to_string(&source.query_value)
                                            .unwrap_or_default()
                                    )
                                )
                                .finish()
                        ),
                        count: source.count,
                        latest: source.latest.map(|date| date.to_rfc3339()),
                        site_url: source.site_url.clone(),
                        engine: Some(source.engine.clone()),
                        category: source.category.as_ref().map(|name| client::Category {
                            name: name.clone(),
                            slug: super::context::category_slug(name),
                        }),
                        error: source.error.as_ref().map(|error| ClientSourceError {
                            message: error.message.clone(),
                            since: error.since.to_rfc3339(),
                        }),
                    })
                    .collect()
            } else {
                categories
                    .iter()
                    .filter(|category| category.count > 0)
                    .map(|category| ClientDirectoryEntry {
                        name: category.name.clone(),
                        url: format!(
                            "?{}",
                            url::form_urlencoded::Serializer::new(String::new())
                                .append_pair(
                                    "q",
                                    &format!(
                                        "{}:{}",
                                        if directory_kind == "tags" {
                                            "tag"
                                        } else {
                                            "category"
                                        },
                                        serde_json::to_string(&category.slug).unwrap_or_default()
                                    )
                                )
                                .finish()
                        ),
                        count: category.count,
                        latest: category.latest.map(|date| date.to_rfc3339()),
                        site_url: None,
                        engine: None,
                        category: None,
                        error: None,
                    })
                    .collect()
            };
            groups.push(ClientDirectoryGroup {
                kind: directory_kind.into(),
                title: title.into(),
                entries,
            });
        }
        ClientDirectory { groups }
    }

    fn url(&self, page: &PageCtx, path: &str) -> String {
        let asset = self.renderer.asset_site_path(path);
        let site_path = asset.trim_start_matches('/');
        let site_path = site_path.strip_prefix("./").unwrap_or(site_path);
        if site_path.is_empty() {
            page.root.clone()
        } else {
            content::rebase_site_url(site_path, &page.root)
        }
    }

    pub(super) fn manifest(&self) -> Result<String> {
        let asset = |path: &str| format!("./{}", self.renderer.asset_site_path(path));
        let icon = asset("assets/icon-192.png");
        Ok(serde_json::to_string(&serde_json::json!({
            "name": self.site.title, "short_name": "aggr", "description": self.site.description,
            "lang": self.site.language, "start_url": "./", "scope": "./", "display": "standalone",
            "display_override": ["standalone", "minimal-ui"], "background_color": "#f5f6fa", "theme_color": "#291a1b",
            "icons": [
                {"src":icon,"sizes":"192x192","type":"image/png","purpose":"any"},
                {"src":asset("assets/icon-512.png"),"sizes":"512x512","type":"image/png","purpose":"any"},
                {"src":asset("assets/icon-maskable-512.png"),"sizes":"512x512","type":"image/png","purpose":"maskable"},
                {"src":asset("assets/apple-touch-icon.png"),"sizes":"180x180","type":"image/png"}
            ],
            "shortcuts": [
                {"name":"Feed","short_name":"Feed","url":"./","icons":[{"src":icon,"sizes":"192x192"}]},
                {"name":"Search","short_name":"Search","url":"./","icons":[{"src":icon,"sizes":"192x192"}]},
                {"name":"Browse","short_name":"Browse","url":"./browse/","icons":[{"src":icon,"sizes":"192x192"}]}
            ], "categories":["news","productivity"], "prefer_related_applications":false
        }))?)
    }

    fn head(
        &self,
        page: &PageCtx,
        item: Option<&ItemCtx>,
        schema: Option<serde_json::Value>,
    ) -> ClientHead {
        let site = self.site;
        let mut head = ClientHead {
            title: page.document_title.clone(),
            schema,
            bootstrap: self.url(page, "assets/app/bootstrap.js"),
            ..ClientHead::default()
        };
        let mut named = |name: &str, value: String| {
            head.meta.push(ClientMeta {
                name: Some(name.into()),
                content: value,
                ..ClientMeta::default()
            })
        };
        named(
            "viewport",
            "width=device-width, initial-scale=1, viewport-fit=cover".into(),
        );
        named("color-scheme", "light dark".into());
        named("referrer", "strict-origin-when-cross-origin".into());
        named(
            "robots",
            if page.indexable {
                "index,follow,max-image-preview:large,max-snippet:-1,max-video-preview:-1"
            } else {
                "noindex,follow"
            }
            .into(),
        );
        if !page.description.is_empty() {
            named("description", page.description.clone());
        }
        named("application-name", site.title.clone());
        named(
            "generator",
            format!("aggr {}", self.shared.client.build.version),
        );
        if site.pwa {
            for (name, value) in [
                ("mobile-web-app-capable", "yes"),
                ("apple-mobile-web-app-capable", "yes"),
                ("apple-mobile-web-app-status-bar-style", "black-translucent"),
            ] {
                named(name, value.into());
            }
            named("apple-mobile-web-app-title", site.title.clone());
        }
        named("aggr:network", site.network_url.into());
        named("aggr:instance", self.url(page, "aggr.json"));
        named(
            "aggr:source",
            site.config_url
                .clone()
                .unwrap_or_else(|| self.url(page, "aggr.toml")),
        );
        if let Some(repository) = &site.repository {
            named(
                "aggr:repository",
                format!("https://github.com/{repository}"),
            );
        }
        if let Some(item) = item {
            for author in &item.authors {
                named("author", author.clone());
            }
        }
        head.meta.push(ClientMeta {
            name: Some("theme-color".into()),
            id: Some("theme-color".into()),
            content: "#291a1b".into(),
            ..ClientMeta::default()
        });
        if site.hermetic {
            head.meta.push(ClientMeta { http_equiv:Some("Content-Security-Policy".into()), content:"default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self'; media-src 'self'; object-src 'self'; frame-src 'none'; base-uri 'self'; form-action 'none'".into(), ..ClientMeta::default() });
        }
        let mut property = |name: &str, value: String| {
            head.meta.push(ClientMeta {
                property: Some(name.into()),
                content: value,
                ..ClientMeta::default()
            })
        };
        property(
            "og:type",
            if item.is_some() { "article" } else { "website" }.into(),
        );
        property("og:site_name", site.title.clone());
        property("og:title", page.title.clone());
        property("og:locale", site.og_locale.clone());
        let description = item
            .filter(|item| !item.excerpt.is_empty())
            .map(|item| item.excerpt.clone())
            .unwrap_or_else(|| page.description.clone());
        if !description.is_empty() {
            property("og:description", description.clone());
        }
        if let Some(canonical) = &page.canonical_url {
            property("og:url", canonical.clone());
        }
        if let Some(item) = item {
            if let Some(published) = item.published {
                property("article:published_time", published.to_rfc3339());
            }
            if let Some(updated) = item.updated {
                property("article:modified_time", updated.to_rfc3339());
            }
            if let Some(category) = &item.category {
                property("article:section", category.clone());
            }
            for label in &item.labels {
                property("article:tag", label.clone());
            }
        }
        let lead = item.and_then(|item| item.article_preview.as_ref());
        let image = site.base_url.as_ref().map(|root| {
            if let Some(lead) = lead {
                (
                    format!("{root}{}", self.renderer.asset_site_path(&lead.url)),
                    Some((lead.width, lead.height)),
                    item.and_then(|item| item.preview.as_ref())
                        .and_then(|preview| preview.alt.clone()),
                )
            } else if let Some(preview) = item.and_then(|item| item.preview.as_ref()) {
                (
                    format!("{root}{}", preview.url),
                    Some((preview.width, preview.height)),
                    preview.alt.clone(),
                )
            } else {
                (
                    format!(
                        "{root}{}",
                        self.renderer.asset_site_path("assets/icon-512.png")
                    ),
                    None,
                    Some(format!("{} app icon", site.title)),
                )
            }
        });
        if let Some((url, dimensions, alt)) = &image {
            property("og:image", url.clone());
            if let Some((width, height)) = dimensions {
                property("og:image:width", width.to_string());
                property("og:image:height", height.to_string());
            }
            if let Some(alt) = alt {
                property("og:image:alt", alt.clone());
            }
        }
        let mut named = |name: &str, value: String| {
            head.meta.push(ClientMeta {
                name: Some(name.into()),
                content: value,
                ..ClientMeta::default()
            })
        };
        named(
            "twitter:card",
            if lead.is_some() && site.base_url.is_some() {
                "summary_large_image"
            } else {
                "summary"
            }
            .into(),
        );
        named("twitter:title", page.title.clone());
        if !description.is_empty() {
            named("twitter:description", description);
        }
        if let Some((url, _, alt)) = image {
            named("twitter:image", url);
            if let Some(alt) = alt {
                named("twitter:image:alt", alt);
            }
        }
        let mut link = |rel: &str,
                        href: String,
                        mime: Option<&str>,
                        title: Option<String>,
                        sizes: Option<&str>| {
            head.links.push(ClientLink {
                rel: rel.into(),
                href,
                mime_type: mime.map(str::to_string),
                title,
                sizes: sizes.map(str::to_string),
            })
        };
        link("type", site.instance_type_url.into(), None, None, None);
        link(
            "service-meta",
            self.url(page, "aggr.json"),
            Some("application/json"),
            Some("aggr instance metadata".into()),
            None,
        );
        if site.base_url.is_some() {
            link(
                "linkset",
                self.url(page, "linkset.json"),
                Some("application/linkset+json"),
                Some("Original URL to archived snapshot links".into()),
                None,
            );
        }
        link(
            "via",
            site.config_url
                .clone()
                .unwrap_or_else(|| self.url(page, "aggr.toml")),
            Some("text/toml"),
            Some("aggr source configuration".into()),
            None,
        );
        if let Some(canonical) = &page.canonical_url {
            link("canonical", canonical.clone(), None, None, None);
        }
        if let Some(pager) = &page.paginator {
            link("first", self.url(page, &pager.first), None, None, None);
            link("last", self.url(page, &pager.last), None, None, None);
            if let Some(previous) = &pager.previous {
                link("prev", self.url(page, previous), None, None, None);
            }
            if let Some(next) = &pager.next {
                link("next", self.url(page, next), None, None, None);
            }
        }
        if let Some(prefix) = &page.feed_path {
            for (file, mime) in [
                ("atom.xml", "application/atom+xml"),
                ("rss.xml", "application/rss+xml"),
                ("feed.json", "application/feed+json"),
            ] {
                link(
                    "alternate",
                    self.url(page, &format!("{prefix}{file}")),
                    Some(mime),
                    page.feed_title.clone(),
                    None,
                );
            }
            link(
                "alternate",
                self.url(page, "sources.opml"),
                Some("text/x-opml"),
                Some("Subscriptions".into()),
                None,
            );
        }
        if site.base_url.is_some() {
            link(
                "search",
                self.url(page, "opensearch.xml"),
                Some("application/opensearchdescription+xml"),
                Some(site.title.clone()),
                None,
            );
        }
        link(
            "icon",
            self.url(page, "assets/favicon-32.png"),
            Some("image/png"),
            None,
            Some("32x32"),
        );
        link(
            "apple-touch-icon",
            self.url(page, "assets/apple-touch-icon.png"),
            None,
            None,
            None,
        );
        if site.pwa {
            link(
                "manifest",
                self.url(page, "manifest.webmanifest"),
                None,
                None,
                None,
            );
        }
        if let Some(item) = item {
            link("via", item.link.clone(), None, None, None);
            link("original", item.link.clone(), None, None, None);
            for (extension, mime, title) in [
                ("md", "text/markdown", "Markdown"),
                ("txt", "text/plain", "Plain text"),
                ("rst", "text/x-rst", "reStructuredText"),
                ("json", "application/ld+json", "JSON-LD"),
            ] {
                link(
                    "alternate",
                    self.url(
                        page,
                        &format!("{}.{}", item.url.trim_matches('/'), extension),
                    ),
                    Some(mime),
                    Some(title.into()),
                    None,
                );
            }
            if let Some(previous) = &item.previous_article {
                link("prev", self.url(page, &previous.url), None, None, None);
            }
            if let Some(next) = &item.next_article {
                link("next", self.url(page, &next.url), None, None, None);
            }
        }
        head
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
    use chrono::TimeZone;

    fn day(d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, d, 0, 0, 0).unwrap()
    }

    fn request_site() -> SiteCtx {
        SiteCtx {
            hermetic: false,
            title: "Reader".into(),
            description: String::new(),
            identity: None,
            language: "en".into(),
            og_locale: "en".into(),
            base_path: "/reader/".into(),
            base_url: Some("https://reader.test/reader/".into()),
            indexing: false,
            repository: None,
            data_branch: "aggr".into(),
            network_url: outputs::AGGR_NETWORK,
            instance_type_url: outputs::AGGR_INSTANCE_TYPE,
            pwa: true,
            preferences: serde_json::json!({}),
            preference_schema: crate::config::preferences::ReaderPreferences::default().schema(),
            config_page_url: None,
            config_url: None,
            has_categories: false,
            discussions: Vec::new(),
            entry_shortcuts: Vec::new(),
        }
    }

    fn request_build() -> BuildCtx {
        BuildCtx {
            time: day(20),
            version: "2.0.0".into(),
            app_version: "app".into(),
            content_version: "content".into(),
            config_sha: None,
            data_sha: None,
            generation: "generation".into(),
            release: false,
        }
    }

    #[test]
    fn article_request_rebases_only_site_resources_and_keeps_body_out_of_page_json() {
        let site = request_site();
        let renderer = Renderer::new().unwrap();
        let item = Item {
            path: "items/blog/2026/09/article".into(),
            front: crate::model::FrontMatter {
                title: "Article".into(),
                link: "https://blog.example/article".into(),
                source: "blog".into(),
                first_seen: day(20),
                ..Default::default()
            },
            body: "Distinct article body".into(),
        };
        let mut item = ItemCtx::from_item(
            &item,
            ItemOptions {
                reading_metrics: content::reading_metrics(&item.body),
                source_name: "Blog",
                category: None,
                links: None,
                excerpt: String::new(),
                discussions: &[],
                resolutions: &crate::discussions::ResolutionSet::default(),
                now: day(20),
            },
        );
        item.body_html = Some(r##"<p>Distinct article body <a href="#note">note</a><img src="assets/picture.png"></p>"##.into());
        let shared = SharedCtx::new(&site, &request_build(), &[], &[], &[]);
        let pages = Pages::new(&site, &renderer, shared, &[], 25);
        let request = pages.simple_request(SimplePage {
            item: Some(&item),
            ..SimplePage::new("item", &item.title, &item.url)
        });
        assert!(request.body_html.contains(r##"href="#note""##));
        assert!(
            request
                .body_html
                .contains(&format!(r#"src="{}assets/picture.png""#, request.page.base))
        );
        assert!(
            !serde_json::to_string(&request.page)
                .unwrap()
                .contains("Distinct article body")
        );
        assert!(matches!(request.page.page, ClientView::Article(_)));
        assert!(
            request
                .page
                .head
                .links
                .iter()
                .any(|link| link.rel == "alternate"
                    && link.mime_type.as_deref() == Some("text/markdown"))
        );
        assert!(
            request
                .page
                .head
                .meta
                .iter()
                .any(|meta| meta.name.as_deref() == Some("robots")
                    && meta.content == "noindex,follow")
        );
    }

    #[test]
    fn directory_requests_keep_configured_empty_sources_and_exclude_unlisted_publishers() {
        let site = request_site();
        let renderer = Renderer::new().unwrap();
        let source = SourceCtx {
            slug: "blog.example".into(),
            query_value: "blog.example".into(),
            name: "blog.example".into(),
            url: Some("https://blog.example/feed".into()),
            feed_url: None,
            site_url: Some("https://blog.example/".into()),
            language: None,
            category: None,
            engine: "feed".into(),
            count: 0,
            latest: None,
            error: None,
            page: "sources/blog.example/".into(),
            listed: true,
        };
        let mut hidden = source.clone();
        hidden.slug = "inferred.example".into();
        hidden.listed = false;
        let shared = SharedCtx::new(&site, &request_build(), &[source, hidden], &[], &[]);
        let pages = Pages::new(&site, &renderer, shared, &[], 25);
        let request = pages.simple_request(SimplePage::new("sources", "Sources", "sources/"));
        let ClientView::Directory(directory) = request.page.page else {
            panic!("directory model required")
        };
        assert_eq!(directory.groups.len(), 1);
        assert_eq!(directory.groups[0].entries.len(), 1);
        assert_eq!(
            directory.groups[0].entries[0].url,
            "?q=source%3A%22blog.example%22"
        );
        assert!(request.body_html.is_empty());
        let missing = pages.simple_request(SimplePage::new("404", "Not found", "404.html"));
        assert_eq!(missing.page.base, "/reader/");
        let ClientView::NotFound(not_found) = missing.page.page else {
            panic!("not-found model required")
        };
        assert_eq!(not_found.data_branch, site.data_branch);
        assert_eq!(
            not_found.archive_url,
            site.repository.as_ref().map(|repository| format!(
                "https://github.com/{repository}/tree/{}/items",
                site.data_branch
            ))
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
                hermetic: false,
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
            let renderer = Renderer::new().unwrap();
            let shared = SharedCtx::new(&site, &build, &[], &[], &[]);
            let pages = Pages::new(&site, &renderer, shared, &[], 25);
            let html = pages
                .simple(SimplePage {
                    item: Some(item),
                    ..SimplePage::new("item", &item.title, &item.url)
                })
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
