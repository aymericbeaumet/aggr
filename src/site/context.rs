//! The template contract: everything a theme can see, as plain serializable structs. Documented
//! for theme authors in `docs/themes.md`; changing a field here is a theme-facing change.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::{
    content,
    model::{ContentKind, Item, normalize_category, normalize_labels},
};

#[derive(Debug, Clone, Serialize)]
pub struct SiteCtx {
    pub title: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<SiteIdentityCtx>,
    pub language: String,
    /// Path prefix every site link is built on, always starting and ending with `/`.
    pub base_path: String,
    /// Absolute URL of the site root when known (feed and canonical links).
    pub base_url: Option<String>,
    pub repository: Option<String>,
    pub data_branch: String,
    /// Canonical identity shared by every generated aggr instance.
    pub network_url: &'static str,
    /// Machine-readable semantic type for an aggr instance.
    pub instance_type_url: &'static str,
    /// Whether `manifest.webmanifest` and `sw.js` are built (`[site] pwa`).
    pub pwa: bool,
    /// Browser-facing GitHub page for the source config when the build commit is known.
    pub config_page_url: Option<String>,
    /// Raw source config URL used by machine-readable discovery metadata.
    pub config_url: Option<String>,
    /// Whether the build has at least one non-empty category archive.
    pub has_categories: bool,
    pub discussions: Vec<DiscussionLinkCtx>,
    /// First nine feed entries, available to global `g 1` … `g 9` shortcuts.
    pub entry_shortcuts: Vec<String>,
    pub params: toml::Table,
}

