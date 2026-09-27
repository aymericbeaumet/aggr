//! Best-effort expansion of public ActivityPub self-reply threads.
//!
//! Publishers remain the source of truth: every followed URL stays on the status' origin,
//! traversal is tightly bounded, and an unavailable or unfamiliar representation simply leaves
//! the normal article extraction path in charge.
//! X threads use a separate public HTML adapter; Bluesky uses AT Protocol.

mod embeds;
mod x;
pub use embeds::{expand_embedded_x, format_archived_x_embeds};
pub use x::canonical_url as canonical_x_url;
pub use x::expand as expand_x;

use std::collections::{BTreeSet, HashSet, VecDeque};
use std::sync::LazyLock;

use anyhow::{Context as _, Result, bail};
use chrono::{DateTime, Utc};
use regex::Regex;
use scraper::{Html, Selector};
use serde_json::Value;
use url::{Origin, Url};

use crate::cache::ArticleCache;
use crate::config::Source;
use crate::content::{self, ExtractedArticle, escape_html};
use crate::http;

const ACTIVITY_ACCEPT: &str = "application/activity+json, application/ld+json; profile=\"https://www.w3.org/ns/activitystreams\"";
const ACTIVITYSTREAMS_NAMESPACE: &str = "https://www.w3.org/ns/activitystreams#";
#[cfg(test)]
const PUBLIC_AUDIENCE: &str = "https://www.w3.org/ns/activitystreams#Public";
const MAX_POSTS: usize = 32;
const MAX_DEPTH: usize = 20;
const MAX_COLLECTION_PAGES: usize = 32;
const MAX_REQUESTS: usize = 64;
const MAX_COLLECTION_ITEMS: usize = 64;
const MAX_ALTERNATES: usize = 3;
const MAX_LINK_HOPS: usize = 4;
const EXPANSION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Expand a public self-reply thread advertised by `page`, or by a conservative
/// Mastodon-compatible status URL when the page omits discovery metadata.
#[cfg(test)]
pub async fn expand(
    page: &str,
    page_url: &Url,
    source: &Source,
    client: &http::Client,
    cache: &ArticleCache,
) -> Result<Option<ExtractedArticle>> {
    expand_alternates(activity_candidates(page, page_url), source, client, cache).await
}

/// [`expand`] from representations already discovered with [`activity_candidates_in`]; a page
/// that advertised nothing costs no request and no timer.
pub async fn expand_alternates(
    candidates: Vec<Url>,
    source: &Source,
    client: &http::Client,
    cache: &ArticleCache,
) -> Result<Option<ExtractedArticle>> {
    if candidates.is_empty() {
        return Ok(None);
    }
    tokio::time::timeout(
        EXPANSION_TIMEOUT,
        expand_bounded(candidates, source, client, cache),
    )
    .await
    .context("ActivityPub thread expansion exceeded 10 seconds")?
}

async fn expand_bounded(
    candidates: Vec<Url>,
    source: &Source,
    client: &http::Client,
    cache: &ArticleCache,
) -> Result<Option<ExtractedArticle>> {
    let mut last_error = None;
    let mut remaining_requests = MAX_REQUESTS;
    for candidate in candidates {
        if remaining_requests == 0 {
            break;
        }
        let mut remote = Remote::new(
            candidate.origin(),
            source,
            client,
            cache,
            remaining_requests,
        );
        let result = remote.expand_from(&candidate).await;
        remaining_requests = remaining_requests.saturating_sub(remote.requests);
        match result {
            Ok(Some(expanded)) => return Ok(Some(expanded)),
            Ok(None) => {}
            Err(error) => last_error = Some(error),
        }
    }
    match last_error {
        Some(error) => Err(error),
        None => Ok(None),
    }
}

/// Whether discovery can find anything on `page` at all: an advertised `activity+json` (or an
/// `ld+json` alternate carrying the ActivityStreams profile) or a conservative status URL. The
/// scan is case-insensitive like the `type` matching it stands in for, and cheap enough to run
/// before any parse; ordinary articles fail it and are never parsed for alternates.
pub fn may_advertise_activity(page: &str, page_url: &Url) -> bool {
    conservative_status_url(page_url)
        || page
            .as_bytes()
            .windows(8)
            .any(|window| window.eq_ignore_ascii_case(b"activity"))
}

#[cfg(test)]
fn activity_candidates(page: &str, page_url: &Url) -> Vec<Url> {
    if !may_advertise_activity(page, page_url) {
        return Vec::new();
    }
    activity_candidates_in(&Html::parse_document(page), page_url)
}

/// The representations worth a discovery request, in preference order: advertised same-origin
/// alternates first, then the page itself when it looks like a status URL. Callers pre-check
/// with [`may_advertise_activity`].
pub fn activity_candidates_in(document: &Html, page_url: &Url) -> Vec<Url> {
    let mut candidates = Vec::new();
    let mut seen = BTreeSet::new();
    if let Ok(selector) = Selector::parse("link[rel][href]") {
        for link in document.select(&selector) {
            let rel = link.value().attr("rel").unwrap_or_default();
            if !rel
                .split_ascii_whitespace()
                .any(|value| value.eq_ignore_ascii_case("alternate"))
            {
                continue;
            }
            let media_type = link.value().attr("type").unwrap_or_default();
            if !is_activity_alternate_content_type(media_type) {
                continue;
            }
            let Some(url) = safe_same_origin_url(
                link.value().attr("href").unwrap_or_default(),
                page_url,
                &page_url.origin(),
            ) else {
                continue;
            };
            if seen.insert(url.as_str().to_string()) {
                candidates.push(url);
                if candidates.len() >= MAX_ALTERNATES {
                    break;
                }
            }
        }
    }
    if conservative_status_url(page_url) {
        let mut url = page_url.clone();
        url.set_fragment(None);
        if seen.insert(url.as_str().to_string()) {
            candidates.push(url);
        }
    }
    candidates
}

fn conservative_status_url(url: &Url) -> bool {
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return false;
    }
    let Some(segments) = url.path_segments() else {
        return false;
    };
    let segments = segments
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    let id = match segments.as_slice() {
        [account, id] if account.starts_with('@') && account.len() > 1 => Some(*id),
        [account, "statuses", id] if account.starts_with('@') && account.len() > 1 => Some(*id),
        ["users", account, "statuses", id] if !account.is_empty() => Some(*id),
        ["notice", id] => Some(*id),
        _ => None,
    };
    id.is_some_and(|id| {
        !id.is_empty()
            && id.len() <= 128
            && id.bytes().any(|byte| byte.is_ascii_digit())
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    })
}

struct Remote<'a> {
    origin: Origin,
    source: &'a Source,
    client: &'a http::Client,
    cache: &'a ArticleCache,
    requests: usize,
    request_limit: usize,
    collection_pages: usize,
    visited_pages: HashSet<String>,
}

impl<'a> Remote<'a> {
    fn new(
        origin: Origin,
        source: &'a Source,
        client: &'a http::Client,
        cache: &'a ArticleCache,
        request_limit: usize,
    ) -> Self {
        Self {
            origin,
            source,
            client,
            cache,
            requests: 0,
            request_limit,
            collection_pages: 0,
            visited_pages: HashSet::new(),
        }
    }

