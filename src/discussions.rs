//! Build-time discovery of conversations about an article. Lookups are cached outside the data
//! branch, bounded, concurrent, and strictly optional: any provider or credential failure leaves
//! the configured search URL in place.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use url::Url;

use crate::config::{DiscussionLinkConfig, DiscussionProvider};
use crate::http;
use crate::model::{Item, normalize_link, sha1_hex};

const CACHE_NAMESPACE: &str = "discussions-v1";
const MAX_LOOKUPS: usize = 100;
const CONCURRENCY: usize = 8;
const FOUND_TTL: Duration = Duration::hours(24);
const EMPTY_TTL: Duration = Duration::hours(6);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Found {
    pub url: String,
    pub score: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResolutionSet(BTreeMap<String, Found>);

impl ResolutionSet {
    pub fn get(&self, provider: DiscussionProvider, link: &str) -> Option<&Found> {
        self.0.get(&key(provider, link))
    }

    pub fn fingerprint(&self) -> String {
        sha1_hex(serde_json::to_vec(&self.0).unwrap_or_default())
    }

    fn insert(&mut self, provider: DiscussionProvider, link: &str, found: Found) {
        self.0.insert(key(provider, link), found);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    checked_at: DateTime<Utc>,
    found: Option<Found>,
}

enum Lookup {
    Found(Found),
    Empty,
    /// Credentials or platform access are unavailable; do not turn that into a negative cache.
    Unavailable,
}

struct Job {
    provider: DiscussionProvider,
    link: String,
    cache: PathBuf,
}

pub async fn resolve(
    configs: &[DiscussionLinkConfig],
    items: &[Item],
    client: Arc<http::Client>,
    cache_root: &Path,
    now: DateTime<Utc>,
) -> Result<ResolutionSet> {
    let providers: BTreeSet<_> = configs
        .iter()
        .filter_map(|config| config.provider)
        .collect();
    if providers.is_empty() {
        return Ok(ResolutionSet::default());
    }
    let cache_root = cache_root.join(CACHE_NAMESPACE);
    std::fs::create_dir_all(&cache_root)
        .with_context(|| format!("creating {}", cache_root.display()))?;

    let mut ordered: Vec<_> = items.iter().collect();
    ordered.sort_by(|a, b| {
        b.created_at()
            .cmp(&a.created_at())
            .then_with(|| b.path.cmp(&a.path))
    });
    let mut seen = BTreeSet::new();
    let mut jobs = Vec::new();
    let mut out = ResolutionSet::default();
    for item in ordered {
        for &provider in &providers {
            let lookup_key = key(provider, &item.front.link);
            if !seen.insert(lookup_key) {
                continue;
            }
            let cache = cache_path(&cache_root, provider, &item.front.link);
            if let Some(entry) = read_cache(&cache)
                && cache_is_fresh(&entry, now)
            {
                if let Some(found) = entry.found {
                    out.insert(provider, &item.front.link, found);
                }
                continue;
            }
            if jobs.len() < MAX_LOOKUPS {
                jobs.push(Job {
                    provider,
                    link: item.front.link.clone(),
                    cache,
                });
            }
        }
    }

    let semaphore = Arc::new(Semaphore::new(CONCURRENCY));
    let mut tasks = JoinSet::new();
    for job in jobs {
        let permit = Arc::clone(&semaphore).acquire_owned().await?;
        let client = Arc::clone(&client);
        tasks.spawn(async move {
            let _permit = permit;
            let result = lookup(job.provider, &job.link, &client).await;
            (job, result)
        });
    }
    while let Some(result) = tasks.join_next().await {
        let (job, result) = result.context("discussion lookup task stopped")?;
        match result {
            Ok(Lookup::Found(found)) => {
                if let Err(err) = write_cache(
                    &job.cache,
                    &CacheEntry {
                        checked_at: now,
                        found: Some(found.clone()),
                    },
                ) {
                    log::debug!("discussion cache: {err:#}");
                }
                out.insert(job.provider, &job.link, found);
            }
            Ok(Lookup::Empty) => {
                if let Err(err) = write_cache(
                    &job.cache,
                    &CacheEntry {
                        checked_at: now,
                        found: None,
                    },
                ) {
                    log::debug!("discussion cache: {err:#}");
                }
            }
            Ok(Lookup::Unavailable) => {}
            Err(err) => log::debug!(
                "{} discussion lookup for {} fell back to search: {err:#}",
                job.provider.as_str(),
                job.link
            ),
        }
    }
    Ok(out)
}

fn key(provider: DiscussionProvider, link: &str) -> String {
    format!("{}:{}", provider.as_str(), normalize_link(link))
}

fn cache_path(root: &Path, provider: DiscussionProvider, link: &str) -> PathBuf {
    root.join(provider.as_str())
        .join(format!("{}.json", sha1_hex(normalize_link(link))))
}

fn read_cache(path: &Path) -> Option<CacheEntry> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

fn write_cache(path: &Path, entry: &CacheEntry) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, serde_json::to_vec(entry)?)
        .with_context(|| format!("writing {}", path.display()))
}

