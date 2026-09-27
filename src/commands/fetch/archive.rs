//! Best-effort public archive lookup for a positively identified subscription wall.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use scraper::{Html, Selector};
use url::Url;

use crate::{cache::ArticleCache, content, http};

#[cfg(test)]
tokio::task_local! { static TEST_ENDPOINTS: (Url, Url); }

#[cfg(test)]
pub(super) async fn with_test_endpoints<T>(
    api: Url,
    lookup: Url,
    future: impl std::future::Future<Output = T>,
) -> T {
    TEST_ENDPOINTS.scope((api, lookup), future).await
}

const DEADLINE: Duration = Duration::from_secs(15);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(6);
static PROVIDERS: std::sync::LazyLock<
    tokio::sync::Mutex<std::collections::BTreeMap<String, tokio::time::Instant>>,
> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(std::collections::BTreeMap::new()));

pub(super) struct Recovered {
    pub article: content::ExtractedArticle,
    pub snapshot: String,
    pub captured_at: Option<String>,
}

pub(super) async fn recover(
    original: &Url,
    title: &str,
    client: &http::Client,
    cache: &ArticleCache,
) -> Option<Recovered> {
    if !matches!(original.scheme(), "http" | "https")
        || !original.username().is_empty()
        || original.password().is_some()
    {
        return None;
    }
    let now = chrono::Utc::now().timestamp();
    if !cache.archive_retry_ready(original, now).unwrap_or(true) {
        return None;
    }
    let api = Url::parse("https://archive.org/wayback/available").ok()?;
    let lookup = Url::parse(&content::archive_lookup_url(original)).ok()?;
    #[cfg(test)]
    let (api, lookup) = TEST_ENDPOINTS
        .try_with(Clone::clone)
        .unwrap_or((api, lookup));
    let recovered = match tokio::time::timeout(
        DEADLINE,
        recover_from(original, title, client, cache, &api, &lookup),
    )
    .await
    {
        Ok(Ok(article)) => article,
        Ok(Err(error)) => {
            log::debug!("public archive unavailable for {original}: {error:#}");
            None
        }
        Err(_) => {
            log::debug!("public archive lookup timed out for {original}");
            None
        }
    };
    if recovered.is_none()
        && let Err(error) = cache.record_archive_failure(original, now)
    {
        log::debug!("could not save public archive retry backoff: {error:#}");
    }
    recovered
}

async fn fetch(
    url: &Url,
    client: &http::Client,
    cache: &ArticleCache,
) -> Result<crate::cache::ArticleResponse> {
    let headers = Vec::new();
    let cached = cache.load(url, &headers)?;
    let provider = url.origin().ascii_serialization();
    let mut providers = PROVIDERS.lock().await;
    if providers
        .get(&provider)
        .is_some_and(|until| *until > tokio::time::Instant::now())
    {
        return cached.context("public archive provider is backed off");
    }
    let request = client.get(http::Request {
        url,
        headers: &headers,
        etag: cached.as_ref().and_then(|entry| entry.etag.as_deref()),
        last_modified: cached
            .as_ref()
            .and_then(|entry| entry.last_modified.as_deref()),
    });
    match tokio::time::timeout(REQUEST_TIMEOUT, request).await {
        Ok(Ok(http::Response::Ok(body))) => cache.store(url, &headers, &body),
        Ok(Ok(http::Response::NotModified)) => {
            cached.context("archive returned 304 without a cached copy")
        }
        Ok(Err(error)) => {
            providers.insert(
                provider,
                tokio::time::Instant::now() + Duration::from_secs(900),
            );
            cached.ok_or(error)
        }
        Err(_) => {
            providers.insert(
                provider,
                tokio::time::Instant::now() + Duration::from_secs(900),
            );
            cached.context("public archive request timed out")
        }
    }
}

async fn recover_from(
    original: &Url,
    title: &str,
    client: &http::Client,
    cache: &ArticleCache,
    api: &Url,
    lookup: &Url,
) -> Result<Option<Recovered>> {
    let mut availability = api.clone();
    availability
        .query_pairs_mut()
        .append_pair("url", original.as_str());
    if let Ok(response) = fetch(&availability, client, cache).await
        && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&response.bytes)
        && let Some(snapshot) = wayback_candidate(&value, original)
        && let Ok(response) = fetch(&snapshot, client, cache).await
        && let Ok(article) =
            verified_article(&response.html_text(), &response.final_url, original, title).await
    {
        return Ok(Some(article));
    }
    let response = match fetch(lookup, client, cache).await {
        Ok(response) => response,
        Err(error) => {
            log::debug!("public archive lookup unavailable: {error:#}");
            return Ok(None);
        }
    };
    if !archive_host(&response.final_url) {
        return Ok(None);
    }
    match verified_article(&response.html_text(), &response.final_url, original, title).await {
        Ok(article) => Ok(Some(article)),
        Err(error) => {
            log::debug!("public archive copy rejected for {original}: {error:#}");
            Ok(None)
        }
    }
}

