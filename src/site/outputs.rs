//! Non-template outputs: syndicated feeds, discovery documents, sitemaps and redirect stubs.

use std::collections::BTreeSet;
use std::ops::Range;

use anyhow::{Result, bail};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Map, Value};

use super::context::{BuildCtx, ItemCtx, SiteCtx};

const XML_DECLARATION: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n";
const SITEMAP_NAMESPACE: &str = "http://www.sitemaps.org/schemas/sitemap/0.9";

/// A ~200 byte page that sends old URLs to the item's GitHub permalink.
#[cfg(test)]
pub fn redirect_stub(target: &str) -> String {
    let target = escape(target);
    format!(
        "<!doctype html><meta charset=\"utf-8\"><meta http-equiv=\"refresh\" content=\"0;url={target}\">\
         <link rel=\"canonical\" href=\"{target}\"><title>Moved</title><a href=\"{target}\">Moved</a>\n"
    )
}

/// Atom feed of the river's first page, so the site itself can be followed.
pub fn atom_feed(site: &SiteCtx, build: &BuildCtx, items: &[ItemCtx]) -> String {
    atom_collection(site, build, &site.title, "", items)
}

/// Serialize an Atom 1.0 collection.
///
/// A path-derived URN is the stable entry identity and the generated aggr page is its alternate
/// representation. The upstream article is provenance (`rel=via`), so consumers can distinguish
/// the durable local archive from the content it was derived from.
pub fn atom_collection(
    site: &SiteCtx,
    build: &BuildCtx,
    title: &str,
    path: &str,
    items: &[ItemCtx],
) -> String {
    let path = collection_path(path);
    let home = site_url(site, &path);
    let self_url = site_url(site, &format!("{path}atom.xml"));
    let updated = collection_updated(items, build.time);
    let mut out = String::with_capacity(4096);

    out.push_str(XML_DECLARATION);
    out.push_str(&format!(
        "<feed xmlns=\"http://www.w3.org/2005/Atom\" xml:lang=\"{}\">\n",
        escape(&site.language)
    ));
    element(&mut out, 2, "title", title);
    if !site.description.is_empty() {
        element(&mut out, 2, "subtitle", &site.description);
    }
    element(&mut out, 2, "id", &feed_id(site, &path));
    out.push_str(&format!(
        "  <link rel=\"alternate\" type=\"text/html\" href=\"{}\"/>\n",
        escape(&home)
    ));
    out.push_str(&format!(
        "  <link rel=\"self\" type=\"application/atom+xml\" href=\"{}\"/>\n",
        escape(&self_url)
    ));
    element(&mut out, 2, "updated", &updated.to_rfc3339());
    out.push_str(&format!(
        "  <generator version=\"{}\" uri=\"https://github.com/aymericbeaumet/aggr\">aggr</generator>\n",
        escape(&build.version)
    ));

    for item in items {
        let local = site_url(site, &item.url);
        let item_updated = item.updated.unwrap_or(item.date);
        out.push_str("  <entry>\n");
        element(&mut out, 4, "id", &item_uid(item));
        out.push_str(&format!(
            "    <title type=\"text\">{}</title>\n",
            escape(&item.title)
        ));
        out.push_str(&format!(
            "    <link rel=\"alternate\" type=\"text/html\" href=\"{}\"/>\n",
            escape(&local)
        ));
        if !item.link.is_empty() && item.link != local {
            out.push_str(&format!(
                "    <link rel=\"via\" type=\"text/html\" href=\"{}\"/>\n",
                escape(&item.link)
            ));
        }
        if let Some(published) = item.published {
            element(&mut out, 4, "published", &published.to_rfc3339());
        }
        element(&mut out, 4, "updated", &item_updated.to_rfc3339());
        let authors: Vec<_> = if item.authors.is_empty() {
            vec![item.source_name.as_str()]
        } else {
            item.authors.iter().map(String::as_str).collect()
        };
        for author in authors {
            out.push_str("    <author>\n");
            element(&mut out, 6, "name", author);
            out.push_str("    </author>\n");
        }
        for category in item_categories(item) {
            out.push_str(&format!("    <category term=\"{}\"/>\n", escape(&category)));
        }
        if !item.source_name.is_empty() {
            out.push_str("    <source>\n");
            element(&mut out, 6, "title", &item.source_name);
            out.push_str("    </source>\n");
        }
        if !item.excerpt.is_empty() {
            out.push_str(&format!(
                "    <summary type=\"text\">{}</summary>\n",
                escape(&item.excerpt)
            ));
        }
        if let Some(content) = &item.body_html {
            out.push_str(&format!(
                "    <content type=\"html\">{}</content>\n",
                escape(content)
            ));
        }
        out.push_str("  </entry>\n");
    }
    out.push_str("</feed>\n");
    out
}

