//! The typed page model the reader boots from. Every HTML page embeds one `ClientPage` as JSON
//! (`<script id="aggr-model">`); the Svelte client mounts from it instead of reading the DOM, and
//! `ts-rs` exports these structs to `web/src/generated/` so both sides share one contract. Field
//! names stay snake_case, matching the templates and the other JSON outputs. Every URL is a site
//! path (relative to the site root), which the client joins onto `base` once.

use serde::Serialize;

use super::context::{
    BuildCtx, CategoryCtx, DiscussionLinkCtx, ItemCtx, PageCtx, PaginatorCtx, PreviewCtx, SiteCtx,
    SourceCtx,
};
use super::display::Metadata;
use crate::config::preferences::PreferenceSchema;
use crate::content::ResourceLink;
use crate::model::ContentKind;

#[cfg(test)]
mod parity;

/// Everything the client needs to render and navigate one page.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
pub struct ClientPage {
    /// Relative path from this page to the site root (`../../`); the client resolves it once.
    pub base: String,
    /// Site path of this page, `""` for the root.
    pub path: String,
    /// `river`, `source`, `category`, `tag`, `item`, `browse`, `preferences`, `404`, `offline`, …
    pub kind: String,
    pub title: String,
    pub site: ClientSite,
    pub build: ClientBuild,
    pub page: ClientView,
}

/// Site-wide values, identical on every page of one build.
#[derive(Debug, Clone, Default, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
pub struct ClientSite {
    pub title: String,
    pub language: String,
    /// Whether a manifest and service worker were built.
    pub pwa: bool,
    /// Whether rows show excerpts (`[site.params] excerpts`).
    pub excerpts: bool,
    pub discussions: Vec<DiscussionLinkCtx>,
    /// The first nine feed entries, for the `g 1` … `g 9` shortcuts.
    pub entries: Vec<String>,
    /// Browser-facing page for the source config, when the build knows one; the header links
    /// the site's own `aggr.toml` copy otherwise.
    pub config_url: Option<String>,
    pub assets: ClientAssets,
}