fn archive_host(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.username().is_empty()
        && url.password().is_none()
        && matches!(
            url.host_str(),
            Some(
                "web.archive.org"
                    | "archive.ph"
                    | "archive.is"
                    | "archive.today"
                    | "archive.li"
                    | "archive.vn"
                    | "archive.fo"
                    | "archive.md"
            )
        )
}

fn same_original(candidate: &Url, original: &Url) -> bool {
    let mut candidate = candidate.clone();
    let mut original = original.clone();
    candidate.set_fragment(None);
    original.set_fragment(None);
    candidate == original
}

fn wayback_original(snapshot: &Url) -> Option<(Url, String)> {
    if snapshot.host_str()? != "web.archive.org" || !archive_host(snapshot) {
        return None;
    }
    let rest = snapshot.path().strip_prefix("/web/")?;
    let (timestamp, original) = rest.split_once('/')?;
    let timestamp = timestamp.strip_suffix("id_").unwrap_or(timestamp);
    if timestamp.len() != 14 || !timestamp.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let mut original = Url::parse(original).ok()?;
    original.set_query(snapshot.query());
    Some((original, timestamp.to_string()))
}

fn wayback_candidate(value: &serde_json::Value, original: &Url) -> Option<Url> {
    let closest = value.get("archived_snapshots")?.get("closest")?;
    if !closest.get("available")?.as_bool()? || closest.get("status")?.as_str()? != "200" {
        return None;
    }
    let snapshot = Url::parse(closest.get("url")?.as_str()?).ok()?;
    let (archived, timestamp) = wayback_original(&snapshot)?;
    if !same_original(&archived, original) {
        return None;
    }
    Url::parse(&format!(
        "https://web.archive.org/web/{timestamp}id_/{original}"
    ))
    .ok()
}

