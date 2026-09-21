//! Preserve embedded public X posts as quotes within their publisher's article.

use std::collections::BTreeMap;
use std::time::Duration;

use comrak::nodes::NodeValue;
use scraper::{ElementRef, Html, Selector};
use url::Url;

use crate::{cache::ArticleCache, config::Source, content, http};

const MAX_EMBEDS: usize = 4;
const DEADLINE: Duration = Duration::from_secs(10);

struct Card {
    original: String,
    fallback: String,
    url: Url,
}

fn cards(html: &str) -> (String, Vec<Card>) {
    let document = Html::parse_fragment(html);
    let normalized = document.root_element().inner_html();
    let Ok(selector) = Selector::parse("blockquote.twitter-tweet, p") else {
        return (normalized, Vec::new());
    };
    let Ok(links) = Selector::parse("a[href]") else {
        return (normalized, Vec::new());
    };
    let mut cards = Vec::new();
    for element in document.select(&selector) {
        if element
            .ancestors()
            .filter_map(ElementRef::wrap)
            .any(|parent| matches!(parent.value().name(), "blockquote" | "pre" | "code"))
        {
            continue;
        }
        let is_embed = element.value().name() == "blockquote";
        let candidates = element
            .select(&links)
            .filter_map(|link| {
                let url = Url::parse(link.value().attr("href")?).ok()?;
                Some((link, super::canonical_x_url(&url)?))
            })
            .collect::<Vec<_>>();
        let Some((link, url)) = candidates.last() else {
            continue;
        };
        if !is_embed
            && (element.select(&links).count() != 1
                || element.text().collect::<String>().trim()
                    != link.text().collect::<String>().trim()
                || element
                    .children()
                    .filter_map(ElementRef::wrap)
                    .any(|child| child.id() != link.id()))
        {
            continue;
        }
        let fallback = if is_embed {
            element.inner_html()
        } else {
            format!("<p>{}</p>", link.inner_html())
        };
        cards.push(Card {
            original: element.html(),
            fallback,
            url: url.clone(),
        });
        if cards.len() == MAX_EMBEDS {
            break;
        }
    }
    (normalized, cards)
}

fn quote(body: &str, url: &Url) -> String {
    let author = url
        .path_segments()
        .and_then(|mut parts| parts.next())
        .unwrap_or("post");
    format!(
        "<blockquote>{body}<p><a href=\"{}\">@{} on X</a></p></blockquote>",
        content::escape_html(url.as_str()),
        content::escape_html(author)
    )
}

fn keep_remote(remote: &str, fallback: &str) -> bool {
    let fallback = content::html_to_text(fallback);
    let fallback = fallback
        .split_once("): ")
        .map_or(fallback.as_str(), |(_, quote)| quote);
    let remote = content::html_to_text(remote);
    !remote.trim().is_empty()
        && (fallback.chars().count() <= 64 || remote.chars().count() >= fallback.chars().count())
}

fn expanded_body(remote: Option<&str>, fallback: &str) -> String {
    let Some(remote) = remote else {
        return fallback.to_string();
    };
    if keep_remote(remote, fallback) {
        return remote.to_string();
    }
    let mut preserved = fallback.to_string();
    let document = Html::parse_fragment(remote);
    if let Ok(selector) = Selector::parse("figure") {
        for figure in document.select(&selector).take(16) {
            preserved.push_str(&figure.html());
        }
    }
    preserved
}

pub async fn expand_embedded_x(
    html: &str,
    source: &Source,
    client: &http::Client,
    cache: &ArticleCache,
) -> String {
    let (mut rendered, cards) = cards(html);
    if cards.is_empty() {
        return html.to_string();
    }
    let deadline = tokio::time::Instant::now() + DEADLINE;
    let mut fetched = BTreeMap::new();
    for card in cards {
        if !fetched.contains_key(card.url.as_str()) {
            let post = match tokio::time::timeout_at(
                deadline,
                super::x::embedded(&card.url, source, client, cache),
            )
            .await
            {
                Ok(Ok(post)) => post,
                Ok(Err(error)) => {
                    log::debug!("embedded X post unavailable at {}: {error:#}", card.url);
                    None
                }
                Err(_) => None,
            };
            fetched.insert(card.url.to_string(), post);
        }
        let body = expanded_body(
            fetched.get(card.url.as_str()).and_then(Option::as_deref),
            &card.fallback,
        );
        rendered = rendered.replacen(&card.original, &quote(&body, &card.url), 1);
    }
    rendered
}

