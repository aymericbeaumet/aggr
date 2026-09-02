//! RSS / Atom / JSON Feed via feed-rs, with conditional GET and a body-hash short circuit.

use anyhow::{Context as _, Result};
use feed_rs::model::{Entry, Feed, Link, Text};
use url::Url;

use super::{Context, Fetch, SourceMeta, Validators};
use crate::config::Source;
use crate::content;
use crate::http::{Request, Response};
use crate::model::{RawItem, sha1_hex};

pub async fn fetch(url: &Url, source: &Source, ctx: &Context<'_>) -> Result<Fetch> {
    let previous = Validators::from_state(ctx.state);
    let response = ctx
        .client
        .get(Request {
            url,
            headers: &source.headers,
            etag: previous.etag.as_deref(),
            last_modified: previous.last_modified.as_deref(),
        })
        .await?;
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
    };
    if validators.body_hash == previous.body_hash {
        return Ok(Fetch::Unchanged { validators });
    }

    let feed = parse(&body.bytes, &body.final_url)?;
    let (meta, items) = convert(&feed, &body.final_url);
    Ok(Fetch::Changed {
        validators,
        meta,
        items,
    })
}

/// RSS, Atom or JSON Feed bytes; relative links resolve against the URL the body came from.
pub fn parse(bytes: &[u8], base: &Url) -> Result<Feed> {
    feed_rs::parser::Builder::new()
        .base_uri(Some(base.as_str()))
        .build()
        .parse(bytes)
        .context("parsing feed")
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
    let link = pick_link(&entry.links)
        .map(|link| link.href.clone())
        .or_else(|| Url::parse(&entry.id).ok().map(|url| url.to_string()))?;
    let link = feed_url
        .join(&link)
        .map(|url| url.to_string())
        .unwrap_or(link);
    let title = entry
        .title
        .as_ref()
        .map(text_of)
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| untitled(&link));

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
        .filter(|html| !html.trim().is_empty());

    let summary = entry
        .summary
        .as_ref()
        .map(text_of)
        .filter(|text| !text.is_empty());

    let mut extra = std::collections::BTreeMap::new();
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
    fn escapes_plain_text_content() {
        assert_eq!(
            text_to_html("a < b\n\nc & d"),
            "<p>a &lt; b</p>\n<p>c &amp; d</p>"
        );
    }
}