fn title_key(title: &str) -> String {
    title
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn verified_identity(
    html: &str,
    snapshot: &Url,
    original: &Url,
    title: &str,
) -> Option<Option<String>> {
    if !archive_host(snapshot) {
        return None;
    }
    let document = Html::parse_document(html);
    let titles = Selector::parse("title, h1, meta[property='og:title']").ok()?;
    let expected = title_key(title);
    if expected.is_empty()
        || !document.select(&titles).any(|node| {
            let text = node
                .value()
                .attr("content")
                .map(str::to_string)
                .unwrap_or_else(|| node.text().collect::<String>());
            title_key(&text) == expected
                || text
                    .split(['|', '–'])
                    .any(|part| title_key(part) == expected)
        })
    {
        return None;
    }
    if let Some((archived, timestamp)) = wayback_original(snapshot) {
        return same_original(&archived, original).then_some(Some(timestamp));
    }
    let identifier = snapshot.path().trim_matches('/');
    if identifier.len() != 5
        || !identifier.bytes().all(|byte| byte.is_ascii_alphanumeric())
        || snapshot.query().is_some()
    {
        return None;
    }
    let identities = Selector::parse(
        "link[rel=canonical], meta[property='og:url'], input[name=q], input#SHARE_LONGLINK",
    )
    .ok()?;
    document
        .select(&identities)
        .any(|node| {
            ["href", "content", "value"]
                .iter()
                .filter_map(|name| node.value().attr(name))
                .filter_map(|value| Url::parse(value).ok())
                .any(|url| same_original(&url, original))
        })
        .then_some(None)
}

async fn verified_article(
    html: &str,
    snapshot: &Url,
    original: &Url,
    title: &str,
) -> Result<Recovered> {
    let captured_at = verified_identity(html, snapshot, original, title)
        .context("archive URL/title does not match the requested article")?;
    let article = content::extract_article_async(html.to_string(), original.clone()).await?;
    if content::is_subscription_wall(&article.html, original)
        || content::html_to_text(&article.html)
            .split_whitespace()
            .count()
            < 80
    {
        bail!("archive has no complete readable article");
    }
    Ok(Recovered {
        article,
        snapshot: snapshot.to_string(),
        captured_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn archive_http_uses_conditional_cache_and_backs_off_failed_attempts() {
        use httpmock::prelude::*;
        crate::http::install_crypto_provider();
        let server = MockServer::start();
        let mut first = server.mock(|when, then| {
            when.method(GET)
                .path("/snapshot")
                .header_missing("authorization")
                .header_missing("cookie");
            then.status(200)
                .header("etag", "archive-v1")
                .body("<p>Cached public snapshot.</p>");
        });
        let dir = tempfile::tempdir().unwrap();
        let cache = ArticleCache::new(dir.path());
        let config = crate::config::Config::parse("[fetch]\nretries=0").unwrap();
        let client = http::Client::new(&config.fetch).unwrap();
        let url = Url::parse(&server.url("/snapshot")).unwrap();
        let saved = fetch(&url, &client, &cache).await.unwrap();
        first.assert_calls(1);
        first.delete();
        let mut unchanged = server.mock(|when, then| {
            when.method(GET)
                .path("/snapshot")
                .header("if-none-match", "archive-v1");
            then.status(304);
        });
        assert_eq!(
            fetch(&url, &client, &cache).await.unwrap().bytes,
            saved.bytes
        );
        unchanged.assert_calls(1);
        unchanged.delete();
        let failed = server.mock(|when, then| {
            when.method(GET).path("/snapshot");
            then.status(429);
        });
        assert_eq!(
            fetch(&url, &client, &cache).await.unwrap().bytes,
            saved.bytes
        );
        assert_eq!(
            fetch(&url, &client, &cache).await.unwrap().bytes,
            saved.bytes
        );
        failed.assert_calls(1);
        assert!(cache.archive_retry_ready(&url, 1000).unwrap());
        cache.record_archive_failure(&url, 1000).unwrap();
        assert!(!cache.archive_retry_ready(&url, 1001).unwrap());
        assert!(cache.archive_retry_ready(&url, 87_400).unwrap());
        assert!(
            cache.archive_retry_ready(&url, 999).unwrap(),
            "clock corrections cannot block forever"
        );
    }

    #[tokio::test]
    async fn archive_recovery_accepts_matching_readable_prose_and_preserves_provenance() {
        let original = Url::parse("https://publisher.example/article").unwrap();
        let snapshot = Url::parse(
            "https://web.archive.org/web/20260921120000id_/https://publisher.example/article",
        )
        .unwrap();
        let prose = "The research examines how automated tools answer complex questions. Its authors describe the methodology, compare several systems, and discuss limitations in detail. They emphasize that real-world decisions require accurate evidence and careful verification. ".repeat(4);
        let page = format!(
            "<html><head><title>The article</title></head><body><article><h1>The article</h1><p>{prose}</p></article></body></html>"
        );
        let recovered = verified_article(&page, &snapshot, &original, "The article")
            .await
            .unwrap();
        assert!(recovered.article.html.contains("The research examines"));
        assert_eq!(recovered.snapshot, snapshot.as_str());
        assert_eq!(recovered.captured_at.as_deref(), Some("20260921120000"));
        assert!(
            verified_article(&page, &snapshot, &original, "Another article")
                .await
                .is_err()
        );
        let gate = "<title>The article</title><div class='paywall'>Subscribe to continue reading. Already a subscriber? Sign in.</div>";
        assert!(
            verified_article(gate, &snapshot, &original, "The article")
                .await
                .is_err()
        );
    }

    #[test]
    fn archive_identity_requires_exact_original_and_matching_title() {
        let original = Url::parse("https://publisher.example/article").unwrap();
        let snapshot = Url::parse(
            "https://web.archive.org/web/20260921120000id_/https://publisher.example/article",
        )
        .unwrap();
        assert_eq!(
            verified_identity(
                "<title>The article</title>",
                &snapshot,
                &original,
                "The article"
            ),
            Some(Some("20260921120000".into()))
        );
        assert!(
            verified_identity(
                "<title>Other article</title>",
                &snapshot,
                &original,
                "The article"
            )
            .is_none()
        );
        assert!(
            verified_identity(
                "<title>The article</title>",
                &snapshot,
                &Url::parse("https://publisher.example/different").unwrap(),
                "The article"
            )
            .is_none()
        );
        let listing = Url::parse("https://archive.ph/https://publisher.example/article").unwrap();
        assert!(verified_identity("<title>The article</title><input name='q' value='https://publisher.example/article'>", &listing, &original, "The article").is_none());
        let short = Url::parse("https://archive.ph/Ab123").unwrap();
        assert!(
            verified_identity(
                "<title>The article</title>",
                &short,
                &original,
                "The article"
            )
            .is_none()
        );
        assert!(verified_identity("<title>The article</title><input name='q' value='https://publisher.example/article'>", &short, &original, "The article").is_some());
    }
    #[test]
    fn archive_availability_rejects_wrong_urls_and_non_success_snapshots() {
        let original = Url::parse("https://publisher.example/article").unwrap();
        let entry = |url, status| serde_json::json!({"archived_snapshots":{"closest":{"available":true,"url":url,"status":status}}});
        assert!(
            wayback_candidate(
                &entry(
                    "https://web.archive.org/web/20260921120000/https://publisher.example/article",
                    "200"
                ),
                &original
            )
            .unwrap()
            .as_str()
            .contains("id_/https://publisher.example/article")
        );
        assert!(
            wayback_candidate(
                &entry(
                    "https://evil.example/web/20260921120000/https://publisher.example/article",
                    "200"
                ),
                &original
            )
            .is_none()
        );
        assert!(
            wayback_candidate(
                &entry(
                    "https://web.archive.org/web/20260921120000/https://publisher.example/other",
                    "200"
                ),
                &original
            )
            .is_none()
        );
        assert!(
            wayback_candidate(
                &entry(
                    "https://web.archive.org/web/20260921120000/https://publisher.example/article",
                    "404"
                ),
                &original
            )
            .is_none()
        );
    }
}
