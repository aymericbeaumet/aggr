//! RSS / Atom / JSON Feed via feed-rs, with conditional GET and a body-hash short circuit.

use std::borrow::Cow;
use std::io::Read;

use anyhow::{Context as _, Result, bail};
use feed_rs::model::{Entry, Feed, Link, Text};
use url::Url;

use super::{Context, Fetch, SourceMeta, Validators};
use crate::config::Source;
use crate::content;
use crate::http::{Request, Response};
use crate::model::{RawItem, sha1_hex};

pub async fn fetch(url: &Url, source: &Source, ctx: &Context<'_>) -> Result<Fetch> {
    if url.scheme() == "file" {
        return fetch_local(url, source, ctx).await;
    }
    let remembered = (ctx.state.identity == source.identity)
        .then_some(ctx.state.resolved_url.as_deref())
        .flatten()
        .and_then(|url| Url::parse(url).ok());
    let endpoint = remembered.as_ref().unwrap_or(url);
    let previous = if ctx.state.identity == source.identity {
        Validators::from_state(ctx.state)
    } else {
        Validators::default()
    };
    let response = match request(endpoint, source, ctx, &previous).await {
        Ok(response) => response,
        Err(err) if endpoint != url => {
            log::debug!(
                "{}: cached endpoint {endpoint} failed, rediscovering: {err:#}",
                source.slug
            );
            return fetch_fresh(url, source, ctx).await;
        }
        Err(primary) => {
            if let Some(discovered) = discover_common(url, source, ctx).await {
                return Ok(discovered);
            }
            return Err(primary).context("fetching the configured URL and common feed endpoints");
        }
    };
    interpret(response, source, ctx, previous, remembered.is_none()).await
}

async fn fetch_local(url: &Url, source: &Source, ctx: &Context<'_>) -> Result<Fetch> {
    let path = url
        .to_file_path()
        .map_err(|()| anyhow::anyhow!("invalid local feed path: {url}"))?;
    let max_body_bytes = ctx.client.max_body_bytes();
    let bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
        let file = std::fs::File::open(&path).context("opening local feed")?;
        let mut bytes = Vec::new();
        file.take((max_body_bytes as u64).saturating_add(1))
            .read_to_end(&mut bytes)
            .context("reading local feed")?;
        if bytes.len() > max_body_bytes {
            bail!("body exceeds {max_body_bytes} bytes");
        }
        Ok(bytes)
    })
    .await
    .context("reading local feed")??;
    let validators = Validators {
        body_hash: Some(sha1_hex(&bytes)),
        resolved_url: Some(url.to_string()),
        ..Default::default()
    };
    if ctx.state.identity == source.identity && validators.body_hash == ctx.state.body_hash {
        return Ok(Fetch::Unchanged { validators });
    }
    let feed = parse(&bytes, url)?;
    let base = local_document_base(&feed).unwrap_or_else(|| url.clone());
    let feed = if base == *url {
        feed
    } else {
        parse(&bytes, &base)?
    };
    let site_url = pick_link(&feed.links)
        .filter(|link| link.rel.as_deref() != Some("self") && is_web_url(&link.href))
        .map(|link| link.href.clone());
    let mut result = changed(feed, &base, validators, &bytes);
    if let Fetch::Changed { meta, items, .. } = &mut result {
        meta.site_url = site_url;
        items.retain(|item| is_web_url(&item.link));
        for item in items {
            let thumbnail = item
                .extra
                .remove("thumbnail")
                .and_then(|value| value.as_str().and_then(|value| base.join(value).ok()))
                .filter(|url| is_web_url(url.as_str()));
            if let Some(thumbnail) = thumbnail {
                item.extra
                    .insert("thumbnail".into(), thumbnail.to_string().into());
            }
            item.preview_candidates
                .retain(|candidate| is_web_url(&candidate.url));
        }
    }
    Ok(result)
}

fn is_web_url(value: &str) -> bool {
    Url::parse(value).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
}

fn local_document_base(feed: &Feed) -> Option<Url> {
    // A saved feed's public endpoint preserves relative article and image links.
    feed.links
        .iter()
        .filter(|link| {
            is_web_url(&link.href)
                && matches!(link.rel.as_deref(), None | Some("self" | "alternate"))
        })
        .min_by_key(|link| match link.rel.as_deref() {
            Some("self") => 0,
            None | Some("alternate") => 1,
            _ => 2,
        })
        .and_then(|link| Url::parse(&link.href).ok())
}

async fn fetch_fresh(url: &Url, source: &Source, ctx: &Context<'_>) -> Result<Fetch> {
    let response = match request(url, source, ctx, &Validators::default()).await {
        Ok(response) => response,
        Err(primary) => {
            if let Some(discovered) = discover_common(url, source, ctx).await {
                return Ok(discovered);
            }
            return Err(primary).context("fetching the configured URL and common feed endpoints");
        }
    };
    interpret(response, source, ctx, Validators::default(), true).await
}

pub(super) async fn request(
    url: &Url,
    source: &Source,
    ctx: &Context<'_>,
    previous: &Validators,
) -> Result<Response> {
    ctx.client
        .get(Request {
            url,
            headers: crate::http::source_headers(source, url),
            etag: previous.etag.as_deref(),
            last_modified: previous.last_modified.as_deref(),
        })
        .await
}

/// Read a discovered publisher endpoint without accepting an unrelated HTML listing.
pub(super) async fn fetch_endpoint(url: &Url, source: &Source, ctx: &Context<'_>) -> Result<Fetch> {
    let Response::Ok(body) = request(url, source, ctx, &Validators::default()).await? else {
        bail!("publisher endpoint returned no feed");
    };
    let feed = parse(&body.bytes, &body.final_url).context("parsing publisher podcast feed")?;
    let validators = Validators {
        etag: body.etag.clone(),
        last_modified: body.last_modified.clone(),
        body_hash: Some(sha1_hex(&body.bytes)),
        resolved_url: Some(body.final_url.to_string()),
    };
    Ok(changed(feed, &body.final_url, validators, &body.bytes))
}