    async fn expand_from(&mut self, url: &Url) -> Result<Option<ExtractedArticle>> {
        let root_value = self.fetch_json(url).await?;
        let requested = self.parse_remote_post(root_value, url).await?;
        if requested.id.origin() != self.origin {
            bail!("ActivityPub object moved to another origin");
        }

        let root_author = requested.author.clone();
        let mut lineage = vec![requested];
        let mut lineage_ids = HashSet::from([lineage[0].id.as_str().to_string()]);
        while lineage.len() <= MAX_DEPTH && lineage.len() < MAX_POSTS {
            let Some(child) = lineage.last() else {
                break;
            };
            let Some(parent_url) = child.parent.clone() else {
                break;
            };
            if parent_url.origin() != self.origin
                || !lineage_ids.insert(parent_url.as_str().to_string())
            {
                break;
            }
            let parent = match self.fetch_json(&parent_url).await {
                Ok(value) => match self.parse_remote_post(value, &parent_url).await {
                    Ok(parent) => parent,
                    Err(error) => {
                        log::debug!(
                            "stopping at an unsupported ActivityPub parent {parent_url}: {error:#}"
                        );
                        break;
                    }
                },
                Err(error) => {
                    log::debug!(
                        "stopping at an unavailable ActivityPub parent {parent_url}: {error:#}"
                    );
                    break;
                }
            };
            if parent.id != parent_url || parent.author != root_author {
                break;
            }
            lineage.push(parent);
        }
        lineage.reverse();

        let mut posts = lineage;
        let mut children = vec![Vec::<usize>::new(); posts.len()];
        for index in 1..posts.len() {
            children[index - 1].push(index);
        }
        let mut accepted = posts
            .iter()
            .map(|post| post.id.as_str().to_string())
            .collect::<HashSet<_>>();
        let mut pending = posts
            .iter()
            .enumerate()
            .map(|(index, _)| (index, index))
            .collect::<VecDeque<_>>();

        while let Some((parent_index, depth)) = pending.pop_front() {
            if depth >= MAX_DEPTH || posts.len() >= MAX_POSTS {
                continue;
            }
            let parent_id = posts[parent_index].id.clone();
            let replies = posts[parent_index].replies.clone();
            let candidates = self.reply_candidates(replies, &parent_id).await;
            let mut direct = Vec::new();
            for candidate in candidates {
                let parsed = match candidate {
                    ReplyCandidate::Inline { value, base } => {
                        self.parse_remote_post(value, &base).await
                    }
                    ReplyCandidate::Url(url) => match self.fetch_json(&url).await {
                        Ok(value) => self.parse_remote_post(value, &url).await,
                        Err(error) => {
                            log::debug!("skipping unavailable ActivityPub reply {url}: {error:#}");
                            continue;
                        }
                    },
                };
                let Ok(post) = parsed else {
                    continue;
                };
                if post.id.origin() != self.origin
                    || post.author != root_author
                    || post.parent.as_ref() != Some(&parent_id)
                    || accepted.contains(post.id.as_str())
                {
                    continue;
                }
                direct.push(post);
            }
            direct.sort_by(post_order);
            for post in direct {
                if posts.len() >= MAX_POSTS {
                    break;
                }
                if !accepted.insert(post.id.as_str().to_string()) {
                    continue;
                }
                let child_index = posts.len();
                posts.push(post);
                children.push(Vec::new());
                children[parent_index].push(child_index);
                pending.push_back((child_index, depth + 1));
            }
        }

        for child_indexes in &mut children {
            child_indexes.sort_by(|left, right| post_order(&posts[*left], &posts[*right]));
        }

        if posts.len() < 2 {
            return Ok(None);
        }
        let mut order = Vec::with_capacity(posts.len());
        preorder(0, &children, &mut order);
        Ok(Some(render(&posts, &order)))
    }

    async fn parse_remote_post(&mut self, mut value: Value, base: &Url) -> Result<Post> {
        let mut current_base = base.clone();
        let mut visited = HashSet::new();
        for hop in 0..=MAX_LINK_HOPS {
            let Some(object) = value.as_object() else {
                bail!("ActivityPub response is not an object");
            };
            let is_create = contains_activitystreams_term(object.get("type"), "Create");
            if !is_create {
                return parse_post(&value, &current_base);
            }
            let Some(target) = object.get("object") else {
                return parse_post(&value, &current_base);
            };
            if !is_activity_link(target) {
                return parse_post(&value, &current_base);
            }
            if hop == MAX_LINK_HOPS {
                bail!("ActivityPub Create object links exceeded their hop limit");
            }
            let url = activity_link_url(target, &current_base, &self.origin)
                .context("ActivityPub Create object link is invalid or cross-origin")?;
            if !visited.insert(url.as_str().to_string()) {
                bail!("ActivityPub Create object links contain a cycle");
            }
            value = self.fetch_json(&url).await?;
            current_base = url;
        }
        bail!("ActivityPub Create object links exceeded their hop limit")
    }

    async fn reply_candidates(
        &mut self,
        replies: Option<Value>,
        base: &Url,
    ) -> Vec<ReplyCandidate> {
        let Some(replies) = replies else {
            return Vec::new();
        };
        let mut current = Some((replies, base.clone()));
        let mut candidates = Vec::new();

        while let Some((value, current_base)) = current.take() {
            if self.collection_pages >= MAX_COLLECTION_PAGES
                || candidates.len() >= MAX_COLLECTION_ITEMS
            {
                break;
            }
            self.collection_pages += 1;
            let (value, document_base) = if is_activity_link(&value) {
                let Some(url) = activity_link_url(&value, &current_base, &self.origin) else {
                    break;
                };
                if points_to_other_accounts(&url)
                    || !self.visited_pages.insert(url.as_str().to_string())
                {
                    break;
                }
                match self.fetch_json(&url).await {
                    Ok(value) => (value, url),
                    Err(error) => {
                        log::debug!(
                            "stopping at unavailable ActivityPub replies page {url}: {error:#}"
                        );
                        break;
                    }
                }
            } else {
                (value, current_base)
            };
            let Some(object) = value.as_object() else {
                break;
            };
            if !object.contains_key("items") && !object.contains_key("orderedItems") {
                current = object
                    .get("first")
                    .cloned()
                    .map(|first| (first, document_base.clone()));
                if current.is_none() {
                    break;
                }
                continue;
            }
            let items = object.get("items").or_else(|| object.get("orderedItems"));
            for item in values(items) {
                if candidates.len() >= MAX_COLLECTION_ITEMS {
                    break;
                }
                if is_activity_link(item) {
                    if let Some(url) = activity_link_url(item, &document_base, &self.origin) {
                        candidates.push(ReplyCandidate::Url(url));
                    }
                } else if item.is_object() {
                    candidates.push(ReplyCandidate::Inline {
                        value: item.clone(),
                        base: document_base.clone(),
                    });
                }
            }
            current = object
                .get("next")
                .cloned()
                .filter(|next| {
                    if is_activity_link(next) {
                        activity_link_url(next, &document_base, &self.origin)
                            .is_some_and(|url| !points_to_other_accounts(&url))
                    } else {
                        value_url(next, &document_base)
                            .as_ref()
                            .is_none_or(|url| !points_to_other_accounts(url))
                    }
                })
                .map(|next| (next, document_base));
        }
        candidates
    }