/// Site paths of the compiled client, resolved through Vite's manifest at build time.
#[derive(Debug, Clone, Default, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
pub struct ClientAssets {
    pub app: Option<String>,
    pub css: Option<String>,
    /// Chunks the entry imports eagerly, for `<link rel="modulepreload">`.
    pub imports: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
pub struct ClientBuild {
    pub version: String,
    /// Binary release and effective template/static bytes, independent of feed or config data.
    pub app: String,
    /// Semantic content and rendered configuration, independent of rebuild time and app bytes.
    pub content: String,
}

/// What the page shows. Lists and articles are client-rendered; preferences and the offline page
/// are client-only; everything else is static markup the client adopts whole.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
#[serde(tag = "view", content = "data", rename_all = "snake_case")]
pub enum ClientView {
    List(Box<ClientList>),
    Article(Box<ClientArticle>),
    Preferences(Box<ClientPreferences>),
    Offline,
    Static,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
pub struct ClientList {
    pub rows: Vec<ClientRow>,
    pub paginator: PaginatorCtx,
    /// The source, category or tag this list is scoped to; `None` on the river.
    pub scope: Option<ClientScope>,
    pub error: Option<ClientSourceError>,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
pub struct ClientScope {
    /// `source`, `category` or `tag`.
    pub kind: String,
    pub slug: String,
    pub name: String,
    pub site_url: Option<String>,
    /// Readable profile label of the source's URL, shown beside its name.
    pub profile: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
pub struct ClientSourceError {
    pub message: String,
    /// RFC 3339.
    pub since: String,
}

/// One row: a feed entry, a related card, an article header or a search result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
pub struct ClientRow {
    /// `items/<source>/<yyyy>/<mm>/<stem>`, the identity used by tab state and search.
    pub path: String,
    /// Site path of the item page.
    pub url: String,
    pub link: String,
    pub title: String,
    pub language: Option<String>,
    pub excerpt: String,
    pub age_band: String,
    pub category: Option<String>,
    pub labels: Vec<ClientLabel>,
    pub preview: Option<ClientPreview>,
    pub metadata: Metadata,
}

/// A topic label with the slug its tag page and query use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
pub struct ClientLabel {
    pub name: String,
    pub slug: String,
}

/// What a row needs to draw a thumbnail. The ThumbHash stays out: the build has already decoded
/// it into `data_url`, and nothing in the reader reads the hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
pub struct ClientPreview {
    pub url: String,
    pub width: u32,
    pub height: u32,
    pub alt: Option<String>,
    pub color: Option<String>,
    pub placeholder: ClientPlaceholder,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
pub struct ClientPlaceholder {
    pub data_url: String,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
pub struct ClientArticle {
    pub header: ClientRow,
    pub authors: Vec<String>,
    pub resources: Vec<ResourceLink>,
    pub access: ClientAccess,
    pub has_margin_notes: bool,
    pub previous: Option<ClientRow>,
    pub next: Option<ClientRow>,
    pub recommended: Vec<ClientRow>,
}

/// Why an article body may be missing.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClientAccess {
    Normal,
    SubscriptionRequired {
        archive_lookup_url: Option<String>,
    },
    TitlesOnly,
    /// A feed kept no article text and no media. The original link is the way to read it.
    Missing,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
pub struct ClientPreferences {
    pub schema: PreferenceSchema,
    /// Typed initial browser settings, keyed like preference exports.
    pub values: serde_json::Value,
}

/// The per-build halves of a `ClientPage`, prepared once and cloned into every page.
#[derive(Debug, Clone, Default)]
pub(super) struct Shared {
    pub site: ClientSite,
    pub build: ClientBuild,
}

impl Shared {
    pub(super) fn new(site: &SiteCtx, build: &BuildCtx) -> Self {
        Self {
            site: ClientSite {
                title: site.title.clone(),
                language: site.language.clone(),
                pwa: site.pwa,
                excerpts: site
                    .params
                    .get("excerpts")
                    .and_then(toml::Value::as_bool)
                    .unwrap_or(false),
                discussions: site.discussions.clone(),
                entries: site.entry_shortcuts.clone(),
                config_url: site
                    .config_page_url
                    .clone()
                    .or_else(|| site.config_url.clone()),
                assets: ClientAssets::default(),
            },
            build: ClientBuild {
                version: build.version.clone(),
                app: build.app_version.clone(),
                content: build.content_version.clone(),
            },
        }
    }
}

impl ClientPage {
    fn new(shared: &Shared, page: &PageCtx, view: ClientView) -> Self {
        Self {
            base: page.root.clone(),
            path: page.path.clone(),
            kind: page.kind.clone(),
            title: page.title.clone(),
            site: shared.site.clone(),
            build: shared.build.clone(),
            page: view,
        }
    }

    /// One pager of a list: the river, or a source, category or tag archive.
    pub(super) fn list(
        shared: &Shared,
        page: &PageCtx,
        paginator: &PaginatorCtx,
        rows: &[ItemCtx],
        source: Option<&SourceCtx>,
        category: Option<&CategoryCtx>,
    ) -> Self {
        let scope = match (page.kind.as_str(), source, category) {
            ("source", Some(source), _) => Some(ClientScope {
                kind: "source".into(),
                slug: source.slug.clone(),
                name: source.name.clone(),
                site_url: source.site_url.clone(),
                profile: source
                    .url
                    .as_deref()
                    .or(source.site_url.as_deref())
                    .map(super::context::profile_label),
            }),
            (kind @ ("category" | "tag"), _, Some(category)) => Some(ClientScope {
                kind: kind.into(),
                slug: category.slug.clone(),
                name: category.name.clone(),
                site_url: None,
                profile: None,
            }),
            _ => None,
        };
        let error = source
            .and_then(|source| source.error.as_ref())
            .map(|error| ClientSourceError {
                message: error.message.clone(),
                since: error
                    .since
                    .to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true),
            });
        Self::new(
            shared,
            page,
            ClientView::List(Box::new(ClientList {
                rows: rows.iter().map(ClientRow::from).collect(),
                paginator: paginator.clone(),
                scope,
                error,
            })),
        )
    }