async fn interpret(
    response: Response,
    source: &Source,
    ctx: &Context<'_>,
    previous: Validators,
    first_discovery: bool,
) -> Result<Fetch> {
    let body = match response {
        Response::NotModified => {
            return Ok(Fetch::Unchanged {
                validators: previous,
            });
        }
        Response::Ok(body) => body,
    };

    let validators = Validators {
        etag: body.etag.clone(),
        last_modified: body.last_modified.clone(),
        body_hash: Some(sha1_hex(&body.bytes)),
        resolved_url: Some(body.final_url.to_string()),
    };
    if validators.body_hash == previous.body_hash {
        return Ok(Fetch::Unchanged { validators });
    }

    if let Ok(feed) = parse(&body.bytes, &body.final_url) {
        return Ok(changed(feed, &body.final_url, validators, &body.bytes));
    }

    let page = body.html_text();
    let mut candidates = crate::sources::html::feed_links(&page, &body.final_url);
    candidates.extend(super::podcast::feed_links(&page, &body.final_url));
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|url| seen.insert(url.clone()));
    for candidate in candidates {
        if candidate == body.final_url {
            continue;
        }
        match request(&candidate, source, ctx, &Validators::default()).await {
            Ok(Response::Ok(feed_body)) => match parse(&feed_body.bytes, &feed_body.final_url) {
                Ok(feed) => {
                    return Ok(changed(
                        feed,
                        &feed_body.final_url,
                        Validators {
                            etag: feed_body.etag,
                            last_modified: feed_body.last_modified,
                            body_hash: Some(sha1_hex(&feed_body.bytes)),
                            resolved_url: Some(feed_body.final_url.to_string()),
                        },
                        &feed_body.bytes,
                    ));
                }
                Err(err) => log::debug!(
                    "{}: advertised feed {candidate} did not parse: {err:#}",
                    source.slug
                ),
            },
            Ok(Response::NotModified) => {}
            Err(err) => log::debug!(
                "{}: advertised feed {candidate} failed: {err:#}",
                source.slug
            ),
        }
    }

    if super::podcast::is_spotify_show(&body.final_url) {
        let (meta, items) = super::podcast::spotify_items(&page, &body.final_url)?;
        return Ok(Fetch::Changed {
            validators,
            meta,
            items,
        });
    }
    if super::podcast::is_deezer_show(&body.final_url) {
        let (meta, items) = super::podcast::deezer_items(&page, &body.final_url)?;
        return Ok(Fetch::Changed {
            validators,
            meta,
            items,
        });
    }

    // A real feed beats heuristic card extraction, so probe the conventional endpoints before
    // reading the listing. Only the first resolution pays for it: afterwards the source remembers
    // whichever endpoint answered.
    if first_discovery && let Some(discovered) = discover_at(&body.final_url, source, ctx).await {
        return Ok(discovered);
    }

    match crate::sources::html::extract(&page, &body.final_url) {
        Ok((meta, items)) => Ok(Fetch::Changed {
            validators,
            meta,
            items,
        }),
        Err(html_err) => {
            if let Some(discovered) = discover_common(&body.final_url, source, ctx).await {
                return Ok(discovered);
            }
            Err(html_err).context("discovering a feed or article listing")
        }
    }
}

fn changed(feed: Feed, url: &Url, validators: Validators, bytes: &[u8]) -> Fetch {
    let (meta, mut items) = convert(&feed, url);
    supplement_json_images(bytes, url, &mut items);
    Fetch::Changed {
        validators,
        meta,
        items,
    }
}

fn supplement_json_images(bytes: &[u8], base: &Url, items: &mut [RawItem]) {
    if bytes
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace())
        != Some(b'{')
    {
        return;
    }
    let Ok(document) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return;
    };
    let Some(entries) = document.get("items").and_then(serde_json::Value::as_array) else {
        return;
    };
    let by_id: std::collections::HashMap<_, _> = entries
        .iter()
        .filter_map(|entry| Some((entry.get("id")?.as_str()?, entry)))
        .collect();
    for item in items {
        let Some(entry) = item.id.as_deref().and_then(|id| by_id.get(id)) else {
            continue;
        };
        for name in ["image", "banner_image"] {
            if let Some(url) = entry
                .get(name)
                .and_then(serde_json::Value::as_str)
                .and_then(|value| base.join(value).ok())
                .filter(|url| matches!(url.scheme(), "http" | "https"))
            {
                item.preview_candidates.push(crate::preview::Candidate {
                    url: url.to_string(),
                    alt: None,
                });
            }
        }
    }
}

/// Conventional feed endpoints under the configured section only: a site-wide feed found at the
/// root would quietly replace the section the user asked for.
async fn discover_at(url: &Url, source: &Source, ctx: &Context<'_>) -> Option<Fetch> {
    // A listing that reads fine is only worth replacing by a feed that actually carries entries:
    // sites routinely keep an empty stock feed beside the posts they really publish.
    probe_feeds(section_feed_urls(url), url, source, ctx, true).await
}

async fn discover_common(url: &Url, source: &Source, ctx: &Context<'_>) -> Option<Fetch> {
    probe_feeds(common_feed_urls(url), url, source, ctx, false).await
}

async fn probe_feeds(
    candidates: Vec<Url>,
    url: &Url,
    source: &Source,
    ctx: &Context<'_>,
    require_entries: bool,
) -> Option<Fetch> {
    for candidate in candidates {
        if candidate == *url {
            continue;
        }
        let body = match request(&candidate, source, ctx, &Validators::default()).await {
            Ok(Response::Ok(body)) => body,
            Ok(Response::NotModified) => continue,
            Err(err) => {
                log::debug!("{}: common feed {candidate} failed: {err:#}", source.slug);
                continue;
            }
        };
        let feed = match parse(&body.bytes, &body.final_url) {
            Ok(feed) => feed,
            Err(err) => {
                log::debug!(
                    "{}: common feed {candidate} did not parse: {err:#}",
                    source.slug
                );
                continue;
            }
        };
        if require_entries && feed.entries.is_empty() {
            log::debug!("{}: common feed {candidate} is empty", source.slug);
            continue;
        }
        return Some(changed(
            feed,
            &body.final_url,
            Validators {
                etag: body.etag,
                last_modified: body.last_modified,
                body_hash: Some(sha1_hex(&body.bytes)),
                resolved_url: Some(body.final_url.to_string()),
            },
            &body.bytes,
        ));
    }
    None
}