    async fn fetch_json(&mut self, url: &Url) -> Result<Value> {
        if self.requests >= self.request_limit {
            bail!("ActivityPub thread exceeded its request limit");
        }
        if url.origin() != self.origin {
            bail!("refusing a cross-origin ActivityPub request");
        }
        self.requests += 1;

        let headers = http::with_accept(http::source_headers(self.source, url), ACTIVITY_ACCEPT);
        let cached = self.cache.load(url, &headers)?;
        let response = self
            .client
            .get(http::Request {
                url,
                headers: &headers,
                etag: cached.as_ref().and_then(|entry| entry.etag.as_deref()),
                last_modified: cached
                    .as_ref()
                    .and_then(|entry| entry.last_modified.as_deref()),
            })
            .await;
        let response = match response {
            Ok(http::Response::Ok(body)) => self.cache.store(url, &headers, &body)?,
            Ok(http::Response::NotModified) => cached
                .context("ActivityPub endpoint returned not modified without a cached response")?,
            Err(error) => match cached {
                Some(cached) => {
                    log::debug!("using cached ActivityPub response after fetch failed: {error:#}");
                    cached
                }
                None => return Err(error),
            },
        };
        if response.final_url.origin() != self.origin {
            bail!("ActivityPub endpoint redirected to another origin");
        }
        if !is_activity_content_type(response.content_type.as_deref()) {
            bail!("ActivityPub endpoint did not return JSON");
        }
        serde_json::from_slice(&response.bytes).context("parsing ActivityPub response")
    }
}

#[derive(Debug)]
enum ReplyCandidate {
    Inline { value: Value, base: Url },
    Url(Url),
}

#[derive(Debug, Clone)]
struct Post {
    id: Url,
    author: Url,
    parent: Option<Url>,
    published: Option<DateTime<Utc>>,
    content: String,
    warning: Option<String>,
    sensitive: bool,
    attachments: Vec<Attachment>,
    replies: Option<Value>,
}

#[derive(Debug, Clone)]
struct Attachment {
    url: Url,
    alt: String,
    width: Option<u32>,
    height: Option<u32>,
}

fn parse_post(value: &Value, base: &Url) -> Result<Post> {
    let value = unwrap_create(value).context("ActivityPub response is not an object")?;
    let supported_post = ["Note", "Article", "Page", "Question"]
        .iter()
        .any(|kind| contains_activitystreams_term(value.get("type"), kind));
    if contains_activitystreams_term(value.get("type"), "Announce") || !supported_post {
        bail!("ActivityPub object is not a supported post");
    }
    if !contains_activitystreams_term(value.get("to"), "Public")
        && !contains_activitystreams_term(value.get("cc"), "Public")
    {
        bail!("ActivityPub post is not public");
    }

    let id = required_url(value.get("id"), base, "post id")?;
    let author = required_url(value.get("attributedTo"), &id, "post author")?;
    let parent = value
        .get("inReplyTo")
        .and_then(|value| value_url(value, &id));
    let attachments = parse_attachments(value.get("attachment"), &id);
    let raw_content = localized_string(value, "content", "contentMap").unwrap_or_default();
    let content = content::sanitize(&raw_content, Some(&id));
    if content::html_to_text(&content).trim().is_empty() && attachments.is_empty() {
        bail!("ActivityPub post has no readable content");
    }
    let warning = localized_string(value, "summary", "summaryMap")
        .map(|warning| warning.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|warning| !warning.is_empty())
        .map(|warning| warning.chars().take(500).collect());
    let published = value
        .get("published")
        .and_then(Value::as_str)
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc));
    Ok(Post {
        id,
        author,
        parent,
        published,
        content,
        warning,
        sensitive: value
            .get("sensitive")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        attachments,
        replies: value.get("replies").cloned(),
    })
}

fn unwrap_create(value: &Value) -> Option<&serde_json::Map<String, Value>> {
    let object = value.as_object()?;
    if contains_activitystreams_term(object.get("type"), "Create") {
        object.get("object")?.as_object()
    } else {
        Some(object)
    }
}

fn parse_attachments(value: Option<&Value>, base: &Url) -> Vec<Attachment> {
    values(value)
        .filter_map(Value::as_object)
        .filter_map(|attachment| {
            let media_type = attachment
                .get("mediaType")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            let image_type = contains_activitystreams_term(attachment.get("type"), "Image");
            let url = attachment
                .get("url")
                .and_then(|value| value_url(value, base))?;
            let path = url.path().to_ascii_lowercase();
            let supported_type = matches!(
                media_type.as_str(),
                "image/jpeg" | "image/png" | "image/gif" | "image/webp"
            );
            let supported_path = [".jpg", ".jpeg", ".png", ".gif", ".webp"]
                .iter()
                .any(|extension| path.ends_with(extension));
            if !(supported_type || image_type && supported_path)
                || path.ends_with(".svg")
                || path.ends_with(".svgz")
            {
                return None;
            }
            let alt = attachment
                .get("name")
                .and_then(Value::as_str)
                .map(|value| value.split_whitespace().collect::<Vec<_>>().join(" "))
                .filter(|value| !value.is_empty())
                .map(|value| value.chars().take(300).collect())
                .unwrap_or_else(|| "Image attached to this post".into());
            let dimension = |name| {
                attachment
                    .get(name)
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
                    .filter(|value| (1..=16_384).contains(value))
            };
            Some(Attachment {
                url,
                alt,
                width: dimension("width"),
                height: dimension("height"),
            })
        })
        .collect()
}

fn render(posts: &[Post], order: &[usize]) -> ExtractedArticle {
    let image = order.iter().find_map(|index| {
        posts[*index]
            .attachments
            .first()
            .map(|attachment| attachment.url.to_string())
    });
    let mut html = String::from("<section data-aggr-thread=\"activitypub\">");
    for index in order {
        let post = &posts[*index];
        html.push_str("<article data-aggr-thread-post>");
        if let Some(warning) = &post.warning {
            html.push_str("<p><strong>Content warning:</strong> ");
            html.push_str(&escape_html(warning));
            html.push_str("</p>");
        } else if post.sensitive {
            html.push_str("<p><strong>Sensitive content</strong></p>");
        }
        html.push_str(&strip_thread_counter(&post.content));
        for attachment in &post.attachments {
            html.push_str("<figure><img src=\"");
            html.push_str(&escape_html(attachment.url.as_str()));
            html.push_str("\" alt=\"");
            html.push_str(&escape_html(&attachment.alt));
            html.push('"');
            if let (Some(width), Some(height)) = (attachment.width, attachment.height) {
                html.push_str(&format!(" width=\"{width}\" height=\"{height}\""));
            }
            html.push_str("></figure>");
        }
        html.push_str("</article>");
    }
    html.push_str("</section>");
    ExtractedArticle {
        html,
        image,
        labels: Vec::new(),
    }
}

/// A terminal post counter in archived Markdown, where comrak escapes the square brackets.
static MARKDOWN_COUNTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:^|\s)(\(([0-9]{1,4})/([0-9]{1,4})\)|\\?\[([0-9]{1,4})/([0-9]{1,4})\\?\]|([0-9]{1,4})/([0-9]{1,4}))\s*$",
    )
    .expect("valid markdown thread counter pattern")
});

