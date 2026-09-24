//! Resolve public podcast show pages to publisher feeds or public episode metadata.

use std::collections::BTreeSet;

use anyhow::{Context as _, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use scraper::{Html, Selector};
use serde_json::Value;
use url::Url;

use super::{Context, Fetch, SourceMeta, Validators, feed};
use crate::{config::Source, http::Response, model::RawItem};

fn host(url: &Url) -> &str {
    url.host_str()
        .unwrap_or_default()
        .strip_prefix("www.")
        .unwrap_or_else(|| url.host_str().unwrap_or_default())
}

fn subdomain<'a>(url: &'a Url, domain: &str) -> Option<&'a str> {
    host(url)
        .strip_suffix(domain)?
        .strip_suffix('.')
        .filter(|name| !name.is_empty() && !name.contains('.'))
}

pub fn is_spotify_show(url: &Url) -> bool {
    host(url) == "open.spotify.com" && spotify_show_id(url).is_some()
}

pub fn is_deezer_show(url: &Url) -> bool {
    host(url) == "deezer.com" && deezer_show_id(url).is_some()
}

pub fn is_show_url(url: &Url) -> bool {
    apple_lookup(url).is_some()
        || is_spotify_show(url)
        || is_deezer_show(url)
        || direct_feed(url).is_some()
}

pub async fn fetch(url: &Url, source: &Source, ctx: &Context<'_>) -> Result<Fetch> {
    if is_spotify_show(url) || is_deezer_show(url) {
        return fetch_streaming_show(url, source, ctx).await;
    }
    if ctx.state.identity == source.identity && ctx.state.resolved_url.is_some() {
        match feed::fetch(url, source, ctx).await {
            Ok(result) => return Ok(result),
            Err(error) => log::debug!("{}: rediscovering podcast feed: {error:#}", source.slug),
        }
    }
    if let Some(lookup) = apple_lookup(url) {
        return fetch_apple(&lookup, url, source, ctx).await;
    }
    if let Some(endpoint) = direct_feed(url) {
        match feed::fetch(&endpoint, source, ctx).await {
            Ok(result) => return Ok(result),
            Err(error) => log::debug!(
                "{}: podcast endpoint unavailable, reading show page: {error:#}",
                source.slug
            ),
        }
    }
    feed::fetch(url, source, ctx).await
}

async fn fetch_apple(
    lookup: &Url,
    page: &Url,
    source: &Source,
    ctx: &Context<'_>,
) -> Result<Fetch> {
    let mut lookups = vec![lookup.clone()];
    if lookup.query_pairs().any(|(key, _)| key == "country") {
        let mut default = lookup.clone();
        default.set_query(None);
        default
            .query_pairs_mut()
            .extend_pairs(lookup.query_pairs().filter(|(key, _)| key != "country"));
        lookups.push(default);
    }
    let mut last_error = None;
    for lookup in lookups {
        match feed::request(&lookup, source, ctx, &Validators::default()).await {
            Ok(Response::Ok(body)) => {
                if let Some(endpoint) = apple_feed(&body.bytes, page) {
                    return feed::fetch(&endpoint, source, ctx).await;
                }
            }
            Ok(Response::NotModified) => {}
            Err(error) => last_error = Some(error),
        }
    }
    if let Some(error) = last_error {
        return Err(error).context(
            "Apple Podcasts catalog lookup failed; a direct publisher RSS URL can be used instead",
        );
    }
    bail!(
        "Apple Podcasts did not publish an RSS feed for show {}; checked its storefront and default catalog; use the publisher's RSS URL if available",
        apple_id(page).unwrap_or_default()
    )
}

/// Streaming directories (Spotify, Deezer) publish episode metadata but no feed. Read their public
/// listing once, then upgrade to the publisher's own RSS whenever the catalog identifies it.
async fn fetch_streaming_show(url: &Url, source: &Source, ctx: &Context<'_>) -> Result<Fetch> {
    let remembered = ctx.state.resolved_url.as_deref().and_then(web_url);
    if ctx.state.identity == source.identity
        && remembered
            .as_ref()
            .is_some_and(|endpoint| !is_streaming_show(endpoint))
    {
        match feed::fetch(url, source, ctx).await {
            Ok(result) if !streaming_result(&result) => return Ok(result),
            Ok(result) => return prefer_publisher_rss(result, source, ctx).await,
            Err(error) => log::debug!(
                "{}: rediscovering streaming publisher feed: {error:#}",
                source.slug
            ),
        }
    }
    // Streaming fallback captures remember the show page itself. Reparse it once to discover the
    // publisher feed even when that page's HTTP validators have not changed.
    let state = crate::store::SourceState::default();
    let fresh = Context {
        state: &state,
        ..*ctx
    };
    let result = feed::fetch(url, source, &fresh).await?;
    prefer_publisher_rss(result, source, ctx).await
}

fn is_streaming_show(url: &Url) -> bool {
    is_spotify_show(url) || is_deezer_show(url)
}

fn streaming_result(result: &Fetch) -> bool {
    let validators = match result {
        Fetch::Changed { validators, .. } | Fetch::Unchanged { validators } => validators,
    };
    validators
        .resolved_url
        .as_deref()
        .and_then(web_url)
        .is_some_and(|url| is_streaming_show(&url))
}

async fn prefer_publisher_rss(result: Fetch, source: &Source, ctx: &Context<'_>) -> Result<Fetch> {
    if let Fetch::Changed { meta, items, .. } = &result
        && streaming_result(&result)
        && let Some(lookup) = catalog_query(meta, items)
    {
        match resolve_publisher_rss(&lookup, meta, items, source, ctx).await {
            Ok(feed) => return Ok(feed),
            Err(error) => log::debug!(
                "{}: keeping public episode metadata: {error:#}",
                source.slug
            ),
        }
    }
    Ok(result)
}