/// Serialize an RSS 2.0 collection with the Atom self-link and RSS content module extensions.
pub fn rss_collection(
    site: &SiteCtx,
    build: &BuildCtx,
    title: &str,
    path: &str,
    items: &[ItemCtx],
) -> String {
    let path = collection_path(path);
    let home = site_url(site, &path);
    let self_url = site_url(site, &format!("{path}rss.xml"));
    let updated = collection_updated(items, build.time);
    let mut out = String::with_capacity(4096);

    out.push_str(XML_DECLARATION);
    out.push_str(
        "<rss version=\"2.0\" xmlns:atom=\"http://www.w3.org/2005/Atom\" \
         xmlns:content=\"http://purl.org/rss/1.0/modules/content/\">\n<channel>\n",
    );
    element(&mut out, 2, "title", title);
    element(&mut out, 2, "link", &home);
    element(&mut out, 2, "description", &site.description);
    element(&mut out, 2, "language", &site.language);
    out.push_str(&format!(
        "  <atom:link href=\"{}\" rel=\"self\" type=\"application/rss+xml\"/>\n",
        escape(&self_url)
    ));
    element(&mut out, 2, "lastBuildDate", &updated.to_rfc2822());
    element(&mut out, 2, "generator", "aggr");

    for item in items {
        let local = site_url(site, &item.url);
        out.push_str("  <item>\n");
        element(&mut out, 4, "title", &item.title);
        element(&mut out, 4, "link", &local);
        out.push_str(&format!(
            "    <guid isPermaLink=\"false\">{}</guid>\n",
            escape(&item_uid(item))
        ));
        if !item.link.is_empty() && item.link != local {
            out.push_str(&format!(
                "    <atom:link href=\"{}\" rel=\"via\" type=\"text/html\"/>\n",
                escape(&item.link)
            ));
        }
        element(&mut out, 4, "pubDate", &item.date.to_rfc2822());
        if !item.excerpt.is_empty() {
            element(&mut out, 4, "description", &item.excerpt);
        }
        if let Some(content) = &item.body_html {
            element(&mut out, 4, "content:encoded", content);
        }
        for category in item_categories(item) {
            element(&mut out, 4, "category", &category);
        }
        out.push_str("  </item>\n");
    }
    out.push_str("</channel>\n</rss>\n");
    out
}

/// Serialize a JSON Feed 1.1 collection.
pub fn json_collection(
    site: &SiteCtx,
    title: &str,
    path: &str,
    items: &[ItemCtx],
) -> Result<String> {
    let path = collection_path(path);
    let home = site_url(site, &path);
    let self_url = site_url(site, &format!("{path}feed.json"));
    let entries: Vec<_> = items
        .iter()
        .map(|item| {
            let local = site_url(site, &item.url);
            let mut entry = Map::new();
            entry.insert("id".into(), Value::String(item_uid(item)));
            entry.insert("url".into(), Value::String(local));
            if !item.link.is_empty() {
                entry.insert("external_url".into(), Value::String(item.link.clone()));
            }
            entry.insert("title".into(), Value::String(item.title.clone()));
            if !item.excerpt.is_empty() {
                entry.insert("summary".into(), Value::String(item.excerpt.clone()));
            }
            if let Some(content) = &item.body_html {
                entry.insert("content_html".into(), Value::String(content.clone()));
            } else {
                entry.insert("content_text".into(), Value::String(item.excerpt.clone()));
            }
            entry.insert(
                "date_published".into(),
                Value::String(item.date.to_rfc3339()),
            );
            if let Some(date) = item.updated {
                entry.insert("date_modified".into(), Value::String(date.to_rfc3339()));
            }
            if !item.authors.is_empty() {
                entry.insert(
                    "authors".into(),
                    Value::Array(
                        item.authors
                            .iter()
                            .map(|name| serde_json::json!({"name": name}))
                            .collect(),
                    ),
                );
            }
            let categories = item_categories(item);
            if !categories.is_empty() {
                entry.insert(
                    "tags".into(),
                    Value::Array(categories.into_iter().map(Value::String).collect()),
                );
            }
            Value::Object(entry)
        })
        .collect();

    let mut feed = Map::new();
    feed.insert(
        "version".into(),
        Value::String("https://jsonfeed.org/version/1.1".into()),
    );
    feed.insert("title".into(), Value::String(title.into()));
    feed.insert("home_page_url".into(), Value::String(home));
    feed.insert("feed_url".into(), Value::String(self_url));
    if !site.description.is_empty() {
        feed.insert(
            "description".into(),
            Value::String(site.description.clone()),
        );
    }
    feed.insert("language".into(), Value::String(site.language.clone()));
    feed.insert("items".into(), Value::Array(entries));
    Ok(serde_json::to_string_pretty(&Value::Object(feed))?)
}

