//! Generic scraper for sites without a feed: one CSS selector picks the item elements, a few
//! more pick fields inside each. A field is `css`, `css@attr` or `@attr` (the item element's own
//! attribute); plain `css` reads the element's text.

use anyhow::{Result, bail};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use scraper::{ElementRef, Html, Selector};
use url::Url;

use super::{Context, Fetch, SourceMeta, Validators};
use crate::config::{HtmlFields, Source};
use crate::http::{Request, Response};
use crate::model::{RawItem, sha1_hex};

pub async fn fetch(
    url: &Url,
    fields: &HtmlFields,
    source: &Source,
    ctx: &Context<'_>,
) -> Result<Fetch> {
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

    let (meta, items) = extract(
        &String::from_utf8_lossy(&body.bytes),
        &body.final_url,
        fields,
    )?;
    Ok(Fetch::Changed {
        validators,
        meta,
        items,
    })
}

/// One field spec, parsed. `css` empty means "the item element itself".
#[derive(Debug, Clone)]
pub struct Field {
    css: Option<Selector>,
    attr: Option<String>,
}

impl Field {
    /// `h2 a@href`, `@href`, `time@datetime`, `p.summary`.
    pub fn parse(spec: &str) -> Result<Self> {
        let (css, attr) = match spec.rsplit_once('@') {
            Some((css, attr)) => (css.trim(), Some(attr.trim())),
            None => (spec.trim(), None),
        };
        if let Some(attr) = attr
            && attr.is_empty()
        {
            bail!("{spec:?}: empty attribute name after `@`");
        }
        if css.is_empty() && attr.is_none() {
            bail!("empty selector");
        }
        let css = (!css.is_empty()).then(|| selector(css)).transpose()?;
        Ok(Self {
            css,
            attr: attr.map(str::to_string),
        })
    }

    fn read(&self, item: &ElementRef<'_>) -> Option<String> {
        let element = match &self.css {
            Some(css) => item.select(css).next()?,
            None => *item,
        };
        let value = match &self.attr {
            Some(attr) => element.attr(attr)?.to_string(),
            None => text(&element),
        };
        (!value.is_empty()).then_some(value)
    }
}

pub fn selector(css: &str) -> Result<Selector> {
    Selector::parse(css).map_err(|err| anyhow::anyhow!("invalid selector {css:?}: {err}"))
}

/// Pure mapping from a listing page to raw items. The page is parsed here and dropped before
/// returning: scraper's tree is not `Send`, so it must never live across an `await`.
pub fn extract(
    page: &str,
    page_url: &Url,
    fields: &HtmlFields,
) -> Result<(SourceMeta, Vec<RawItem>)> {
    let items = selector(&fields.items)?;
    let field = |spec: &Option<String>| spec.as_deref().map(Field::parse).transpose();
    let (title, link, date, summary) = (
        field(&fields.title)?,
        field(&fields.link)?,
        field(&fields.date)?,
        field(&fields.summary)?,
    );
    let (self_href, any_href, time_attr, time_text) = (
        Field::parse("@href")?,
        Field::parse("a@href")?,
        Field::parse("time@datetime")?,
        Field::parse("time")?,
    );

    let document = Html::parse_document(page);
    let meta = SourceMeta {
        title: document
            .select(&selector("title")?)
            .next()
            .map(|element| text(&element))
            .filter(|title| !title.is_empty()),
        site_url: Some(page_url.origin().ascii_serialization() + "/"),
    };

    let elements: Vec<ElementRef<'_>> = document.select(&items).collect();
    if elements.is_empty() {
        bail!("`items` selector {:?} matched nothing", fields.items);
    }
    let mut out = Vec::with_capacity(elements.len());
    for element in &elements {
        let Some(href) = link
            .as_ref()
            .and_then(|field| field.read(element))
            .or_else(|| self_href.read(element))
            .or_else(|| any_href.read(element))
        else {
            continue;
        };
        let Ok(link) = page_url.join(href.trim()) else {
            continue;
        };
        if !matches!(link.scheme(), "http" | "https") {
            continue;
        }
        let link = link.to_string();
        let title = title
            .as_ref()
            .and_then(|field| field.read(element))
            .unwrap_or_else(|| text(element))
            .trim()
            .to_string();
        let title = if title.is_empty() {
            super::feed::untitled(&link)
        } else {
            title
        };
        let published = date
            .as_ref()
            .and_then(|field| field.read(element))
            .or_else(|| time_attr.read(element))
            .or_else(|| time_text.read(element))
            .and_then(|raw| parse_date(&raw, fields.date_format.as_deref()));
        let summary = summary.as_ref().and_then(|field| field.read(element));
        out.push(RawItem {
            id: None,
            title,
            link,
            published,
            updated: None,
            authors: vec![],
            labels: vec![],
            summary,
            content_html: None,
            extra: Default::default(),
        });
    }
    Ok((meta, out))
}