fn section_feed_urls(page: &Url) -> Vec<Url> {
    let base = as_directory(page);
    let mut urls = Vec::new();
    for name in [
        "rss.xml",
        "feed.xml",
        "atom.xml",
        "feed.atom",
        "index.xml",
        "feed",
        "rss",
    ] {
        if let Ok(url) = base.join(name) {
            urls.push(url);
        }
    }
    dedupe(urls)
}

fn common_feed_urls(page: &Url) -> Vec<Url> {
    let mut urls = section_feed_urls(page);
    if let Ok(mut root) = page.join("/") {
        for name in ["feed.xml", "rss.xml", "atom.xml", "feed.atom", "index.xml"] {
            root.set_path(&format!("/{name}"));
            urls.push(root.clone());
        }
    }
    dedupe(urls)
}

/// A listing URL names a section, so `…/blog` and `…/blog/` must resolve the same candidates.
/// A final segment carrying an extension is a document and keeps its parent as the base.
fn as_directory(page: &Url) -> Url {
    let last = page
        .path_segments()
        .and_then(Iterator::last)
        .unwrap_or_default();
    if last.is_empty() || last.contains('.') {
        return page.clone();
    }
    let mut directory = page.clone();
    directory.set_path(&format!("{}/", page.path()));
    directory
}

fn dedupe(mut urls: Vec<Url>) -> Vec<Url> {
    let mut seen = std::collections::HashSet::new();
    urls.retain(|url| seen.insert(url.as_str().to_owned()));
    urls
}

/// RSS, Atom or JSON Feed bytes; relative links resolve against the URL the body came from.
pub fn parse(bytes: &[u8], base: &Url) -> Result<Feed> {
    let normalized = normalize_podcast_durations(bytes);
    feed_rs::parser::Builder::new()
        .base_uri(Some(base.as_str()))
        .build()
        .parse(normalized.as_ref())
        .context("parsing feed")
}

