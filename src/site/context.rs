//! The template contract: everything a theme can see, as plain serializable structs. Documented
//! for theme authors in `docs/themes.md`; changing a field here is a theme-facing change.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::model::{ContentKind, Item};

#[derive(Debug, Clone, Serialize)]
pub struct SiteCtx {
    pub title: String,
    pub description: String,
    /// Path prefix every site link is built on, always starting and ending with `/`.
    pub base_path: String,
    /// Absolute URL of the site root when known (feed and canonical links).
    pub base_url: Option<String>,
    pub repository: Option<String>,
    pub data_branch: String,
    /// Whether `manifest.webmanifest` and `sw.js` are built (`[site] pwa`).
    pub pwa: bool,
    pub config_url: Option<String>,
    pub discussions: Vec<DiscussionLinkCtx>,
    pub params: toml::Table,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiscussionLinkCtx {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildCtx {
    pub time: DateTime<Utc>,
    pub version: String,
    pub config_sha: Option<String>,
    pub data_sha: Option<String>,
    pub release: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PageCtx {
    /// `river`, `source`, `category`, `tag`, taxonomy indexes, `item`, `sources`, `search`,
    /// `404`, or `offline`.
    pub kind: String,
    pub title: String,
    /// Site path of this page, e.g. `sources/rust-blog/`.
    pub path: String,
    pub number: usize,
    pub total: usize,
    /// Rank of the first item on this page, 0-based.
    pub offset: usize,
    pub prev: Option<String>,
    pub next: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ItemCtx {
    /// `items/<source>/<yyyy>/<mm>/<stem>` — the identity used by localStorage and search.
    pub path: String,
    /// Site path of the item page, e.g. `items/rust-blog/2026/09/2026-09-02-hello/`.
    pub url: String,
    pub title: String,
    pub link: String,
    pub domain: String,
    pub source: String,
    pub source_name: String,
    pub category: Option<String>,
    pub date: DateTime<Utc>,
    pub published: Option<DateTime<Utc>>,
    pub updated: Option<DateTime<Utc>>,
    pub first_seen: DateTime<Utc>,
    pub authors: Vec<String>,
    pub labels: Vec<String>,
    pub discussions: Vec<DiscussionLinkCtx>,
    pub summary: Option<String>,
    pub excerpt: String,
    pub search_text: String,
    pub content: ContentKind,
    pub has_html: bool,
    pub extra: BTreeMap<String, serde_yaml_ng::Value>,
    /// GitHub URLs pinned to the data commit; `None` when the repository is unknown.
    pub permalink: Option<String>,
    pub raw_url: Option<String>,
    pub history_url: Option<String>,
    pub edit_url: Option<String>,
    /// What `git hash-object` gives for the `.md` file.
    pub blob_sha: String,
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
    format!("{path}/")
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
    pub fn from_item(
        item: &Item,
        source_name: &str,
        category: Option<&str>,
        links: Option<&GitHubLinks<'_>>,
        blob_sha: String,
        excerpt: String,
        discussions: &[crate::config::DiscussionLinkConfig],
    ) -> Self {
        let md = item.md_path();
        Self {
            path: item.path.clone(),
            url: item_url(&item.path),
            title: item.front.title.clone(),
            link: item.front.link.clone(),
            domain: domain_of(&item.front.link),
            source: item.front.source.clone(),
            source_name: source_name.to_string(),
            category: category.map(str::to_string),
            date: item.created_at(),
            published: item.front.published,
            updated: item.front.updated,
            first_seen: item.front.first_seen,
            authors: item.front.authors.clone(),
            labels: item.front.labels.clone(),
            discussions: discussions
                .iter()
                .map(|discussion| DiscussionLinkCtx {
                    name: discussion.name.clone(),
                    url: discussion
                        .url
                        .replace("{url}", &encode_query(&item.front.link))
                        .replace("{title}", &encode_query(&item.front.title)),
                })
                .collect(),
            summary: item.front.summary.clone(),
            excerpt,
            search_text: item.body.clone(),
            content: item.front.content,
            has_html: item.front.html.is_some(),
            extra: item.front.extra.clone(),
            permalink: links.map(|l| l.permalink(&md)),
            raw_url: links.map(|l| l.raw(&md)),
            history_url: links.map(|l| l.history(&md)),
            edit_url: links.map(|l| l.edit(&md)),
            blob_sha,
            body_html: None,
        }
    }
}

fn encode_query(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
