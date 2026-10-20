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

/// The paragraph's own source, so inline markup survives being requoted.
fn paragraph_source<'a, 'b>(
    markdown: &'a str,
    offsets: &[usize],
    node: &'b comrak::nodes::AstNode<'b>,
) -> &'a str {
    let source = node.data.borrow().sourcepos;
    let start = offsets.get(source.start.line - 1).copied().unwrap_or(0);
    let end = offsets
        .get(source.end.line)
        .copied()
        .unwrap_or(markdown.len());
    markdown[start..end].trim()
}

/// What a paragraph says. A picture's alt text describes the picture rather than being part of
/// the prose, so the walk stops at one instead of descending into it.
fn plain<'a>(node: &'a comrak::nodes::AstNode<'a>) -> String {
    let mut text = String::new();
    let mut stack: Vec<_> = node.children().collect();
    stack.reverse();
    while let Some(child) = stack.pop() {
        match &child.data.borrow().value {
            NodeValue::Image(_) => continue,
            NodeValue::Text(value) => text.push_str(value.as_ref()),
            NodeValue::Code(code) => text.push_str(&code.literal),
            NodeValue::SoftBreak | NodeValue::LineBreak => text.push(' '),
            _ => {}
        }
        let mut nested: Vec<_> = child.children().collect();
        nested.reverse();
        stack.extend(nested);
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A paragraph holding one picture and nothing else: the poster's avatar, which the quote does
/// not need and which reads as a stray image dropped into the prose.
fn avatar_only<'a>(node: &'a comrak::nodes::AstNode<'a>) -> bool {
    if !plain(node).is_empty() {
        return false;
    }
    let mut images = 0;
    let mut stack: Vec<_> = node.children().collect();
    while let Some(child) = stack.pop() {
        match &child.data.borrow().value {
            NodeValue::Image(_) => images += 1,
            NodeValue::Link(_) => stack.extend(child.children()),
            NodeValue::Text(value) if value.trim().is_empty() => {}
            NodeValue::SoftBreak | NodeValue::LineBreak => {}
            _ => return false,
        }
    }
    images == 1
}

/// `12:54 PM · Jul 22, 2026 · 505K Views` and `177 Replies · 683 Reposts · 4.88K Likes`: the
/// provider's own chrome, true only while it was captured and not part of what was said.
fn engagement_line(text: &str) -> bool {
    let lower = text.to_lowercase();
    if !lower.contains('·') {
        return false;
    }
    let counters = ["replies", "reposts", "likes", "views", "quotes"];
    counters.iter().filter(|word| lower.contains(*word)).count() >= 2
        || (lower.contains("views") && lower.contains(':'))
}