fn cache_is_fresh(entry: &CacheEntry, now: DateTime<Utc>) -> bool {
    let ttl = if entry.found.is_some() {
        FOUND_TTL
    } else {
        EMPTY_TTL
    };
    now.signed_duration_since(entry.checked_at) < ttl
}

async fn lookup(provider: DiscussionProvider, link: &str, client: &http::Client) -> Result<Lookup> {
    match provider {
        DiscussionProvider::HackerNews => hacker_news(link, client).await,
        DiscussionProvider::Reddit => reddit(link, client).await,
        DiscussionProvider::X => x(link, client).await,
    }
}

async fn get_json<T: for<'de> Deserialize<'de>>(
    url: &Url,
    headers: &[(String, String)],
    client: &http::Client,
) -> Result<T> {
    let response = client
        .get(http::Request {
            url,
            headers,
            etag: None,
            last_modified: None,
        })
        .await?;
    let http::Response::Ok(body) = response else {
        anyhow::bail!("unexpected not-modified response")
    };
    serde_json::from_slice(&body.bytes).context("parsing discussion response")
}

#[derive(Deserialize)]
struct HackerNewsResponse {
    #[serde(default)]
    hits: Vec<HackerNewsHit>,
}

#[derive(Deserialize)]
struct HackerNewsHit {
    #[serde(rename = "objectID")]
    id: String,
    url: Option<String>,
    #[serde(default)]
    points: i64,
}

async fn hacker_news(link: &str, client: &http::Client) -> Result<Lookup> {
    let mut url = Url::parse("https://hn.algolia.com/api/v1/search")?;
    url.query_pairs_mut()
        .append_pair("query", link)
        .append_pair("tags", "story")
        .append_pair("restrictSearchableAttributes", "url")
        .append_pair("hitsPerPage", "50");
    let response: HackerNewsResponse = get_json(&url, &[], client).await?;
    Ok(best_hacker_news(link, response.hits)
        .map(Lookup::Found)
        .unwrap_or(Lookup::Empty))
}

fn best_hacker_news(link: &str, hits: Vec<HackerNewsHit>) -> Option<Found> {
    let wanted = normalize_link(link);
    hits.into_iter()
        .filter(|hit| {
            hit.url
                .as_deref()
                .is_some_and(|candidate| normalize_link(candidate) == wanted)
        })
        .max_by_key(|hit| hit.points)
        .map(|hit| Found {
            url: format!("https://news.ycombinator.com/item?id={}", hit.id),
            score: hit.points,
        })
}

#[derive(Deserialize)]
struct RedditResponse {
    data: RedditListing,
}

#[derive(Deserialize)]
struct RedditListing {
    #[serde(default)]
    children: Vec<RedditChild>,
}

#[derive(Deserialize)]
struct RedditChild {
    data: RedditPost,
}

#[derive(Deserialize)]
struct RedditPost {
    url: Option<String>,
    permalink: String,
    #[serde(default)]
    score: i64,
}

async fn reddit(link: &str, client: &http::Client) -> Result<Lookup> {
    let mut url = Url::parse("https://www.reddit.com/search.json")?;
    url.query_pairs_mut()
        .append_pair("q", &format!("url:{link}"))
        .append_pair("sort", "top")
        .append_pair("t", "all")
        .append_pair("limit", "25")
        .append_pair("raw_json", "1");
    let response: RedditResponse = get_json(&url, &[], client).await?;
    Ok(best_reddit(link, response.data.children)
        .map(Lookup::Found)
        .unwrap_or(Lookup::Empty))
}

fn best_reddit(link: &str, children: Vec<RedditChild>) -> Option<Found> {
    let wanted = normalize_link(link);
    children
        .into_iter()
        .map(|child| child.data)
        .filter(|post| {
            post.url
                .as_deref()
                .is_some_and(|candidate| normalize_link(candidate) == wanted)
        })
        .max_by_key(|post| post.score)
        .map(|post| Found {
            url: format!(
                "https://www.reddit.com{}",
                if post.permalink.starts_with('/') {
                    post.permalink
                } else {
                    format!("/{}", post.permalink)
                }
            ),
            score: post.score,
        })
}

