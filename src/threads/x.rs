//! Public X threads retrieved through Xcancel, with canonical X attribution and links.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use scraper::{ElementRef, Html, Selector, node::Node};
use url::Url;

use crate::{cache::ArticleCache, config::Source, content, content::ExtractedArticle, http};

const MAX_REQUESTS: usize = 32;
const MAX_POSTS: usize = 128;
const TIMEOUT: Duration = Duration::from_secs(30);

pub fn canonical_url(url: &Url) -> Option<Url> {
    status_url(url).map(|status| status.url)
}

pub async fn expand(
    url: &Url,
    source: &Source,
    client: &http::Client,
    cache: &ArticleCache,
) -> Result<Option<ExtractedArticle>> {
    let Some(status) = status_url(url) else {
        return Ok(None);
    };
    let mirror = Url::parse("https://xcancel.com/").context("parsing public X thread host")?;
    expand_from(&status, &mirror, source, client, cache)
        .await
        .map(Some)
}

#[derive(Clone, Debug)]
struct Status {
    user: String,
    id: u64,
    url: Url,
}

fn status_url(url: &Url) -> Option<Status> {
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || !matches!(
            url.host_str(),
            Some(
                "x.com"
                    | "www.x.com"
                    | "twitter.com"
                    | "www.twitter.com"
                    | "mobile.twitter.com"
                    | "xcancel.com"
                    | "www.xcancel.com"
            )
        )
    {
        return None;
    }
    status_path(url)
}

fn status_path(url: &Url) -> Option<Status> {
    let parts = url.path().trim_matches('/').split('/').collect::<Vec<_>>();
    let (user, id) = match parts.as_slice() {
        [user, "status", id] => (*user, *id),
        [user, "status", id, kind, number]
            if matches!(*kind, "photo" | "video") && number.parse::<u8>().is_ok() =>
        {
            (*user, *id)
        }
        _ => return None,
    };
    if user.is_empty()
        || user.len() > 15
        || !user.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        || !id.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let id = id.parse::<u64>().ok().filter(|id| *id > 0)?;
    Some(Status {
        user: user.to_string(),
        id,
        url: Url::parse(&format!("https://x.com/{user}/status/{id}")).ok()?,
    })
}

#[derive(Debug)]
struct Post {
    status: Status,
    html: String,
    image: Option<String>,
}

struct Page {
    posts: BTreeMap<u64, Post>,
    next: Vec<Url>,
    incomplete: bool,
}

async fn expand_from(
    status: &Status,
    mirror: &Url,
    source: &Source,
    client: &http::Client,
    cache: &ArticleCache,
) -> Result<ExtractedArticle> {
    let start = mirror.join(status.url.path())?;
    let mut pending = VecDeque::from([start]);
    let mut visited = HashSet::new();
    let mut posts = BTreeMap::new();
    let mut incomplete = false;
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    while let Some(url) = pending.pop_front() {
        if visited.contains(url.as_str()) {
            continue;
        }
        if visited.len() >= MAX_REQUESTS || posts.len() >= MAX_POSTS {
            incomplete = true;
            break;
        }
        visited.insert(url.to_string());
        let fetched = tokio::time::timeout_at(
            deadline,
            fetch_page(&url, &status.user, source, client, cache),
        )
        .await;
        let page = match fetched {
            Ok(Ok(page)) => page,
            Ok(Err(error)) if posts.is_empty() => return Err(error),
            Ok(Err(error)) => {
                incomplete = true;
                log::debug!("could not extend public X thread at {url}: {error:#}");
                continue;
            }
            Err(_) if posts.is_empty() => bail!("public X thread retrieval exceeded 30 seconds"),
            Err(_) => {
                incomplete = true;
                break;
            }
        };
        incomplete |= page.incomplete;
        for (id, post) in page.posts {
            if posts.len() >= MAX_POSTS {
                incomplete = true;
                break;
            }
            if let std::collections::btree_map::Entry::Vacant(entry) = posts.entry(id) {
                let next = mirror.join(post.status.url.path())?;
                if !visited.contains(next.as_str()) {
                    pending.push_back(next);
                }
                entry.insert(post);
            }
        }
        for next in page.next {
            if !visited.contains(next.as_str()) {
                pending.push_back(next);
            }
        }
    }
    if !posts.contains_key(&status.id) {
        bail!("public X thread did not contain the requested post");
    }
    // The article metadata already links the original; the body stays uninterrupted prose.
    if incomplete {
        log::warn!(
            "some public X replies could not be retrieved for {}",
            status.url
        );
    }
    Ok(render(posts.into_values().collect()))
}