/// Present an archived social thread as uninterrupted prose: remove terminal post counters,
/// post separators, and generated per-post links or partial-thread notices from earlier
/// captures. The metadata original link already reaches the thread. Stored sources stay intact.
pub fn clean_archived_thread(markdown: &str, link: &str) -> String {
    let Ok(url) = Url::parse(link) else {
        return markdown.to_string();
    };
    if canonical_x_url(&url).is_none() && !conservative_status_url(&url) {
        return markdown.to_string();
    }
    use comrak::nodes::NodeValue;
    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, markdown, &comrak::Options::default());
    let blocks = root.children().collect::<Vec<_>>();
    let mut lines = vec![0];
    lines.extend(markdown.match_indices('\n').map(|(index, _)| index + 1));
    let source_range = |node: &comrak::nodes::AstNode<'_>| {
        let position = node.data.borrow().sourcepos;
        let line_start = *lines.get(position.start.line.saturating_sub(1))?;
        let line_end = *lines.get(position.end.line.saturating_sub(1))?;
        let start = line_start + position.start.column.saturating_sub(1);
        let end = line_end + position.end.column;
        markdown.get(start..end).map(|_| start..end)
    };
    let mut removed = Vec::new();
    for post in blocks.split(|node| matches!(node.data.borrow().value, NodeValue::ThematicBreak)) {
        if let Some(paragraph) = post
            .iter()
            .rev()
            .find(|node| !thread_footer_paragraph(node) && !media_only_paragraph(node))
            && matches!(paragraph.data.borrow().value, NodeValue::Paragraph)
            && paragraph
                .last_child()
                .is_some_and(|node| matches!(node.data.borrow().value, NodeValue::Text(_)))
            && let Some(range) = source_range(paragraph)
            && let Some(captures) = MARKDOWN_COUNTER.captures(&markdown[range.clone()])
            && let Some(marker) = captures.get(1)
        {
            let raw = &markdown[range.clone()];
            let numbers = [2, 4, 6].into_iter().find_map(|index| {
                Some((
                    captures.get(index)?.as_str().parse::<u32>().ok()?,
                    captures.get(index + 1)?.as_str().parse::<u32>().ok()?,
                ))
            });
            let valid = numbers
                .is_some_and(|(position, total)| position > 0 && total > 1 && position <= total);
            let bare_on_shared_line = captures.get(6).is_some()
                && raw[..marker.start()]
                    .rsplit('\n')
                    .next()
                    .is_some_and(|line| !line.trim().is_empty());
            if valid && !bare_on_shared_line {
                removed.push((range.start + raw[..marker.start()].trim_end().len())..range.end);
            }
        }
    }
    for node in &blocks {
        let generated = matches!(node.data.borrow().value, NodeValue::ThematicBreak)
            || thread_footer_paragraph(node);
        if generated && let Some(range) = source_range(node) {
            // Consume the blank lines after a removed block so spacing does not accumulate.
            let trailing = markdown[range.end..]
                .bytes()
                .take_while(|byte| *byte == b'\n')
                .count();
            removed.push(range.start..range.end + trailing);
        }
    }
    removed.sort_by_key(|range| range.start);
    let mut cleaned = markdown.to_string();
    for range in removed.into_iter().rev() {
        cleaned.replace_range(range, "");
    }
    let trimmed = cleaned.trim_end();
    if trimmed.len() != cleaned.len() {
        cleaned.truncate(trimmed.len());
        cleaned.push('\n');
    }
    cleaned
}

fn media_only_paragraph<'a>(node: &'a comrak::nodes::AstNode<'a>) -> bool {
    use comrak::nodes::NodeValue;
    matches!(node.data.borrow().value, NodeValue::Paragraph)
        && node
            .children()
            .all(|child| match &child.data.borrow().value {
                NodeValue::Image(_) | NodeValue::SoftBreak | NodeValue::LineBreak => true,
                NodeValue::Text(text) => text.trim().is_empty(),
                _ => false,
            })
}

/// Paragraphs written by earlier thread expansion rather than the author: per-post links and the
/// partial-thread notice.
fn thread_footer_paragraph<'a>(node: &'a comrak::nodes::AstNode<'a>) -> bool {
    use comrak::nodes::NodeValue;
    if !matches!(node.data.borrow().value, NodeValue::Paragraph) {
        return false;
    }
    let children = node.children().collect::<Vec<_>>();
    match children.as_slice() {
        [prefix, link, suffix] => {
            matches!(&prefix.data.borrow().value, NodeValue::Text(text) if text == "Some replies could not be loaded. ")
                && matches!(&suffix.data.borrow().value, NodeValue::Text(text) if text == ".")
                && thread_footer_link(link, &["View the full thread on X"])
        }
        [link] => thread_footer_link(link, &["View post on X", "Watch video on X"]),
        _ => false,
    }
}

fn thread_footer_link<'a>(node: &'a comrak::nodes::AstNode<'a>, labels: &[&str]) -> bool {
    use comrak::nodes::NodeValue;
    let data = node.data.borrow();
    let NodeValue::Link(link) = &data.value else {
        return false;
    };
    let children = node.children().collect::<Vec<_>>();
    matches!(children.as_slice(), [text] if matches!(&text.data.borrow().value, NodeValue::Text(text) if labels.contains(&text.as_ref())))
        && Url::parse(&link.url)
            .ok()
            .and_then(|url| canonical_x_url(&url))
            .is_some()
}

/// A terminal post counter in a captured post's HTML text node.
static HTML_COUNTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:^|\s)(\(([0-9]{1,4})/([0-9]{1,4})\)|\[([0-9]{1,4})/([0-9]{1,4})\]|^([0-9]{1,4})/([0-9]{1,4}))\s*$",
    )
    .expect("valid HTML thread counter pattern")
});
/// Only whitespace, closing tags and line breaks may follow a counter for it to be terminal.
static TRAILING_MARKUP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:\s|</[a-z][a-z0-9]*\s*>|<br\s*/?>)*$")
        .expect("valid trailing markup pattern")
});

fn strip_thread_counter(html: &str) -> String {
    let document = Html::parse_fragment(html);
    let Some(last) = document.tree.nodes().rfind(
        |node| matches!(node.value(), scraper::node::Node::Text(text) if !text.trim().is_empty()),
    ) else {
        return html.to_string();
    };
    if last
        .ancestors()
        .filter_map(scraper::ElementRef::wrap)
        .any(|element| matches!(element.value().name(), "code" | "pre" | "a"))
    {
        return html.to_string();
    }
    let scraper::node::Node::Text(text) = last.value() else {
        return html.to_string();
    };
    let Some(captures) = HTML_COUNTER.captures(text) else {
        return html.to_string();
    };
    let Some(marker) = captures.get(1) else {
        return html.to_string();
    };
    if captures.get(6).is_some() {
        let block = last
            .ancestors()
            .filter_map(scraper::ElementRef::wrap)
            .find(|element| {
                matches!(
                    element.value().name(),
                    "p" | "div" | "li" | "section" | "article"
                )
            });
        let standalone = block.map_or_else(
            || document.root_element().text().collect::<String>(),
            |element| element.text().collect::<String>(),
        );
        let starts_line = last
            .prev_sibling()
            .and_then(scraper::ElementRef::wrap)
            .is_some_and(|element| element.value().name() == "br");
        if standalone.trim() != marker.as_str() && !starts_line {
            return html.to_string();
        }
    }
    let numbers = [2, 4, 6].into_iter().find_map(|index| {
        Some((
            captures.get(index)?.as_str().parse::<u32>().ok()?,
            captures.get(index + 1)?.as_str().parse::<u32>().ok()?,
        ))
    });
    if !numbers.is_some_and(|(position, total)| position > 0 && total > 1 && position <= total) {
        return html.to_string();
    }
    let Some(start) = html.rfind(marker.as_str()) else {
        return html.to_string();
    };
    let end = start + marker.len();
    if !TRAILING_MARKUP.is_match(&html[end..]) {
        return html.to_string();
    }
    format!("{}{}", html[..start].trim_end(), &html[end..])
}

fn preorder(index: usize, children: &[Vec<usize>], order: &mut Vec<usize>) {
    order.push(index);
    for child in &children[index] {
        preorder(*child, children, order);
    }
}

fn post_order(left: &Post, right: &Post) -> std::cmp::Ordering {
    match (&left.published, &right.published) {
        (Some(left), Some(right)) => left.cmp(right),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
    .then_with(|| left.id.as_str().cmp(right.id.as_str()))
}

fn is_activity_content_type(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        matches!(
            normalized_media_type(value).as_str(),
            "application/activity+json" | "application/ld+json" | "application/json"
        )
    })
}