fn normalize_podcast_durations(bytes: &[u8]) -> Cow<'_, [u8]> {
    use quick_xml::{events::Event, name::ResolveResult, reader::NsReader};

    if !bytes
        .windows(b"duration".len())
        .any(|part| part == b"duration")
    {
        return Cow::Borrowed(bytes);
    }
    let mut reader = NsReader::from_reader(bytes);
    let mut replacements = Vec::new();
    loop {
        let Ok((namespace, event)) = reader.read_resolved_event() else {
            return Cow::Borrowed(bytes);
        };
        match event {
            Event::Start(element)
                if element.local_name().as_ref() == b"duration"
                    && matches!(namespace, ResolveResult::Bound(namespace) if namespace.as_ref() == b"http://www.itunes.com/dtds/podcast-1.0.dtd") =>
            {
                let start = reader.buffer_position() as usize;
                let Some((end, text)) = podcast_duration_text(&mut reader) else {
                    return Cow::Borrowed(bytes);
                };
                // feed-rs treats MM:SS as seconds and accepts numeric substrings in invalid values.
                let value = crate::media_duration::parse(&text)
                    .map(|seconds| seconds.to_string())
                    .unwrap_or_default();
                if &bytes[start..end] != value.as_bytes() {
                    replacements.push((start..end, value));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if replacements.is_empty() {
        return Cow::Borrowed(bytes);
    }
    let mut normalized = Vec::with_capacity(bytes.len());
    let mut copied = 0;
    for (range, value) in replacements {
        normalized.extend_from_slice(&bytes[copied..range.start]);
        normalized.extend_from_slice(value.as_bytes());
        copied = range.end;
    }
    normalized.extend_from_slice(&bytes[copied..]);
    Cow::Owned(normalized)
}

fn podcast_duration_text(
    reader: &mut quick_xml::reader::NsReader<&[u8]>,
) -> Option<(usize, String)> {
    use quick_xml::events::Event;
    let mut text = String::new();
    let mut depth = 1;
    let mut nested = false;
    loop {
        let position = reader.buffer_position() as usize;
        match reader.read_event().ok()? {
            Event::Text(value) if depth == 1 => text.push_str(&value.decode().ok()?),
            Event::CData(value) if depth == 1 => text.push_str(&value.decode().ok()?),
            Event::GeneralRef(value) if depth == 1 => {
                let escaped = format!("&{};", value.decode().ok()?);
                text.push_str(&quick_xml::escape::unescape(&escaped).ok()?);
            }
            Event::Start(_) => {
                depth += 1;
                nested = true;
            }
            Event::Empty(_) => nested = true,
            Event::End(_) => {
                depth -= 1;
                if depth == 0 {
                    return Some((position, if nested { String::new() } else { text }));
                }
            }
            Event::Eof => return None,
            _ => {}
        }
    }
}

/// Pure mapping from a parsed feed to raw items; entries without a usable link are dropped.
pub fn convert(feed: &Feed, feed_url: &Url) -> (SourceMeta, Vec<RawItem>) {
    let meta = SourceMeta {
        title: feed.title.as_ref().map(text_of).filter(|t| !t.is_empty()),
        site_url: pick_link(&feed.links)
            .map(|link| link.href.clone())
            .filter(|href| href != feed_url.as_str()),
    };
    let items = feed
        .entries
        .iter()
        .filter_map(|entry| convert_entry(entry, feed_url))
        .collect();
    (meta, items)
}

fn convert_entry(entry: &Entry, feed_url: &Url) -> Option<RawItem> {
    let audio = entry
        .media
        .iter()
        .flat_map(|media| media.content.iter())
        .filter(|content| {
            content
                .content_type
                .as_ref()
                .is_some_and(|kind| kind.ty() == "audio")
        })
        .filter_map(|content| content.url.as_ref())
        .find(|url| matches!(url.scheme(), "http" | "https"));
    let link = pick_link(&entry.links)
        .map(|link| link.href.clone())
        .or_else(|| {
            Url::parse(&entry.id)
                .ok()
                .filter(|url| matches!(url.scheme(), "http" | "https"))
                .map(|url| url.to_string())
        })
        .or_else(|| audio.map(Url::to_string))?;
    let link = feed_url
        .join(&link)
        .map(|url| url.to_string())
        .unwrap_or(link);
    if Url::parse(&link).is_ok_and(|url| super::youtube::is_short_url(&url)) {
        return None;
    }
    let title = entry
        .title
        .as_ref()
        .map(text_of)
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| untitled(&link));

    let video_description = Url::parse(&link)
        .is_ok_and(|url| super::youtube::is_video_url(&url))
        .then(|| {
            entry
                .media
                .iter()
                .filter_map(|media| media.description.as_ref())
                .find(|description| !description.content.trim().is_empty())
        })
        .flatten();
    let content_html = entry
        .content
        .as_ref()
        .and_then(|content| content.body.as_deref())
        .map(|body| {
            let is_html = entry.content.as_ref().is_some_and(|c| {
                c.content_type.subty() == "html" || c.content_type.subty() == "xhtml"
            });
            if is_html {
                body.to_string()
            } else {
                text_to_html(body)
            }
        })
        .or_else(|| {
            entry
                .summary
                .as_ref()
                .filter(|summary| is_html(summary))
                .map(|summary| summary.content.clone())
        })
        .filter(|html| !html.trim().is_empty())
        .or_else(|| {
            video_description.map(|description| {
                if is_html(description) {
                    description.content.clone()
                } else {
                    text_to_html(&description.content)
                }
            })
        });

    let summary = entry
        .summary
        .as_ref()
        .map(text_of)
        .filter(|text| !text.is_empty())
        .or_else(|| {
            video_description.map(|description| {
                if is_html(description) {
                    text_of(description)
                } else {
                    description.content.clone()
                }
            })
        });

    let mut extra = std::collections::BTreeMap::new();
    if let Some(url) = audio {
        extra.insert("audio_url".to_string(), url.to_string().into());
    }
    let duration = entry
        .media
        .iter()
        .find_map(|media| {
            media
                .content
                .iter()
                .find(|content| {
                    content
                        .url
                        .as_ref()
                        .is_some_and(|url| Some(url) == audio || url.as_str() == link)
                })
                .and_then(|content| content.duration.or(media.duration))
        })
        .or_else(|| {
            // A single media object may carry iTunes duration independently of its enclosure.
            (entry.media.len() == 1)
                .then(|| &entry.media[0])
                .and_then(|media| {
                    media.duration.or_else(|| {
                        media
                            .content
                            .iter()
                            .filter(|content| {
                                content.content_type.as_ref().is_some_and(|kind| {
                                    matches!(kind.ty().as_str(), "audio" | "video")
                                })
                            })
                            .find_map(|content| content.duration)
                    })
                })
        });
    if let Some(seconds) = duration
        .map(|duration| duration.as_secs())
        .filter(|seconds| *seconds > 0)
    {
        extra.insert("duration_seconds".into(), seconds.into());
    }
    if let Some(thumbnail) = entry
        .media
        .iter()
        .flat_map(|media| media.thumbnails.iter())
        .map(|thumbnail| thumbnail.image.uri.clone())
        .next()
    {
        extra.insert("thumbnail".to_string(), thumbnail.into());
    }

    Some(RawItem {
        id: Some(entry.id.clone()).filter(|id| !id.trim().is_empty()),
        title,
        link,
        published: entry.published.or(entry.updated),
        updated: entry.updated,
        first_seen: None,
        authors: entry.authors.iter().filter_map(person_name).collect(),
        labels: entry
            .categories
            .iter()
            .map(|category| {
                category
                    .label
                    .clone()
                    .unwrap_or_else(|| category.term.clone())
            })
            .map(|tag| tag.trim().to_string())
            .filter(|tag| !tag.is_empty())
            .collect(),
        summary,
        content_html,
        extra,
        preview_candidates: entry
            .media
            .iter()
            .flat_map(|media| {
                media
                    .thumbnails
                    .iter()
                    .map(|thumbnail| crate::preview::Candidate {
                        url: feed_url
                            .join(&thumbnail.image.uri)
                            .map(|url| url.to_string())
                            .unwrap_or_else(|_| thumbnail.image.uri.clone()),
                        alt: thumbnail.image.title.clone(),
                    })
                    .chain(
                        media
                            .content
                            .iter()
                            .filter(|content| {
                                content
                                    .content_type
                                    .as_ref()
                                    .is_some_and(|kind| kind.ty() == "image")
                            })
                            .filter_map(|content| content.url.as_ref())
                            .map(|url| crate::preview::Candidate {
                                url: url.to_string(),
                                alt: None,
                            }),
                    )
            })
            .collect(),
        preview: None,
        images: Vec::new(),
    })
}

/// The `alternate` HTML link when there is one, else the first link.
fn pick_link(links: &[Link]) -> Option<&Link> {
    links
        .iter()
        .find(|link| {
            link.rel.as_deref().is_none_or(|rel| rel == "alternate")
                && link
                    .media_type
                    .as_deref()
                    .is_none_or(|media_type| media_type.starts_with("text/html"))
        })
        .or_else(|| links.first())
}

fn is_html(text: &Text) -> bool {
    matches!(text.content_type.subty().as_str(), "html" | "xhtml")
}

/// Feeds routinely declare HTML titles and summaries as plain text; stripping is harmless on
/// real plain text, so it is done regardless of the declared type.
fn text_of(text: &Text) -> String {
    content::html_to_text(&text.content)
}

fn text_to_html(text: &str) -> String {
    let escaped = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    escaped
        .split("\n\n")
        .map(|para| format!("<p>{}</p>", para.trim().replace('\n', "<br>")))
        .collect::<Vec<_>>()
        .join("\n")
}

/// feed-rs names RSS `<author>` persons "author" and keeps `mail@host (Real Name)` as the email.
fn person_name(person: &feed_rs::model::Person) -> Option<String> {
    let name = person.name.trim();
    if !name.is_empty() && name != "author" {
        return Some(name.to_string());
    }
    let email = person.email.as_deref()?.trim();
    let name = email
        .split_once('(')
        .and_then(|(_, rest)| rest.strip_suffix(')'))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(email);
    (!name.is_empty()).then(|| name.to_string())
}

pub(super) fn untitled(link: &str) -> String {
    Url::parse(link)
        .ok()
        .and_then(|url| url.host_str().map(|host| host.to_string()))
        .unwrap_or_else(|| "Untitled".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    const RSS: &str = r#"<?xml version="1.0"?>
<rss version="2.0"><channel>
  <title>Example &amp; Co</title>
  <link>https://example.com/</link>
  <item>
    <title>First &lt;b&gt;post&lt;/b&gt;</title>
    <link>https://example.com/first</link>
    <guid>first-guid</guid>
    <pubDate>Tue, 01 Sep 2026 10:00:00 GMT</pubDate>
    <author>a@example.com (Alice)</author>
    <category>rust</category>
    <description><![CDATA[<p>Hello <em>world</em></p>]]></description>
  </item>
  <item>
    <title></title>
    <link>/relative</link>
    <description>plain text only</description>
  </item>
  <item>
    <title>No link at all</title>
    <description>dropped</description>
  </item>
</channel></rss>"#;

    const ATOM: &str = r#"<?xml version="1.0"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:media="http://search.yahoo.com/mrss/">
  <title>Atom</title>
  <link rel="self" href="https://example.com/feed.xml"/>
  <link rel="alternate" type="text/html" href="https://example.com/"/>
  <entry>
    <id>tag:example.com,2026:1</id>
    <title type="html">&lt;i&gt;Fancy&lt;/i&gt; title</title>
    <link rel="enclosure" href="https://example.com/a.mp3"/>
    <link rel="alternate" href="https://example.com/a"/>
    <updated>2026-09-02T10:00:00Z</updated>
    <content type="text">line one
line two</content>
    <media:thumbnail url="https://example.com/a.jpg"/>
  </entry>
</feed>"#;

    fn parse(text: &str, url: &str) -> (SourceMeta, Vec<RawItem>) {
        let url = Url::parse(url).unwrap();
        let feed = feed_rs::parser::Builder::new()
            .base_uri(Some(url.as_str()))
            .build()
            .parse(text.as_bytes())
            .unwrap();
        convert(&feed, &url)
    }

    #[test]
    fn youtube_uses_media_description_and_drops_shorts() {
        let xml = r#"<feed xmlns="http://www.w3.org/2005/Atom" xmlns:media="http://search.yahoo.com/mrss/">
          <title>Videos</title>
          <entry>
            <id>yt:video:long</id><title>Full video</title>
            <link href="https://www.youtube.com/watch?v=long"/>
            <media:group><media:description type="plain">A &lt; B &amp; C

Second paragraph.</media:description></media:group>
          </entry>
          <entry>
            <id>yt:video:short</id><title>Short video</title>
            <link href="https://www.youtube.com/shorts/short"/>
          </entry>
        </feed>"#;
        let (_, items) = parse(xml, "https://example.com/feed.xml");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Full video");
        assert_eq!(
            items[0].content_html.as_deref(),
            Some("<p>A &lt; B &amp; C</p>\n<p>Second paragraph.</p>")
        );
        assert_eq!(
            items[0].summary.as_deref(),
            Some("A < B & C\n\nSecond paragraph.")
        );
    }

    #[test]
    fn converts_rss() {
        let (meta, items) = parse(RSS, "https://example.com/feed.xml");
        assert_eq!(meta.title.as_deref(), Some("Example & Co"));
        assert_eq!(meta.site_url.as_deref(), Some("https://example.com/"));
        assert_eq!(items.len(), 2, "entries without a link are dropped");

        let first = &items[0];
        assert_eq!(first.title, "First post");
        assert_eq!(first.link, "https://example.com/first");
        assert_eq!(first.id.as_deref(), Some("first-guid"));
        assert_eq!(
            first.published.unwrap().to_rfc3339(),
            "2026-09-01T10:00:00+00:00"
        );
        assert_eq!(first.authors, vec!["Alice"]);
        assert_eq!(first.labels, vec!["rust"]);
        assert_eq!(first.summary.as_deref(), Some("Hello world"));
        assert_eq!(
            first.content_html.as_deref(),
            Some("<p>Hello <em>world</em></p>")
        );

        let second = &items[1];
        assert_eq!(second.link, "https://example.com/relative");
        assert_eq!(second.title, "example.com");
        assert_eq!(second.summary.as_deref(), Some("plain text only"));
    }

    #[test]
    fn converts_atom() {
        let (meta, items) = parse(ATOM, "https://example.com/feed.xml");
        assert_eq!(meta.site_url.as_deref(), Some("https://example.com/"));
        let item = &items[0];
        assert_eq!(item.title, "Fancy title");
        assert_eq!(
            item.link, "https://example.com/a",
            "alternate wins over enclosure"
        );
        assert_eq!(
            item.published, item.updated,
            "published falls back to updated"
        );
        assert_eq!(
            item.content_html.as_deref(),
            Some("<p>line one<br>line two</p>")
        );
        assert_eq!(
            item.extra["thumbnail"],
            serde_yaml_ng::Value::from("https://example.com/a.jpg")
        );
    }

    #[test]
    fn podcast_without_an_episode_page_keeps_its_audio_link() {
        let xml = r#"<rss version="2.0"><channel><title>Podcast</title><link>https://example.com</link><description>Show</description><item><guid isPermaLink="false">episode-id</guid><title>Episode</title><description>Notes</description><enclosure url="https://example.com/episode.mp3" type="audio/mpeg" length="100"/></item></channel></rss>"#;
        let (_, items) = parse(xml, "https://example.com/feed.xml");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].link, "https://example.com/episode.mp3");
        assert_eq!(
            items[0].extra["audio_url"].as_str(),
            Some("https://example.com/episode.mp3")
        );
    }

    #[test]
    fn duration_normalization_respects_namespaces_and_preserves_article_markup() {
        let xml = br#"<rss version="2.0" xmlns:pod="http://www.itunes.com/dtds/podcast-1.0.dtd" xmlns:other="https://example.com/extensions"><channel><title>Podcast</title><item><guid>one</guid><title>Episode</title><link>https://example.com/episode</link><enclosure url="https://example.com/audio.mp3" type="audio/mpeg" length="987654"/><pod:duration>27&#58;51</pod:duration><other:duration>15:20</other:duration><description><![CDATA[<p>The example is <pod:duration>12:00</pod:duration>.</p>]]></description></item></channel></rss>"#;
        let expected = String::from_utf8(xml.to_vec())
            .unwrap()
            .replace("27&#58;51", "1671");
        assert_eq!(
            normalize_podcast_durations(xml).as_ref(),
            expected.as_bytes()
        );
        assert!(
            matches!(
                normalize_podcast_durations(expected.as_bytes()),
                Cow::Borrowed(_)
            ),
            "canonical feed durations do not copy the response again"
        );
        let base = Url::parse("https://example.com/feed").unwrap();
        let feed = super::parse(xml, &base).unwrap();
        let raw = convert_entry(&feed.entries[0], &base).unwrap();
        assert_eq!(raw.extra["duration_seconds"].as_u64(), Some(1671));
        assert!(
            raw.content_html
                .unwrap()
                .contains("<pod:duration>12:00</pod:duration>")
        );
    }

    #[test]
    fn media_duration_preserves_publisher_seconds_instead_of_enclosure_byte_lengths() {
        let base = Url::parse("https://example.com/feed").unwrap();
        for (element, expected) in [
            ("<itunes:duration>1:00:01</itunes:duration>", Some(3601)),
            ("<itunes:duration>61</itunes:duration>", Some(61)),
            ("<itunes:duration>27:51</itunes:duration>", Some(1671)),
            (
                "<itunes:duration><![CDATA[18:26]]></itunes:duration>",
                Some(1106),
            ),
            ("<itunes:duration>0</itunes:duration>", None),
            ("<itunes:duration>unknown</itunes:duration>", None),
            ("<itunes:duration>27 minutes</itunes:duration>", None),
            ("<itunes:duration>2:99</itunes:duration>", None),
            (
                "<itunes:duration>184467440737095516150</itunes:duration>",
                None,
            ),
            ("", None),
        ] {
            let xml = format!(
                r#"<rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd"><channel><title>Podcast</title><item><guid>one</guid><title>Episode</title><link>https://example.com/episode</link><enclosure url="https://example.com/audio.mp3" type="audio/mpeg" length="12345"/>{element}</item></channel></rss>"#
            );
            let feed = super::parse(xml.as_bytes(), &base).unwrap();
            let raw = convert_entry(&feed.entries[0], &base).unwrap();
            assert_eq!(
                raw.extra
                    .get("duration_seconds")
                    .and_then(serde_yaml_ng::Value::as_u64),
                expected,
                "{element}"
            );
        }
        let xml = br#"<rss version="2.0" xmlns:media="http://search.yahoo.com/mrss/"><channel><title>Videos</title><item><guid>movie</guid><title>Movie</title><link>https://example.com/movie.mp4</link><media:content url="https://example.com/movie.mp4" type="video/mp4" duration="125"/></item></channel></rss>"#;
        let feed = super::parse(xml, &base).unwrap();
        assert_eq!(
            convert_entry(&feed.entries[0], &base).unwrap().extra["duration_seconds"].as_u64(),
            Some(125)
        );
    }

    #[tokio::test]
    async fn embedded_podcast_feed_is_remembered_and_conditionally_fetched() {
        let server = MockServer::start_async().await;
        let endpoint = server.url("/opaque-feed-id");
        let page = server.mock_async(|when, then| {
            when.method(GET).path("/show");
            then.status(200).body(format!(r#"<script type="application/json">{{"podcast":{{"rssFeedUrl":"{endpoint}"}}}}</script>"#));
        }).await;
        let rss = server
            .mock_async(|when, then| {
                when.method(GET).path("/opaque-feed-id");
                then.status(200).header("etag", "podcast-v1").body(RSS);
            })
            .await;
        let configured = Url::parse(&server.url("/show")).unwrap();
        let source = source(configured.clone());
        let client = crate::http::Client::new(&crate::config::FetchConfig::default()).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let mut state = crate::store::SourceState::default();
        let result = fetch(
            &configured,
            &source,
            &Context {
                client: &client,
                state: &state,
                cache_dir: directory.path(),
            },
        )
        .await
        .unwrap();
        let Fetch::Changed {
            validators, items, ..
        } = result
        else {
            panic!("expected podcast episodes");
        };
        assert_eq!(items.len(), 2);
        assert_eq!(validators.resolved_url.as_deref(), Some(endpoint.as_str()));
        validators.apply(&mut state);
        state.identity = source.identity.clone();
        rss.delete_async().await;
        let cached = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/opaque-feed-id")
                    .header("if-none-match", "podcast-v1");
                then.status(304);
            })
            .await;
        let result = fetch(
            &configured,
            &source,
            &Context {
                client: &client,
                state: &state,
                cache_dir: directory.path(),
            },
        )
        .await
        .unwrap();
        assert!(matches!(result, Fetch::Unchanged { .. }));
        page.assert_calls_async(1).await;
        cached.assert_calls_async(1).await;
    }

    #[test]
    fn escapes_plain_text_content() {
        assert_eq!(
            text_to_html("a < b\n\nc & d"),
            "<p>a &lt; b</p>\n<p>c &amp; d</p>"
        );
    }

    fn source(url: Url) -> Source {
        let public_url = url.to_string();
        Source {
            slug: "site".into(),
            name: None,
            category: None,
            labels: vec![],
            identity: "site-v1".into(),
            public_url: Some(public_url),
            persist_endpoint: true,
            headers: vec![],
            html: true,
            previews: false,
            images: true,
            content: crate::config::ContentMode::Light,
            engine: crate::config::Engine::Feed { url },
        }
    }

    #[tokio::test]
    async fn local_feed_documents_share_parsing_and_body_hash_validation() {
        let directory = tempfile::tempdir().unwrap();
        let client = crate::http::Client::new(&crate::config::FetchConfig::default()).unwrap();
        let json = r#"{"version":"https://jsonfeed.org/version/1.1","title":"JSON","home_page_url":"https://example.com/","feed_url":"https://example.com/feed.json","items":[{"id":"post","url":"https://example.com/post","content_text":"Body","image":"/image.jpg"}]}"#;
        for (name, body, expected_count) in [
            ("rss.xml", RSS, 2),
            ("atom.xml", ATOM, 1),
            ("feed.json", json, 1),
        ] {
            let path = directory.path().join(name);
            std::fs::write(&path, body).unwrap();
            let url = Url::from_file_path(&path).unwrap();
            let source = source(url.clone());
            let mut state = crate::store::SourceState::default();
            let first = fetch(
                &url,
                &source,
                &Context {
                    client: &client,
                    state: &state,
                    cache_dir: directory.path(),
                },
            )
            .await
            .unwrap();
            let Fetch::Changed {
                validators,
                items,
                meta,
            } = first
            else {
                panic!("expected local feed items")
            };
            assert_eq!(items.len(), expected_count);
            assert_eq!(meta.site_url.as_deref(), Some("https://example.com/"));
            assert!(
                items
                    .iter()
                    .all(|item| item.link.starts_with("https://example.com/"))
            );
            assert_eq!(validators.body_hash, Some(sha1_hex(body.as_bytes())));
            assert_eq!(validators.resolved_url.as_deref(), Some(url.as_str()));
            if name == "feed.json" {
                assert_eq!(
                    items[0].preview_candidates[0].url,
                    "https://example.com/image.jpg"
                );
            }
            validators.apply(&mut state);
            state.identity = source.identity.clone();
            let second = fetch(
                &url,
                &source,
                &Context {
                    client: &client,
                    state: &state,
                    cache_dir: directory.path(),
                },
            )
            .await
            .unwrap();
            assert!(matches!(second, Fetch::Unchanged { .. }));
            std::fs::write(
                &path,
                body.replace("Example", "Changed")
                    .replace("Atom", "Changed")
                    .replace("JSON", "Changed"),
            )
            .unwrap();
            let third = fetch(
                &url,
                &source,
                &Context {
                    client: &client,
                    state: &state,
                    cache_dir: directory.path(),
                },
            )
            .await
            .unwrap();
            assert!(matches!(third, Fetch::Changed { .. }));
        }
    }

    #[tokio::test]
    async fn local_feed_without_public_base_drops_filesystem_article_links() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("feed.xml");
        let body = RSS.replace("<link>https://example.com/</link>", "");
        std::fs::write(&path, body).unwrap();
        let url = Url::from_file_path(&path).unwrap();
        let source = source(url.clone());
        let client = crate::http::Client::new(&crate::config::FetchConfig::default()).unwrap();
        let state = crate::store::SourceState::default();
        let Fetch::Changed { items, meta, .. } = fetch(
            &url,
            &source,
            &Context {
                client: &client,
                state: &state,
                cache_dir: directory.path(),
            },
        )
        .await
        .unwrap() else {
            panic!("expected local feed items")
        };
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].link, "https://example.com/first");
        assert!(meta.site_url.is_none());
    }

    #[tokio::test]
    async fn local_feed_without_public_base_drops_filesystem_media_links() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("feed.xml");
        let body = ATOM
            .replace(
                "<link rel=\"self\" href=\"https://example.com/feed.xml\"/>",
                "",
            )
            .replace(
                "<link rel=\"alternate\" type=\"text/html\" href=\"https://example.com/\"/>",
                "",
            )
            .replace("https://example.com/a.jpg", "image.jpg");
        std::fs::write(&path, body).unwrap();
        let url = Url::from_file_path(&path).unwrap();
        let source = source(url.clone());
        let client = crate::http::Client::new(&crate::config::FetchConfig::default()).unwrap();
        let state = crate::store::SourceState::default();
        let Fetch::Changed { items, .. } = fetch(
            &url,
            &source,
            &Context {
                client: &client,
                state: &state,
                cache_dir: directory.path(),
            },
        )
        .await
        .unwrap() else {
            panic!("expected local feed items")
        };
        assert_eq!(items.len(), 1);
        assert!(!items[0].extra.contains_key("thumbnail"));
        assert!(items[0].preview_candidates.is_empty());
    }

    #[tokio::test]
    async fn local_feed_read_errors_do_not_expose_filesystem_paths() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("missing.xml");
        let url = Url::from_file_path(&path).unwrap();
        let source = source(url.clone());
        let client = crate::http::Client::new(&crate::config::FetchConfig::default()).unwrap();
        let state = crate::store::SourceState::default();
        let error = fetch(
            &url,
            &source,
            &Context {
                client: &client,
                state: &state,
                cache_dir: directory.path(),
            },
        )
        .await
        .err()
        .expect("missing local file must fail");
        let message = format!("{error:#}");
        assert!(message.contains("opening local feed"));
        assert!(!message.contains(directory.path().to_str().unwrap()));
        assert!(!message.contains("file://"));
    }

    #[test]
    fn local_document_base_prefers_the_public_feed_endpoint() {
        let url = Url::parse("file:///tmp/feed.xml").unwrap();
        let atom = ATOM.replace(
            "https://example.com/feed.xml",
            "https://example.com/news/feed.xml",
        );
        let feed = super::parse(atom.as_bytes(), &url).unwrap();
        assert_eq!(
            local_document_base(&feed).unwrap().as_str(),
            "https://example.com/news/feed.xml"
        );
    }

    #[tokio::test]
    async fn local_feed_documents_enforce_the_configured_body_limit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("feed.xml");
        std::fs::write(&path, RSS).unwrap();
        let url = Url::from_file_path(&path).unwrap();
        let source = source(url.clone());
        let client = crate::http::Client::new(&crate::config::FetchConfig {
            max_body_bytes: 64,
            ..Default::default()
        })
        .unwrap();
        let state = crate::store::SourceState::default();
        let result = fetch(
            &url,
            &source,
            &Context {
                client: &client,
                state: &state,
                cache_dir: directory.path(),
            },
        )
        .await;
        let error = result.err().expect("oversized local feed must fail");
        assert!(format!("{error:#}").contains("body exceeds 64 bytes"));
    }

    #[test]
    fn conventional_feed_endpoints_treat_a_listing_path_as_a_section() {
        let expected = [
            "https://example.com/blog/rss.xml",
            "https://example.com/blog/feed.xml",
            "https://example.com/blog/atom.xml",
            "https://example.com/blog/feed.atom",
            "https://example.com/blog/index.xml",
            "https://example.com/blog/feed",
            "https://example.com/blog/rss",
        ];
        for page in ["https://example.com/blog", "https://example.com/blog/"] {
            let urls = section_feed_urls(&Url::parse(page).unwrap());
            assert_eq!(
                urls.iter().map(Url::as_str).collect::<Vec<_>>(),
                expected,
                "{page}"
            );
        }
        // A section probe never reaches the root: a site-wide feed is not what the user asked for.
        assert!(
            section_feed_urls(&Url::parse("https://example.com/blog").unwrap())
                .iter()
                .all(|url| url.path().starts_with("/blog/"))
        );
        assert!(
            common_feed_urls(&Url::parse("https://example.com/blog").unwrap())
                .iter()
                .any(|url| url.path() == "/feed.xml")
        );
        // A document keeps its directory as the base instead of growing a segment.
        assert!(
            section_feed_urls(&Url::parse("https://example.com/blog/index.html").unwrap())
                .iter()
                .all(|url| url.path().starts_with("/blog/") && !url.path().contains("index.html"))
        );
    }

    #[test]
    fn json_feed_images_survive_feed_rs_conversion() {
        let url = Url::parse("https://example.com/feed.json").unwrap();
        let bytes = br#"{"version":"https://jsonfeed.org/version/1.1","title":"Example","items":[{"id":"post","url":"https://example.com/post","content_text":"Body","image":"/image.jpg","banner_image":"/banner.webp"}]}"#;
        let Fetch::Changed { items, .. } = changed(
            super::parse(bytes, &url).unwrap(),
            &url,
            Validators::default(),
            bytes,
        ) else {
            panic!("changed feed")
        };
        assert_eq!(
            items[0]
                .preview_candidates
                .iter()
                .map(|image| image.url.as_str())
                .collect::<Vec<_>>(),
            [
                "https://example.com/image.jpg",
                "https://example.com/banner.webp"
            ]
        );
    }

    #[test]
    fn configured_headers_do_not_follow_unrelated_article_origins() {
        let mut source = source(Url::parse("https://example.com/feed").unwrap());
        source
            .headers
            .push(("Authorization".into(), "Bearer private".into()));
        assert_eq!(
            crate::http::source_headers(
                &source,
                &Url::parse("https://example.com/article").unwrap()
            ),
            source.headers
        );
        assert!(
            crate::http::source_headers(
                &source,
                &Url::parse("https://other.example/article").unwrap()
            )
            .is_empty()
        );
        assert!(
            crate::http::source_headers(
                &source,
                &Url::parse("https://example.com:8443/article").unwrap()
            )
            .is_empty()
        );
    }

    #[tokio::test]
    async fn discovers_then_reuses_the_feed_endpoint() {
        let server = MockServer::start_async().await;
        let homepage = server
            .mock_async(|when, then| {
                when.method(GET).path("/blog");
                then.status(200).body(
                    "<link rel=\"alternate\" type=\"application/rss+xml\" href=\"/feed.xml\">",
                );
            })
            .await;
        let first_feed = server
            .mock_async(|when, then| {
                when.method(GET).path("/feed.xml");
                then.status(200).header("etag", "\"v1\"").body(RSS);
            })
            .await;
        let configured = Url::parse(&server.url("/blog")).unwrap();
        let source = source(configured.clone());
        let client = crate::http::Client::new(&crate::config::FetchConfig::default()).unwrap();
        let cache = tempfile::tempdir().unwrap();
        let mut state = crate::store::SourceState::default();
        let ctx = Context {
            client: &client,
            state: &state,
            cache_dir: cache.path(),
        };
        let Fetch::Changed {
            validators, items, ..
        } = fetch(&configured, &source, &ctx).await.unwrap()
        else {
            panic!("expected discovered feed");
        };
        assert_eq!(items.len(), 2);
        assert_eq!(
            validators.resolved_url.as_deref(),
            Some(server.url("/feed.xml").as_str())
        );
        validators.apply(&mut state);
        state.identity = source.identity.clone();
        first_feed.delete_async().await;

        let conditional = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/feed.xml")
                    .header("if-none-match", "\"v1\"");
                then.status(304);
            })
            .await;
        let ctx = Context {
            client: &client,
            state: &state,
            cache_dir: cache.path(),
        };
        assert!(matches!(
            fetch(&configured, &source, &ctx).await.unwrap(),
            Fetch::Unchanged { .. }
        ));
        homepage.assert_calls_async(1).await;
        conditional.assert_calls_async(1).await;
    }

    #[tokio::test]
    async fn falls_back_to_article_cards_when_a_site_has_no_feed() {
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(GET).path("/news");
                then.status(200).body(
                    "<title>News</title><article><h2><a href=\"/news/one\">One story</a></h2><time datetime=\"2026-09-02\"></time></article>",
                );
            })
            .await;
        let configured = Url::parse(&server.url("/news")).unwrap();
        let source = source(configured.clone());
        let client = crate::http::Client::new(&crate::config::FetchConfig::default()).unwrap();
        let state = crate::store::SourceState::default();
        let cache = tempfile::tempdir().unwrap();
        let ctx = Context {
            client: &client,
            state: &state,
            cache_dir: cache.path(),
        };
        let Fetch::Changed {
            items, validators, ..
        } = fetch(&configured, &source, &ctx).await.unwrap()
        else {
            panic!("expected HTML fallback");
        };
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].link, server.url("/news/one"));
        assert_eq!(
            validators.resolved_url.as_deref(),
            Some(configured.as_str())
        );
    }

    #[tokio::test]
    async fn a_bare_section_url_prefers_its_conventional_feed_over_article_cards() {
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(GET).path("/news");
                then.status(200).body(
                    "<title>News</title><article><h2><a href=\"/news/one\">One story</a></h2><time datetime=\"2026-09-02\"></time></article>",
                );
            })
            .await;
        // The first conventional candidate exists but publishes nothing, so probing continues.
        server
            .mock_async(|when, then| {
                when.method(GET).path("/news/rss.xml");
                then.status(200)
                    .body("<rss version=\"2.0\"><channel><title>Empty</title></channel></rss>");
            })
            .await;
        server
            .mock_async(|when, then| {
                when.method(GET).path("/news/feed.xml");
                then.status(200).body(
                    "<rss version=\"2.0\"><channel><title>News</title><item><title>One story</title><link>https://example.com/news/one</link></item></channel></rss>",
                );
            })
            .await;
        let configured = Url::parse(&server.url("/news")).unwrap();
        let source = source(configured.clone());
        let client = crate::http::Client::new(&crate::config::FetchConfig::default()).unwrap();
        let state = crate::store::SourceState::default();
        let cache = tempfile::tempdir().unwrap();
        let ctx = Context {
            client: &client,
            state: &state,
            cache_dir: cache.path(),
        };
        let Fetch::Changed {
            items, validators, ..
        } = fetch(&configured, &source, &ctx).await.unwrap()
        else {
            panic!("expected the discovered feed");
        };
        assert_eq!(items.len(), 1);
        assert_eq!(
            validators.resolved_url.as_deref(),
            Some(server.url("/news/feed.xml").as_str())
        );
    }
}