/// OpenSearch 1.1 metadata for a static search page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenSearchDescription<'a> {
    /// Human-readable name. OpenSearch limits this to 16 Unicode characters.
    pub short_name: &'a str,
    /// Human-readable description. OpenSearch limits this to 1,024 Unicode characters.
    pub description: &'a str,
    /// Results URL template containing `{searchTerms}`.
    pub search_url: &'a str,
    /// Absolute URL of this OpenSearch description, when one is known.
    pub self_url: Option<&'a str>,
}

/// Serialize an OpenSearch 1.1 description document.
pub fn opensearch_description(description: &OpenSearchDescription<'_>) -> String {
    let short_name = truncate_chars(description.short_name, 16);
    let summary = truncate_chars(description.description, 1024);
    let mut out = String::with_capacity(512);
    out.push_str(XML_DECLARATION);
    out.push_str("<OpenSearchDescription xmlns=\"http://a9.com/-/spec/opensearch/1.1/\">\n");
    element(&mut out, 2, "ShortName", short_name);
    element(&mut out, 2, "Description", summary);
    out.push_str(&format!(
        "  <Url type=\"text/html\" rel=\"results\" template=\"{}\"/>\n",
        escape(description.search_url)
    ));
    if let Some(self_url) = description.self_url {
        out.push_str(&format!(
            "  <Url type=\"application/opensearchdescription+xml\" rel=\"self\" template=\"{}\"/>\n",
            escape(self_url)
        ));
    }
    element(&mut out, 2, "InputEncoding", "UTF-8");
    element(&mut out, 2, "OutputEncoding", "UTF-8");
    out.push_str("</OpenSearchDescription>\n");
    out
}

/// One absolute URL supplied by the site builder for sitemap serialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SitemapUrl {
    pub loc: String,
    pub lastmod: Option<DateTime<Utc>>,
}

impl SitemapUrl {
    pub fn new(loc: impl Into<String>, lastmod: Option<DateTime<Utc>>) -> Self {
        Self {
            loc: loc.into(),
            lastmod,
        }
    }
}

/// Uncompressed protocol limits. Smaller values make boundary behavior easy to unit test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SitemapLimits {
    pub max_urls: usize,
    pub max_bytes: usize,
}

impl Default for SitemapLimits {
    fn default() -> Self {
        Self {
            max_urls: 50_000,
            max_bytes: 50 * 1024 * 1024,
        }
    }
}

/// A child urlset referenced by a sitemap index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SitemapChunk {
    /// File name relative to the root `sitemap.xml`.
    pub name: String,
    /// Absolute public URL used by the index.
    pub loc: String,
    /// Most recent URL modification in this chunk.
    pub lastmod: Option<DateTime<Utc>>,
    pub xml: String,
}

/// A single urlset, or a root sitemap index plus its child urlsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SitemapOutput {
    UrlSet {
        xml: String,
    },
    Index {
        xml: String,
        chunks: Vec<SitemapChunk>,
    },
}

impl SitemapOutput {
    pub fn root_xml(&self) -> &str {
        match self {
            Self::UrlSet { xml } | Self::Index { xml, .. } => xml,
        }
    }

    pub fn chunks(&self) -> &[SitemapChunk] {
        match self {
            Self::UrlSet { .. } => &[],
            Self::Index { chunks, .. } => chunks,
        }
    }
}