#[derive(Debug, Clone, Serialize)]
pub struct SiteIdentityCtx {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub same_as: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiscussionLinkCtx {
    pub name: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shortcut: Option<String>,
    /// A direct link to a matching discussion rather than the provider's search page.
    pub found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildCtx {
    pub time: DateTime<Utc>,
    pub version: String,
    pub config_sha: Option<String>,
    pub data_sha: Option<String>,
    /// Content + semantic-time fingerprint used to version offline caches exactly when output
    /// can change without a new data commit.
    pub generation: String,
    pub release: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PageCtx {
    /// `river`, `source`, `category`, `tag`, `library`, `item`, `search`, `preferences`, `404`,
    /// or `offline`.
    pub kind: String,
    pub title: String,
    pub document_title: String,
    pub description: String,
    /// Whether crawlers should index this page. Search, preferences, offline and error shells
    /// remain useful to people and link discovery, but are not useful search results.
    pub indexable: bool,
    /// Site path of this page, e.g. `sources/rust-blog/`.
    pub path: String,
    /// Relative path from this page back to the output root (`../../`).
    pub root: String,
    /// Absolute public URL when this build knows one.
    pub canonical_url: Option<String>,
    /// Site-relative collection whose Atom/RSS/JSON feeds this page advertises.
    pub feed_path: Option<String>,
    /// Present for list pages. The shape follows Zola's paginator template contract so themes
    /// can use the same first/last/previous/next mental model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paginator: Option<PaginatorCtx>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PaginatorCtx {
    /// Maximum number of entries in one pager.
    pub paginate_by: usize,
    /// Route prefix for numbered pagers, e.g. `sources/rust/page/`.
    pub base_url: String,
    /// Number of generated pagers.
    pub number_pagers: usize,
    /// Routes for the two edges. These are always present, including on an edge pager.
    pub first: String,
    pub last: String,
    pub previous: Option<String>,
    pub next: Option<String>,
    /// Current pager, 1-indexed.
    pub current_index: usize,
    /// Number of entries across every pager.
    pub total_items: usize,
    /// Rank of the first entry on this pager, 0-based.
    pub offset: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ItemCtx {
    /// `items/<source>/<yyyy>/<mm>/<stem>` — the identity used by tab state and search.
    pub path: String,
    /// Flat site path of the item page, e.g. `items/rust-blog/2026-09-02-hello/`.
    pub url: String,
    pub title: String,
    pub link: String,
    pub domain: String,
    pub source: String,
    pub source_name: String,
    /// Publisher host, retaining the channel/profile path on shared platforms.
    pub source_display: String,
    /// Website/channel title, or publisher with feed provenance for aggregate feeds.
    pub source_title: String,
    pub source_url: String,
    /// Configured feed identity, independent of the publisher of an individual item.
    pub feed_display: String,
    pub is_aggregated: bool,
    pub is_youtube: bool,
    pub category: Option<String>,
    pub date: DateTime<Utc>,
    /// Stable build-time age bucket used for the 1h, 3h and 24h visual boundaries.
    pub age_band: &'static str,
    pub published: Option<DateTime<Utc>>,
    pub updated: Option<DateTime<Utc>>,
    pub first_seen: DateTime<Utc>,
    pub replicated_at: Option<DateTime<Utc>>,
    pub authors: Vec<String>,
    pub labels: Vec<String>,
    pub discussions: Vec<DiscussionLinkCtx>,
    pub summary: Option<String>,
    pub excerpt: String,
    pub content: ContentKind,
    /// Visible body words and a rounded-up estimate at 225 words per minute.
    pub word_count: usize,
    pub reading_minutes: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<PreviewCtx>,
    /// Responsive local lead image or video poster, populated only on the article page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub article_preview: Option<ArticlePreviewCtx>,
    /// Validated provider settings, populated only on the article page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video: Option<super::video::VideoCtx>,
    pub extra: BTreeMap<String, serde_yaml_ng::Value>,
    /// GitHub URLs pinned to the data commit; `None` when the repository is unknown.
    pub permalink: Option<String>,
    pub raw_url: Option<String>,
    pub history_url: Option<String>,
    pub edit_url: Option<String>,
    /// Chronological navigation and non-adjacent suggestions resolved once at build time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_article: Option<ArticleLinkCtx>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_article: Option<ArticleLinkCtx>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub recommended_articles: Vec<ArticleLinkCtx>,
    /// Rendered Markdown; only filled on the item's own page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_html: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PreviewCtx {
    pub url: String,
    pub width: u32,
    pub height: u32,
    pub alt: Option<String>,
    pub color: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArticlePreviewCtx {
    pub url: String,
    pub width: u32,
    pub height: u32,
    pub srcset: String,
    pub color: String,
}

impl ArticlePreviewCtx {
    pub fn lead_image(
        body_html: &str,
        base: &url::Url,
        images: &[crate::content::LocalImage],
    ) -> Option<Self> {
        let body_sources = crate::media::body_candidates(body_html, base)
            .into_iter()
            .map(|candidate| candidate.url.to_string())
            .collect::<BTreeSet<_>>();
        let body_originals = images
            .iter()
            .filter(|image| body_sources.contains(&image.source))
            .map(|image| image.original.as_str())
            .collect::<BTreeSet<_>>();
        images
            .iter()
            .find(|image| {
                !body_sources.contains(&image.source)
                    && !body_originals.contains(image.original.as_str())
            })
            .map(Self::from_image)
    }

    pub fn from_image(image: &crate::content::LocalImage) -> Self {
        let mut candidates = image
            .variants
            .iter()
            .filter(|variant| variant.width >= 320 && variant.width < image.width)
            .map(|variant| (variant.width, variant.url.as_str()))
            .collect::<BTreeMap<_, _>>();
        candidates.insert(image.width, image.original.as_str());
        let url = candidates
            .range(640..)
            .next()
            .or_else(|| candidates.last_key_value())
            .map(|(_, url)| (*url).to_string())
            .unwrap_or_else(|| image.original.clone());
        Self {
            url,
            width: image.width,
            height: image.height,
            srcset: candidates
                .iter()
                .map(|(width, url)| format!("{url} {width}w"))
                .collect::<Vec<_>>()
                .join(", "),
            color: image.color.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceCtx {
    pub slug: String,
    pub name: String,
    pub url: Option<String>,
    pub site_url: Option<String>,
    pub category: Option<String>,
    pub engine: String,
    pub count: usize,
    pub latest: Option<DateTime<Utc>>,
    pub error: Option<SourceErrorCtx>,
    /// Site path of the per-source page.
    pub page: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceErrorCtx {
    pub message: String,
    pub since: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CategoryCtx {
    pub name: String,
    pub slug: String,
    pub count: usize,
    pub latest: Option<DateTime<Utc>>,
    pub page: String,
}

/// GitHub URLs for a file on the data branch.
pub struct GitHubLinks<'a> {
    pub repository: &'a str,
    pub branch: &'a str,
    pub data_sha: Option<&'a str>,
}

impl GitHubLinks<'_> {
    pub fn permalink(&self, file: &str) -> String {
        let rev = self.data_sha.unwrap_or(self.branch);
        format!("https://github.com/{}/blob/{rev}/{file}", self.repository)
    }

    pub fn raw(&self, file: &str) -> String {
        let rev = self.data_sha.unwrap_or(self.branch);
        format!(
            "https://raw.githubusercontent.com/{}/{rev}/{file}",
            self.repository
        )
    }

    pub fn history(&self, file: &str) -> String {
        format!(
            "https://github.com/{}/commits/{}/{file}",
            self.repository, self.branch
        )
    }

    pub fn edit(&self, file: &str) -> String {
        format!(
            "https://github.com/{}/edit/{}/{file}",
            self.repository, self.branch
        )
    }
}

pub fn item_url(path: &str) -> String {
    let mut parts = path.split('/');
    let _items = parts.next();
    let source = parts.next().unwrap_or("unknown");
    let slug = path.rsplit('/').next().unwrap_or("item");
    format!("items/{source}/{slug}/")
}

pub fn domain_of(link: &str) -> String {
    url::Url::parse(link)
        .ok()
        .and_then(|url| {
            url.host_str()
                .map(|host| host.trim_start_matches("www.").to_string())
        })
        .unwrap_or_default()
}

struct SourceIdentity {
    display: String,
    title: String,
    url: String,
    feed_display: String,
    is_aggregated: bool,
}

fn publisher_url(link: &str) -> String {
    url::Url::parse(link)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https"))
        .map(|mut url| {
            url.set_path("/");
            url.set_query(None);
            url.set_fragment(None);
            url.to_string()
        })
        .unwrap_or_default()
}

fn source_host(url: &url::Url) -> &str {
    match url
        .host_str()
        .unwrap_or_default()
        .trim_start_matches("www.")
    {
        "youtu.be" | "m.youtube.com" => "youtube.com",
        "twitter.com" => "x.com",
        host => host,
    }
}

fn profile_url(value: &str) -> Option<url::Url> {
    let mut url = url::Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let mut path = url.path().trim_end_matches('/').to_string();
    if source_host(&url) == "youtube.com" {
        if path == "/feeds/videos.xml" {
            path = url
                .query_pairs()
                .find_map(|(key, value)| match key.as_ref() {
                    "channel_id" => Some(format!("/channel/{value}")),
                    "user" => Some(format!("/user/{value}")),
                    _ => None,
                })?;
        } else if !(path.starts_with("/@")
            || path.starts_with("/channel/")
            || path.starts_with("/user/")
            || path.starts_with("/c/"))
        {
            return None;
        }
        let segments: Vec<_> = path.trim_start_matches('/').split('/').collect();
        let length = if segments.first()?.starts_with('@') {
            1
        } else {
            2
        };
        path = format!("/{}", segments[..segments.len().min(length)].join("/"));
    } else {
        // Feed URLs describe the transport; the discovered website describes its publisher.
        let endpoint = path.rsplit('/').next().unwrap_or_default();
        if matches!(
            endpoint,
            "feed" | "rss" | "atom" | "feed.xml" | "rss.xml" | "atom.xml" | "index.xml"
        ) {
            path.truncate(path.len() - endpoint.len());
        } else if path.ends_with(".rss") || path.ends_with(".atom") {
            path.truncate(path.rfind('.').unwrap_or(path.len()));
        } else if path.ends_with(".xml") {
            return None;
        }
        if path.split('/').any(|segment| segment == "api") {
            return None;
        }
    }
    url.set_path(path.trim_end_matches('/'));
    url.set_query(None);
    url.set_fragment(None);
    Some(url)
}

fn url_label(url: &url::Url) -> String {
    format!("{}{}", source_host(url), url.path().trim_end_matches('/'))
}

pub fn profile_label(value: &str) -> String {
    profile_url(value)
        .as_ref()
        .map(url_label)
        .unwrap_or_else(|| domain_of(value))
}

fn source_identity(
    article: &str,
    title: &str,
    configured: Option<&str>,
    website: Option<&str>,
) -> SourceIdentity {
    let configured_profile = configured.and_then(profile_url);
    let website_profile = website.and_then(profile_url);
    let profile = configured_profile.as_ref().or(website_profile.as_ref());
    let feed_display = profile.map(url_label).unwrap_or_else(|| title.to_string());
    let article_url = url::Url::parse(article).ok();
    let matching_profile = [configured_profile.as_ref(), website_profile.as_ref()]
        .into_iter()
        .flatten()
        .find(|profile| {
            article_url
                .as_ref()
                .is_some_and(|article| source_host(article) == source_host(profile))
        });
    let is_aggregated = profile.is_some() && matching_profile.is_none();
    let display = matching_profile
        .map(url_label)
        .unwrap_or_else(|| domain_of(article));
    SourceIdentity {
        title: if is_aggregated {
            format!("{display} · via {title}")
        } else {
            title.to_string()
        },
        url: matching_profile
            .map(ToString::to_string)
            .unwrap_or_else(|| publisher_url(article)),
        display,
        feed_display,
        is_aggregated,
    }
}

pub fn category_slug(name: &str) -> String {
    let slug = slug::slugify(name);
    if slug.is_empty() {
        "other".into()
    } else {
        slug
    }
}

impl ItemCtx {
    pub fn set_source(&mut self, source: &SourceCtx) {
        let identity = source_identity(
            &self.link,
            &source.name,
            source.url.as_deref(),
            source.site_url.as_deref(),
        );
        self.source_display = identity.display;
        self.source_title = identity.title;
        self.source_url = identity.url;
        self.feed_display = identity.feed_display;
        self.is_aggregated = identity.is_aggregated;
    }

    pub fn from_item(item: &Item, options: ItemOptions<'_>) -> Self {
        let md = item.md_path();
        let (word_count, reading_minutes) = content::reading_metrics(&item.body);
        Self {
            path: item.path.clone(),
            url: item_url(&item.path),
            title: item.front.title.clone(),
            link: item.front.link.clone(),
            domain: domain_of(&item.front.link),
            source: item.front.source.clone(),
            source_name: options.source_name.to_string(),
            source_display: domain_of(&item.front.link),
            source_title: options.source_name.to_string(),
            source_url: publisher_url(&item.front.link),
            feed_display: options.source_name.to_string(),
            is_aggregated: false,
            is_youtube: url::Url::parse(&item.front.link)
                .is_ok_and(|url| crate::sources::youtube::is_video_url(&url)),
            category: options.category.and_then(normalize_category),
            date: item.created_at(),
            age_band: age_band(options.now, item.created_at()),
            published: item.front.published,
            updated: item.front.updated,
            first_seen: item.front.first_seen,
            replicated_at: item.front.replicated_at,
            authors: item.front.authors.clone(),
            labels: normalize_labels(&item.front.labels),
            discussions: options
                .discussions
                .iter()
                .filter_map(|discussion| {
                    let provider = discussion.provider?;
                    let found = options.resolutions.get(provider, &item.front.link)?;
                    Some(DiscussionLinkCtx {
                        name: compact_name(&discussion.name),
                        url: found.url.clone(),
                        shortcut: None,
                        found: true,
                        score: Some(found.score),
                    })
                })
                .collect(),
            summary: item.front.summary.clone(),
            excerpt: options.excerpt,
            content: item.front.content,
            word_count,
            reading_minutes,
            preview: None,
            article_preview: None,
            video: None,
            extra: item.front.extra.clone(),
            permalink: options.links.map(|l| l.permalink(&md)),
            raw_url: options.links.map(|l| l.raw(&md)),
            history_url: options.links.map(|l| l.history(&md)),
            edit_url: options.links.map(|l| l.edit(&md)),
            previous_article: None,
            next_article: None,
            recommended_articles: Vec::new(),
            body_html: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArticleLinkCtx {
    pub title: String,
    pub url: String,
    pub domain: String,
    pub source_title: String,
    pub source: String,
    pub date: DateTime<Utc>,
}

impl From<&ItemCtx> for ArticleLinkCtx {
    fn from(item: &ItemCtx) -> Self {
        Self {
            title: item.title.clone(),
            url: item.url.clone(),
            domain: item.source_display.clone(),
            source_title: item.source_title.clone(),
            source: item.source.clone(),
            date: item.date,
        }
    }
}

pub struct ItemOptions<'a> {
    pub source_name: &'a str,
    pub category: Option<&'a str>,
    pub links: Option<&'a GitHubLinks<'a>>,
    pub excerpt: String,
    pub discussions: &'a [crate::config::NetworkConfig],
    pub resolutions: &'a crate::discussions::ResolutionSet,
    pub now: DateTime<Utc>,
}

/// Age bands are half-open: exactly 1h enters `h1`, exactly 3h enters `h3`, and exactly 24h
/// enters `h24`. Future dates are treated as fresh.
pub fn age_band(now: DateTime<Utc>, date: DateTime<Utc>) -> &'static str {
    let age = now
        .signed_duration_since(date)
        .max(chrono::Duration::zero());
    if age < chrono::Duration::hours(1) {
        "fresh"
    } else if age < chrono::Duration::hours(3) {
        "h1"
    } else if age < chrono::Duration::hours(24) {
        "h3"
    } else {
        "h24"
    }
}

pub fn compact_name(name: &str) -> String {
    name.chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Assign deterministic, collision-free uppercase shortcuts to enabled discussion networks.
/// Existing reader keys are reserved; later networks fall through to the next unused letter in
/// their configured name.
pub fn discussion_shortcuts(networks: &[crate::config::NetworkConfig]) -> Vec<Option<String>> {
    let mut used = BTreeSet::from(['D', 'G', 'J', 'K', 'O', 'U']);
    networks
        .iter()
        .map(|network| {
            network
                .name
                .chars()
                .filter(|character| character.is_ascii_alphabetic())
                .map(|character| character.to_ascii_uppercase())
                .find(|character| used.insert(*character))
                .map(|character| character.to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn article_lead_is_local_and_never_duplicates_a_body_image() {
        let base = url::Url::parse("https://publisher.test/article").unwrap();
        let image = crate::content::LocalImage {
            source: "https://publisher.test/lead.jpg".into(),
            original: "assets/images/master.jpg".into(),
            variants: vec![],
            width: 1200,
            height: 800,
            color: "#123456".into(),
        };
        let lead = super::ArticlePreviewCtx::lead_image(
            "<p>Article</p>",
            &base,
            std::slice::from_ref(&image),
        )
        .unwrap();
        assert_eq!(lead.url, "assets/images/master.jpg");
        assert!(
            super::ArticlePreviewCtx::lead_image(
                "<img src='/lead.jpg#photo'>",
                &base,
                std::slice::from_ref(&image)
            )
            .is_none()
        );
        let alias = crate::content::LocalImage {
            source: "https://publisher.test/lead.jpg?size=full".into(),
            ..image.clone()
        };
        assert!(
            super::ArticlePreviewCtx::lead_image("<img src='/lead.jpg'>", &base, &[alias, image])
                .is_none()
        );
    }

    #[test]
    fn article_poster_uses_responsive_images_without_tiny_placeholder() {
        let image = crate::content::LocalImage {
            source: "https://i.ytimg.com/vi/video/maxresdefault.jpg".into(),
            original: "assets/images/master.jpg".into(),
            variants: [48, 320, 640, 960, 1280]
                .into_iter()
                .map(|width| crate::content::LocalImageVariant {
                    url: format!("assets/images/{width}.webp"),
                    width,
                    height: width * 9 / 16,
                })
                .collect(),
            width: 1280,
            height: 720,
            color: "#123456".into(),
        };
        let poster = super::ArticlePreviewCtx::from_image(&image);
        assert_eq!(poster.url, "assets/images/640.webp");
        assert_eq!((poster.width, poster.height), (1280, 720));
        assert_eq!(
            poster.srcset,
            "assets/images/320.webp 320w, assets/images/640.webp 640w, assets/images/960.webp 960w, assets/images/master.jpg 1280w"
        );
        let fallback = super::ArticlePreviewCtx::from_image(&crate::content::LocalImage {
            variants: vec![],
            width: 480,
            height: 360,
            ..image
        });
        assert_eq!(fallback.url, "assets/images/master.jpg");
        assert_eq!(fallback.srcset, "assets/images/master.jpg 480w");
    }

    use super::*;
    use chrono::TimeZone;

    #[test]
    fn discussion_shortcuts_are_uppercase_distinct_and_reserve_original() {
        let network = |name: &str| crate::config::NetworkConfig {
            name: name.into(),
            url: "https://example.test/?q={url}".into(),
            provider: None,
        };
        let shortcuts = discussion_shortcuts(&[
            network("Hacker News"),
            network("Reddit"),
            network("X"),
            network("Open Web"),
            network("Hacker Forum"),
        ]);
        assert_eq!(
            shortcuts,
            vec![Some("H"), Some("R"), Some("X"), Some("P"), Some("A")]
                .into_iter()
                .map(|shortcut| shortcut.map(str::to_string))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn age_bands_change_at_exact_visual_boundaries() {
        let now = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap();
        assert_eq!(age_band(now, now + chrono::Duration::minutes(1)), "fresh");
        assert_eq!(age_band(now, now - chrono::Duration::minutes(59)), "fresh");
        assert_eq!(age_band(now, now - chrono::Duration::hours(1)), "h1");
        assert_eq!(age_band(now, now - chrono::Duration::minutes(179)), "h1");
        assert_eq!(age_band(now, now - chrono::Duration::hours(3)), "h3");
        assert_eq!(age_band(now, now - chrono::Duration::hours(24)), "h24");
    }

    #[test]
    fn github_links_pin_to_the_data_sha() {
        let links = GitHubLinks {
            repository: "o/r",
            branch: "aggr",
            data_sha: Some("abc123"),
        };
        assert_eq!(
            links.permalink("items/x/a.md"),
            "https://github.com/o/r/blob/abc123/items/x/a.md"
        );
        assert_eq!(
            links.raw("items/x/a.md"),
            "https://raw.githubusercontent.com/o/r/abc123/items/x/a.md"
        );
        assert_eq!(
            links.history("items/x/a.md"),
            "https://github.com/o/r/commits/aggr/items/x/a.md"
        );
        assert_eq!(
            links.edit("items/x/a.md"),
            "https://github.com/o/r/edit/aggr/items/x/a.md"
        );
        let unpushed = GitHubLinks {
            data_sha: None,
            ..links
        };
        assert_eq!(
            unpushed.permalink("a.md"),
            "https://github.com/o/r/blob/aggr/a.md"
        );
    }

    #[test]
    fn domains_and_category_slugs() {
        assert_eq!(domain_of("https://www.example.com/x"), "example.com");
        assert_eq!(domain_of("nope"), "");
        assert_eq!(category_slug("Rust & Friends"), "rust-friends");
        assert_eq!(category_slug("???"), "other");
    }

    #[test]
    fn source_identity_preserves_channel_profiles_and_titles() {
        let cases = [
            (
                "https://www.youtube.com/@Computerphile",
                "https://www.youtube.com/channel/UC123",
                "https://www.youtube.com/watch?v=abc",
                "youtube.com/@Computerphile",
            ),
            (
                "https://www.youtube.com/feeds/videos.xml?channel_id=UC123",
                "https://www.youtube.com/channel/UC123",
                "https://youtu.be/abc",
                "youtube.com/channel/UC123",
            ),
            (
                "https://www.reddit.com/r/rust/.rss",
                "https://www.reddit.com/r/rust/",
                "https://www.reddit.com/r/rust/comments/123/title",
                "reddit.com/r/rust",
            ),
            (
                "https://mastodon.social/@rustlang.rss",
                "https://mastodon.social/@rustlang",
                "https://mastodon.social/@rustlang/123",
                "mastodon.social/@rustlang",
            ),
            (
                "https://example.com/feed.xml",
                "https://example.com/",
                "https://example.com/articles/read",
                "example.com",
            ),
        ];
        for (configured, website, article, expected) in cases {
            let identity = source_identity(
                article,
                "The channel title",
                Some(configured),
                Some(website),
            );
            assert_eq!(identity.display, expected, "{configured}");
            assert_eq!(identity.title, "The channel title");
            assert!(!identity.is_aggregated);
            assert!(!identity.url.contains(".rss"));
            assert!(!identity.url.contains("feeds/videos.xml"));
        }
    }

    #[test]
    fn aggregators_preserve_the_article_publisher_and_feed_provenance() {
        let identity = source_identity(
            "https://www.edge.org/conversation/impedance-matching",
            "Hacker News: Front Page",
            Some("https://hnrss.org/frontpage"),
            Some("https://news.ycombinator.com/"),
        );
        assert_eq!(identity.display, "edge.org");
        assert_eq!(identity.url, "https://www.edge.org/");
        assert_eq!(identity.title, "edge.org · via Hacker News: Front Page");
        assert_eq!(identity.feed_display, "hnrss.org/frontpage");
        assert!(identity.is_aggregated);
    }

    #[test]
    fn source_identity_uses_discovered_profile_for_feed_endpoints() {
        let identity = source_identity(
            "https://example.social/@alice/123",
            "Alice",
            Some("https://example.social/api/feed?user=alice"),
            Some("https://example.social/@alice"),
        );
        assert_eq!(identity.display, "example.social/@alice");
        assert_eq!(identity.url, "https://example.social/@alice");
    }

    #[test]
    fn item_urls_are_flat_while_storage_remains_date_partitioned() {
        assert_eq!(
            item_url("items/techmeme/2026/09/2026-09-02-a-story"),
            "items/techmeme/2026-09-02-a-story/"
        );
    }

    #[test]
    fn items_expose_only_resolved_discussions() {
        let now = Utc.with_ymd_and_hms(2026, 9, 5, 12, 0, 0).unwrap();
        let item = Item {
            path: "items/example/2026/09/article".into(),
            front: crate::model::FrontMatter {
                title: "An article".into(),
                link: "https://example.com/article".into(),
                source: "example".into(),
                first_seen: now,
                ..Default::default()
            },
            body: String::new(),
        };
        let configured = vec![
            crate::config::NetworkConfig {
                name: "Hacker News".into(),
                url: "https://hn.algolia.com/?q={url}".into(),
                provider: Some(crate::config::NetworkProvider::HackerNews),
            },
            crate::config::NetworkConfig {
                name: "Reddit".into(),
                url: "https://www.reddit.com/search/?q=url%3A{url}".into(),
                provider: Some(crate::config::NetworkProvider::Reddit),
            },
            crate::config::NetworkConfig {
                name: "X".into(),
                url: "https://x.com/search?q={url}".into(),
                provider: Some(crate::config::NetworkProvider::X),
            },
            crate::config::NetworkConfig {
                name: "Custom".into(),
                url: "https://discussion.example/search?q={url}".into(),
                provider: None,
            },
        ];
        let resolutions: crate::discussions::ResolutionSet =
            serde_json::from_value(serde_json::json!({
                "hackernews:https://example.com/article": {
                    "url": "https://news.ycombinator.com/item?id=42",
                    "score": 12
                },
                "x:https://example.com/article": {
                    "url": "https://x.com/example/status/42",
                    "score": 3
                }
            }))
            .unwrap();

        let context = ItemCtx::from_item(
            &item,
            ItemOptions {
                source_name: "Example",
                category: None,
                links: None,
                excerpt: String::new(),
                discussions: &configured,
                resolutions: &resolutions,
                now,
            },
        );

        assert_eq!(
            context
                .discussions
                .iter()
                .map(|discussion| (discussion.name.as_str(), discussion.url.as_str()))
                .collect::<Vec<_>>(),
            [
                ("hackernews", "https://news.ycombinator.com/item?id=42"),
                ("x", "https://x.com/example/status/42"),
            ]
        );
        assert!(
            context
                .discussions
                .iter()
                .all(|discussion| discussion.found)
        );
        let serialized = serde_json::to_string(&context).unwrap();
        assert!(!serialized.contains("reddit"));
        assert!(!serialized.contains("discussion.example"));
    }
}