/// A publisher that flattens an embedded post writes it as loose paragraphs: the poster's avatar
/// linking to the post, their name, what they said, then any post they were quoting, and the
/// provider's counters. Read back that way it is prose with stray avatars in it, so the run is
/// gathered into the quote it was, nested quote and all, and the counters are left behind.
fn flattened_embed<'a>(
    markdown: &str,
    offsets: &[usize],
    blocks: &[&'a comrak::nodes::AstNode<'a>],
    index: usize,
) -> Option<(usize, String)> {
    const MAX_PARAGRAPHS: usize = 10;
    let opener = blocks[index];
    if !matches!(opener.data.borrow().value, NodeValue::Paragraph) || !avatar_only(opener) {
        return None;
    }
    let url = opener
        .descendants()
        .find_map(|node| match &node.data.borrow().value {
            NodeValue::Link(link) => Url::parse(&link.url)
                .ok()
                .and_then(|url| super::canonical_x_url(&url)),
            _ => None,
        })?;

    let mut said: Vec<String> = Vec::new();
    let mut quoted: Vec<String> = Vec::new();
    let mut nested = false;
    let mut counted = false;
    let mut last = index;
    for (offset, block) in blocks
        .iter()
        .enumerate()
        .skip(index + 1)
        .take(MAX_PARAGRAPHS)
    {
        if !matches!(block.data.borrow().value, NodeValue::Paragraph) {
            break;
        }
        let text = plain(block);
        if avatar_only(block) {
            // The second avatar opens the post this one was quoting.
            if nested {
                break;
            }
            nested = true;
            last = offset;
            continue;
        }
        if text.is_empty() {
            break;
        }
        if engagement_line(&text) {
            // The counters close the embed; the publisher's own prose resumes after them.
            last = offset;
            counted = true;
            continue;
        }
        if counted {
            break;
        }
        let source = paragraph_source(markdown, offsets, block).to_string();
        if nested {
            quoted.push(source)
        } else {
            said.push(source)
        }
        last = offset;
    }
    if said.is_empty() {
        return None;
    }

    let author = url
        .path_segments()
        .and_then(|mut parts| parts.next())
        .unwrap_or("post");
    let blocks: Vec<(usize, String)> = said
        .into_iter()
        .map(|text| (1, text))
        .chain(quoted.into_iter().map(|text| (2, text)))
        .chain(std::iter::once((1, format!("[@{author} on X]({url})"))))
        .collect();
    let mut quote = String::new();
    let mut previous = None;
    for (depth, text) in blocks {
        // A blank line carries the shallower of the two blocks it separates, which is what both
        // closes a paragraph and opens or closes the quote nested inside it.
        if let Some(before) = previous {
            quote.push_str("> ".repeat(usize::min(before, depth)).trim_end());
            quote.push('\n');
        }
        for line in text.lines() {
            quote.push_str(&"> ".repeat(depth));
            quote.push_str(line);
            quote.push('\n');
        }
        previous = Some(depth);
    }
    Some((last, quote))
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
    let blocks = root.children().collect::<Vec<_>>();
    let mut index = 0;
    while index < blocks.len() {
        let Some((last, quote)) = flattened_embed(markdown, &offsets, &blocks, index) else {
            index += 1;
            continue;
        };
        let start = offsets
            .get(blocks[index].data.borrow().sourcepos.start.line - 1)
            .copied()
            .unwrap_or(0);
        let end = offsets
            .get(blocks[last].data.borrow().sourcepos.end.line)
            .copied()
            .unwrap_or(markdown.len());
        replacements.push((start..end, quote));
        index = last + 1;
    }
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
    replacements.sort_by_key(|(range, _)| range.start);
    for (range, replacement) in replacements.into_iter().rev() {
        result.replace_range(range, &replacement);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flattened_embed_is_read_back_as_the_quote_it_was() {
        // substack.com: an embedded post arrives as the avatar linking to it, the poster, what
        // they said, the post they were quoting, and the provider's counters.
        let markdown = concat!(
            "Lead paragraph.\n\n",
            "[![X avatar for @ClementDelangue](https://cdn.example/a.png)]",
            "(https://x.com/ClementDelangue/status/2079913058554585089)\n\n",
            "clem 🤗 @ClementDelangue\n\n",
            "So proud of our security team! Grateful to @Zai\\_org for the open weights.\n\n",
            "![X avatar for @XciD_](https://cdn.example/b.jpg)\n\n",
            "Adrien Carreira @XciD\\_\n\n",
            "Hardest IR of my career: one narrow objective, endless parallel paths.\n\n",
            "12:54 PM · Jul 22, 2026 · 505K Views\n\n",
            "177 Replies · 683 Reposts · 4.88K Likes\n\n",
            "Trailing prose.\n",
        );
        assert_eq!(
            format_archived_x_embeds(markdown),
            concat!(
                "Lead paragraph.\n\n",
                "> clem 🤗 @ClementDelangue\n",
                ">\n",
                "> So proud of our security team! Grateful to @Zai\\_org for the open weights.\n",
                ">\n",
                "> > Adrien Carreira @XciD\\_\n",
                "> >\n",
                "> > Hardest IR of my career: one narrow objective, endless parallel paths.\n",
                ">\n",
                "> [@ClementDelangue on X](https://x.com/ClementDelangue/status/2079913058554585089)\n",
                "\nTrailing prose.\n",
            )
        );
    }

    #[test]
    fn only_a_picture_that_links_to_a_post_opens_one() {
        // An illustration, a picture linking somewhere else, and an avatar with nothing after it
        // are all just pictures.
        for markdown in [
            "![An illustration](https://cdn.example/a.png)\n\nSome prose.\n",
            "[![Logo](https://cdn.example/a.png)](https://example.com/about)\n\nSome prose.\n",
            concat!(
                "[![X avatar](https://cdn.example/a.png)]",
                "(https://x.com/someone/status/12345)\n\n",
                "## A heading, not a post\n",
            ),
        ] {
            assert_eq!(format_archived_x_embeds(markdown), markdown, "{markdown}");
        }
    }

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