fn catalog_query(meta: &SourceMeta, items: &[RawItem]) -> Option<Url> {
    let title = meta.title.as_deref()?.trim();
    if title.is_empty() {
        return None;
    }
    let publisher = items
        .first()
        .and_then(|item| item.authors.first())
        .map(|publisher| publisher.trim())
        .filter(|publisher| !publisher.is_empty());
    let mut url = Url::parse("https://itunes.apple.com/search").ok()?;
    url.query_pairs_mut()
        .append_pair(
            "term",
            &match publisher {
                Some(publisher) => format!("{title} {publisher}"),
                None => title.to_string(),
            },
        )
        .append_pair("entity", "podcast")
        .append_pair("limit", "10");
    Some(url)
}

fn normalized_identity(value: &str) -> String {
    crate::content::html_to_text(value)
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|c| c.is_alphanumeric())
        .collect()
}

/// A directory's publisher label does not always match the catalog's artist name, so an exact
/// title-and-publisher match is preferred and a unique exact title match is the fallback. Either
/// way the caller still checks the resolved feed against the directory's own episodes.
fn catalog_feed(bytes: &[u8], title: &str, publisher: Option<&str>) -> Option<Url> {
    let title = normalized_identity(title);
    let publisher = publisher.map(normalized_identity).filter(|p| !p.is_empty());
    if title.is_empty() {
        return None;
    }
    let catalog: Value = serde_json::from_slice(bytes).ok()?;
    let shows = catalog.get("results")?.as_array()?;
    let feeds = |matching: &dyn Fn(&Value) -> bool| {
        shows
            .iter()
            .filter(|show| {
                show.get("collectionName")
                    .and_then(Value::as_str)
                    .is_some_and(|name| normalized_identity(name) == title)
                    && matching(show)
            })
            .filter_map(|show| {
                show.get("feedUrl")
                    .and_then(Value::as_str)
                    .and_then(web_url)
            })
            .collect::<BTreeSet<_>>()
    };
    let unique = |candidates: BTreeSet<Url>| {
        (candidates.len() == 1)
            .then(|| candidates.into_iter().next())
            .flatten()
    };
    if let Some(publisher) = publisher.as_deref()
        && let Some(url) = unique(feeds(&|show: &Value| {
            show.get("artistName")
                .and_then(Value::as_str)
                .is_some_and(|name| normalized_identity(name) == publisher)
        }))
    {
        return Some(url);
    }
    unique(feeds(&|_: &Value| true))
}

fn matching_episodes(directory: &[RawItem], feed: &[RawItem]) -> bool {
    let keys = |items: &[RawItem]| {
        items
            .iter()
            .filter_map(|item| {
                let title = normalized_identity(&item.title);
                (!title.is_empty()).then_some((title, item.published?.date_naive()))
            })
            .collect::<BTreeSet<_>>()
    };
    keys(directory).intersection(&keys(feed)).take(2).count() == 2
}

async fn resolve_publisher_rss(
    lookup: &Url,
    meta: &SourceMeta,
    items: &[RawItem],
    source: &Source,
    ctx: &Context<'_>,
) -> Result<Fetch> {
    let title = meta.title.as_deref().context("show has no title")?;
    let publisher = items
        .first()
        .and_then(|item| item.authors.first())
        .map(String::as_str);
    let Response::Ok(body) = ctx
        .client
        .get(crate::http::Request {
            url: lookup,
            headers: &[],
            etag: None,
            last_modified: None,
        })
        .await?
    else {
        bail!("podcast catalog returned no results");
    };
    let endpoint = catalog_feed(&body.bytes, title, publisher)
        .context("catalog has no unique exact show match")?;
    let result = feed::fetch_endpoint(&endpoint, source, ctx).await?;
    if let Fetch::Changed {
        items: episodes, ..
    } = &result
        && matching_episodes(items, episodes)
    {
        return Ok(result);
    }
    bail!("publisher feed does not match at least two public episode titles and dates")
}

fn apple_id(url: &Url) -> Option<&str> {
    if !matches!(host(url), "podcasts.apple.com" | "itunes.apple.com") {
        return None;
    }
    url.path_segments()?
        .find_map(|part| part.strip_prefix("id").filter(|id| numeric(id)))
}

fn numeric(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn apple_lookup(url: &Url) -> Option<Url> {
    let id = apple_id(url)?;
    let mut lookup = Url::parse("https://itunes.apple.com/lookup").ok()?;
    lookup
        .query_pairs_mut()
        .append_pair("id", id)
        .append_pair("entity", "podcast");
    if let Some(country) = url
        .path_segments()?
        .next()
        .filter(|part| part.len() == 2 && part.bytes().all(|byte| byte.is_ascii_alphabetic()))
    {
        lookup.query_pairs_mut().append_pair("country", country);
    }
    Some(lookup)
}

fn apple_feed(bytes: &[u8], page: &Url) -> Option<Url> {
    let id = apple_id(page)?.parse::<u64>().ok()?;
    let catalog: Value = serde_json::from_slice(bytes).ok()?;
    catalog
        .get("results")?
        .as_array()?
        .iter()
        .find(|show| show.get("collectionId").and_then(Value::as_u64) == Some(id))?
        .get("feedUrl")?
        .as_str()
        .and_then(web_url)
}

fn direct_feed(url: &Url) -> Option<Url> {
    let parts: Vec<_> = url
        .path_segments()?
        .filter(|part| !part.is_empty())
        .collect();
    if matches!(host(url), "youtube.com" | "music.youtube.com") {
        let (key, id) = match parts.as_slice() {
            ["channel", id, ..] => ("channel_id", id.to_string()),
            ["playlist"] => (
                "playlist_id",
                url.query_pairs()
                    .find(|(key, _)| key == "list")?
                    .1
                    .into_owned(),
            ),
            _ => return None,
        };
        if id.is_empty()
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return None;
        }
        let mut endpoint = web_url("https://www.youtube.com/feeds/videos.xml")?;
        endpoint.query_pairs_mut().append_pair(key, &id);
        return Some(endpoint);
    }
    let target = if let Some(name) =
        subdomain(url, "podbean.com").filter(|name| !matches!(*name, "feed" | "help" | "admin"))
    {
        format!("https://feed.podbean.com/{name}/feed.xml")
    } else if let Some(name) = subdomain(url, "libsyn.com")
        .filter(|name| !matches!(*name, "feeds" | "directory" | "traffic"))
    {
        format!("https://{name}.libsyn.com/rss")
    } else if (host(url) == "buzzsprout.com" || subdomain(url, "buzzsprout.com").is_some())
        && parts.first().is_some_and(|id| numeric(id))
    {
        format!("https://feeds.buzzsprout.com/{}.rss", parts[0])
    } else if host(url) == "shows.acast.com" {
        let slug = if parts.first() == Some(&"shows") {
            *parts.get(1)?
        } else {
            *parts.first()?
        };
        format!("https://feeds.acast.com/public/shows/{slug}")
    } else if host(url) == "spreaker.com" {
        let id = match parts.as_slice() {
            ["show", id, ..] if numeric(id) => *id,
            ["podcast", slug, ..] => slug
                .rsplit_once("--")
                .map(|(_, id)| id)
                .filter(|id| numeric(id))?,
            _ => return None,
        };
        format!("https://www.spreaker.com/show/{id}/episodes/feed")
    } else {
        return None;
    };
    web_url(&target)
}