/// Text of an element with whitespace collapsed, the way a browser would show it.
fn text(element: &ElementRef<'_>) -> String {
    element
        .text()
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ")
}

/// With `format`, chrono's syntax decides; without it, only unambiguous shapes are accepted
/// (`06.23.25` could be June 23 or 6 Nov, so it needs `date_format = "%m.%d.%y"`).
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

    const PAGE: &str = r#"<!doctype html><html><head><title>Blog | Example</title></head><body>
<ul>
  <li><a class="card" href="/blog/newest"><h2>Newest &amp; best</h2><span>06.23.25</span><p>A summary</p></a></li>
  <li><a class="card" href="https://other.example/x"><h2>Absolute</h2><span>05.22.25</span></a></li>
  <li><a class="card" href="javascript:void(0)"><h2>Skipped</h2></a></li>
  <li><a class="card" href="/blog/untitled"><span>garbage</span></a></li>
</ul>
<article><h3><a href="/posts/1">Article</a></h3><time datetime="2026-09-01T10:00:00Z">Sep 1</time></article>
</body></html>"#;

    fn fields(items: &str) -> HtmlFields {
        HtmlFields {
            items: items.into(),
            title: None,
            link: None,
            date: None,
            date_format: None,
            summary: None,
        }
    }

    #[test]
    fn extracts_with_explicit_fields() {
        let url = Url::parse("https://example.com/blog").unwrap();
        let fields = HtmlFields {
            title: Some("h2".into()),
            link: Some("@href".into()),
            date: Some("span".into()),
            date_format: Some("%m.%d.%y".into()),
            summary: Some("p".into()),
            ..fields("li > a.card")
        };
        let (meta, items) = extract(PAGE, &url, &fields).unwrap();
        assert_eq!(meta.title.as_deref(), Some("Blog | Example"));
        assert_eq!(meta.site_url.as_deref(), Some("https://example.com/"));
        assert_eq!(items.len(), 3, "javascript: link dropped");

        assert_eq!(items[0].title, "Newest & best");
        assert_eq!(items[0].link, "https://example.com/blog/newest");
        assert_eq!(
            items[0].published.unwrap().to_rfc3339(),
            "2025-06-23T00:00:00+00:00"
        );
        assert_eq!(items[0].summary.as_deref(), Some("A summary"));
        assert_eq!(items[1].link, "https://other.example/x");
        assert_eq!(items[1].summary, None);
        assert_eq!(items[2].title, "garbage", "no h2: the element's text");
        assert_eq!(items[2].published, None, "garbage date is ignored");
    }

    #[test]
    fn defaults_find_links_and_time_elements() {
        let url = Url::parse("https://example.com/").unwrap();
        let (_, items) = extract(PAGE, &url, &fields("article")).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Article Sep 1", "whole element text");
        assert_eq!(items[0].link, "https://example.com/posts/1");
        assert_eq!(
            items[0].published.unwrap().to_rfc3339(),
            "2026-09-01T10:00:00+00:00"
        );
    }

    #[test]
    fn rejects_bad_selectors_and_empty_matches() {
        let url = Url::parse("https://example.com/").unwrap();
        let err = extract(PAGE, &url, &fields("li >")).unwrap_err();
        assert!(format!("{err:#}").contains("invalid selector"), "{err:#}");
        let err = extract(PAGE, &url, &fields("section.none")).unwrap_err();
        assert!(format!("{err:#}").contains("matched nothing"), "{err:#}");
        assert!(Field::parse("h2@").is_err());
        assert!(Field::parse("").is_err());
        assert!(Field::parse("@href").is_ok());
    }

    #[test]
    fn parses_common_dates_without_a_format() {
        let day = |raw: &str| parse_date(raw, None).map(|d| d.to_rfc3339());
        assert_eq!(
            day("2026-09-01T10:00:00+02:00").as_deref(),
            Some("2026-09-01T08:00:00+00:00")
        );
        assert_eq!(
            day("Tue, 01 Sep 2026 10:00:00 GMT").as_deref(),
            Some("2026-09-01T10:00:00+00:00")
        );
        assert_eq!(
            day("2026-09-01").as_deref(),
            Some("2026-09-01T00:00:00+00:00")
        );
        assert_eq!(
            day("September 1, 2026").as_deref(),
            Some("2026-09-01T00:00:00+00:00")
        );
        assert_eq!(
            day("1 Sep 2026").as_deref(),
            Some("2026-09-01T00:00:00+00:00")
        );
        assert_eq!(day("06.23.25"), None, "ambiguous without a format");
        assert_eq!(
            parse_date("06.23.25", Some("%m.%d.%y")).map(|d| d.to_rfc3339()),
            Some("2025-06-23T00:00:00+00:00".into())
        );
    }
}