/// Serialize sitemap output, splitting urlsets by both count and uncompressed byte size.
///
/// `sitemap_url` and every [`SitemapUrl::loc`] must be absolute HTTP(S) URLs. If splitting is
/// necessary, `sitemap_url` becomes an index and sibling files are named `sitemap-1.xml`, etc.
pub fn sitemap(
    urls: &[SitemapUrl],
    sitemap_url: &str,
    limits: SitemapLimits,
) -> Result<SitemapOutput> {
    validate_limits(limits)?;
    let sitemap_url = absolute_http_url(sitemap_url, "sitemap URL")?;
    if sitemap_url.query().is_some() || sitemap_url.fragment().is_some() {
        bail!("sitemap URL must not contain a query or fragment");
    }
    let fragments = urls
        .iter()
        .map(sitemap_url_fragment)
        .collect::<Result<Vec<_>>>()?;
    let ranges = sitemap_ranges(&fragments, limits)?;

    if ranges.len() == 1 {
        return Ok(SitemapOutput::UrlSet {
            xml: sitemap_urlset(&fragments, ranges[0].clone()),
        });
    }
    if ranges.len() > limits.max_urls {
        bail!(
            "sitemap index needs {} entries, above the configured limit of {}",
            ranges.len(),
            limits.max_urls
        );
    }

    let mut chunks = Vec::with_capacity(ranges.len());
    for (index, range) in ranges.into_iter().enumerate() {
        let name = sitemap_chunk_name(sitemap_url.path(), index + 1)?;
        let loc = sitemap_url
            .join(&name)
            .map_err(|error| anyhow::anyhow!("joining sitemap chunk URL: {error}"))?
            .to_string();
        let lastmod = urls[range.clone()]
            .iter()
            .filter_map(|url| url.lastmod)
            .max();
        chunks.push(SitemapChunk {
            name,
            loc,
            lastmod,
            xml: sitemap_urlset(&fragments, range),
        });
    }
    let xml = sitemap_index(&chunks);
    if xml.len() > limits.max_bytes {
        bail!(
            "sitemap index is {} bytes, above the configured limit of {}",
            xml.len(),
            limits.max_bytes
        );
    }
    Ok(SitemapOutput::Index { xml, chunks })
}

pub fn item_json(site: &SiteCtx, item: &ItemCtx, markdown: &str) -> Result<String> {
    let local = site_url(site, &item.url);
    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "@context": "https://schema.org",
        "@type": "BlogPosting",
        "@id": item_uid(item),
        "id": item_uid(item),
        "url": local,
        "isBasedOn": item.link,
        "headline": item.title,
        "external_url": item.link,
        "title": item.title,
        "source": item.source,
        "source_name": item.source_name,
        "category": item.category,
        "tags": item.labels,
        "date_created": item.date.to_rfc3339(),
        "date_published": item.published.map(|date| date.to_rfc3339()),
        "date_modified": item.updated.map(|date| date.to_rfc3339()),
        "authors": item.authors,
        "summary": item.summary,
        "description": item.excerpt,
        "articleBody": markdown,
        "content_html": item.body_html,
        "content_markdown": markdown,
    }))?)
}

pub fn text_item(item: &ItemCtx) -> String {
    let body = item
        .body_html
        .as_deref()
        .map(crate::content::html_to_text)
        .unwrap_or_default();
    format!("{}\n\n{}\n", item.title, body.trim())
}

pub fn rst_item(item: &ItemCtx) -> String {
    let title = &item.title;
    let underline = "=".repeat(title.chars().count().max(1));
    format!(
        "{title}\n{underline}\n\n{}",
        text_item(item)
            .split_once("\n\n")
            .map_or("", |(_, body)| body)
    )
}

fn collection_path(path: &str) -> String {
    let path = path.trim_matches('/');
    if path.is_empty() {
        String::new()
    } else {
        format!("{path}/")
    }
}

fn site_url(site: &SiteCtx, path: &str) -> String {
    let root = site.base_url.as_deref().unwrap_or(&site.base_path);
    join_url(root, path)
}

fn feed_id(site: &SiteCtx, path: &str) -> String {
    site.base_url.as_ref().map_or_else(
        || {
            format!(
                "urn:aggr:feed:{}",
                crate::model::sha1_hex(format!("{}:{path}", site.title))
            )
        },
        |_| site_url(site, path),
    )
}

fn item_uid(item: &ItemCtx) -> String {
    format!("urn:aggr:item:{}", crate::model::sha1_hex(&item.path))
}

fn join_url(root: &str, path: &str) -> String {
    let path = path.trim_start_matches('/');
    if let Ok(mut root) = url::Url::parse(root) {
        if !root.path().ends_with('/') {
            let normalized = format!("{}/", root.path());
            root.set_path(&normalized);
        }
        if let Ok(joined) = root.join(path) {
            return joined.to_string();
        }
    }
    let root = format!("{}/", root.trim_end_matches('/'));
    format!("{root}{path}")
}