async fn fetch_page(
    url: &Url,
    author: &str,
    source: &Source,
    client: &http::Client,
    cache: &ArticleCache,
) -> Result<Page> {
    let headers = http::source_headers(source, url);
    let cached = cache.load(url, headers)?;
    let response = client
        .get(http::Request {
            url,
            headers,
            etag: cached.as_ref().and_then(|entry| entry.etag.as_deref()),
            last_modified: cached
                .as_ref()
                .and_then(|entry| entry.last_modified.as_deref()),
        })
        .await;
    match response {
        Ok(http::Response::Ok(body)) => {
            if body.final_url.origin() != url.origin() || body.final_url.path() != url.path() {
                bail!("public X thread redirected away from the requested status");
            }
            let parsed = parse_page(&body.html_text(), url, author)?;
            cache.store(url, headers, &body)?;
            Ok(parsed)
        }
        Ok(http::Response::NotModified) => {
            let response = cached.context("public X thread returned 304 without a cached page")?;
            parse_page(&response.html_text(), url, author)
        }
        Err(error) => match cached {
            Some(response) => {
                log::debug!("using cached public X thread after request failed: {error:#}");
                parse_page(&response.html_text(), url, author)
            }
            None => Err(error),
        },
    }
}

fn selector(value: &str) -> Result<Selector> {
    Selector::parse(value).map_err(|error| anyhow::anyhow!("{error}"))
}

fn parse_page(html: &str, base: &Url, author: &str) -> Result<Page> {
    let requested = status_path(base).context("expected a public X status path")?;
    let document = Html::parse_document(html);
    let main_selector = selector(".main-tweet .timeline-item")?;
    let main = document
        .select(&main_selector)
        .next()
        .context("public X page exposes no post data")?;
    let main_post =
        parse_post(main, base, author)?.context("public X page belongs to another author")?;
    if main_post.status.id != requested.id {
        bail!("public X page does not match the requested status");
    }
    let mut posts = BTreeMap::from([(main_post.status.id, main_post)]);
    let mut incomplete = false;
    let main_items = document
        .select(&selector(".main-thread .timeline-item")?)
        .take(512)
        .collect::<Vec<_>>();
    if let Some(index) = main_items.iter().position(|item| item.id() == main.id()) {
        for direction in [
            main_items[..index].iter().rev().collect::<Vec<_>>(),
            main_items[index + 1..].iter().collect(),
        ] {
            for item in direction {
                if unavailable_author_post(*item, author)? {
                    incomplete = true;
                    continue;
                }
                let Some(post) = parse_post(*item, base, author)? else {
                    break;
                };
                posts.entry(post.status.id).or_insert(post);
            }
        }
    }
    for item in document
        .select(&selector(".replies .timeline-item")?)
        .take(512)
    {
        if unavailable_author_post(item, author)? {
            incomplete = true;
            continue;
        }
        if let Some(post) = parse_post(item, base, author)? {
            posts.entry(post.status.id).or_insert(post);
        }
    }
    let mut next = Vec::new();
    for link in document.select(&selector(".show-more a[href]")?).take(16) {
        let Some(href) = link.value().attr("href") else {
            continue;
        };
        let Ok(mut url) = base.join(href) else {
            continue;
        };
        url.set_fragment(None);
        if url.origin() != base.origin() || !url.username().is_empty() || url.password().is_some() {
            continue;
        }
        let Some(status) = status_path(&url) else {
            continue;
        };
        if status.user.eq_ignore_ascii_case(author)
            && ((status.id == requested.id
                && url
                    .query_pairs()
                    .any(|(name, value)| name == "cursor" && !value.is_empty()))
                || posts.contains_key(&status.id)
                || link
                    .ancestors()
                    .filter_map(ElementRef::wrap)
                    .any(|element| {
                        element.value().has_class(
                            "main-thread",
                            scraper::CaseSensitivity::AsciiCaseInsensitive,
                        )
                    }))
            && !next.contains(&url)
        {
            next.push(url);
        }
    }
    Ok(Page {
        posts,
        next,
        incomplete,
    })
}