#[derive(Deserialize)]
struct XResponse {
    #[serde(default)]
    data: Vec<XPost>,
}

#[derive(Deserialize)]
struct XPost {
    id: String,
    #[serde(default)]
    public_metrics: XMetrics,
}

#[derive(Default, Deserialize)]
struct XMetrics {
    #[serde(default)]
    like_count: i64,
    #[serde(default)]
    retweet_count: i64,
    #[serde(default)]
    quote_count: i64,
    #[serde(default)]
    reply_count: i64,
}

impl XMetrics {
    fn score(&self) -> i64 {
        self.like_count + self.retweet_count + self.quote_count + self.reply_count
    }
}

async fn x(link: &str, client: &http::Client) -> Result<Lookup> {
    let Some(token) = std::env::var("X_BEARER_TOKEN")
        .ok()
        .filter(|token| !token.trim().is_empty())
    else {
        return Ok(Lookup::Unavailable);
    };
    let mut url = Url::parse("https://api.x.com/2/tweets/search/recent")?;
    url.query_pairs_mut()
        .append_pair("query", &format!("url:\"{link}\""))
        .append_pair("max_results", "100")
        .append_pair("tweet.fields", "public_metrics");
    let headers = vec![("Authorization".into(), format!("Bearer {token}"))];
    let response: XResponse = get_json(&url, &headers, client).await?;
    Ok(response
        .data
        .into_iter()
        .max_by_key(|post| post.public_metrics.score())
        .map(|post| {
            Lookup::Found(Found {
                url: format!("https://x.com/i/web/status/{}", post.id),
                score: post.public_metrics.score(),
            })
        })
        .unwrap_or(Lookup::Empty))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hn_requires_an_exact_normalized_link_and_uses_the_highest_score() {
        let hits = vec![
            HackerNewsHit {
                id: "wrong".into(),
                url: Some("https://example.com/other".into()),
                points: 999,
            },
            HackerNewsHit {
                id: "low".into(),
                url: Some("http://www.example.com/post/?utm_source=x".into()),
                points: 4,
            },
            HackerNewsHit {
                id: "high".into(),
                url: Some("https://example.com/post".into()),
                points: 42,
            },
        ];
        assert_eq!(
            best_hacker_news("https://example.com/post#section", hits),
            Some(Found {
                url: "https://news.ycombinator.com/item?id=high".into(),
                score: 42,
            })
        );
    }

    #[test]
    fn cache_ttl_is_shorter_for_a_miss() {
        let now = Utc::now();
        let miss = CacheEntry {
            checked_at: now - Duration::hours(7),
            found: None,
        };
        let hit = CacheEntry {
            checked_at: now - Duration::hours(7),
            found: Some(Found {
                url: "https://example.com".into(),
                score: 1,
            }),
        };
        assert!(!cache_is_fresh(&miss, now));
        assert!(cache_is_fresh(&hit, now));
    }

    #[test]
    fn reddit_requires_an_exact_normalized_link_and_uses_the_highest_score() {
        let post = |url: &str, permalink: &str, score| RedditChild {
            data: RedditPost {
                url: Some(url.into()),
                permalink: permalink.into(),
                score,
            },
        };
        let children = vec![
            post("https://example.com/elsewhere", "/r/rust/wrong", 900),
            post(
                "http://www.example.com/post/?utm_source=reddit",
                "r/rust/low",
                3,
            ),
            post("https://example.com/post", "/r/programming/high", 81),
        ];
        assert_eq!(
            best_reddit("https://example.com/post#comments", children),
            Some(Found {
                url: "https://www.reddit.com/r/programming/high".into(),
                score: 81,
            })
        );
    }

    #[test]
    fn x_engagement_score_includes_every_public_metric() {
        assert_eq!(
            XMetrics {
                like_count: 5,
                retweet_count: 3,
                quote_count: 2,
                reply_count: 1,
            }
            .score(),
            11
        );
    }

    #[test]
    fn resolution_fingerprint_is_stable_and_lookup_uses_normalized_urls() {
        let mut first = ResolutionSet::default();
        first.insert(
            DiscussionProvider::HackerNews,
            "http://www.example.com/a/?utm_source=x",
            Found {
                url: "https://news.ycombinator.com/item?id=1".into(),
                score: 5,
            },
        );
        assert_eq!(
            first
                .get(DiscussionProvider::HackerNews, "https://example.com/a")
                .unwrap()
                .score,
            5
        );
        assert_eq!(first.fingerprint(), first.fingerprint());
    }
}