fn is_activity_alternate_content_type(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized_media_type(&normalized).as_str() {
        "application/activity+json" => true,
        "application/ld+json" => normalized.contains("activitystreams"),
        _ => false,
    }
}

fn normalized_media_type(value: &str) -> String {
    value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

fn values(value: Option<&Value>) -> impl Iterator<Item = &Value> {
    value.into_iter().flat_map(|value| match value {
        Value::Array(values) => values.as_slice(),
        value => std::slice::from_ref(value),
    })
}

fn string_values(value: Option<&Value>) -> Vec<String> {
    values(value)
        .filter_map(|value| match value {
            Value::String(value) => Some(value.clone()),
            Value::Object(object) => object
                .get("id")
                .or_else(|| object.get("href"))
                .and_then(Value::as_str)
                .map(str::to_string),
            _ => None,
        })
        .collect()
}

fn activitystreams_term(value: &str) -> &str {
    value
        .strip_prefix(ACTIVITYSTREAMS_NAMESPACE)
        .or_else(|| value.strip_prefix("as:"))
        .unwrap_or(value)
}

fn contains_activitystreams_term(value: Option<&Value>, expected: &str) -> bool {
    string_values(value)
        .iter()
        .any(|value| activitystreams_term(value) == expected)
}

fn localized_string(
    object: &serde_json::Map<String, Value>,
    key: &str,
    map: &str,
) -> Option<String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            object
                .get(map)
                .and_then(Value::as_object)
                .and_then(|values| values.iter().min_by_key(|(language, _)| *language))
                .and_then(|(_, value)| value.as_str())
                .map(str::to_string)
        })
}

fn required_url(value: Option<&Value>, base: &Url, name: &str) -> Result<Url> {
    value
        .and_then(|value| value_url(value, base))
        .with_context(|| format!("ActivityPub {name} is missing or invalid"))
}

fn value_url(value: &Value, base: &Url) -> Option<Url> {
    match value {
        Value::String(value) => safe_url(value, base),
        Value::Array(values) => values.iter().find_map(|value| value_url(value, base)),
        Value::Object(object) => object
            .get("id")
            .or_else(|| object.get("href"))
            .or_else(|| object.get("url"))
            .and_then(|value| value_url(value, base)),
        _ => None,
    }
}

fn is_activity_link(value: &Value) -> bool {
    match value {
        Value::String(_) => true,
        Value::Object(object) => contains_activitystreams_term(object.get("type"), "Link"),
        _ => false,
    }
}

fn activity_link_url(value: &Value, base: &Url, origin: &Origin) -> Option<Url> {
    let url = match value {
        Value::String(value) => safe_url(value, base),
        Value::Object(object) if is_activity_link(value) => object
            .get("href")
            .or_else(|| object.get("id"))
            .and_then(|value| value_url(value, base)),
        _ => None,
    }?;
    (url.origin() == *origin).then_some(url)
}