/// Older captures already contain the quote text; formatting them must never make build requests.
pub fn format_archived_x_embeds(markdown: &str) -> String {
    if !markdown.contains("/status/") {
        return markdown.to_string();
    }
    let arena = comrak::Arena::new();
    let options = comrak::Options::default();
    let root = comrak::parse_document(&arena, markdown, &options);
    let mut offsets = vec![0];
    offsets.extend(markdown.match_indices('\n').map(|(index, _)| index + 1));
    let mut replacements = Vec::new();
    for paragraph in root.children() {
        if !matches!(paragraph.data.borrow().value, NodeValue::Paragraph) {
            continue;
        }
        let children = paragraph.children().collect::<Vec<_>>();
        if children.len() != 1 {
            continue;
        }
        let link = children[0];
        let url = match &link.data.borrow().value {
            NodeValue::Link(link) => Url::parse(&link.url)
                .ok()
                .and_then(|url| super::canonical_x_url(&url)),
            _ => None,
        };
        let Some(url) = url else { continue };
        if link
            .descendants()
            .any(|node| matches!(node.data.borrow().value, NodeValue::Image(_)))
        {
            continue;
        }
        let source = paragraph.data.borrow().sourcepos;
        let Some(start) = offsets.get(source.start.line - 1).copied() else {
            continue;
        };
        let end = offsets
            .get(source.end.line)
            .copied()
            .unwrap_or(markdown.len());
        for child in link.children().collect::<Vec<_>>() {
            link.insert_before(child);
        }
        link.detach();
        let mut text = String::new();
        if comrak::format_commonmark(paragraph, &options, &mut text).is_err() {
            continue;
        }
        let author = url
            .path_segments()
            .and_then(|mut parts| parts.next())
            .unwrap_or("post");
        let body = text
            .trim()
            .lines()
            .map(|line| format!("> {line}\n"))
            .collect::<String>();
        let replacement = format!("{body}>\n> [@{author} on X]({url})\n");
        replacements.push((start..end, replacement));
    }
    let mut result = markdown.to_string();
    for (range, replacement) in replacements.into_iter().rev() {
        result.replace_range(range, &replacement);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_x_cards_exclude_inline_references_and_nested_quotes() {
        let html = "<p>Prose <a href='https://x.com/one/status/100'>reference</a> stays.</p><p><a href='https://twitter.com/one/status/101'>A full standalone quoted post.</a></p><blockquote class='twitter-tweet'><p>Embedded text.</p><a href='https://x.com/two/status/102'>Date</a></blockquote><blockquote><p><a href='https://x.com/three/status/103'>An existing quote</a></p></blockquote>";
        let (_, found) = cards(html);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].url.as_str(), "https://x.com/one/status/101");
        assert!(found[0].fallback.contains("full standalone"));
        assert!(found[1].fallback.contains("Embedded text"));
        let bounded = (0..10)
            .map(|index| {
                format!(
                    "<p><a href='https://x.com/a/status/{}'>post</a></p>",
                    index + 1
                )
            })
            .collect::<String>();
        assert_eq!(cards(&bounded).1.len(), MAX_EMBEDS);
    }

    #[test]
    fn archived_x_cards_become_portable_quotes_without_altering_prose() {
        let input = "Before.\n\n[Malte Ubl (@cramforce): A **complete** quoted post.](https://x.com/cramforce/status/2096609086649647324?ref=share)\n\nAfter [ordinary reference](https://x.com/other/status/100).\n";
        let (formatted, _) = content::normalize_article_body(input, "", None, "publisher");
        assert!(
            formatted.starts_with("Before.\n\n> Malte Ubl"),
            "{formatted}"
        );
        assert!(formatted.contains("**complete**"));
        assert!(
            formatted.contains(
                "> [@cramforce on X](https://x.com/cramforce/status/2096609086649647324)"
            )
        );
        assert!(
            formatted.ends_with("After [ordinary reference](https://x.com/other/status/100).\n")
        );
        assert_eq!(format_archived_x_embeds(&formatted), formatted);
        assert!(content::render_markdown(&formatted).contains("<blockquote>"));
    }

    #[test]
    fn embedded_x_truncated_remote_never_replaces_a_complete_source_quote() {
        let quote = "<p>Malte (@cramforce): This is a complete source quotation whose full text should never be lost just because the public mirror provides an abbreviated version.</p>";
        assert!(!keep_remote(
            "<p>This is a complete source quotation…</p>",
            quote
        ));
        let preserved = expanded_body(
            Some(
                "<p>A short fragment…</p><figure><img src='https://pbs.twimg.com/media/post.jpg'></figure>",
            ),
            quote,
        );
        assert!(preserved.contains("full text should never be lost"));
        assert!(preserved.contains("https://pbs.twimg.com/media/post.jpg"));
        assert!(!preserved.contains("A short fragment"));
        assert!(!keep_remote("", "View post"));
        assert!(keep_remote("<p>Full public post content.</p>", "View post"));
    }
}
