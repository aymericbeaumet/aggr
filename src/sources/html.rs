//! Automatic discovery from HTML: advertised RSS/Atom/JSON feeds first, then conservative
//! article-card and JSON-LD extraction for sites that publish no feed at all.

use std::collections::BTreeSet;

use anyhow::{Result, bail};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use scraper::{ElementRef, Html, Selector};
use serde_json::Value;
use url::Url;

use super::SourceMeta;
use crate::model::{RawItem, normalize_link};

/// Feed endpoints advertised in metadata or recognizable body links, in document order. The
/// candidates are still fetched and parsed before aggr trusts them.
pub fn feed_links(page: &str, page_url: &Url) -> Vec<Url> {
    let document = Html::parse_document(page);
    let Ok(links) = Selector::parse("link[href]") else {
        return Vec::new();
    };
    let base = Selector::parse("base[href]")
        .ok()
        .and_then(|selector| document.select(&selector).next())
        .and_then(|element| page_url.join(element.value().attr("href")?).ok())
        .unwrap_or_else(|| page_url.clone());
    let mut seen = BTreeSet::new();
    let mut feeds = Vec::new();
    for link in document.select(&links) {
        let advertised = {
            let rel = link.value().attr("rel").unwrap_or_default();
            let media = link
                .value()
                .attr("type")
                .unwrap_or_default()
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            rel.split_ascii_whitespace().any(|part| {
                part.eq_ignore_ascii_case("alternate") || part.eq_ignore_ascii_case("feed")
            }) && matches!(
                media.as_str(),
                "application/rss+xml"
                    | "application/atom+xml"
                    | "application/feed+json"
                    | "application/json"
            )
        };
        if advertised
            && let Some(url) = link
                .value()
                .attr("href")
                .and_then(|href| base.join(href).ok())
            && matches!(url.scheme(), "http" | "https")
            && seen.insert(url.as_str().to_string())
        {
            feeds.push(url);
        }
    }

    let Ok(anchors) = Selector::parse("a[href]") else {
        return feeds;
    };
    for anchor in document.select(&anchors) {
        if let Some(url) = anchor
            .value()
            .attr("href")
            .and_then(|href| base.join(href).ok())
            .filter(feedish)
            && seen.insert(url.as_str().to_string())
        {
            feeds.push(url);
        }
    }
    feeds
}

fn feedish(url: &Url) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let last = url
        .path_segments()
        .and_then(Iterator::last)
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        last.as_str(),
        "feed"
            | "rss"
            | "atom"
            | "feed.xml"
            | "rss.xml"
            | "atom.xml"
            | "index.xml"
            | "feed.atom"
            | "feed.json"
    ) || last.ends_with(".rss")
        || last.ends_with(".atom")
        || url.query_pairs().any(|(key, value)| {
            matches!(key.as_ref(), "format" | "output")
                && matches!(value.to_ascii_lowercase().as_str(), "rss" | "atom" | "feed")
        })
}

/// Extract likely article entries without per-site selectors. False positives are avoided by
/// requiring a heading-associated link, or a dated link with a title-like child (or structured
/// Article JSON-LD), and a distinct HTTP URL.
pub fn extract(page: &str, page_url: &Url) -> Result<(SourceMeta, Vec<RawItem>)> {
    let document = Html::parse_document(page);
    let meta = SourceMeta {
        title: select_text(&document, "title"),
        site_url: Some(page_url.origin().ascii_serialization() + "/"),
    };
    let mut items = json_ld_items(&document, page_url);
    items.extend(card_items(&document, page_url)?);

    let mut seen = BTreeSet::new();
    items.retain(|item| seen.insert(normalize_link(&item.link)));
    if items.is_empty() {
        bail!("no advertised feed or article entries found at {page_url}");
    }
    Ok((meta, items))
}