fn web_url(value: &str) -> Option<Url> {
    Url::parse(value).ok().filter(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.username().is_empty()
            && url.password().is_none()
    })
}

/// Provider feeds often have opaque paths that ordinary RSS-link heuristics cannot recognize.
pub fn feed_links(page: &str, url: &Url) -> Vec<Url> {
    let document = Html::parse_document(page);
    let mut candidates = Vec::new();
    if let Ok(selector) = Selector::parse("a[href]") {
        for element in document.select(&selector) {
            if let Some(link) = element
                .value()
                .attr("href")
                .and_then(|href| url.join(href).ok())
                .filter(|link| {
                    matches!(
                        host(link),
                        "feeds.simplecast.com"
                            | "feeds.acast.com"
                            | "feeds.buzzsprout.com"
                            | "feeds.soundcloud.com"
                            | "feed.podbean.com"
                    )
                })
            {
                candidates.push(link);
            }
        }
    }
    if let Ok(selector) = Selector::parse(
        "script[type='application/json'],script[type='application/ld+json'],script#__NEXT_DATA__",
    ) {
        for element in document.select(&selector) {
            if let Ok(value) = serde_json::from_str::<Value>(&element.inner_html()) {
                collect_feeds(&value, &mut candidates, 0);
            }
        }
    }
    if host(url) == "soundcloud.com"
        && let Ok(selector) =
            Selector::parse("meta[property='al:ios:url'],meta[property='al:android:url']")
    {
        for element in document.select(&selector) {
            if let Some(id) = element
                .value()
                .attr("content")
                .and_then(|value| value.strip_prefix("soundcloud://users:"))
                .filter(|id| numeric(id))
                && let Some(endpoint) = web_url(&format!(
                    "https://feeds.soundcloud.com/users/soundcloud:users:{id}/sounds.rss"
                ))
            {
                candidates.push(endpoint);
            }
        }
    }
    candidates.extend(media_host_feeds(page));
    let mut seen = BTreeSet::new();
    candidates.retain(|candidate| {
        matches!(candidate.scheme(), "http" | "https") && seen.insert(candidate.to_string())
    });
    candidates.truncate(16);
    candidates
}

/// A page that advertises no feed still has to name the audio it plays, and a hosting provider's
/// media URL carries the show it belongs to: Deezer's player embeds
/// `acast.com/p/acast/s/<show>/e/<id>/media.mp3`, whose show is the public Acast feed. Only the
/// shapes whose feed is derivable from the media path are read; a host that names an episode
/// alone says nothing about where the rest of them live.
fn media_host_feeds(page: &str) -> Vec<Url> {
    let mut found = Vec::new();
    for (start, _) in page.match_indices("://") {
        let rest = &page[start + 3..];
        let end = rest
            .find(|c: char| {
                c.is_whitespace() || matches!(c, '"' | '\'' | '\\' | '<' | '>' | ')' | ',')
            })
            .unwrap_or(rest.len());
        let Some(url) = web_url(&format!("https://{}", &rest[..end])) else {
            continue;
        };
        if let Some(feed) = media_show_feed(&url) {
            found.push(feed);
        }
        if found.len() >= 4 {
            break;
        }
    }
    found
}

/// The show feed a media URL belongs to, for the hosts whose media path names its show.
fn media_show_feed(media: &Url) -> Option<Url> {
    let parts: Vec<&str> = media
        .path_segments()
        .map(|segments| segments.filter(|part| !part.is_empty()).collect())
        .unwrap_or_default();
    let after = |marker: &str| {
        parts
            .iter()
            .position(|part| *part == marker)
            .and_then(|at| parts.get(at + 1))
            .copied()
            .filter(|show| !show.is_empty())
    };
    let target = if host(media) == "acast.com" || subdomain(media, "acast.com").is_some() {
        // `…/p/<network>/s/<show>/e/<episode>/media.mp3`
        format!("https://feeds.acast.com/public/shows/{}", after("s")?)
    } else if subdomain(media, "libsyn.com").is_some_and(|name| name == "traffic") {
        // `traffic.libsyn.com/<show>/<episode>.mp3`, with an optional `secure` in front.
        let show = parts
            .iter()
            .find(|part| **part != "secure")
            .filter(|show| !show.contains('.'))?;
        format!("https://{show}.libsyn.com/rss")
    } else if host(media) == "buzzsprout.com" || subdomain(media, "buzzsprout.com").is_some() {
        // `…/<show id>/episodes/<episode>.mp3`
        let id = parts.first().filter(|id| numeric(id))?;
        format!("https://feeds.buzzsprout.com/{id}.rss")
    } else {
        return None;
    };
    web_url(&target)
}

fn collect_feeds(value: &Value, result: &mut Vec<Url>, depth: usize) {
    if depth > 32 || result.len() >= 16 {
        return;
    }
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if matches!(
                    key.as_str(),
                    "feedUrl"
                        | "feedURL"
                        | "feed_url"
                        | "rssFeed"
                        | "rssFeedUrl"
                        | "rss_feed_url"
                        | "rss_url"
                        | "rssUrl"
                ) && let Some(url) = value.as_str().and_then(web_url)
                {
                    result.push(url);
                } else {
                    collect_feeds(value, result, depth + 1);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_feeds(value, result, depth + 1);
            }
        }
        _ => {}
    }
}

