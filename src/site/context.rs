//! The template contract: everything a theme can see, as plain serializable structs. Documented
//! for theme authors in `docs/themes.md`; changing a field here is a theme-facing change.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::model::{ContentKind, Item, normalize_labels};

#[derive(Debug, Clone, Serialize)]
pub struct SiteCtx {
    pub title: String,
    pub description: String,
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
    pub config_url: Option<String>,
    /// Whether the build has at least one non-empty category archive.
    pub has_categories: bool,
    pub discussions: Vec<DiscussionLinkCtx>,
    /// First nine feed entries, available to global `g 1` … `g 9` shortcuts.
    pub entry_shortcuts: Vec<String>,
    pub params: toml::Table,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiscussionLinkCtx {
    pub name: String,
    pub url: String,
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
    /// `river`, `source`, `category`, `tag`, taxonomy indexes, `item`, `sources`, `search`,
    /// `preferences`, `404`, or `offline`.
    pub kind: String,
    pub title: String,
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
    /// `items/<source>/<yyyy>/<mm>/<stem>` — the identity used by localStorage and search.
    pub path: String,
    /// Flat site path of the item page, e.g. `items/rust-blog/2026-09-02-hello/`.
    pub url: String,
    pub title: String,
    pub link: String,
    pub domain: String,
    pub source: String,
    pub source_name: String,
    pub category: Option<String>,
    pub date: DateTime<Utc>,
    /// Stable build-time age bucket used for the 1h, 3h and 24h visual boundaries.
    pub age_band: &'static str,
    pub published: Option<DateTime<Utc>>,
    pub updated: Option<DateTime<Utc>>,
    pub first_seen: DateTime<Utc>,
    pub authors: Vec<String>,
    pub labels: Vec<String>,
    pub discussions: Vec<DiscussionLinkCtx>,
    pub summary: Option<String>,
    pub excerpt: String,
    pub content: ContentKind,
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

pub fn category_slug(name: &str) -> String {
    let slug = slug::slugify(name);
    if slug.is_empty() {
        "other".into()
    } else {
        slug
    }
}

impl ItemCtx {
    pub fn from_item(item: &Item, options: ItemOptions<'_>) -> Self {
        let md = item.md_path();
        Self {
            path: item.path.clone(),
            url: item_url(&item.path),
            title: item.front.title.clone(),
            link: item.front.link.clone(),
            domain: domain_of(&item.front.link),
            source: item.front.source.clone(),
            source_name: options.source_name.to_string(),
            category: options.category.map(str::to_string),
            date: item.created_at(),
            age_band: age_band(options.now, item.created_at()),
            published: item.front.published,
            updated: item.front.updated,
            first_seen: item.front.first_seen,
            authors: item.front.authors.clone(),
            labels: normalize_labels(&item.front.labels),
            discussions: options
                .discussions
                .iter()
                .map(|discussion| {
                    let found = discussion
                        .provider
                        .and_then(|provider| options.resolutions.get(provider, &item.front.link));
                    DiscussionLinkCtx {
                        name: compact_name(&discussion.name),
                        url: found.map(|found| found.url.clone()).unwrap_or_else(|| {
                            discussion
                                .url
                                .replace("{url}", &encode_query(&item.front.link))
                                .replace("{title}", &encode_query(&item.front.title))
                        }),
                        found: found.is_some(),
                        score: found.map(|found| found.score),
                    }
                })
                .collect(),
            summary: item.front.summary.clone(),
            excerpt: options.excerpt,
            content: item.front.content,
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
    pub source: String,
    pub date: DateTime<Utc>,
}

impl From<&ItemCtx> for ArticleLinkCtx {
    fn from(item: &ItemCtx) -> Self {
        Self {
            title: item.title.clone(),
            url: item.url.clone(),
            domain: item.domain.clone(),
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

fn encode_query(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

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
    fn item_urls_are_flat_while_storage_remains_date_partitioned() {
        assert_eq!(
            item_url("items/techmeme/2026/09/2026-09-02-a-story"),
            "items/techmeme/2026-09-02-a-story/"
        );
    }
}