fn unavailable_author_post(item: ElementRef<'_>, author: &str) -> Result<bool> {
    Ok(item
        .value()
        .attr("data-username")
        .is_some_and(|username| username.eq_ignore_ascii_case(author))
        && item.value().has_class(
            "unavailable",
            scraper::CaseSensitivity::AsciiCaseInsensitive,
        )
        && item.select(&selector(".unavailable-box")?).next().is_some())
}

fn parse_post(item: ElementRef<'_>, base: &Url, author: &str) -> Result<Option<Post>> {
    let Some(username) = item.value().attr("data-username") else {
        return Ok(None);
    };
    if !username.eq_ignore_ascii_case(author) {
        return Ok(None);
    }
    let status_selector = selector(".tweet-header .tweet-date a[href]")?;
    let Some(link) = item
        .select(&status_selector)
        .next()
        .and_then(|e| e.value().attr("href"))
    else {
        return Ok(None);
    };
    let Ok(url) = base.join(link) else {
        return Ok(None);
    };
    if url.origin() != base.origin() && status_url(&url).is_none() {
        return Ok(None);
    }
    let Some(status) = status_path(&url) else {
        return Ok(None);
    };
    if !status.user.eq_ignore_ascii_case(author) {
        return Ok(None);
    }
    let body_selector = selector(".tweet-body")?;
    let Some(body) = item.select(&body_selector).next() else {
        return Ok(None);
    };
    for replying_to in body
        .children()
        .filter_map(ElementRef::wrap)
        .filter(|element| {
            element.value().has_class(
                "replying-to",
                scraper::CaseSensitivity::AsciiCaseInsensitive,
            )
        })
    {
        for reply in replying_to.select(&selector("a[href]")?) {
            let target = reply
                .value()
                .attr("href")
                .and_then(|value| base.join(value).ok());
            if target.is_none_or(|url| !url.path().trim_matches('/').eq_ignore_ascii_case(author)) {
                return Ok(None);
            }
        }
    }
    let mut html = String::new();
    let mut image = None;
    for child in body.children().filter_map(ElementRef::wrap) {
        if child.value().has_class(
            "tweet-content",
            scraper::CaseSensitivity::AsciiCaseInsensitive,
        ) {
            html.push_str("<p>");
            html.push_str(&super::strip_thread_counter(&render_text(
                child,
                &status.url,
            )));
            html.push_str("</p>");
        } else if child.value().has_class(
            "attachments",
            scraper::CaseSensitivity::AsciiCaseInsensitive,
        ) {
            for media in child.select(&selector("img, video")?) {
                if media.value().name() == "video" {
                    // Playback cannot be retained; the poster keeps the media position and the
                    // metadata original link reaches the video.
                    if let Some(poster) = media
                        .value()
                        .attr("poster")
                        .and_then(|url| media_url(url, base))
                    {
                        image.get_or_insert_with(|| poster.to_string());
                        render_image(&mut html, &poster, "Video poster");
                    }
                    continue;
                }
                let original = media
                    .ancestors()
                    .filter_map(ElementRef::wrap)
                    .find(|e| {
                        e.value().name() == "a"
                            && e.value().has_class(
                                "still-image",
                                scraper::CaseSensitivity::AsciiCaseInsensitive,
                            )
                    })
                    .and_then(|e| e.value().attr("href"));
                let candidate = original.or_else(|| media.value().attr("src"));
                if let Some(url) = candidate.and_then(|url| media_url(url, base)) {
                    image.get_or_insert_with(|| url.to_string());
                    render_image(
                        &mut html,
                        &url,
                        media.value().attr("alt").unwrap_or_default(),
                    );
                }
            }
        }
    }
    if html.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(Post {
        status,
        html,
        image,
    }))
}