fn spotify_show_id(url: &Url) -> Option<&str> {
    let mut parts = url.path_segments()?;
    let first = parts.next()?;
    let kind = if first.starts_with("intl-") {
        parts.next()?
    } else {
        first
    };
    if kind != "show" {
        return None;
    }
    parts
        .next()
        .filter(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_alphanumeric()))
}

/// `/show/<id>`, optionally behind a locale segment such as `/fr/show/<id>`.
fn deezer_show_id(url: &Url) -> Option<&str> {
    let parts: Vec<_> = url
        .path_segments()?
        .filter(|part| !part.is_empty())
        .collect();
    let id = match parts.as_slice() {
        ["show", id, ..] => id,
        [locale, "show", id, ..] if locale.len() <= 5 => id,
        _ => return None,
    };
    numeric(id).then_some(*id)
}

pub fn deezer_items(page: &str, url: &Url) -> Result<(SourceMeta, Vec<RawItem>)> {
    let id = deezer_show_id(url).context("expected a Deezer show URL")?;
    let state: Value = serde_json::from_str(
        embedded_object(page, "window.__DZR_APP_STATE__")
            .context("Deezer did not expose public episode metadata")?,
    )
    .context("parsing Deezer public show metadata")?;
    let show = state.get("DATA").context("Deezer metadata has no show")?;
    let meta = SourceMeta {
        title: show
            .get("SHOW_NAME")
            .and_then(Value::as_str)
            .map(crate::content::html_to_text),
        site_url: Some(format!("https://www.deezer.com/show/{id}")),
        language: None,
        extracted: false,
    };
    // The label publishes the show; the catalog may credit a different host, so it is only a hint.
    let publisher = show
        .get("LABEL_NAME")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty());
    let episodes = state
        .pointer("/EPISODES/data")
        .and_then(Value::as_array)
        .context("Deezer did not expose this show's public episode listing")?;
    let mut items = Vec::new();
    let mut seen = BTreeSet::new();
    for episode in episodes {
        let Some(id) = episode
            .get("EPISODE_ID")
            .and_then(Value::as_str)
            .filter(|id| numeric(id))
        else {
            continue;
        };
        if !seen.insert(id.to_owned()) {
            continue;
        }
        let Some(title) = episode
            .get("EPISODE_TITLE")
            .and_then(Value::as_str)
            .filter(|title| !title.trim().is_empty())
        else {
            continue;
        };
        let summary = episode
            .get("EPISODE_DESCRIPTION")
            .and_then(Value::as_str)
            .map(crate::content::html_to_text)
            .filter(|value| !value.is_empty());
        let published = episode
            .get("EPISODE_PUBLISHED_TIMESTAMP")
            .and_then(Value::as_str)
            .and_then(|value| {
                crate::sources::html::parse_date(value, Some("%Y-%m-%d %H:%M:%S"))
                    .or_else(|| crate::sources::html::parse_date(value, None))
            });
        let mut extra = std::collections::BTreeMap::new();
        if let Some(seconds) = episode
            .get("DURATION")
            .and_then(|value| {
                value
                    .as_u64()
                    .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
            })
            .filter(|duration| *duration > 0)
        {
            extra.insert("duration_seconds".into(), seconds.into());
        }
        if let Some(audio) = episode
            .get("EPISODE_DIRECT_STREAM_URL")
            .and_then(Value::as_str)
            .and_then(web_url)
        {
            extra.insert("audio_url".into(), audio.to_string().into());
        }
        let artwork = episode
            .get("EPISODE_IMAGE_MD5")
            .or_else(|| episode.get("SHOW_ART_MD5"))
            .or_else(|| show.get("SHOW_ART_MD5"))
            .and_then(Value::as_str)
            .filter(|md5| md5.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .and_then(|md5| {
                web_url(&format!(
                    "https://cdn-images.dzcdn.net/images/talk/{md5}/1000x1000-000000-80-0-0.jpg"
                ))
            });
        items.push(RawItem {
            id: Some(format!("deezer:episode:{id}")),
            title: crate::content::html_to_text(title),
            link: format!("https://www.deezer.com/episode/{id}"),
            published,
            updated: None,
            first_seen: None,
            authors: publisher
                .map(|name| vec![name.to_string()])
                .unwrap_or_default(),
            labels: vec![],
            summary,
            content_html: None,
            extra,
            preview_candidates: artwork
                .map(|url| {
                    vec![crate::preview::Candidate {
                        url: url.to_string(),
                        alt: None,
                    }]
                })
                .unwrap_or_default(),
            preview: None,
            images: vec![],
            document: None,
        });
    }
    if items.is_empty() {
        bail!(
            "Deezer did not expose any public episodes; configure the publisher's RSS feed instead"
        );
    }
    Ok((meta, items))
}

/// The JSON literal assigned to `<name>` in an inline script.
fn embedded_object<'a>(page: &'a str, name: &str) -> Option<&'a str> {
    let start = page.find(name)? + name.len();
    let rest = page[start..].trim_start().strip_prefix('=')?.trim_start();
    rest.starts_with('{')
        .then(|| crate::content::balanced_json_object(rest))
        .flatten()
}