fn card_items(document: &Html, page_url: &Url) -> Result<Vec<RawItem>> {
    let anchors = selector("a[href]")?;
    let headings = selector("h1,h2,h3,h4,h5,h6")?;
    let classed = selector("[class]")?;
    let times = selector("time")?;
    let paragraphs = selector("p")?;
    let mut out = Vec::new();
    for anchor in document.select(&anchors) {
        let nested_heading = anchor.select(&headings).next();
        let parent_heading = anchor.parent().and_then(ElementRef::wrap).filter(|parent| {
            matches!(
                parent.value().name(),
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
            )
        });
        let titled_child = anchor.select(&classed).find(|element| {
            element
                .value()
                .attr("class")
                .is_some_and(|classes| classes.to_ascii_lowercase().contains("title"))
        });
        let title_element = nested_heading
            .or(parent_heading)
            .or_else(|| anchor.select(&times).next().and(titled_child));
        let Some(title_element) = title_element else {
            continue;
        };
        let title = text(&title_element);
        if title.len() < 3 {
            continue;
        }
        let Some(link) = article_url(page_url, anchor.value().attr("href").unwrap_or_default())
        else {
            continue;
        };
        if normalize_link(link.as_str()) == normalize_link(page_url.as_str()) {
            continue;
        }
        let block = anchor
            .ancestors()
            .skip(1)
            .filter_map(ElementRef::wrap)
            .find(|element| matches!(element.value().name(), "article" | "li"))
            .unwrap_or(anchor);
        let published = block
            .select(&times)
            .find_map(|time| {
                time.value()
                    .attr("datetime")
                    .and_then(|date| parse_date(date, None))
                    .or_else(|| parse_date(&text(&time), None))
            })
            .or_else(|| find_date(&text(&block)));
        let summary = block
            .select(&paragraphs)
            .map(|paragraph| text(&paragraph))
            .find(|summary| !summary.is_empty());
        out.push(RawItem {
            id: Some(link.to_string()),
            title,
            link: link.to_string(),
            published,
            updated: None,
            first_seen: None,
            authors: vec![],
            labels: vec![],
            summary,
            content_html: None,
            extra: Default::default(),
        });
    }
    Ok(out)
}

fn json_ld_items(document: &Html, page_url: &Url) -> Vec<RawItem> {
    let Ok(scripts) = Selector::parse("script[type='application/ld+json']") else {
        return Vec::new();
    };
    let mut values = Vec::new();
    for script in document.select(&scripts) {
        let body = script.text().collect::<String>();
        if let Ok(value) = serde_json::from_str::<Value>(&body) {
            collect_articles(&value, page_url, &mut values);
        }
    }
    values
}

fn collect_articles(value: &Value, page_url: &Url, out: &mut Vec<RawItem>) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_articles(value, page_url, out);
            }
        }
        Value::Object(object) => {
            if article_type(object.get("@type")) {
                let title = string(object.get("headline"))
                    .or_else(|| string(object.get("name")))
                    .unwrap_or_default();
                let href = string(object.get("url"))
                    .or_else(|| entity_url(object.get("mainEntityOfPage")));
                if !title.is_empty()
                    && let Some(link) = href.and_then(|href| article_url(page_url, href))
                {
                    let published =
                        string(object.get("datePublished")).and_then(|date| parse_date(date, None));
                    let updated =
                        string(object.get("dateModified")).and_then(|date| parse_date(date, None));
                    let summary = string(object.get("description")).map(str::to_string);
                    out.push(RawItem {
                        id: Some(link.to_string()),
                        title: title.to_string(),
                        link: link.to_string(),
                        published,
                        updated,
                        first_seen: None,
                        authors: vec![],
                        labels: vec![],
                        summary,
                        content_html: None,
                        extra: Default::default(),
                    });
                }
            }
            for nested in object.values() {
                if nested.is_array() || nested.is_object() {
                    collect_articles(nested, page_url, out);
                }
            }
        }
        _ => {}
    }
}

fn article_type(value: Option<&Value>) -> bool {
    match value {
        Some(Value::String(kind)) => {
            matches!(kind.as_str(), "Article" | "BlogPosting" | "NewsArticle")
        }
        Some(Value::Array(kinds)) => kinds.iter().any(|kind| article_type(Some(kind))),
        _ => false,
    }
}

fn entity_url(value: Option<&Value>) -> Option<&str> {
    match value? {
        Value::String(value) => Some(value),
        Value::Object(value) => string(value.get("@id")).or_else(|| string(value.get("url"))),
        _ => None,
    }
}