fn collection_updated(items: &[ItemCtx], fallback: DateTime<Utc>) -> DateTime<Utc> {
    items
        .iter()
        .map(|item| item.updated.unwrap_or(item.date))
        .max()
        .unwrap_or(fallback)
}

fn item_categories(item: &ItemCtx) -> Vec<String> {
    let mut seen = BTreeSet::new();
    item.category
        .iter()
        .chain(&item.labels)
        .filter_map(|category| {
            let category = category.trim();
            (!category.is_empty() && seen.insert(category.to_lowercase()))
                .then(|| category.to_string())
        })
        .collect()
}

fn element(out: &mut String, indent: usize, name: &str, value: &str) {
    out.push_str(&" ".repeat(indent));
    out.push('<');
    out.push_str(name);
    out.push('>');
    out.push_str(&escape(value));
    out.push_str("</");
    out.push_str(name);
    out.push_str(">\n");
}

fn truncate_chars(text: &str, max: usize) -> &str {
    text.char_indices()
        .nth(max)
        .map_or(text, |(index, _)| &text[..index])
}

fn validate_limits(limits: SitemapLimits) -> Result<()> {
    if limits.max_urls == 0 {
        bail!("sitemap max_urls must be greater than zero");
    }
    let empty = sitemap_urlset(&[], 0..0).len();
    if limits.max_bytes < empty {
        bail!("sitemap max_bytes must be at least {empty} bytes for an empty document");
    }
    Ok(())
}

