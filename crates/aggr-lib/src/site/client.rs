//! Prepare the typed page contract from local archive and site values.

use super::context::{BuildCtx, CategoryCtx, ItemCtx, PageCtx, PreviewCtx, SiteCtx, SourceCtx};
use crate::model::ContentKind;
pub use aggr_ui_model::*;

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
                hermetic: site.hermetic,
                preference_rules: site.preference_schema.bootstrap.clone(),
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

impl Shared {
    fn page(&self, page: &PageCtx, view: ClientView) -> ClientPage {
        ClientPage {
            head: ClientHead::default(),
            base: page.root.clone(),
            path: page.path.clone(),
            kind: page.kind.clone(),
            title: page.title.clone(),
            site: self.site.clone(),
            build: self.build.clone(),
            page: view,
        }
    }

    /// One pager of a list: the river, or a source, category or tag archive.
    pub(super) fn list(
        &self,
        page: &PageCtx,
        paginator: &PaginatorCtx,
        rows: &[ItemCtx],
        source: Option<&SourceCtx>,
        category: Option<&CategoryCtx>,
    ) -> ClientPage {
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
        self.page(
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
        &self,
        page: &PageCtx,
        item: Option<&ItemCtx>,
        site: &SiteCtx,
    ) -> ClientPage {
        let view = match (page.kind.as_str(), item) {
            (_, Some(item)) => ClientView::Article(Box::new(ClientArticle::from(item))),
            ("preferences", None) => ClientView::Preferences(Box::new(ClientPreferences {
                schema: site.preference_schema.clone(),
                values: site.preferences.clone(),
            })),
            ("offline", None) => ClientView::Offline,
            _ => ClientView::NotFound(ClientNotFound {
                data_branch: site.data_branch.clone(),
                archive_url: site.repository.as_ref().map(|repository| {
                    format!(
                        "https://github.com/{repository}/tree/{}/items",
                        site.data_branch
                    )
                }),
            }),
        };
        self.page(page, view)
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
            access: access(item),
            has_margin_notes: item.has_margin_notes,
            media: media(item),
            lead: item.article_preview.as_ref().map(|preview| ClientLead {
                url: preview.url.clone(),
                srcset: preview.srcset.clone(),
                width: preview.width,
                height: preview.height,
                alt: preview.alt.clone(),
                color: Some(preview.color.clone()),
                placeholder: ClientPlaceholder {
                    data_url: preview.placeholder.data_url.clone(),
                },
            }),
            previous: item.previous_article.clone(),
            next: item.next_article.clone(),
            recommended: item.recommended_articles.clone(),
        }
    }
}

fn access(item: &ItemCtx) -> ClientAccess {
    let flag = |key: &str| {
        item.extra
            .get(key)
            .and_then(serde_yaml_ng::Value::as_bool)
            .unwrap_or(false)
    };
    if flag("subscription_required") {
        return ClientAccess::SubscriptionRequired {
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
            return ClientAccess::TitlesOnly;
        }
        return ClientAccess::Missing;
    }
    ClientAccess::Normal
}

fn media(item: &ItemCtx) -> Option<ClientMedia> {
    if let Some(document) = &item.document {
        Some(ClientMedia::Document {
            url: document.local_url.as_ref().unwrap_or(&document.url).clone(),
        })
    } else if let Some(video) = &item.video {
        Some(ClientMedia::Video {
            provider: video.provider.into(),
            title: video.title.into(),
            embed_url: video.embed_url.clone(),
            requires_parent: video.requires_parent,
        })
    } else if let Some(native) = &item.native_media {
        Some(if native.kind == "video" {
            ClientMedia::NativeVideo {
                url: native.url.clone(),
            }
        } else {
            ClientMedia::NativeAudio {
                url: native.url.clone(),
            }
        })
    } else {
        item.interactive
            .as_ref()
            .map(|interactive| ClientMedia::Interactive {
                url: interactive.source_url.clone(),
            })
    }
}