fn string(value: Option<&Value>) -> Option<&str> {
    value?
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn article_url(base: &Url, href: &str) -> Option<Url> {
    let url = base.join(href.trim()).ok()?;
    matches!(url.scheme(), "http" | "https").then_some(url)
}

fn select_text(document: &Html, css: &str) -> Option<String> {
    document
        .select(&Selector::parse(css).ok()?)
        .next()
        .map(|element| text(&element))
        .filter(|value| !value.is_empty())
}

fn selector(css: &str) -> Result<Selector> {
    Selector::parse(css).map_err(|err| anyhow::anyhow!("invalid selector {css:?}: {err}"))
}

fn text(element: &ElementRef<'_>) -> String {
    element
        .text()
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ")
}

fn find_date(text: &str) -> Option<DateTime<Utc>> {
    let words: Vec<&str> = text.split_whitespace().collect();
    for start in 0..words.len() {
        for length in 1..=4.min(words.len() - start) {
            let candidate = words[start..start + length].join(" ");
            if let Some(date) = parse_date(candidate.trim_matches(['(', ')', '·', '|']), None) {
                return Some(date);
            }
        }
    }
    None
}

pub fn parse_date(raw: &str, format: Option<&str>) -> Option<DateTime<Utc>> {
    let raw = raw.trim();
    let formats: Vec<&str> = match format {
        Some(format) => vec![format],
        None => {
            if let Ok(date) = DateTime::parse_from_rfc3339(raw) {
                return Some(date.with_timezone(&Utc));
            }
            if let Ok(date) = DateTime::parse_from_rfc2822(raw) {
                return Some(date.with_timezone(&Utc));
            }
            vec![
                "%Y-%m-%dT%H:%M:%S%.f",
                "%Y-%m-%d %H:%M:%S",
                "%Y-%m-%d %H:%M",
                "%Y-%m-%d",
                "%Y/%m/%d",
                "%m.%d.%y",
                "%B %d, %Y",
                "%b %d, %Y",
                "%d %B %Y",
                "%d %b %Y",
                "%B %e, %Y",
                "%b %e, %Y",
            ]
        }
    };
    formats.into_iter().find_map(|format| {
        DateTime::parse_from_str(raw, format)
            .map(|date| date.with_timezone(&Utc))
            .ok()
            .or_else(|| {
                NaiveDateTime::parse_from_str(raw, format)
                    .ok()
                    .map(|date| date.and_utc())
            })
            .or_else(|| {
                NaiveDate::parse_from_str(raw, format)
                    .ok()
                    .and_then(|date| date.and_hms_opt(0, 0, 0))
                    .map(|date| date.and_utc())
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<!doctype html><html><head><title>News</title>
<link rel="alternate" type="application/atom+xml" href="/atom.xml"></head><body>
<ul><li><a href="/news/one"><h2>First article</h2><span>08.23.26</span><p>Summary</p></a></li></ul>
<article><h3><a href="/news/two">Second article</a></h3><time datetime="2026-08-22T12:00:00Z">today</time></article>
<ul><li><a href="/news/three"><time>Aug 21, 2026</time><span class="card-title">Third article</span></a></li></ul>
</body></html>"#;

    #[test]
    fn discovers_advertised_feeds() {
        let base = Url::parse("https://example.com/news/").unwrap();
        assert_eq!(
            feed_links(PAGE, &base),
            [Url::parse("https://example.com/atom.xml").unwrap()]
        );
    }

    #[test]
    fn discovers_body_feed_links_and_honours_html_base() {
        let page = r#"<base href="https://cdn.example/blog/"><a href="feed.atom">Atom</a><a href="sitemap.xml">Map</a>"#;
        let base = Url::parse("https://example.com/news/").unwrap();
        assert_eq!(
            feed_links(page, &base),
            [Url::parse("https://cdn.example/blog/feed.atom").unwrap()]
        );
    }

    #[test]
    fn extracts_heading_cards_without_configuration() {
        let base = Url::parse("https://example.com/news/").unwrap();
        let (meta, items) = extract(PAGE, &base).unwrap();
        assert_eq!(meta.title.as_deref(), Some("News"));
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].link, "https://example.com/news/one");
        assert_eq!(items[0].summary.as_deref(), Some("Summary"));
        assert_eq!(
            items[0].published.unwrap().format("%Y-%m-%d").to_string(),
            "2026-08-23"
        );
        assert_eq!(items[1].title, "Second article");
        assert_eq!(items[2].title, "Third article");
    }

    #[test]
    fn extracts_article_json_ld() {
        let page = r#"<script type="application/ld+json">{"@type":"NewsArticle","headline":"Structured","url":"/story","datePublished":"2026-09-01"}</script>"#;
        let (_, items) = extract(page, &Url::parse("https://example.com/").unwrap()).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Structured");
        assert_eq!(items[0].link, "https://example.com/story");
    }

    #[test]
    fn empty_pages_are_not_silently_accepted() {
        let err = extract(
            "<title>Nothing</title>",
            &Url::parse("https://example.com/").unwrap(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("no advertised feed or article"));
    }
}