pub fn spotify_items(page: &str, url: &Url) -> Result<(SourceMeta, Vec<RawItem>)> {
    let id = spotify_show_id(url).context("expected a Spotify show URL")?;
    let document = Html::parse_document(page);
    let selector =
        Selector::parse("script#initialState").map_err(|error| anyhow::anyhow!("{error}"))?;
    let encoded = document.select(&selector).next().context("Spotify did not expose public episode metadata; configure the publisher's RSS feed instead")?.inner_html();
    let decoded = STANDARD
        .decode(encoded.trim())
        .context("decoding Spotify public show metadata")?;
    let state: Value =
        serde_json::from_slice(&decoded).context("parsing Spotify public show metadata")?;
    let show = state
        .get("entities")
        .and_then(|value| value.get("items"))
        .and_then(|value| value.get(format!("spotify:show:{id}")))
        .context("Spotify public metadata does not contain this show")?;
    let meta = SourceMeta {
        title: show.get("name").and_then(Value::as_str).map(str::to_string),
        site_url: Some(format!("https://open.spotify.com/show/{id}")),
        language: None,
        extracted: false,
    };
    let episodes = show
        .pointer("/pages/items")
        .and_then(Value::as_array)
        .context("Spotify did not expose this show's public episode listing")?;
    let mut items = Vec::new();
    let mut seen = BTreeSet::new();
    for wrapper in episodes {
        let Some(episode) = wrapper.pointer("/entity/data") else {
            continue;
        };
        let Some(id) = episode
            .get("uri")
            .and_then(Value::as_str)
            .and_then(|uri| uri.strip_prefix("spotify:episode:"))
            .filter(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_alphanumeric()))
        else {
            continue;
        };
        if !seen.insert(id.to_owned()) {
            continue;
        }
        let Some(title) = episode
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty())
        else {
            continue;
        };
        let content_html = episode
            .get("htmlDescription")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(|html| crate::content::sanitize(html, Some(url)));
        let summary = episode
            .get("description")
            .and_then(Value::as_str)
            .map(crate::content::html_to_text);
        let published = episode
            .pointer("/releaseDate/isoString")
            .and_then(Value::as_str)
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|date| date.with_timezone(&Utc));
        let artwork = episode
            .pointer("/coverArt/sources")
            .and_then(Value::as_array)
            .and_then(|images| images.last())
            .and_then(|image| image.get("url"))
            .and_then(Value::as_str)
            .and_then(web_url);
        let mut extra = std::collections::BTreeMap::new();
        if let Some(milliseconds) = episode
            .pointer("/duration/totalMilliseconds")
            .and_then(Value::as_u64)
            .filter(|duration| *duration > 0)
        {
            extra.insert(
                "duration_seconds".into(),
                milliseconds.div_ceil(1000).into(),
            );
        }
        items.push(RawItem {
            id: Some(format!("spotify:episode:{id}")),
            title: crate::content::html_to_text(title),
            link: format!("https://open.spotify.com/episode/{id}"),
            published,
            updated: None,
            first_seen: None,
            authors: show
                .get("publisher")
                .and_then(|publisher| {
                    publisher
                        .as_str()
                        .or_else(|| publisher.get("name").and_then(Value::as_str))
                })
                .map(|name| vec![name.to_string()])
                .unwrap_or_default(),
            labels: vec![],
            summary,
            content_html,
            extra,
            preview_candidates: artwork
                .map(|url| {
                    vec![crate::preview::Candidate {
                        url: url.to_string(),
                        alt: None,
                    }]
                })
                .unwrap_or_default(),
            preview: None,
            images: vec![],
            document: None,
        });
    }
    if items.is_empty() {
        bail!(
            "Spotify did not expose any public episodes; configure the publisher's RSS feed instead"
        );
    }
    Ok((meta, items))
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_players_media_url_names_the_show_its_feed_belongs_to() {
        // deezer.com/fr/show/8153 advertises no feed; its player embeds the Acast media URL.
        let page = r#"<script>{"MD5_ORIGIN":"x","SOURCES":["https://acast.com/p/acast/s/le-rdv-tech/e/68cf/media.mp3"]}</script>"#;
        let url = Url::parse("https://www.deezer.com/fr/show/8153").unwrap();
        assert!(
            feed_links(page, &url)
                .iter()
                .any(|link| link.as_str() == "https://feeds.acast.com/public/shows/le-rdv-tech"),
            "{:?}",
            feed_links(page, &url)
        );
        // The other hosts whose media path names its show.
        let shapes = [
            (
                "https://traffic.libsyn.com/secure/thisweek/episode-9.mp3",
                "https://thisweek.libsyn.com/rss",
            ),
            (
                "https://www.buzzsprout.com/1234/episodes/99-a-title.mp3",
                "https://feeds.buzzsprout.com/1234.rss",
            ),
        ];
        for (media, feed) in shapes {
            assert_eq!(
                media_show_feed(&Url::parse(media).unwrap())
                    .as_ref()
                    .map(Url::as_str),
                Some(feed),
                "{media}"
            );
        }
        // A media URL on a host that says nothing about the rest of the show stays unread.
        assert!(
            media_show_feed(&Url::parse("https://cdn.example/audio/episode-9.mp3").unwrap())
                .is_none()
        );
    }
    use super::*;
    use url::Url;

    #[test]
    fn catalog_discovery_requires_unique_exact_show_and_publisher_identity() {
        let catalog = br#"{"results":[
            {"collectionName":"Underscore_","artistName":"Impersonator","feedUrl":"https://wrong.example/feed"},
            {"collectionName":"Underscore_","artistName":"Micode","feedUrl":"https://publisher.example/feed"}
        ]}"#;
        assert_eq!(
            catalog_feed(catalog, "Underscore_", Some("Micode"))
                .unwrap()
                .as_str(),
            "https://publisher.example/feed"
        );
        // Two shows share the title, so an unmatched publisher label stays ambiguous.
        assert!(catalog_feed(catalog, "Underscore_", Some("Label")).is_none());
        assert!(catalog_feed(catalog, "Underscore_", None).is_none());
        assert!(catalog_feed(catalog, "Underscore", Some("")).is_none());
        assert!(catalog_feed(catalog, "Another show", Some("Micode")).is_none());
        let ambiguous = br#"{"results":[
            {"collectionName":"Same","artistName":"Same","feedUrl":"https://one.example/feed"},
            {"collectionName":"Same","artistName":"Same","feedUrl":"https://two.example/feed"}
        ]}"#;
        assert!(catalog_feed(ambiguous, "Same", Some("Same")).is_none());
        for url in [
            "javascript:alert(1)",
            "https://user:secret@example.com/feed",
            "file:///tmp/feed",
        ] {
            let catalog = serde_json::to_vec(&serde_json::json!({"results":[{"collectionName":"Same","artistName":"Same","feedUrl":url}]})).unwrap();
            assert!(catalog_feed(&catalog, "Same", Some("Same")).is_none());
        }
    }

    #[test]
    fn a_unique_title_match_stands_in_for_a_publisher_label_the_catalog_does_not_share() {
        let catalog = br#"{"results":[
            {"collectionName":"Le rendez-vous Tech","artistName":"NotPatrick","feedUrl":"https://feeds.example/rdv-tech"},
            {"collectionName":"Le rendez-vous Jeux","artistName":"NotPatrick","feedUrl":"https://feeds.example/rdv-jeux"}
        ]}"#;
        assert_eq!(
            catalog_feed(catalog, "Le rendez-vous Tech", Some("frenchspin"))
                .unwrap()
                .as_str(),
            "https://feeds.example/rdv-tech"
        );
    }

    #[test]
    fn deezer_show_pages_expose_public_episodes() {
        let page = r#"<script>window.__DZR_APP_STATE__ = {"DATA":{"SHOW_ID":"8153","SHOW_NAME":"Le rendez-vous Tech","LABEL_NAME":"frenchspin","SHOW_ART_MD5":"1a968927e0523400f8b9b882f6128939"},"EPISODES":{"data":[
            {"EPISODE_ID":"934706832","EPISODE_TITLE":"Episode with } brace","EPISODE_DESCRIPTION":"Notes","DURATION":"5601","EPISODE_PUBLISHED_TIMESTAMP":"2026-09-15 14:00:00","EPISODE_DIRECT_STREAM_URL":"https://sphinx.example/media.mp3","EPISODE_IMAGE_MD5":"1a968927e0523400f8b9b882f6128939"},
            {"EPISODE_ID":"934706832","EPISODE_TITLE":"Duplicate"},
            {"EPISODE_ID":"not-numeric","EPISODE_TITLE":"Rejected"}
        ]}};</script>"#;
        let url = Url::parse("https://www.deezer.com/fr/show/8153").unwrap();
        assert!(is_deezer_show(&url));
        assert!(!is_deezer_show(
            &Url::parse("https://www.deezer.com/fr/album/8153").unwrap()
        ));
        let (meta, items) = deezer_items(page, &url).unwrap();
        assert_eq!(meta.title.as_deref(), Some("Le rendez-vous Tech"));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Episode with } brace");
        assert_eq!(items[0].link, "https://www.deezer.com/episode/934706832");
        assert_eq!(items[0].authors, ["frenchspin"]);
        assert_eq!(
            items[0].published.unwrap().to_rfc3339(),
            "2026-09-15T14:00:00+00:00"
        );
        assert_eq!(items[0].extra["duration_seconds"], 5601);
        assert_eq!(
            items[0].extra["audio_url"],
            "https://sphinx.example/media.mp3"
        );
    }

    #[tokio::test]
    async fn apple_retries_default_catalog_when_storefront_omits_the_show() {
        use httpmock::prelude::*;
        let server = MockServer::start_async().await;
        let regional = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/lookup")
                    .query_param("country", "fr");
                then.json_body(serde_json::json!({"results":[]}));
            })
            .await;
        let default = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/lookup")
                    .query_param_missing("country")
                    .query_param("id", "12345");
                then.json_body(serde_json::json!({"results":[
                    {"collectionId":999,"feedUrl":server.url("/wrong")},
                    {"collectionId":12345,"feedUrl":server.url("/feed")}
                ]}));
            })
            .await;
        let publisher = server.mock_async(|when, then| {
            when.method(GET).path("/feed");
            then.body(r#"<rss version="2.0"><channel><title>Apple show</title><item><guid>one</guid><title>Episode</title><link>https://publisher.example/one</link><enclosure url="https://publisher.example/one.mp3" type="audio/mpeg" length="100"/></item></channel></rss>"#);
        }).await;
        let page = Url::parse("https://podcasts.apple.com/fr/podcast/show/id12345").unwrap();
        let source = source(page.clone());
        let client = crate::http::Client::new(&crate::config::FetchConfig::default()).unwrap();
        let state = crate::store::SourceState::default();
        let directory = tempfile::tempdir().unwrap();
        let ctx = Context {
            client: &client,
            state: &state,
            cache_dir: directory.path(),
        };
        let result = fetch_apple(
            &Url::parse(&server.url("/lookup?id=12345&entity=podcast&country=fr")).unwrap(),
            &page,
            &source,
            &ctx,
        )
        .await
        .unwrap();
        let Fetch::Changed { items, .. } = result else {
            panic!("expected publisher episodes")
        };
        assert_eq!(
            items[0]
                .extra
                .get("audio_url")
                .and_then(serde_yaml_ng::Value::as_str),
            Some("https://publisher.example/one.mp3")
        );
        regional.assert_calls_async(1).await;
        default.assert_calls_async(1).await;
        publisher.assert_calls_async(1).await;
    }

    #[tokio::test]
    async fn verified_spotify_endpoint_is_reused_conditionally_without_catalog_discovery() {
        use httpmock::prelude::*;
        let server = MockServer::start_async().await;
        let cached = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/feed")
                    .header("if-none-match", "publisher-v1");
                then.status(304);
            })
            .await;
        let url = Url::parse("https://open.spotify.com/show/show123").unwrap();
        let source = source(url.clone());
        let client = crate::http::Client::new(&crate::config::FetchConfig::default()).unwrap();
        let state = crate::store::SourceState {
            identity: source.identity.clone(),
            resolved_url: Some(server.url("/feed")),
            etag: Some("publisher-v1".into()),
            ..Default::default()
        };
        let directory = tempfile::tempdir().unwrap();
        let ctx = Context {
            client: &client,
            state: &state,
            cache_dir: directory.path(),
        };
        let result = fetch(&url, &source, &ctx).await.unwrap();
        assert!(matches!(result, Fetch::Unchanged { .. }));
        cached.assert_calls_async(1).await;
    }

    fn source(url: Url) -> Source {
        Source {
            slug: "podcast".into(),
            name: None,
            category: None,
            labels: vec![],
            identity: "podcast-v1".into(),
            public_url: Some(url.to_string()),
            persist_endpoint: true,
            headers: vec![],
            html: true,
            previews: false,
            images: crate::config::ImagePolicy::Remote,
            content: crate::config::ContentMode::Light,
            engine: crate::config::Engine::Feed { url },
        }
    }

    fn public_spotify_items() -> (SourceMeta, Vec<RawItem>) {
        let episodes = (1..=2)
            .map(|id| {
                serde_json::json!({"entity":{"data":{
                    "uri":format!("spotify:episode:episode{id}"), "name":format!("Episode {id}"),
                    "duration":{"totalMilliseconds":if id == 1 { 3_601_001 } else { 0 }},
                    "releaseDate":{"isoString":format!("2026-08-0{id}T12:00:00Z")}
                }}})
            })
            .collect::<Vec<_>>();
        let state = serde_json::json!({"entities":{"items":{"spotify:show:show123":{
            "name":"The show", "publisher":{"name":"Publisher"}, "pages":{"items":episodes}
        }}}});
        spotify_items(
            &format!(
                "<script id='initialState'>{}</script>",
                STANDARD.encode(serde_json::to_vec(&state).unwrap())
            ),
            &Url::parse("https://open.spotify.com/show/show123").unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn public_spotify_duration_converts_milliseconds_without_inventing_unknown_lengths() {
        let (_, items) = public_spotify_items();
        assert_eq!(items[0].extra["duration_seconds"].as_u64(), Some(3602));
        assert!(!items[1].extra.contains_key("duration_seconds"));
    }

    #[test]
    fn canonical_feed_requires_two_distinct_episode_title_and_date_matches() {
        let (_, items) = public_spotify_items();
        assert_eq!(items[0].authors, ["Publisher"]);
        assert!(matching_episodes(&items, &items));
        assert!(!matching_episodes(&items, &items[..1]));
        assert!(!matching_episodes(
            &items[..1],
            &[items[0].clone(), items[0].clone()]
        ));
        let mut different_dates = items.clone();
        for item in &mut different_dates {
            item.published = Some(Utc::now());
        }
        assert!(!matching_episodes(&items, &different_dates));
    }

    #[tokio::test]
    async fn spotify_catalog_resolution_fetches_verified_full_audio_without_source_credentials() {
        use httpmock::prelude::*;
        let server = MockServer::start_async().await;
        let lookup = server.mock_async(|when, then| {
            when.method(GET).path("/search").header_missing("authorization").header_missing("cookie");
            then.json_body(serde_json::json!({"results":[{"collectionName":"The show", "artistName":"Publisher", "feedUrl":server.url("/feed")}]}));
        }).await;
        let rss = r#"<rss version="2.0"><channel><title>The show</title><link>https://publisher.example</link>
            <item><guid>one</guid><title>Episode 1</title><link>https://publisher.example/one</link><pubDate>Sat, 01 Aug 2026 12:00:00 +0000</pubDate><enclosure url="https://publisher.example/one.mp3" type="audio/mpeg" length="100"/></item>
            <item><guid>two</guid><title>Episode 2</title><link>https://publisher.example/two</link><pubDate>Sun, 02 Aug 2026 12:00:00 +0000</pubDate><enclosure url="https://publisher.example/two.mp3" type="audio/mpeg" length="100"/></item>
            </channel></rss>"#;
        let feed_mock = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/feed")
                    .header_missing("authorization")
                    .header_missing("cookie");
                then.body(rss);
            })
            .await;
        let mut source = source(Url::parse("https://open.spotify.com/show/show123").unwrap());
        source.headers = vec![
            ("authorization".into(), "private".into()),
            ("cookie".into(), "private".into()),
        ];
        let client = crate::http::Client::new(&crate::config::FetchConfig::default()).unwrap();
        let state = crate::store::SourceState::default();
        let directory = tempfile::tempdir().unwrap();
        let ctx = Context {
            client: &client,
            state: &state,
            cache_dir: directory.path(),
        };
        let (meta, items) = public_spotify_items();
        let result = resolve_publisher_rss(
            &Url::parse(&server.url("/search")).unwrap(),
            &meta,
            &items,
            &source,
            &ctx,
        )
        .await
        .unwrap();
        let Fetch::Changed {
            validators, items, ..
        } = result
        else {
            panic!("expected canonical episodes")
        };
        assert_eq!(
            validators.resolved_url.as_deref(),
            Some(server.url("/feed").as_str())
        );
        assert!(
            items
                .iter()
                .all(|item| item.extra.contains_key("audio_url"))
        );
        lookup.assert_calls_async(1).await;
        feed_mock.assert_calls_async(1).await;
        feed_mock.delete_async().await;
        let wrong_feed = server
            .mock_async(|when, then| {
                when.method(GET).path("/feed");
                then.body(rss.replace("Episode 2", "Another show's episode"));
            })
            .await;
        let (meta, public_items) = public_spotify_items();
        let error = resolve_publisher_rss(
            &Url::parse(&server.url("/search")).unwrap(),
            &meta,
            &public_items,
            &source,
            &ctx,
        )
        .await
        .err()
        .expect("one matching episode must not establish identity");
        assert!(error.to_string().contains("at least two"));
        wrong_feed.assert_calls_async(1).await;
    }

    #[tokio::test]
    #[ignore = "explicit live provider verification"]
    async fn live_configured_podcasts_resolve_with_full_audio() {
        let client = crate::http::Client::new(&crate::config::FetchConfig::default()).unwrap();
        let state = crate::store::SourceState::default();
        let directory = tempfile::tempdir().unwrap();
        let ctx = Context {
            client: &client,
            state: &state,
            cache_dir: directory.path(),
        };
        for page in [
            "https://open.spotify.com/show/1sz1NhoHqbpXbzNlpOnFoz",
            "https://podcasts.apple.com/us/podcast/lennys-podcast-product-career-growth/id1627920305",
        ] {
            let url = Url::parse(page).unwrap();
            let source = source(url.clone());
            let Fetch::Changed {
                validators,
                meta,
                items,
            } = fetch(&url, &source, &ctx).await.unwrap()
            else {
                panic!("expected episodes")
            };
            let audio = items
                .iter()
                .filter(|item| item.extra.contains_key("audio_url"))
                .count();
            println!(
                "{}: {} episodes, {} full audio enclosures, {:?}",
                meta.title.unwrap_or_default(),
                items.len(),
                audio,
                validators.resolved_url
            );
            assert!(audio >= 2, "{page}");
        }
    }

    #[test]
    fn provider_endpoints_follow_show_identity() {
        for (page, expected) in [
            (
                "https://example.podbean.com/e/episode",
                "https://feed.podbean.com/example/feed.xml",
            ),
            (
                "https://www.buzzsprout.com/12345/episodes/episode",
                "https://feeds.buzzsprout.com/12345.rss",
            ),
            (
                "https://example.libsyn.com/episode",
                "https://example.libsyn.com/rss",
            ),
            (
                "https://shows.acast.com/example/episodes/episode",
                "https://feeds.acast.com/public/shows/example",
            ),
            (
                "https://www.spreaker.com/podcast/example--12345",
                "https://www.spreaker.com/show/12345/episodes/feed",
            ),
            (
                "https://www.youtube.com/playlist?list=PLabc_123&si=tracking",
                "https://www.youtube.com/feeds/videos.xml?playlist_id=PLabc_123",
            ),
            (
                "https://www.youtube.com/channel/UCabc-123/podcasts",
                "https://www.youtube.com/feeds/videos.xml?channel_id=UCabc-123",
            ),
        ] {
            assert_eq!(
                direct_feed(&Url::parse(page).unwrap()).unwrap().as_str(),
                expected
            );
        }
        assert!(direct_feed(&Url::parse("https://notpodbean.com/example").unwrap()).is_none());
        assert!(
            direct_feed(
                &Url::parse("https://www.spreaker.com/podcast/example--not-an-id").unwrap()
            )
            .is_none()
        );
    }

    #[test]
    fn apple_lookup_uses_id_and_storefront_not_title() {
        let url =
            Url::parse("https://podcasts.apple.com/fr/podcast/renamed-show/id12345?i=678").unwrap();
        let endpoint = apple_lookup(&url).unwrap();
        assert_eq!(
            endpoint.as_str(),
            "https://itunes.apple.com/lookup?id=12345&entity=podcast&country=fr"
        );
        assert!(
            apple_lookup(&Url::parse("https://podcasts.apple.com/fr/charts").unwrap()).is_none()
        );
    }

    #[test]
    fn embedded_feeds_and_soundcloud_identity_are_discovered_safely() {
        let page = r#"<script type="application/json">{"podcast":{"rss_feed_url":"https://feeds.simplecast.com/Ab_Cd"},"bad":{"feedUrl":"javascript:evil"}}</script><a href="https://feeds.simplecast.com/Ab_Cd">Subscribe</a>"#;
        assert_eq!(
            feed_links(page, &Url::parse("https://example.simplecast.com").unwrap()),
            vec![Url::parse("https://feeds.simplecast.com/Ab_Cd").unwrap()]
        );
        let page = r#"<meta property="al:ios:url" content="soundcloud://users:12345">"#;
        assert_eq!(
            feed_links(page, &Url::parse("https://soundcloud.com/example").unwrap()),
            vec![
                Url::parse("https://feeds.soundcloud.com/users/soundcloud:users:12345/sounds.rss")
                    .unwrap()
            ]
        );
    }

    #[test]
    fn apple_catalog_selects_the_matching_show() {
        let catalog = br#"{"results":[{"collectionId":999,"feedUrl":"https://wrong.example/feed"},{"collectionId":12345,"feedUrl":"https://example.com/podcast.xml"}]}"#;
        let page = Url::parse("https://podcasts.apple.com/us/podcast/show/id12345").unwrap();
        assert_eq!(
            apple_feed(catalog, &page).unwrap().as_str(),
            "https://example.com/podcast.xml"
        );
        assert!(apple_feed(br#"{"results":[]}"#, &page).is_none());
    }

    #[test]
    fn spotify_imports_only_the_requested_shows_public_episodes() {
        let episode = serde_json::json!({"entity":{"data":{
            "uri":"spotify:episode:episode123", "name":"Episode &amp; title",
            "description":"Public episode notes", "htmlDescription":"<p>Notes</p><script>evil()</script>",
            "releaseDate":{"isoString":"2026-08-26T21:47:00Z"},
            "coverArt":{"sources":[{"url":"https://example.com/art.jpg"}]}
        }}});
        let state = serde_json::json!({"entities":{"items":{
            "spotify:show:show123":{"name":"The show","publisher":"Publisher","pages":{"items":[episode.clone(),episode]}},
            "spotify:show:recommended":{"name":"Another show","pages":{"items":[{"entity":{"data":{"uri":"spotify:episode:other","name":"Unrelated"}}}]}}
        }}});
        let page = format!(
            "<script id='initialState' type='text/plain'>{}</script>",
            STANDARD.encode(serde_json::to_vec(&state).unwrap())
        );
        let (meta, items) = spotify_items(
            &page,
            &Url::parse("https://open.spotify.com/intl-fr/show/show123?si=tracking").unwrap(),
        )
        .unwrap();
        assert_eq!(meta.title.as_deref(), Some("The show"));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].link, "https://open.spotify.com/episode/episode123");
        assert_eq!(items[0].title, "Episode & title");
        assert_eq!(items[0].authors, ["Publisher"]);
        assert_eq!(
            items[0].published.unwrap().to_rfc3339(),
            "2026-08-26T21:47:00+00:00"
        );
        assert!(!items[0].content_html.as_ref().unwrap().contains("script"));
        assert_eq!(
            items[0].preview_candidates[0].url,
            "https://example.com/art.jpg"
        );
        assert!(items[0].labels.is_empty());
    }

    #[test]
    fn missing_or_empty_spotify_metadata_is_a_failure() {
        let url = Url::parse("https://open.spotify.com/show/show123").unwrap();
        assert!(spotify_items("<title>Sign in</title>", &url).is_err());
        let state = serde_json::json!({"entities":{"items":{"spotify:show:show123":{"pages":{"items":[]}}}}});
        let page = format!(
            "<script id='initialState'>{}</script>",
            STANDARD.encode(serde_json::to_vec(&state).unwrap())
        );
        assert!(spotify_items(&page, &url).is_err());
    }
}