    /// A standalone document: an article, the preferences and offline pages, or static markup.
    pub(super) fn simple(
        shared: &Shared,
        page: &PageCtx,
        item: Option<&ItemCtx>,
        site: &SiteCtx,
    ) -> Self {
        let view = match (page.kind.as_str(), item) {
            (_, Some(item)) => ClientView::Article(Box::new(ClientArticle::from(item))),
            ("preferences", None) => ClientView::Preferences(Box::new(ClientPreferences {
                schema: site.preference_schema.clone(),
                values: site.preferences.clone(),
            })),
            ("offline", None) => ClientView::Offline,
            _ => ClientView::Static,
        };
        Self::new(shared, page, view)
    }
}

impl From<&ItemCtx> for ClientRow {
    fn from(item: &ItemCtx) -> Self {
        Self {
            path: item.path.clone(),
            url: item.url.clone(),
            link: item.link.clone(),
            title: item.title.clone(),
            language: item.language.clone(),
            excerpt: item.excerpt.clone(),
            age_band: item.age_band.to_string(),
            category: item.category.clone(),
            labels: item
                .labels
                .iter()
                .map(|name| ClientLabel {
                    name: name.clone(),
                    slug: slug::slugify(name),
                })
                .collect(),
            preview: item.preview.as_ref().map(ClientPreview::from),
            metadata: Metadata::from(item),
        }
    }
}

impl From<&PreviewCtx> for ClientPreview {
    fn from(preview: &PreviewCtx) -> Self {
        Self {
            url: preview.url.clone(),
            width: preview.width,
            height: preview.height,
            alt: preview.alt.clone(),
            color: preview.color.clone(),
            placeholder: ClientPlaceholder {
                data_url: preview.placeholder.data_url.clone(),
            },
        }
    }
}

impl From<&ItemCtx> for ClientArticle {
    fn from(item: &ItemCtx) -> Self {
        Self {
            header: ClientRow::from(item),
            authors: item.authors.clone(),
            resources: item.resources.clone(),
            access: ClientAccess::of(item),
            has_margin_notes: item.has_margin_notes,
            previous: item.previous_article.clone(),
            next: item.next_article.clone(),
            recommended: item.recommended_articles.clone(),
        }
    }
}

impl ClientAccess {
    /// Mirrors the notices `item.html` renders above the body.
    fn of(item: &ItemCtx) -> Self {
        let flag = |key: &str| {
            item.extra
                .get(key)
                .and_then(serde_yaml_ng::Value::as_bool)
                .unwrap_or(false)
        };
        if flag("subscription_required") {
            return Self::SubscriptionRequired {
                archive_lookup_url: item
                    .extra
                    .get("archive_lookup_url")
                    .and_then(serde_yaml_ng::Value::as_str)
                    .map(str::to_string),
            };
        }
        let has_media = item.video.is_some()
            || item.document.is_some()
            || item.native_media.is_some()
            || item.interactive.is_some()
            || item.article_preview.is_some();
        let blank = item
            .body_html
            .as_deref()
            .is_none_or(|html| html.trim().is_empty());
        if blank && !has_media {
            // Titles-only sources announce themselves. An empty feed body is an article the
            // capture did not keep — aggregator bookkeeping that was stripped, or a page that
            // yielded nothing — and the original link is what a reader can open.
            if item.content == ContentKind::None {
                return Self::TitlesOnly;
            }
            return Self::Missing;
        }
        Self::Normal
    }
}