fn media_url(raw: &str, base: &Url) -> Option<Url> {
    let url = base.join(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    if url.origin() == base.origin() {
        let encoded = url
            .path()
            .strip_prefix("/pic/")?
            .strip_prefix("orig/")
            .unwrap_or(url.path().strip_prefix("/pic/")?);
        let decoded = url::form_urlencoded::parse(format!("url={encoded}").as_bytes())
            .next()?
            .1
            .into_owned();
        let decoded = if decoded.starts_with("http") {
            decoded
        } else {
            format!("https://{decoded}")
        };
        let original = Url::parse(&decoded).ok()?;
        return (matches!(original.scheme(), "http" | "https")
            && original.username().is_empty()
            && original.password().is_none()
            && matches!(
                original.host_str(),
                Some("pbs.twimg.com" | "video.twimg.com")
            ))
        .then_some(original);
    }
    Some(url)
}

fn render_image(html: &mut String, url: &Url, alt: &str) {
    html.push_str(&format!(
        "<figure><img src=\"{}\" alt=\"{}\"></figure>",
        content::escape_html(url.as_str()),
        content::escape_html(alt)
    ));
}

fn render_text(element: ElementRef<'_>, base: &Url) -> String {
    let mut html = String::new();
    for node in element.children() {
        match node.value() {
            Node::Text(text) => html.push_str(&content::escape_html(text).replace('\n', "<br>")),
            Node::Element(_) => {
                let Some(child) = ElementRef::wrap(node) else {
                    continue;
                };
                match child.value().name() {
                    "script" | "style" | "iframe" => {}
                    "br" => html.push_str("<br>"),
                    "a" => {
                        if let Some(mut url) = child
                            .value()
                            .attr("href")
                            .and_then(|href| base.join(href).ok())
                            && matches!(url.scheme(), "http" | "https")
                            && url.username().is_empty()
                            && url.password().is_none()
                        {
                            if matches!(
                                url.host_str(),
                                Some(
                                    "xcancel.com"
                                        | "www.xcancel.com"
                                        | "twitter.com"
                                        | "www.twitter.com"
                                        | "mobile.twitter.com"
                                )
                            ) {
                                let _ = url.set_host(Some("x.com"));
                                let _ = url.set_scheme("https");
                                url.set_fragment(None);
                            }
                            html.push_str(&format!(
                                "<a href=\"{}\">{}</a>",
                                content::escape_html(url.as_str()),
                                render_text(child, base)
                            ));
                        } else {
                            html.push_str(&render_text(child, base));
                        }
                    }
                    "b" | "strong" | "em" | "i" | "code" => {
                        let tag = child.value().name();
                        html.push_str(&format!("<{tag}>{}</{tag}>", render_text(child, base)));
                    }
                    _ => html.push_str(&render_text(child, base)),
                }
            }
            _ => {}
        }
    }
    html
}

fn render(mut posts: Vec<Post>) -> ExtractedArticle {
    posts.sort_by_key(|post| post.status.id);
    let image = posts.iter().find_map(|post| post.image.clone());
    let mut html = String::from("<section data-aggr-thread=\"x\">");
    for post in &posts {
        html.push_str("<article data-aggr-thread-post>");
        html.push_str(&post.html);
        html.push_str("</article>");
    }
    html.push_str("</section>");
    let (html, _) = content::storage_html(&html, 2 * 1024 * 1024);
    ExtractedArticle { html, image }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post(user: &str, id: u64, content: &str, media: &str) -> String {
        format!(
            r#"<div class="timeline-item" data-username="{user}"><div class="tweet-body"><div class="tweet-header"><a class="username" href="/{user}">@{user}</a><span class="tweet-date"><a href="/{user}/status/{id}#m">date</a></span></div><div class="tweet-content">{content}</div>{media}</div></div>"#
        )
    }

    fn page(main: &str, replies: &str) -> String {
        format!(
            "<div class='conversation'><div class='main-thread'><div class='main-tweet'>{main}</div></div><div class='replies'>{replies}</div></div>"
        )
    }

    #[test]
    fn extracts_only_author_posts_in_chronological_order_with_interleaved_original_media() {
        let main = post(
            "author",
            100,
            "Opening\n\nSecond paragraph",
            "<div class='attachments'><a class='still-image' href='https://pbs.twimg.com/media/one.jpg?name=orig'><img src='https://pbs.twimg.com/media/one.jpg?name=small'></a></div>",
        );
        let later = post("author", 102, "Last", "");
        let reply = post(
            "author",
            101,
            "Middle <a href='https://xcancel.com/other/status/80#m'>reference</a>",
            "<div class='attachments'><a class='still-image' href='https://pbs.twimg.com/media/two.jpg?name=orig'><img src='/pic/two'></a><a class='still-image' href='https://pbs.twimg.com/media/three.jpg?name=orig'><img src='/pic/three'></a></div>",
        );
        let other = post("outsider", 103, "Unrelated reply", "");
        let html = page(&main, &(later + &reply + &other + &reply));
        let parsed = parse_page(
            &html,
            &Url::parse("https://xcancel.com/author/status/100").unwrap(),
            "author",
        )
        .unwrap();
        let result = render(parsed.posts.into_values().collect());
        let order = [
            "Opening",
            "one.jpg?name=orig",
            "Middle",
            "two.jpg?name=orig",
            "three.jpg?name=orig",
            "Last",
        ]
        .map(|part| result.html.find(part).unwrap());
        assert!(order.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(!result.html.contains("Unrelated reply"));
        assert!(!result.html.contains("xcancel.com"));
        assert_eq!(result.html.matches("Middle").count(), 1);
        assert!(result.html.contains("https://x.com/other/status/80"));
        assert!(result.html.contains("Opening<br><br>Second paragraph"));
    }

    #[test]
    fn rejects_wrong_status_challenges_and_unrelated_hosts() {
        let url = Url::parse("https://xcancel.com/author/status/100").unwrap();
        assert!(parse_page("<title>Verification</title>", &url, "author").is_err());
        assert!(
            parse_page(
                &page(&post("author", 999, "Other thread", ""), ""),
                &url,
                "author"
            )
            .is_err()
        );
        assert!(
            status_url(&Url::parse("https://evil.example/author/status/100").unwrap()).is_none()
        );
        assert!(
            status_url(&Url::parse("https://x.com/author/status/100/photo/1").unwrap()).is_some()
        );
        for host in ["x.com", "twitter.com", "mobile.twitter.com", "xcancel.com"] {
            assert_eq!(
                canonical_url(
                    &Url::parse(&format!("https://{host}/author/status/100?ref=share#m")).unwrap()
                )
                .unwrap()
                .as_str(),
                "https://x.com/author/status/100"
            );
        }
    }

    #[test]
    fn canonicalizes_proxy_images_and_keeps_video_position() {
        let main = post(
            "author",
            100,
            "Opening (1/3)",
            "<div class='attachments'><img src='/pic/orig/pbs.twimg.com%2Fmedia%2Fone.jpg'><video poster='https://pbs.twimg.com/media/poster.jpg'></video><img src='https://pbs.twimg.com/media/last.jpg'></div>",
        );
        let parsed = parse_page(
            &page(&main, ""),
            &Url::parse("https://xcancel.com/author/status/100").unwrap(),
            "author",
        )
        .unwrap();
        let rendered = render(parsed.posts.into_values().collect());
        assert!(!rendered.html.contains("(1/3)"));
        let positions =
            ["one.jpg", "poster.jpg", "last.jpg"].map(|text| rendered.html.find(text).unwrap());
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(!rendered.html.contains("xcancel"));
        assert!(!rendered.html.contains("Watch video on X"));
        assert!(!rendered.html.contains("View post on X"));
        assert!(!rendered.html.contains("<hr"));
    }

    #[test]
    fn revalidates_decoded_proxy_media_urls() {
        let base = Url::parse("https://xcancel.com/author/status/100").unwrap();
        for original in [
            "https://user:secret@pbs.twimg.com/media/image.jpg",
            "https://user@pbs.twimg.com/media/image.jpg",
            "https://:secret@video.twimg.com/video.mp4",
            "httpx://pbs.twimg.com/media/image.jpg",
            "https://unrelated.example/media/image.jpg",
        ] {
            let encoded =
                url::form_urlencoded::byte_serialize(original.as_bytes()).collect::<String>();
            assert!(
                media_url(&format!("/pic/orig/{encoded}"), &base).is_none(),
                "{original}"
            );
        }
        for original in [
            "https://pbs.twimg.com/media/image.jpg",
            "http://pbs.twimg.com/media/image.jpg",
            "https://video.twimg.com/video.mp4",
        ] {
            let encoded =
                url::form_urlencoded::byte_serialize(original.as_bytes()).collect::<String>();
            assert_eq!(
                media_url(&format!("/pic/orig/{encoded}"), &base)
                    .unwrap()
                    .as_str(),
                original
            );
        }
    }

    #[test]
    fn quoted_reply_metadata_does_not_change_the_owning_posts_parent() {
        let root = post("author", 100, "Opening", "");
        let quoted = post("author", 101, "Continuation", "<div class='quote'><div class='replying-to'><a href='/outsider'>Someone else</a></div></div>")
            .replace("<div class=\"tweet-content\">", "<div class='replying-to'><a href='/author'>OP</a></div><div class=\"tweet-content\">");
        let unrelated = post("author", 102, "Reply to another person", "")
            .replace("<div class=\"tweet-content\">", "<div class='replying-to'><a href='/outsider'>Someone else</a></div><div class=\"tweet-content\">");
        let parsed = parse_page(
            &page(&root, &(quoted + &unrelated)),
            &Url::parse("https://xcancel.com/author/status/100").unwrap(),
            "author",
        )
        .unwrap();
        assert_eq!(parsed.posts.keys().copied().collect::<Vec<_>>(), [100, 101]);
    }

    #[test]
    fn skips_author_tombstones_and_keeps_later_visible_posts_until_another_author() {
        let root = post("author", 100, "Opening", "");
        let tombstone = "<div class='timeline-item unavailable' data-username='author'><div class='unavailable-box'>Post unavailable</div></div>";
        let visible = post("author", 102, "Still available", "");
        let outsider = post("outsider", 103, "Other author", "");
        let unrelated = post("author", 104, "Beyond other author", "");
        let html = format!(
            "<div class='main-thread'><div class='main-tweet'>{root}</div>{tombstone}{visible}{outsider}{unrelated}</div>"
        );
        let parsed = parse_page(
            &html,
            &Url::parse("https://xcancel.com/author/status/100").unwrap(),
            "author",
        )
        .unwrap();
        assert_eq!(parsed.posts.keys().copied().collect::<Vec<_>>(), [100, 102]);
        assert!(parsed.incomplete);
    }

    #[tokio::test]
    async fn traverses_author_replies_and_cursor_pages_without_forwarding_source_headers() {
        use httpmock::prelude::*;
        crate::http::install_crypto_provider();
        let server = MockServer::start();
        let opening = post("author", 100, "Opening (1/4)", "");
        let first = post("author", 101, "First reply (2/4)", "");
        let second = post("author", 102, "From pagination (3/4)", "");
        let last = post("author", 103, "Nested self reply (4/4)", "");
        let cursor = server.mock(|when, then| {
            when.method(GET)
                .path("/author/status/100")
                .query_param("cursor", "next")
                .header_missing("authorization");
            then.status(200).body(page(&opening, &second));
        });
        let root = server.mock(|when, then| {
            when.method(GET)
                .path("/author/status/100")
                .header_missing("authorization");
            then.status(200)
                .header("etag", "root-version")
                .body(format!(
                    "{}<div class='show-more'><a href='?cursor=next'>Load more</a></div>",
                    page(&opening, &first)
                ));
        });
        let child = server.mock(|when, then| {
            when.method(GET)
                .path("/author/status/101")
                .header_missing("authorization");
            then.status(200).body(page(&first, &last));
        });
        let paged = server.mock(|when, then| {
            when.method(GET).path("/author/status/102");
            then.status(200).body(page(&second, ""));
        });
        let nested = server.mock(|when, then| {
            when.method(GET).path("/author/status/103");
            then.status(200).body(page(&last, &opening));
        });
        let config = crate::config::Config::parse("[fetch]\nretries=0\n[[sources]]\nurl='https://x.com/author/status/100'\nheaders={Authorization='source-only'}").unwrap();
        let source = config.sources().unwrap().remove(0);
        let client = http::Client::new(&config.fetch).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let cache = ArticleCache::new(dir.path());
        let status = status_url(&Url::parse("https://x.com/author/status/100").unwrap()).unwrap();
        let article = expand_from(
            &status,
            &Url::parse(&server.base_url()).unwrap(),
            &source,
            &client,
            &cache,
        )
        .await
        .unwrap();
        assert_eq!(article.html.matches("data-aggr-thread-post").count(), 4);
        assert!(!article.html.contains("(1/4)"));
        assert!(!article.html.contains("could not be loaded"));
        let positions = [
            "Opening",
            "First reply",
            "From pagination",
            "Nested self reply",
        ]
        .map(|text| article.html.find(text).unwrap());
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(!article.html.contains("<hr"));
        assert!(!article.html.contains("View post on X"));
        let markdown = content::to_markdown(&article.html, None);
        assert!(
            markdown.contains("Opening\n\nFirst reply\n\nFrom pagination\n\nNested self reply"),
            "{markdown}"
        );
        assert!(!markdown.contains("* * *"), "{markdown}");
        root.assert_calls(1);
        child.assert_calls(1);
        cursor.assert_calls(1);
        paged.assert_calls(1);
        nested.assert_calls(1);
    }

    #[tokio::test]
    async fn discloses_unavailable_continuations_without_discarding_known_posts() {
        use httpmock::prelude::*;
        crate::http::install_crypto_provider();
        let server = MockServer::start();
        let mut root = server.mock(|when, then| {
            when.method(GET)
                .path("/author/status/100")
                .query_param_missing("cursor");
            then.status(200).body(format!(
                "{}<div class='show-more'><a href='?cursor=missing'>More</a></div>",
                page(&post("author", 100, "Opening", ""), "")
            ));
        });
        let mut unavailable = server.mock(|when, then| {
            when.method(GET)
                .path("/author/status/100")
                .query_param("cursor", "missing");
            then.status(404);
        });
        let config = crate::config::Config::parse(
            "[fetch]\nretries=0\n[[sources]]\nurl='https://x.com/author/status/100'",
        )
        .unwrap();
        let source = config.sources().unwrap().remove(0);
        let client = http::Client::new(&config.fetch).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let cache = ArticleCache::new(dir.path());
        let status = status_url(&Url::parse("https://x.com/author/status/100").unwrap()).unwrap();
        let article = expand_from(
            &status,
            &Url::parse(&server.base_url()).unwrap(),
            &source,
            &client,
            &cache,
        )
        .await
        .unwrap();
        assert!(article.html.contains("Opening"));
        assert!(!article.html.contains("could not be loaded"));
        assert!(!article.html.contains("View the full thread"));
        root.assert_calls(1);
        unavailable.assert_calls(1);
        root.delete();
        unavailable.delete();
        let tombstone = server.mock(|when, then| {
            when.method(GET).path("/author/status/100");
            then.status(200).body(format!(
                "<div class='main-thread'><div class='main-tweet'>{}</div><div class='timeline-item unavailable' data-username='author'><div class='unavailable-box'>Deleted</div></div></div>",
                post("author", 100, "Opening", "")
            ));
        });
        let article = expand_from(
            &status,
            &Url::parse(&server.base_url()).unwrap(),
            &source,
            &client,
            &cache,
        )
        .await
        .unwrap();
        assert!(article.html.contains("Opening"));
        assert!(!article.html.contains("could not be loaded"));
        assert!(!article.html.contains("View the full thread"));
        tombstone.assert_calls(1);
    }
}