fn safe_url(value: &str, base: &Url) -> Option<Url> {
    let mut url = base.join(value.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    url.set_fragment(None);
    Some(url)
}

fn safe_same_origin_url(value: &str, base: &Url, origin: &Origin) -> Option<Url> {
    safe_url(value, base).filter(|url| &url.origin() == origin)
}

fn points_to_other_accounts(url: &Url) -> bool {
    url.query_pairs().any(|(name, value)| {
        if name != "only_other_accounts" {
            return false;
        }
        matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes")
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn cleans_archived_x_threads_into_uninterrupted_prose() {
        let markdown = "First post preserves `1/3` and the fraction 1/3. (1/3)\n\n![Original media](https://pbs.twimg.com/media/one.jpg)\n\n[View post on X](https://x.com/author/status/100)\n\n* * *\n\nEdited second post. \\[2/3\\]\n\n[Watch video on X](https://x.com/author/status/101)\n\n* * *\n\nFinal post.\n3/3\n\n[View post on X](https://x.com/author/status/102)\n\nSome replies could not be loaded. [View the full thread on X](https://x.com/author/status/100).\n";
        let cleaned = super::clean_archived_thread(markdown, "https://x.com/author/status/100");
        assert_eq!(
            cleaned,
            "First post preserves `1/3` and the fraction 1/3.\n\n![Original media](https://pbs.twimg.com/media/one.jpg)\n\nEdited second post.\n\nFinal post.\n"
        );
        assert_eq!(
            super::clean_archived_thread(markdown, "https://example.com/math"),
            markdown
        );
        let mastodon = "Opening. (1/2)\n\n* * *\n\nClosing.\n\n2/2\n";
        assert_eq!(
            super::clean_archived_thread(
                mastodon,
                "https://mathstodon.xyz/@tao/117244102901892965"
            ),
            "Opening.\n\nClosing.\n"
        );
    }

    #[test]
    fn archived_thread_cleanup_preserves_nonterminal_markers_fractions_code_and_links() {
        for markdown in [
            "Use 1/3",
            "Use *1/3*",
            "Use `(1/3)`",
            "[1/3](https://example.com)",
            "First paragraph. (1/3)\n\nMore prose follows.",
            "First paragraph. (1/3)\n\n```text\n2/3\n```",
            "First paragraph. (1/3)\n\nSome replies could not be loaded, but this is my own explanation.",
            "First paragraph. (1/3)\n\nSome replies could not be loaded. [View the full thread on X](https://example.com).",
            "[View post on X](https://example.com/not-x)\n\nMy own [View post on X](https://x.com/author/status/1) sentence.",
        ] {
            assert_eq!(
                super::clean_archived_thread(
                    markdown,
                    "https://mathstodon.xyz/@tao/117244102901892965"
                ),
                markdown,
                "{markdown}"
            );
        }
    }

    #[test]
    fn removes_only_terminal_thread_counters() {
        for (before, after) in [
            ("<p>Opening. (1/3)</p>", "<p>Opening.</p>"),
            ("<p>Continued. [2/3]</p>", "<p>Continued.</p>"),
            ("<p>Ending.</p><p>3/3</p>", "<p>Ending.</p><p></p>"),
            ("<p>Use 1/3</p>", "<p>Use 1/3</p>"),
            ("<p>Use <em>1/3</em></p>", "<p>Use <em>1/3</em></p>"),
            ("Opening<br><br>1/3", "Opening<br><br>"),
            ("<p>Opening<br>1/3</p>", "<p>Opening<br></p>"),
            (
                "<p>Use (1/3) of the input.</p>",
                "<p>Use (1/3) of the input.</p>",
            ),
            (
                "<p>Division: <code>(1/3)</code></p>",
                "<p>Division: <code>(1/3)</code></p>",
            ),
            ("<pre>(1/3)</pre>", "<pre>(1/3)</pre>"),
            ("<p>Invalid (4/3)</p>", "<p>Invalid (4/3)</p>"),
            ("<p>Single (1/1)</p>", "<p>Single (1/1)</p>"),
        ] {
            assert_eq!(super::strip_thread_counter(before), after, "{before}");
        }
    }

    use super::*;
    use httpmock::prelude::*;

    fn source(url: &str) -> Source {
        Source {
            slug: "social".into(),
            name: None,
            category: None,
            labels: Vec::new(),
            identity: "social".into(),
            public_url: Some(url.into()),
            persist_endpoint: true,
            headers: Vec::new(),
            html: true,
            content: crate::config::ContentMode::Heavy,
            previews: true,
            images: crate::config::ImagePolicy::Original,
            engine: crate::config::Engine::Feed {
                url: Url::parse(url).unwrap(),
            },
        }
    }

    fn public_note(
        server: &MockServer,
        id: &str,
        parent: Option<&str>,
        content: &str,
        replies: Value,
    ) -> Value {
        let second = id.parse::<u64>().unwrap_or_default() % 60;
        serde_json::json!({
            "id": server.url(format!("/users/alice/statuses/{id}")),
            "type": "Note",
            "attributedTo": server.url("/users/alice"),
            "inReplyTo": parent.map(|id| server.url(format!("/users/alice/statuses/{id}"))),
            "published": format!("2026-08-08T00:00:{second:02}Z"),
            "to": [PUBLIC_AUDIENCE],
            "content": content,
            "replies": replies
        })
    }

    #[test]
    fn discovers_standard_alternates_and_only_conservative_social_fallbacks() {
        let page = Url::parse("https://social.example/@alice/123").unwrap();
        let candidates = activity_candidates(
            r#"<link rel="alternate" type="Application/Activity+JSON; charset=utf-8" href="/users/alice/statuses/123">"#,
            &page,
        );
        assert_eq!(
            candidates.iter().map(Url::as_str).collect::<Vec<_>>(),
            [
                "https://social.example/users/alice/statuses/123",
                "https://social.example/@alice/123"
            ]
        );
        assert!(conservative_status_url(
            &Url::parse("https://social.example/notice/9hptFmVJ02khbzYJaS").unwrap()
        ));
        assert!(conservative_status_url(
            &Url::parse("https://social.example/@alice/statuses/01FC3GSQ8A3MMJ43BPZSGEG29M")
                .unwrap()
        ));
        assert!(!conservative_status_url(
            &Url::parse("https://x.com/alice/status/123").unwrap()
        ));
        assert!(!conservative_status_url(
            &Url::parse("https://bsky.app/profile/alice.example/post/abc123").unwrap()
        ));
        assert!(
            activity_candidates("", &Url::parse("https://example.com/story/123").unwrap())
                .is_empty()
        );
    }

    #[test]
    fn pages_without_markers_skip_discovery_before_any_parse() {
        let article = Url::parse("https://blog.example/posts/hello").unwrap();
        let status = Url::parse("https://social.example/@alice/123").unwrap();
        let plain = r#"<html><head><link rel="alternate" type="application/rss+xml" href="/feed.xml"><script type="application/ld+json">{"@type":"Article"}</script></head><body><a href="/objects/1">thread</a></body></html>"#;
        assert!(!may_advertise_activity(plain, &article));
        assert!(activity_candidates(plain, &article).is_empty());
        assert!(
            may_advertise_activity(plain, &status),
            "status URLs are probed on shape alone"
        );
        for marker in [
            r#"<link rel="alternate" type="Application/Activity+JSON" href="/objects/1">"#,
            r#"<link rel="alternate" type="application/ld+json; profile=&quot;https://www.w3.org/ns/activitystreams&quot;" href="/objects/1">"#,
        ] {
            assert!(may_advertise_activity(marker, &article), "{marker}");
            assert_eq!(
                activity_candidates(marker, &article)
                    .iter()
                    .map(Url::as_str)
                    .collect::<Vec<_>>(),
                ["https://blog.example/objects/1"],
                "{marker}"
            );
        }
    }

    #[tokio::test]
    async fn expands_only_public_direct_self_replies_in_parent_first_order() {
        crate::http::install_crypto_provider();
        let server = MockServer::start_async().await;
        let root_url = server.url("/users/alice/statuses/100");
        let child_101 = server.url("/users/alice/statuses/101");
        let child_102 = server.url("/users/alice/statuses/102");
        let root = public_note(
            &server,
            "100",
            None,
            "<p>Root post</p><script>bad()</script>",
            serde_json::json!({
                "type": "Collection",
                "first": {
                    "type": "CollectionPage",
                    "items": [
                        child_102,
                        child_101,
                        {
                            "id": server.url("/users/bob/statuses/200"),
                            "type": "Note",
                            "attributedTo": server.url("/users/bob"),
                            "inReplyTo": root_url,
                            "to": [PUBLIC_AUDIENCE],
                            "content": "<p>Other person's branch</p>",
                            "replies": {"items": [{
                                "id": server.url("/users/alice/statuses/201"),
                                "type": "Note",
                                "attributedTo": server.url("/users/alice"),
                                "inReplyTo": server.url("/users/bob/statuses/200"),
                                "to": [PUBLIC_AUDIENCE],
                                "content": "<p>Author replying down another branch</p>"
                            }]}
                        },
                        {
                            "id": server.url("/users/alice/statuses/300"),
                            "type": "Announce",
                            "attributedTo": server.url("/users/alice"),
                            "inReplyTo": root_url,
                            "to": [PUBLIC_AUDIENCE],
                            "content": "<p>Boost</p>"
                        },
                        {
                            "id": server.url("/users/alice/statuses/400"),
                            "type": "Note",
                            "attributedTo": server.url("/users/alice"),
                            "inReplyTo": root_url,
                            "to": [server.url("/users/alice/followers")],
                            "content": "<p>Private post</p>"
                        }
                    ],
                    "next": server.url("/users/alice/statuses/100/replies?only_other_accounts=true&page=true")
                }
            }),
        );
        let child_one = public_note(
            &server,
            "101",
            Some("100"),
            "<p>First continuation</p>",
            serde_json::json!({"items": []}),
        );
        let mut child_two = public_note(
            &server,
            "102",
            Some("100"),
            "<p>Second continuation</p>",
            serde_json::json!({"items": []}),
        );
        child_two["published"] = Value::String("2026-08-08T00:00:59Z".into());
        child_two["summary"] = Value::String("Spoilers & surprises".into());
        child_two["attachment"] = serde_json::json!([{
            "type": "Document",
            "mediaType": "image/png",
            "url": server.url("/media/full.png"),
            "name": "A useful & accurate description",
            "width": 640,
            "height": 480
        }]);

        let root_mock = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/users/alice/statuses/100")
                    .header_matches("accept", ".*application/activity\\+json.*");
                then.status(200)
                    .header("content-type", "application/activity+json")
                    .json_body(root);
            })
            .await;
        let first_mock = server
            .mock_async(|when, then| {
                when.method(GET).path("/users/alice/statuses/101");
                then.status(200)
                    .header("content-type", "application/activity+json")
                    .json_body(child_one);
            })
            .await;
        let second_mock = server
            .mock_async(|when, then| {
                when.method(GET).path("/users/alice/statuses/102");
                then.status(200)
                    .header("content-type", "application/activity+json")
                    .json_body(child_two);
            })
            .await;
        let other_accounts = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/users/alice/statuses/100/replies")
                    .query_param("only_other_accounts", "true");
                then.status(500);
            })
            .await;

        let cache_dir = tempfile::tempdir().unwrap();
        let cache = ArticleCache::new(cache_dir.path());
        let client = http::Client::new(&crate::config::FetchConfig {
            retries: 0,
            ..Default::default()
        })
        .unwrap();
        let page_url = Url::parse(&server.url("/@alice/100")).unwrap();
        let page =
            format!(r#"<link rel="alternate" type="application/activity+json" href="{root_url}">"#);
        let article = expand(
            &page,
            &page_url,
            &source(&server.url("/feed")),
            &client,
            &cache,
        )
        .await
        .unwrap()
        .unwrap();
        let markdown = content::to_markdown(&article.html, Some(&page_url));
        assert!(markdown.contains("Root post"), "{markdown}");
        assert!(markdown.contains("First continuation"), "{markdown}");
        assert!(markdown.contains("Second continuation"), "{markdown}");
        assert!(
            markdown.find("First continuation") < markdown.find("Second continuation"),
            "{markdown}"
        );
        assert!(markdown.contains("Content warning:"), "{markdown}");
        assert!(markdown.contains("Spoilers & surprises"), "{markdown}");
        assert!(
            markdown.contains("A useful & accurate description"),
            "{markdown}"
        );
        assert!(!markdown.contains("bad()"), "{markdown}");
        assert!(!markdown.contains("Other person's branch"), "{markdown}");
        assert!(!markdown.contains("another branch"), "{markdown}");
        assert!(!markdown.contains("Boost"), "{markdown}");
        assert!(!markdown.contains("Private post"), "{markdown}");
        let expected_image = server.url("/media/full.png");
        assert_eq!(article.image.as_deref(), Some(expected_image.as_str()));
        root_mock.assert_calls_async(1).await;
        first_mock.assert_calls_async(1).await;
        second_mock.assert_calls_async(1).await;
        other_accounts.assert_calls_async(0).await;
    }

    #[tokio::test]
    async fn known_mastodon_url_uses_content_negotiation_without_an_alternate() {
        crate::http::install_crypto_provider();
        let server = MockServer::start_async().await;
        let page_url = Url::parse(&server.url("/@alice/100")).unwrap();
        let root = public_note(
            &server,
            "100",
            None,
            "<p>Only post</p>",
            serde_json::json!({"items": []}),
        );
        let requested = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/@alice/100")
                    .header_matches("accept", ".*application/activity\\+json.*");
                then.status(200)
                    .header("content-type", "application/activity+json")
                    .json_body(root);
            })
            .await;
        let cache_dir = tempfile::tempdir().unwrap();
        let cache = ArticleCache::new(cache_dir.path());
        let client = http::Client::new(&crate::config::FetchConfig {
            retries: 0,
            ..Default::default()
        })
        .unwrap();
        assert!(
            expand(
                "<html></html>",
                &page_url,
                &source(&server.url("/feed")),
                &client,
                &cache,
            )
            .await
            .unwrap()
            .is_none()
        );
        requested.assert_calls_async(1).await;
    }

    #[tokio::test]
    async fn tries_a_later_candidate_after_a_single_post_alternate() {
        crate::http::install_crypto_provider();
        let server = MockServer::start_async().await;
        let singleton = public_note(
            &server,
            "100",
            None,
            "<p>Singleton alternate</p>",
            serde_json::json!({"items": []}),
        );
        let child = public_note(
            &server,
            "201",
            Some("200"),
            "<p>Expanded continuation</p>",
            serde_json::json!({"items": []}),
        );
        let expandable = public_note(
            &server,
            "200",
            None,
            "<p>Expanded root</p>",
            serde_json::json!({"items": [child]}),
        );
        let singleton_mock = server
            .mock_async(|when, then| {
                when.method(GET).path("/objects/singleton");
                then.status(200)
                    .header("content-type", "application/activity+json")
                    .json_body(singleton);
            })
            .await;
        let expandable_mock = server
            .mock_async(|when, then| {
                when.method(GET).path("/objects/thread");
                then.status(200)
                    .header("content-type", "application/activity+json")
                    .json_body(expandable);
            })
            .await;

        let cache_dir = tempfile::tempdir().unwrap();
        let cache = ArticleCache::new(cache_dir.path());
        let client = http::Client::new(&crate::config::FetchConfig {
            retries: 0,
            ..Default::default()
        })
        .unwrap();
        let page_url = Url::parse(&server.url("/story/100")).unwrap();
        let page = r#"
            <link rel="alternate" type="application/activity+json" href="/objects/singleton">
            <link rel="alternate" type="application/activity+json" href="/objects/thread">
        "#;
        let article = expand(
            page,
            &page_url,
            &source(&server.url("/feed")),
            &client,
            &cache,
        )
        .await
        .unwrap()
        .unwrap();
        let markdown = content::to_markdown(&article.html, Some(&page_url));

        assert!(!markdown.contains("Singleton alternate"), "{markdown}");
        assert!(markdown.contains("Expanded root"), "{markdown}");
        assert!(markdown.contains("Expanded continuation"), "{markdown}");
        singleton_mock.assert_calls_async(1).await;
        expandable_mock.assert_calls_async(1).await;
    }

    #[tokio::test]
    async fn follows_link_objects_through_collections_and_create_activities() {
        crate::http::install_crypto_provider();
        let server = MockServer::start_async().await;
        let root = public_note(
            &server,
            "100",
            None,
            "<p>Linked collection root</p>",
            serde_json::json!({
                "type": "Link",
                "href": server.url("/collections/root-replies")
            }),
        );
        let collection = serde_json::json!({
            "type": "Collection",
            "first": {
                "type": "Link",
                "id": server.url("/collections/page-one")
            }
        });
        let first_page = serde_json::json!({
            "type": "CollectionPage",
            "items": [{
                "type": "Link",
                "href": server.url("/activities/create-101")
            }, {
                "type": "Link",
                "href": "https://elsewhere.example/users/alice/statuses/999"
            }],
            "next": {
                "type": "Link",
                "href": server.url("/collections/page-two")
            }
        });
        let second_page = serde_json::json!({
            "type": "CollectionPage",
            "items": [{
                "type": "Link",
                "id": server.url("/users/alice/statuses/102")
            }]
        });
        let create = serde_json::json!({
            "id": server.url("/activities/create-101"),
            "type": "Create",
            "actor": server.url("/users/alice"),
            "object": {
                "type": "Link",
                "id": server.url("/users/alice/statuses/101")
            }
        });
        let first_child = public_note(
            &server,
            "101",
            Some("100"),
            "<p>Child behind Create.object</p>",
            serde_json::json!({"items": []}),
        );
        let second_child = public_note(
            &server,
            "102",
            Some("100"),
            "<p>Child on next page</p>",
            serde_json::json!({"items": []}),
        );

        let responses = [
            ("/objects/root", root),
            ("/collections/root-replies", collection),
            ("/collections/page-one", first_page),
            ("/collections/page-two", second_page),
            ("/activities/create-101", create),
            ("/users/alice/statuses/101", first_child),
            ("/users/alice/statuses/102", second_child),
        ];
        let mut mocks = Vec::new();
        for (path, body) in responses {
            mocks.push(
                server
                    .mock_async(move |when, then| {
                        when.method(GET).path(path);
                        then.status(200)
                            .header("content-type", "application/activity+json")
                            .json_body(body);
                    })
                    .await,
            );
        }

        let cache_dir = tempfile::tempdir().unwrap();
        let cache = ArticleCache::new(cache_dir.path());
        let client = http::Client::new(&crate::config::FetchConfig {
            retries: 0,
            ..Default::default()
        })
        .unwrap();
        let page_url = Url::parse(&server.url("/@alice/100")).unwrap();
        let page = format!(
            r#"<link rel="alternate" type="application/activity+json" href="{}">"#,
            server.url("/objects/root")
        );
        let article = expand(
            &page,
            &page_url,
            &source(&server.url("/feed")),
            &client,
            &cache,
        )
        .await
        .unwrap()
        .unwrap();
        let markdown = content::to_markdown(&article.html, Some(&page_url));

        assert!(markdown.contains("Linked collection root"), "{markdown}");
        assert!(
            markdown.contains("Child behind Create.object"),
            "{markdown}"
        );
        assert!(markdown.contains("Child on next page"), "{markdown}");
        assert!(
            markdown.find("Child behind Create.object") < markdown.find("Child on next page"),
            "{markdown}"
        );
        for mock in mocks {
            mock.assert_calls_async(1).await;
        }
    }

    #[tokio::test]
    async fn resolves_collection_page_links_against_their_retrieval_url() {
        crate::http::install_crypto_provider();
        let server = MockServer::start_async().await;
        let root = public_note(
            &server,
            "100",
            None,
            "<p>Relative collection root</p>",
            serde_json::json!({
                "type": "Link",
                "href": "/collections/page-one"
            }),
        );
        let first_page = serde_json::json!({
            "type": "CollectionPage",
            "items": [{
                "type": "Link",
                "href": "children/101"
            }],
            "next": "?page=2"
        });
        let second_page = serde_json::json!({
            "type": "CollectionPage",
            "items": [{
                "id": "children/102",
                "type": "Note",
                "attributedTo": "/users/alice",
                "inReplyTo": "/users/alice/statuses/100",
                "to": [PUBLIC_AUDIENCE],
                "content": "<p>Inline relative continuation</p>",
                "replies": {"items": []}
            }]
        });
        let first_child = public_note(
            &server,
            "101",
            Some("100"),
            "<p>Linked relative continuation</p>",
            serde_json::json!({"items": []}),
        );

        let root_mock = server
            .mock_async(|when, then| {
                when.method(GET).path("/objects/root");
                then.status(200)
                    .header("content-type", "application/activity+json")
                    .json_body(root);
            })
            .await;
        let first_page_mock = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/collections/page-one")
                    .query_param_missing("page");
                then.status(200)
                    .header("content-type", "application/activity+json")
                    .json_body(first_page);
            })
            .await;
        let second_page_mock = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/collections/page-one")
                    .query_param("page", "2");
                then.status(200)
                    .header("content-type", "application/activity+json")
                    .json_body(second_page);
            })
            .await;
        let first_child_mock = server
            .mock_async(|when, then| {
                when.method(GET).path("/collections/children/101");
                then.status(200)
                    .header("content-type", "application/activity+json")
                    .json_body(first_child);
            })
            .await;

        let cache_dir = tempfile::tempdir().unwrap();
        let cache = ArticleCache::new(cache_dir.path());
        let client = http::Client::new(&crate::config::FetchConfig {
            retries: 0,
            ..Default::default()
        })
        .unwrap();
        let page_url = Url::parse(&server.url("/@alice/100")).unwrap();
        let page = format!(
            r#"<link rel="alternate" type="application/activity+json" href="{}">"#,
            server.url("/objects/root")
        );

        let article = expand(
            &page,
            &page_url,
            &source(&server.url("/feed")),
            &client,
            &cache,
        )
        .await
        .unwrap()
        .unwrap();
        let markdown = content::to_markdown(&article.html, Some(&page_url));

        assert!(
            markdown.contains("Linked relative continuation"),
            "{markdown}"
        );
        assert!(
            markdown.contains("Inline relative continuation"),
            "{markdown}"
        );
        root_mock.assert_calls_async(1).await;
        first_page_mock.assert_calls_async(1).await;
        second_page_mock.assert_calls_async(1).await;
        first_child_mock.assert_calls_async(1).await;
    }

    #[test]
    fn activity_link_objects_stay_on_the_root_origin() {
        let base = Url::parse("https://social.example/objects/root").unwrap();
        let origin = base.origin();
        assert_eq!(
            activity_link_url(
                &serde_json::json!({"type": "as:Link", "href": "/objects/child"}),
                &base,
                &origin,
            )
            .as_ref()
            .map(Url::as_str),
            Some("https://social.example/objects/child")
        );
        assert_eq!(
            activity_link_url(
                &serde_json::json!({
                    "type": "https://www.w3.org/ns/activitystreams#Link",
                    "id": "/objects/by-id"
                }),
                &base,
                &origin,
            )
            .as_ref()
            .map(Url::as_str),
            Some("https://social.example/objects/by-id")
        );
        assert!(
            activity_link_url(
                &serde_json::json!({
                    "type": "Link",
                    "href": "https://elsewhere.example/objects/child"
                }),
                &base,
                &origin,
            )
            .is_none()
        );
    }

    #[test]
    fn parses_expanded_and_compact_activitystreams_terms() {
        let base = Url::parse("https://social.example/users/alice/statuses/100").unwrap();
        let expanded_note = serde_json::json!({
            "id": base,
            "type": "https://www.w3.org/ns/activitystreams#Note",
            "attributedTo": "https://social.example/users/alice",
            "to": ["as:Public"],
            "content": "<p>Expanded Note and compact Public</p>"
        });
        assert!(parse_post(&expanded_note, &base).is_ok());

        for create_type in ["as:Create", "https://www.w3.org/ns/activitystreams#Create"] {
            let create = serde_json::json!({
                "type": create_type,
                "object": {
                    "id": base,
                    "type": "as:Note",
                    "attributedTo": "https://social.example/users/alice",
                    "cc": [{"id": PUBLIC_AUDIENCE}],
                    "content": "<p>Compact Note in a Create activity</p>"
                }
            });
            assert!(parse_post(&create, &base).is_ok(), "{create_type}");
        }
    }

    #[tokio::test]
    async fn a_middle_post_includes_same_author_ancestors_and_stops_at_another_author() {
        crate::http::install_crypto_provider();
        let server = MockServer::start_async().await;
        let root_url = server.url("/users/alice/statuses/100");
        let middle_url = server.url("/users/alice/statuses/101");
        let tail_url = server.url("/users/alice/statuses/102");
        let outsider_url = server.url("/users/bob/statuses/99");

        let mut root = public_note(
            &server,
            "100",
            None,
            "<p>Earlier self post</p>",
            serde_json::json!({"items": [middle_url]}),
        );
        root["inReplyTo"] = Value::String(outsider_url.clone());
        let middle = public_note(
            &server,
            "101",
            Some("100"),
            "<p>Linked middle post</p>",
            serde_json::json!({"items": [tail_url]}),
        );
        let tail = public_note(
            &server,
            "102",
            Some("101"),
            "<p>Later self post</p>",
            serde_json::json!({"items": []}),
        );
        let mut outsider = public_note(
            &server,
            "99",
            None,
            "<p>Other author's parent</p>",
            serde_json::json!({"items": [root_url]}),
        );
        outsider["id"] = Value::String(outsider_url.clone());
        outsider["attributedTo"] = Value::String(server.url("/users/bob"));

        for (path, body) in [
            ("/users/alice/statuses/100", root),
            ("/users/alice/statuses/101", middle),
            ("/users/alice/statuses/102", tail),
            ("/users/bob/statuses/99", outsider),
        ] {
            server
                .mock_async(move |when, then| {
                    when.method(GET).path(path);
                    then.status(200)
                        .header("content-type", "application/activity+json")
                        .json_body(body);
                })
                .await;
        }

        let cache_dir = tempfile::tempdir().unwrap();
        let cache = ArticleCache::new(cache_dir.path());
        let client = http::Client::new(&crate::config::FetchConfig {
            retries: 0,
            ..Default::default()
        })
        .unwrap();
        let page_url = Url::parse(&server.url("/@alice/101")).unwrap();
        let page = format!(
            r#"<link rel="alternate" type="application/activity+json" href="{middle_url}">"#
        );
        let article = expand(
            &page,
            &page_url,
            &source(&server.url("/feed")),
            &client,
            &cache,
        )
        .await
        .unwrap()
        .unwrap();
        let markdown = content::to_markdown(&article.html, Some(&page_url));

        let earlier = markdown.find("Earlier self post").unwrap();
        let middle = markdown.find("Linked middle post").unwrap();
        let later = markdown.find("Later self post").unwrap();
        assert!(earlier < middle && middle < later, "{markdown}");
        assert!(!markdown.contains("Other author's parent"), "{markdown}");
    }
}