fn absolute_http_url(value: &str, field: &str) -> Result<url::Url> {
    let url = url::Url::parse(value)
        .map_err(|error| anyhow::anyhow!("{field} must be an absolute URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        bail!("{field} must be an absolute HTTP(S) URL");
    }
    Ok(url)
}

fn sitemap_url_fragment(url: &SitemapUrl) -> Result<String> {
    absolute_http_url(&url.loc, "sitemap entry")?;
    let mut out = String::from("  <url>\n");
    element(&mut out, 4, "loc", &url.loc);
    if let Some(lastmod) = url.lastmod {
        element(
            &mut out,
            4,
            "lastmod",
            &lastmod.to_rfc3339_opts(SecondsFormat::Secs, true),
        );
    }
    out.push_str("  </url>\n");
    Ok(out)
}

fn sitemap_ranges(fragments: &[String], limits: SitemapLimits) -> Result<Vec<Range<usize>>> {
    let overhead = sitemap_urlset(&[], 0..0).len();
    let mut ranges = Vec::new();
    let mut start = 0;
    let mut bytes = overhead;

    for (index, fragment) in fragments.iter().enumerate() {
        if overhead + fragment.len() > limits.max_bytes {
            bail!(
                "sitemap entry {} needs {} bytes, above the configured limit of {}",
                index + 1,
                overhead + fragment.len(),
                limits.max_bytes
            );
        }
        let count = index - start;
        if count == limits.max_urls || bytes + fragment.len() > limits.max_bytes {
            ranges.push(start..index);
            start = index;
            bytes = overhead;
        }
        bytes += fragment.len();
    }
    ranges.push(start..fragments.len());
    Ok(ranges)
}

fn sitemap_urlset(fragments: &[String], range: Range<usize>) -> String {
    let mut out = String::with_capacity(
        XML_DECLARATION.len()
            + 64
            + fragments[range.clone()]
                .iter()
                .map(String::len)
                .sum::<usize>(),
    );
    out.push_str(XML_DECLARATION);
    out.push_str(&format!("<urlset xmlns=\"{SITEMAP_NAMESPACE}\">\n"));
    for fragment in &fragments[range] {
        out.push_str(fragment);
    }
    out.push_str("</urlset>\n");
    out
}

fn sitemap_chunk_name(path: &str, index: usize) -> Result<String> {
    let file = path.rsplit('/').next().unwrap_or_default();
    if file.is_empty() {
        bail!("sitemap URL must end in a file name");
    }
    let stem = file.strip_suffix(".xml").unwrap_or(file);
    Ok(format!("{stem}-{index}.xml"))
}

fn sitemap_index(chunks: &[SitemapChunk]) -> String {
    let mut out = String::with_capacity(256 + chunks.len() * 128);
    out.push_str(XML_DECLARATION);
    out.push_str(&format!("<sitemapindex xmlns=\"{SITEMAP_NAMESPACE}\">\n"));
    for chunk in chunks {
        out.push_str("  <sitemap>\n");
        element(&mut out, 4, "loc", &chunk.loc);
        if let Some(lastmod) = chunk.lastmod {
            element(
                &mut out,
                4,
                "lastmod",
                &lastmod.to_rfc3339_opts(SecondsFormat::Secs, true),
            );
        }
        out.push_str("  </sitemap>\n");
    }
    out.push_str("</sitemapindex>\n");
    out
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '\t'
            | '\n'
            | '\r'
            | '\u{20}'..='\u{d7ff}'
            | '\u{e000}'..='\u{fffd}'
            | '\u{10000}'..='\u{10ffff}' => out.push(c),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::TimeZone;

    use super::*;
    use crate::model::ContentKind;
    use crate::site::context::DiscussionLinkCtx;

    fn at(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 2, hour, 30, 0)
            .single()
            .unwrap()
    }

    fn site() -> SiteCtx {
        SiteCtx {
            title: "Example & Reader".into(),
            description: "Collected <carefully>".into(),
            language: "en-GB".into(),
            base_path: "/reads/".into(),
            base_url: Some("https://example.test/reads/".into()),
            repository: None,
            data_branch: "aggr".into(),
            pwa: true,
            config_url: None,
            discussions: Vec::new(),
            params: toml::Table::new(),
        }
    }

    fn build() -> BuildCtx {
        BuildCtx {
            time: at(12),
            version: "1.2.3".into(),
            config_sha: None,
            data_sha: None,
            release: true,
        }
    }

    fn item() -> ItemCtx {
        ItemCtx {
            path: "items/source/a-story".into(),
            url: "items/source/a-story/".into(),
            title: "A <safe> & useful story".into(),
            link: "https://upstream.test/story?a=1&b=2".into(),
            domain: "upstream.test".into(),
            source: "source".into(),
            source_name: "Upstream".into(),
            category: Some("Engineering".into()),
            date: at(8),
            age_band: "day",
            published: Some(at(8)),
            updated: Some(at(9)),
            first_seen: at(10),
            authors: vec!["A & B".into()],
            labels: vec!["Rust".into(), "engineering".into()],
            discussions: vec![DiscussionLinkCtx {
                name: "hackernews".into(),
                url: "https://news.ycombinator.com/".into(),
                found: false,
                score: None,
            }],
            summary: Some("Summary".into()),
            excerpt: "A concise <summary>".into(),
            content: ContentKind::Feed,
            extra: BTreeMap::new(),
            permalink: None,
            raw_url: None,
            history_url: None,
            edit_url: None,
            body_html: Some("<p>Clean &amp; <strong>complete</strong></p>".into()),
        }
    }

    #[test]
    fn atom_uses_local_identity_and_upstream_provenance() {
        let atom = atom_collection(
            &site(),
            &build(),
            "Engineering",
            "/categories/engineering",
            &[item()],
        );

        assert!(atom.contains("<feed xmlns=\"http://www.w3.org/2005/Atom\" xml:lang=\"en-GB\">"));
        assert!(atom.contains(
            "<link rel=\"self\" type=\"application/atom+xml\" href=\"https://example.test/reads/categories/engineering/atom.xml\"/>"
        ));
        assert!(atom.contains(
            "<link rel=\"alternate\" type=\"text/html\" href=\"https://example.test/reads/items/source/a-story/\"/>"
        ));
        assert!(atom.contains(
            "<link rel=\"via\" type=\"text/html\" href=\"https://upstream.test/story?a=1&amp;b=2\"/>"
        ));
        assert!(atom.contains("<id>urn:aggr:item:"));
        assert!(atom.contains("<category term=\"Engineering\"/>"));
        assert!(atom.contains("<category term=\"Rust\"/>"));
        assert_eq!(atom.matches("category term=\"engineering\"").count(), 0);
        assert!(atom.contains(
            "<content type=\"html\">&lt;p&gt;Clean &amp;amp; &lt;strong&gt;complete&lt;/strong&gt;&lt;/p&gt;</content>"
        ));
        assert!(atom.contains("<updated>2026-09-02T09:30:00+00:00</updated>"));
    }

    #[test]
    fn rss_has_extensions_local_identity_and_escaped_full_content() {
        let rss = rss_collection(&site(), &build(), "Example", "", &[item()]);

        assert!(rss.contains("xmlns:atom=\"http://www.w3.org/2005/Atom\""));
        assert!(rss.contains("xmlns:content=\"http://purl.org/rss/1.0/modules/content/\""));
        assert!(rss.contains(
            "<atom:link href=\"https://example.test/reads/rss.xml\" rel=\"self\" type=\"application/rss+xml\"/>"
        ));
        assert!(rss.contains("<link>https://example.test/reads/items/source/a-story/</link>"));
        assert!(rss.contains("<guid isPermaLink=\"false\">urn:aggr:item:"));
        assert!(rss.contains(
            "<atom:link href=\"https://upstream.test/story?a=1&amp;b=2\" rel=\"via\" type=\"text/html\"/>"
        ));
        assert!(rss.contains(
            "<content:encoded>&lt;p&gt;Clean &amp;amp; &lt;strong&gt;complete&lt;/strong&gt;&lt;/p&gt;</content:encoded>"
        ));
        assert!(rss.contains("<category>Engineering</category>"));
        assert!(rss.contains("<category>Rust</category>"));
    }

    #[test]
    fn json_feed_has_local_urls_external_url_and_full_content() {
        let json =
            json_collection(&site(), "Engineering", "categories/engineering/", &[item()]).unwrap();
        let feed: Value = serde_json::from_str(&json).unwrap();
        let entry = &feed["items"][0];

        assert_eq!(feed["version"], "https://jsonfeed.org/version/1.1");
        assert_eq!(feed["language"], "en-GB");
        assert_eq!(
            feed["home_page_url"],
            "https://example.test/reads/categories/engineering/"
        );
        assert_eq!(
            feed["feed_url"],
            "https://example.test/reads/categories/engineering/feed.json"
        );
        assert_eq!(
            entry["url"],
            "https://example.test/reads/items/source/a-story/"
        );
        assert!(entry["id"].as_str().unwrap().starts_with("urn:aggr:item:"));
        assert_eq!(entry["external_url"], "https://upstream.test/story?a=1&b=2");
        assert_eq!(
            entry["content_html"],
            "<p>Clean &amp; <strong>complete</strong></p>"
        );
        assert_eq!(entry["tags"], serde_json::json!(["Engineering", "Rust"]));
        assert!(entry.get("content_text").is_none());
    }

    #[test]
    fn item_json_is_json_ld_and_reuses_the_feed_identity() {
        let item = item();
        let document: Value =
            serde_json::from_str(&item_json(&site(), &item, "# Complete").unwrap()).unwrap();
        let feed: Value =
            serde_json::from_str(&json_collection(&site(), "Example", "", &[item]).unwrap())
                .unwrap();

        assert_eq!(document["@context"], "https://schema.org");
        assert_eq!(document["@type"], "BlogPosting");
        assert_eq!(document["id"], feed["items"][0]["id"]);
        assert_eq!(document["@id"], document["id"]);
        assert_eq!(document["url"], feed["items"][0]["url"]);
        assert_eq!(document["isBasedOn"], feed["items"][0]["external_url"]);
        assert_eq!(document["articleBody"], "# Complete");
    }

    #[test]
    fn feeds_fall_back_to_text_content_and_relative_site_paths() {
        let mut site = site();
        site.base_url = None;
        let mut item = item();
        item.body_html = None;
        let json: Value =
            serde_json::from_str(&json_collection(&site, "Example", "", &[item.clone()]).unwrap())
                .unwrap();
        assert!(
            json["items"][0]["id"]
                .as_str()
                .unwrap()
                .starts_with("urn:aggr:item:")
        );
        assert_eq!(json["items"][0]["content_text"], item.excerpt);
        assert!(!atom_feed(&site, &build(), &[item]).contains("<content type=\"html\">"));
    }

    #[test]
    fn opensearch_is_bounded_and_escapes_templates() {
        let xml = opensearch_description(&OpenSearchDescription {
            short_name: "A surprisingly long reader",
            description: "Search <everything>",
            search_url: "https://example.test/search/?q={searchTerms}&scope=all",
            self_url: Some("https://example.test/opensearch.xml"),
        });

        assert!(xml.contains("<ShortName>A surprisingly l</ShortName>"));
        assert!(xml.contains("<Description>Search &lt;everything&gt;</Description>"));
        assert!(xml.contains("q={searchTerms}&amp;scope=all"));
        assert!(xml.contains("type=\"application/opensearchdescription+xml\" rel=\"self\""));
    }

    #[test]
    fn sitemap_stays_a_single_urlset_when_it_fits() {
        let output = sitemap(
            &[SitemapUrl::new(
                "https://example.test/a?x=1&y=2",
                Some(at(8)),
            )],
            "https://example.test/sitemap.xml",
            SitemapLimits::default(),
        )
        .unwrap();

        let SitemapOutput::UrlSet { xml } = output else {
            panic!("expected one urlset");
        };
        assert!(xml.contains("<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">"));
        assert!(xml.contains("<loc>https://example.test/a?x=1&amp;y=2</loc>"));
        assert!(xml.contains("<lastmod>2026-09-02T08:30:00Z</lastmod>"));
    }

    #[test]
    fn sitemap_splits_into_an_index_and_named_urlsets() {
        let urls = vec![
            SitemapUrl::new("https://example.test/a", Some(at(8))),
            SitemapUrl::new("https://example.test/b", Some(at(10))),
            SitemapUrl::new("https://example.test/c", None),
        ];
        let output = sitemap(
            &urls,
            "https://example.test/nested/sitemap.xml",
            SitemapLimits {
                max_urls: 2,
                max_bytes: 1_024,
            },
        )
        .unwrap();

        let SitemapOutput::Index { xml, chunks } = output else {
            panic!("expected an index");
        };
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].name, "sitemap-1.xml");
        assert_eq!(chunks[0].loc, "https://example.test/nested/sitemap-1.xml");
        assert_eq!(chunks[0].lastmod, Some(at(10)));
        assert_eq!(chunks[1].lastmod, None);
        assert!(xml.contains("<sitemapindex"));
        assert!(xml.contains("https://example.test/nested/sitemap-2.xml"));
        assert_eq!(chunks[0].xml.matches("<url>").count(), 2);
        assert_eq!(chunks[1].xml.matches("<url>").count(), 1);
    }

    #[test]
    fn sitemap_splits_on_serialized_byte_size() {
        let one = SitemapUrl::new(format!("https://example.test/{}", "a".repeat(160)), None);
        let two = SitemapUrl::new(format!("https://example.test/{}", "b".repeat(160)), None);
        let single_size = sitemap_urlset(&[sitemap_url_fragment(&one).unwrap()], 0..1).len();
        let output = sitemap(
            &[one, two],
            "https://example.test/sitemap.xml",
            SitemapLimits {
                max_urls: 10,
                max_bytes: single_size,
            },
        )
        .unwrap();
        assert_eq!(output.chunks().len(), 2);
        assert!(
            output
                .chunks()
                .iter()
                .all(|chunk| chunk.xml.len() <= single_size)
        );
    }

    #[test]
    fn sitemap_rejects_invalid_limits_urls_and_oversized_entries() {
        assert!(
            sitemap(
                &[],
                "https://example.test/sitemap.xml",
                SitemapLimits {
                    max_urls: 0,
                    max_bytes: 1_024,
                }
            )
            .unwrap_err()
            .to_string()
            .contains("max_urls")
        );
        assert!(
            sitemap(
                &[SitemapUrl::new("/relative", None)],
                "https://example.test/sitemap.xml",
                SitemapLimits::default(),
            )
            .unwrap_err()
            .to_string()
            .contains("absolute")
        );
        assert!(
            sitemap(
                &[SitemapUrl::new("https://example.test/too-large", None)],
                "https://example.test/sitemap.xml",
                SitemapLimits {
                    max_urls: 1,
                    max_bytes: sitemap_urlset(&[], 0..0).len(),
                },
            )
            .unwrap_err()
            .to_string()
            .contains("sitemap entry")
        );
    }

    #[test]
    fn stub_escapes_and_redirects() {
        let stub = redirect_stub("https://github.com/o/r/blob/abc/items/x/a.md?a=1&b=\"2\"");
        assert!(stub.contains("http-equiv=\"refresh\""));
        assert!(stub.contains("&amp;b=&quot;2&quot;"));
        assert!(!stub.contains("&b=\"2\""));
        assert!(stub.len() < 400);
    }

    #[test]
    fn escapes_xml_and_drops_invalid_control_characters() {
        assert_eq!(
            escape("a < b & c > \"d\"\0"),
            "a &lt; b &amp; c &gt; &quot;d&quot;"
        );
    }
}
