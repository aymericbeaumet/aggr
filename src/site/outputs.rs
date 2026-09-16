//! Non-template outputs: syndicated feeds, discovery documents, sitemaps and redirect stubs.

use std::collections::BTreeSet;
use std::ops::Range;

use anyhow::{Result, bail};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Map, Value};

use super::context::{BuildCtx, ItemCtx, SiteCtx, SourceCtx};

const XML_DECLARATION: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n";
const SITEMAP_NAMESPACE: &str = "http://www.sitemaps.org/schemas/sitemap/0.9";
const DUBLIN_CORE_NAMESPACE: &str = "http://purl.org/dc/elements/1.1/";
pub const AGGR_REPOSITORY: &str = "https://github.com/aymericbeaumet/aggr";
pub const AGGR_NETWORK: &str = "https://github.com/aymericbeaumet/aggr#network";
pub const AGGR_INSTANCE_TYPE: &str = "https://github.com/aymericbeaumet/aggr#instance";
const AGGR_PROFILE: &str =
    "https://github.com/aymericbeaumet/aggr/blob/v1/docs/interoperability.md#aggr-network";
const AGGR_SCHEMA: &str =
    "https://raw.githubusercontent.com/aymericbeaumet/aggr/v1/docs/aggr-instance.schema.json";

/// Small network-polled update manifest shared by ordinary pages and installed readers.
pub fn updates(site: &SiteCtx, build: &BuildCtx) -> Result<String> {
    Ok(serde_json::to_string(&serde_json::json!({
        "app_version": build.app_version,
        "content_version": build.content_version,
        "entries": site.entry_shortcuts,
    }))?)
}

/// Public metadata that lets a crawler or another reader recognize and consume an aggr instance.
/// URLs are absolute for release builds and descriptor-relative for portable local builds.
/// `updated` is the newest archive change rather than the build clock: the descriptor sits in the
/// worker's precache list, so rebuilding an unchanged archive must reproduce it byte for byte.
pub fn instance_descriptor(
    site: &SiteCtx,
    build: &BuildCtx,
    updated: DateTime<Utc>,
) -> Result<String> {
    let endpoint = |path: &str| site.endpoint(path);
    let config_url = site
        .config_url
        .clone()
        .unwrap_or_else(|| endpoint("aggr.toml"));
    let mut source = Map::new();
    source.insert("config".into(), Value::String(config_url));
    if let Some(repository) = &site.repository {
        let repository_url = format!("https://github.com/{repository}");
        source.insert("repository".into(), Value::String(repository_url.clone()));
        source.insert(
            "data".into(),
            Value::String(format!(
                "{repository_url}/tree/{}",
                build.data_sha.as_deref().unwrap_or(&site.data_branch)
            )),
        );
    }

    let mut discovery = Map::new();
    discovery.insert("search".into(), Value::String(endpoint("?q=")));
    discovery.insert(
        "search_manifest".into(),
        Value::String(endpoint("search-manifest.json")),
    );
    discovery.insert("opml".into(), Value::String(endpoint("sources.opml")));
    if site.base_url.is_some() {
        discovery.insert(
            "opensearch".into(),
            Value::String(endpoint("opensearch.xml")),
        );
        discovery.insert("linkset".into(), Value::String(endpoint("linkset.json")));
        discovery.insert("sitemap".into(), Value::String(endpoint("sitemap.xml")));
    }
    if site.pwa {
        discovery.insert(
            "webmanifest".into(),
            Value::String(endpoint("manifest.webmanifest")),
        );
    }

    let mut collections = Map::new();
    collections.insert("browse".into(), Value::String(endpoint("browse/")));
    collections.insert("sources".into(), Value::String(endpoint("sources/")));
    collections.insert("tags".into(), Value::String(endpoint("tags/")));
    if site.has_categories {
        collections.insert("categories".into(), Value::String(endpoint("categories/")));
    }

    serde_json::to_string_pretty(&serde_json::json!({
        "$schema": AGGR_SCHEMA,
        "schema_version": 1,
        "type": "aggr-instance",
        "profile": AGGR_PROFILE,
        "network": AGGR_NETWORK,
        "url": endpoint(""),
        "name": site.title,
        "language": site.language,
        "updated": updated.to_rfc3339_opts(SecondsFormat::Secs, true),
        "generator": {
            "name": "aggr",
            "version": build.version,
            "url": AGGR_REPOSITORY,
        },
        "source": Value::Object(source),
        "feeds": {
            "atom": endpoint("atom.xml"),
            "rss": endpoint("rss.xml"),
            "json": endpoint("feed.json"),
        },
        "collections": Value::Object(collections),
        "discovery": Value::Object(discovery),
    }))
    .map_err(Into::into)
}

pub fn llms_txt(site: &SiteCtx) -> String {
    let endpoint = |path: &str| site.endpoint(path);
    let config_url = site
        .config_url
        .clone()
        .unwrap_or_else(|| endpoint("aggr.toml"));
    let mut out = format!(
        "# {}\n\n> {}\n\n## Resources\n\n- [Site]({})\n- [Instance metadata]({})\n- [Atom feed]({})\n- [RSS feed]({})\n- [JSON Feed]({})\n- [OPML subscriptions]({})\n- [Browse]({})\n- [Sources]({})\n",
        site.title,
        site.description,
        endpoint(""),
        endpoint("aggr.json"),
        endpoint("atom.xml"),
        endpoint("rss.xml"),
        endpoint("feed.json"),
        endpoint("sources.opml"),
        endpoint("browse/"),
        endpoint("sources/"),
    );
    if site.has_categories {
        out.push_str(&format!("- [Categories]({})\n", endpoint("categories/")));
    }
    out.push_str(&format!(
        "- [Tags]({})\n- [Source configuration]({config_url})\n",
        endpoint("tags/")
    ));
    if site.base_url.is_some() {
        out.push_str(&format!(
            "- [Original URL to snapshot linkset]({})\n- [Sitemap]({})\n",
            endpoint("linkset.json"),
            endpoint("sitemap.xml")
        ));
    }
    out
}

/// OPML 2.0 subscription list of the followed feeds, so the site's sources can be imported by
/// any other reader (including another aggr through `[[sources]].url`).
///
/// Only sources with a public HTTP(S) feed endpoint are listed: the endpoint the fetch pipeline
/// resolved when it is known, otherwise the configured URL. Mirrored aggr repositories are Git
/// remotes, not feeds, and are left out. `created` is the newest archive change rather than the
/// build clock, so rebuilding an unchanged archive reproduces the document byte for byte.
pub fn sources_opml(site: &SiteCtx, created: DateTime<Utc>, sources: &[SourceCtx]) -> String {
    let mut out = String::with_capacity(512 + sources.len() * 192);
    out.push_str(XML_DECLARATION);
    out.push_str("<opml version=\"2.0\">\n  <head>\n");
    element(&mut out, 4, "title", &site.title);
    element(&mut out, 4, "dateCreated", &created.to_rfc2822());
    out.push_str("  </head>\n  <body>\n");
    for source in sources {
        if source.engine != "web" {
            continue;
        }
        let Some(feed_url) = source
            .feed_url
            .as_deref()
            .or(source.url.as_deref())
            .map(str::trim)
            .filter(|url| absolute_http_url(url, "feed URL").is_ok())
        else {
            continue;
        };
        out.push_str("    <outline");
        attribute(&mut out, "type", "rss");
        attribute(&mut out, "text", &source.name);
        attribute(&mut out, "title", &source.name);
        attribute(&mut out, "xmlUrl", feed_url);
        if let Some(html_url) = source
            .site_url
            .as_deref()
            .filter(|url| absolute_http_url(url, "site URL").is_ok())
        {
            attribute(&mut out, "htmlUrl", html_url);
        }
        if let Some(category) = source
            .category
            .as_deref()
            .map(str::trim)
            .filter(|category| !category.is_empty())
        {
            attribute(&mut out, "category", category);
        }
        out.push_str("/>\n");
    }
    out.push_str("  </body>\n</opml>\n");
    out
}

/// Serialize the original-to-snapshot relationships as an RFC 9264 linkset. The generated
/// article pages remain self-canonical; this separate graph lets crawlers look up every retained
/// readable snapshot by the exact upstream URL without pretending it is the upstream resource.
pub fn linkset_json(site: &SiteCtx, items: &[ItemCtx]) -> Result<String> {
    let base_url = site
        .base_url
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("an RFC 9264 linkset needs an absolute public base URL"))?;
    absolute_http_url(base_url, "linkset base URL")?;
    let mut archived_at: std::collections::BTreeMap<&str, Vec<Value>> =
        std::collections::BTreeMap::new();
    let mut linkset = Vec::with_capacity(items.len() * 2);

    for item in items {
        if absolute_http_url(&item.link, "original URL").is_err() {
            continue;
        }
        let local = site_url(site, &item.url);
        archived_at
            .entry(&item.link)
            .or_default()
            .push(serde_json::json!({"href": local, "type": "text/html"}));

        let representation = local.trim_end_matches('/');
        linkset.push(serde_json::json!({
            "anchor": local,
            "via": [{"href": item.link, "type": "text/html"}],
            "alternate": [
                {"href": format!("{representation}.md"), "type": "text/markdown"},
                {"href": format!("{representation}.txt"), "type": "text/plain"},
                {"href": format!("{representation}.rst"), "type": "text/x-rst"},
                {"href": format!("{representation}.json"), "type": "application/ld+json"},
            ],
        }));
    }

    for (original, snapshots) in archived_at {
        linkset.push(serde_json::json!({
            "anchor": original,
            "https://schema.org/archivedAt": snapshots,
        }));
    }
    linkset.sort_by(|left, right| left["anchor"].as_str().cmp(&right["anchor"].as_str()));

    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "linkset": linkset,
    }))?)
}

/// A tiny, progressively functional migration page for a retired public route.
pub fn redirect_stub(site: &SiteCtx, target: &str) -> String {
    let language = escape(&site.language);
    let target = escape(target);
    format!(
        "<!doctype html><html lang=\"{language}\"><meta charset=\"utf-8\">\
         <meta http-equiv=\"refresh\" content=\"0;url={target}\">\
         <meta name=\"robots\" content=\"noindex,follow\"><link rel=\"canonical\" href=\"{target}\">\
         <title>Moved</title><a href=\"{target}\">Continue</a></html>\n"
    )
}

/// A media file attached to an item: the podcast episode or video an entry is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enclosure {
    pub url: String,
    pub mime: String,
    /// Size in bytes when the publisher declared one.
    pub length: Option<u64>,
    /// Playing time in whole seconds when known.
    pub duration: Option<u64>,
}

/// The enclosure an item carries, from the `audio_url`, `audio_type`, `audio_length` and
/// `duration_seconds` extras written at capture time. Items archived before the type was retained
/// fall back to the file extension; an unrecognized one yields no enclosure rather than a guess.
pub fn enclosure(item: &ItemCtx) -> Option<Enclosure> {
    let url = item
        .extra
        .get("audio_url")
        .and_then(serde_yaml_ng::Value::as_str)
        .map(str::trim)
        .filter(|url| absolute_http_url(url, "enclosure URL").is_ok())?;
    let mime = item
        .extra
        .get("audio_type")
        .and_then(serde_yaml_ng::Value::as_str)
        .map(str::trim)
        .filter(|mime| mime.contains('/') && !mime.chars().any(char::is_whitespace))
        .map(str::to_ascii_lowercase)
        .or_else(|| mime_for_extension(url).map(str::to_string))?;
    let count = |key: &str| {
        item.extra
            .get(key)
            .and_then(|value| {
                value
                    .as_u64()
                    .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
            })
            .filter(|count| *count > 0)
    };
    Some(Enclosure {
        url: url.to_string(),
        mime,
        length: count("audio_length"),
        duration: count("duration_seconds"),
    })
}

/// Media types for the enclosure formats podcast and video feeds actually publish.
fn mime_for_extension(url: &str) -> Option<&'static str> {
    let parsed = url::Url::parse(url).ok()?;
    let file = parsed.path().rsplit('/').next()?;
    let (_, extension) = file.rsplit_once('.')?;
    match extension.to_ascii_lowercase().as_str() {
        "mp3" => Some("audio/mpeg"),
        "m4a" => Some("audio/mp4"),
        "ogg" | "oga" => Some("audio/ogg"),
        "opus" => Some("audio/opus"),
        "wav" => Some("audio/wav"),
        "flac" => Some("audio/flac"),
        "mp4" | "m4v" => Some("video/mp4"),
        "webm" => Some("video/webm"),
        "mov" => Some("video/quicktime"),
        _ => None,
    }
}

/// Author names worth publishing: trimmed, non-empty, in declaration order.
fn item_authors(item: &ItemCtx) -> Vec<&str> {
    item.authors
        .iter()
        .map(|author| author.trim())
        .filter(|author| !author.is_empty())
        .collect()
}

/// Atom feed of the river's first page, so the site itself can be followed.
pub fn atom_feed(site: &SiteCtx, build: &BuildCtx, items: &[ItemCtx]) -> String {
    atom_collection(site, build, &site.title, "", items)
}

/// Language of one entry when it differs from the feed-level `xml:lang` or `language`, compared
/// case-insensitively as BCP 47 requires. `None` keeps a single-language feed byte-identical.
fn entry_language<'a>(item: &'a ItemCtx, site: &SiteCtx) -> Option<&'a str> {
    item.language
        .as_deref()
        .filter(|language| !language.eq_ignore_ascii_case(&site.language))
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
    // Atom requires an author somewhere; the site stands in for entries without a byline.
    out.push_str("  <author>\n");
    element(&mut out, 4, "name", &site.title);
    out.push_str("  </author>\n");
    out.push_str(&format!(
        "  <generator version=\"{}\" uri=\"https://github.com/aymericbeaumet/aggr\">aggr</generator>\n",
        escape(&build.version)
    ));

    for item in items {
        let local = site_url(site, &item.url);
        let item_updated = item.updated.unwrap_or(item.date);
        match entry_language(item, site) {
            Some(language) => {
                out.push_str(&format!("  <entry xml:lang=\"{}\">\n", escape(language)));
            }
            None => out.push_str("  <entry>\n"),
        }
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
        if let Some(enclosure) = enclosure(item) {
            out.push_str(&format!(
                "    <link rel=\"enclosure\" type=\"{}\" href=\"{}\"",
                escape(&enclosure.mime),
                escape(&enclosure.url)
            ));
            if let Some(length) = enclosure.length {
                out.push_str(&format!(" length=\"{length}\""));
            }
            out.push_str("/>\n");
        }
        if let Some(published) = item.published {
            element(&mut out, 4, "published", &published.to_rfc3339());
        }
        element(&mut out, 4, "updated", &item_updated.to_rfc3339());
        let mut authors = item_authors(item);
        if authors.is_empty() && !item.source_name.trim().is_empty() {
            authors.push(item.source_name.trim());
        }
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
    out.push_str(&format!(
        "<rss version=\"2.0\" xmlns:atom=\"http://www.w3.org/2005/Atom\" \
         xmlns:content=\"http://purl.org/rss/1.0/modules/content/\" \
         xmlns:dc=\"{DUBLIN_CORE_NAMESPACE}\">\n<channel>\n",
    ));
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
        if let Some(enclosure) = enclosure(item) {
            // RSS makes `length` mandatory; "0" is the conventional value for an unknown size.
            out.push_str(&format!(
                "    <enclosure url=\"{}\" length=\"{}\" type=\"{}\"/>\n",
                escape(&enclosure.url),
                enclosure.length.unwrap_or(0),
                escape(&enclosure.mime)
            ));
        }
        element(&mut out, 4, "pubDate", &item.date.to_rfc2822());
        for author in item_authors(item) {
            element(&mut out, 4, "dc:creator", author);
        }
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
            if let Some(language) = entry_language(item, site) {
                entry.insert("language".into(), Value::String(language.to_string()));
            }
            if let Some(preview) = &item.preview
                && site.base_url.is_some()
            {
                entry.insert("image".into(), Value::String(site_url(site, &preview.url)));
            }
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
            let authors = item_authors(item);
            if !authors.is_empty() {
                entry.insert(
                    "authors".into(),
                    Value::Array(
                        authors
                            .into_iter()
                            .map(|name| serde_json::json!({"name": name}))
                            .collect(),
                    ),
                );
            }
            if let Some(enclosure) = enclosure(item) {
                let mut attachment = Map::new();
                attachment.insert("url".into(), Value::String(enclosure.url));
                attachment.insert("mime_type".into(), Value::String(enclosure.mime));
                if let Some(length) = enclosure.length {
                    attachment.insert("size_in_bytes".into(), Value::from(length));
                }
                if let Some(duration) = enclosure.duration {
                    attachment.insert("duration_in_seconds".into(), Value::from(duration));
                }
                entry.insert(
                    "attachments".into(),
                    Value::Array(vec![Value::Object(attachment)]),
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
    // JSON Feed defines both as URLs; a portable build only knows paths, and both are optional.
    if site.base_url.is_some() {
        feed.insert("home_page_url".into(), Value::String(home));
        feed.insert("feed_url".into(), Value::String(self_url));
    }
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

pub fn item_json(
    site: &SiteCtx,
    build: &BuildCtx,
    item: &ItemCtx,
    markdown: &str,
) -> Result<String> {
    let local = site_url(site, &item.url);
    let snapshot_id = format!("{local}#webpage");
    let mut original = serde_json::json!({
        "@type": "CreativeWork",
        "@id": item.link,
        "url": item.link,
        "headline": item.title,
        "archivedAt": {"@id": snapshot_id},
    });
    if item.word_count > 0 {
        original["wordCount"] = serde_json::json!(item.word_count);
        original["timeRequired"] = Value::String(format!("PT{}M", item.reading_minutes));
    }
    if let Some(published) = item.published {
        original["datePublished"] = Value::String(published.to_rfc3339());
    }
    if let Some(updated) = item.updated {
        original["dateModified"] = Value::String(updated.to_rfc3339());
    }
    if !item.authors.is_empty() {
        original["author"] = Value::Array(
            item.authors
                .iter()
                .map(|name| serde_json::json!({"@type": "Person", "name": name}))
                .collect(),
        );
    }

    let git = build.data_sha.as_ref().map(|commit| {
        serde_json::json!({
            "commit": commit,
            "path": format!("{}.md", item.path),
            "permalink": item.permalink,
        })
    });

    if let Some(preview) = &item.preview
        && site.base_url.is_some()
    {
        original["image"] = serde_json::json!({
            "@type": "ImageObject",
            "url": site_url(site, &preview.url),
            "width": preview.width,
            "height": preview.height,
            "caption": preview.alt,
        });
    }

    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "@context": "https://schema.org",
        "@type": ["WebPage", "ArchiveComponent"],
        "@id": snapshot_id,
        "id": item_uid(item),
        "url": local,
        "name": format!("Archived snapshot: {}", item.title),
        "dateCreated": item.replicated_at.unwrap_or(item.first_seen).to_rfc3339(),
        "temporalCoverage": item.first_seen.to_rfc3339(),
        "isBasedOn": {"@id": item.link},
        "mainEntity": original,
        "external_url": item.link,
        "title": item.title,
        "source": item.source,
        "source_name": item.source_name,
        "category": item.category,
        "tags": item.labels,
        "date_created": item.date.to_rfc3339(),
        "captured_at": item.first_seen.to_rfc3339(),
        "replicated_at": item.replicated_at.map(|date| date.to_rfc3339()),
        "date_published": item.published.map(|date| date.to_rfc3339()),
        "date_modified": item.updated.map(|date| date.to_rfc3339()),
        "authors": item.authors,
        "summary": item.summary,
        "description": item.excerpt,
        "preview": item.preview,
        "word_count": item.word_count,
        "reading_minutes": item.reading_minutes,
        "git": git,
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
    let mut output = format!("{}\n\n{}\n", item.title, body.trim());
    if !item.resources.is_empty() {
        output.push_str("\nResources\n");
        for resource in &item.resources {
            output.push_str(&format!("{}: {}\n", resource.label, resource.url));
        }
    }
    output
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

/// Feed and discovery links: absolute on a published site, `base_path`-relative on a portable one.
fn site_url(site: &SiteCtx, path: &str) -> String {
    site.absolute(path).unwrap_or_else(|| site.url(path))
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

/// Append ` name="value"` with the value escaped for a double-quoted XML attribute.
fn attribute(out: &mut String, name: &str, value: &str) {
    out.push(' ');
    out.push_str(name);
    out.push_str("=\"");
    out.push_str(&escape(value));
    out.push('"');
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
            identity: None,
            language: "en-GB".into(),
            og_locale: "en_GB".into(),
            base_path: "/reads/".into(),
            base_url: Some("https://example.test/reads/".into()),
            repository: None,
            data_branch: "aggr".into(),
            network_url: AGGR_NETWORK,
            instance_type_url: AGGR_INSTANCE_TYPE,
            pwa: true,
            preferences: serde_json::json!({}),
            config_page_url: None,
            config_url: None,
            has_categories: true,
            discussions: Vec::new(),
            entry_shortcuts: Vec::new(),
            params: toml::Table::new(),
        }
    }

    fn build() -> BuildCtx {
        BuildCtx {
            time: at(12),
            version: "1.2.3".into(),
            app_version: "app".into(),
            content_version: "content".into(),
            config_sha: None,
            data_sha: None,
            generation: "generation".into(),
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
            source_display: "upstream.example".into(),
            source_title: "Upstream".into(),
            source_url: "https://upstream.example/".into(),
            feed_display: "upstream.example".into(),
            is_aggregated: false,
            is_youtube: false,
            language: None,
            category: Some("Engineering".into()),
            date: at(8),
            age_band: "day",
            published: Some(at(8)),
            updated: Some(at(9)),
            first_seen: at(10),
            replicated_at: None,
            authors: vec!["A & B".into()],
            labels: vec!["Rust".into(), "engineering".into()],
            resources: Vec::new(),
            discussions: vec![DiscussionLinkCtx {
                name: "hackernews".into(),
                url: "https://news.ycombinator.com/item?id=42".into(),
                shortcut: None,
                found: true,
                score: Some(12),
            }],
            summary: Some("Summary".into()),
            excerpt: "A concise <summary>".into(),
            content: ContentKind::Feed,
            word_count: 3,
            reading_minutes: 1,
            preview: None,
            article_preview: None,
            video: None,
            document: None,
            interactive: None,
            native_media: None,
            item_type: crate::site::item_type::ItemType::Article,
            extra: BTreeMap::new(),
            metadata: super::super::display::Metadata::default(),
            permalink: None,
            raw_url: None,
            history_url: None,
            edit_url: None,
            previous_article: None,
            next_article: None,
            recommended_articles: Vec::new(),
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
    fn json_feed_preview_has_a_public_local_url_without_changing_provenance() {
        let mut item = item();
        item.preview = Some(super::super::context::PreviewCtx {
            url: "assets/previews/abc.jpg".into(),
            width: 320,
            height: 180,
            alt: Some("A preview".into()),
            color: Some("#285a8c".into()),
            placeholder: crate::media::placeholder::from_image(&image::DynamicImage::new_rgb8(
                4, 4,
            ))
            .unwrap(),
        });
        let feed: Value =
            serde_json::from_str(&json_collection(&site(), "Feed", "", &[item.clone()]).unwrap())
                .unwrap();
        assert_eq!(
            feed["items"][0]["image"],
            "https://example.test/reads/assets/previews/abc.jpg"
        );
        assert_eq!(feed["items"][0]["external_url"], item.link);
        let mut portable = site();
        portable.base_url = None;
        let feed: Value =
            serde_json::from_str(&json_collection(&portable, "Feed", "", &[item]).unwrap())
                .unwrap();
        assert!(feed["items"][0].get("image").is_none());
    }

    #[test]
    fn linkset_maps_exact_original_urls_to_local_snapshots() {
        let first = item();
        let mut second = item();
        second.path = "items/other/a-story".into();
        second.url = "items/other/a-story/".into();

        let json: Value = serde_json::from_str(
            &linkset_json(&site(), &[first.clone(), second]).expect("linkset"),
        )
        .unwrap();
        assert_eq!(json.as_object().unwrap().len(), 1);
        assert!(json.as_object().unwrap().contains_key("linkset"));
        let contexts = json["linkset"].as_array().unwrap();
        let original = contexts
            .iter()
            .find(|context| context["anchor"] == first.link)
            .unwrap();
        assert_eq!(
            original["https://schema.org/archivedAt"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            original["https://schema.org/archivedAt"][0]["href"],
            "https://example.test/reads/items/source/a-story/"
        );
        let snapshot = contexts
            .iter()
            .find(|context| context["anchor"] == "https://example.test/reads/items/source/a-story/")
            .unwrap();
        assert_eq!(snapshot["via"][0]["href"], first.link);
        assert_eq!(
            snapshot["alternate"][0]["href"],
            "https://example.test/reads/items/source/a-story.md"
        );
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
    fn item_json_describes_the_snapshot_separately_from_the_original() {
        let item = item();
        let document: Value =
            serde_json::from_str(&item_json(&site(), &build(), &item, "# Complete").unwrap())
                .unwrap();
        let feed: Value =
            serde_json::from_str(&json_collection(&site(), "Example", "", &[item]).unwrap())
                .unwrap();

        assert_eq!(document["@context"], "https://schema.org");
        assert_eq!(
            document["@type"],
            serde_json::json!(["WebPage", "ArchiveComponent"])
        );
        assert_eq!(document["id"], feed["items"][0]["id"]);
        assert_eq!(
            document["@id"],
            format!("{}#webpage", feed["items"][0]["url"].as_str().unwrap())
        );
        assert_eq!(document["url"], feed["items"][0]["url"]);
        assert_eq!(
            document["isBasedOn"]["@id"],
            feed["items"][0]["external_url"]
        );
        assert_eq!(
            document["mainEntity"]["@id"],
            "https://upstream.test/story?a=1&b=2"
        );
        assert_eq!(document["mainEntity"]["archivedAt"]["@id"], document["@id"]);
        assert_eq!(document["word_count"], 3);
        assert_eq!(document["reading_minutes"], 1);
        assert_eq!(document["mainEntity"]["wordCount"], 3);
        assert_eq!(document["mainEntity"]["timeRequired"], "PT1M");
        assert_eq!(document["dateCreated"], "2026-09-02T10:30:00+00:00");
        assert_eq!(document["articleBody"], "# Complete");
    }

    #[test]
    fn item_json_does_not_mislabel_capture_time_as_publication_time() {
        let mut item = item();
        item.published = None;
        item.updated = None;
        item.replicated_at = Some(at(11));
        let document: Value =
            serde_json::from_str(&item_json(&site(), &build(), &item, "# Complete").unwrap())
                .unwrap();

        assert!(document["mainEntity"].get("datePublished").is_none());
        assert_eq!(document["dateCreated"], "2026-09-02T11:30:00+00:00");
        assert_eq!(document["temporalCoverage"], "2026-09-02T10:30:00+00:00");
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
        assert!(json.get("home_page_url").is_none());
        assert!(json.get("feed_url").is_none());
        assert!(json.get("icon").is_none());
        assert!(json.get("favicon").is_none());
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
    fn instance_descriptor_exposes_network_protocols_and_provenance() {
        let mut site = site();
        site.repository = Some("owner/reader".into());
        site.config_url =
            Some("https://raw.githubusercontent.com/owner/reader/deadbeef/aggr.toml".into());
        let descriptor: Value =
            serde_json::from_str(&instance_descriptor(&site, &build(), at(9)).unwrap()).unwrap();

        assert_eq!(descriptor["schema_version"], 1);
        assert_eq!(descriptor["type"], "aggr-instance");
        assert_eq!(descriptor["network"], AGGR_NETWORK);
        assert_eq!(descriptor["url"], "https://example.test/reads/");
        assert_eq!(descriptor["updated"], "2026-09-02T09:30:00Z");
        assert_eq!(
            descriptor["source"]["config"],
            "https://raw.githubusercontent.com/owner/reader/deadbeef/aggr.toml"
        );
        assert_eq!(
            descriptor["source"]["repository"],
            "https://github.com/owner/reader"
        );
        assert_eq!(
            descriptor["source"]["data"],
            "https://github.com/owner/reader/tree/aggr"
        );
        assert_eq!(
            descriptor["feeds"]["atom"],
            "https://example.test/reads/atom.xml"
        );
        assert_eq!(
            descriptor["collections"]["browse"],
            "https://example.test/reads/browse/"
        );
        assert_eq!(
            descriptor["collections"]["sources"],
            "https://example.test/reads/sources/"
        );
        assert_eq!(
            descriptor["collections"]["tags"],
            "https://example.test/reads/tags/"
        );
        assert_eq!(
            descriptor["collections"]["categories"],
            "https://example.test/reads/categories/"
        );
        assert_eq!(
            descriptor["discovery"]["sitemap"],
            "https://example.test/reads/sitemap.xml"
        );
    }

    #[test]
    fn portable_instance_descriptor_uses_relative_endpoints() {
        let mut site = site();
        site.base_url = None;
        site.config_url = None;
        site.has_categories = false;
        let descriptor: Value =
            serde_json::from_str(&instance_descriptor(&site, &build(), at(12)).unwrap()).unwrap();

        assert_eq!(descriptor["url"], "./");
        assert_eq!(descriptor["source"]["config"], "aggr.toml");
        assert_eq!(descriptor["feeds"]["json"], "feed.json");
        assert_eq!(descriptor["collections"]["browse"], "browse/");
        assert_eq!(descriptor["collections"]["sources"], "sources/");
        assert_eq!(descriptor["collections"]["tags"], "tags/");
        assert!(descriptor["collections"].get("categories").is_none());
        assert!(descriptor["discovery"].get("sitemap").is_none());
    }

    #[test]
    fn llms_inventory_points_to_collection_directories() {
        let inventory = llms_txt(&site());
        assert!(inventory.contains("[Browse](https://example.test/reads/browse/)"));
        assert!(inventory.contains("[Sources](https://example.test/reads/sources/)"));
        assert!(inventory.contains("[Tags](https://example.test/reads/tags/)"));
        assert!(inventory.contains("[Categories](https://example.test/reads/categories/)"));
        assert!(!inventory.contains("browse/#sources"));
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
        let stub = redirect_stub(
            &site(),
            "https://github.com/o/r/blob/abc/items/x/a.md?a=1&b=\"2\"",
        );
        assert!(stub.starts_with("<!doctype html><html lang=\"en-GB\">"));
        assert!(stub.ends_with("</html>\n"));
        assert!(stub.contains("http-equiv=\"refresh\""));
        assert!(stub.contains("&amp;b=&quot;2&quot;"));
        assert!(!stub.contains("&b=\"2\""));
        assert!(stub.len() < 512);
    }

    #[test]
    fn escapes_xml_and_drops_invalid_control_characters() {
        assert_eq!(
            escape("a < b & c > \"d\"\0"),
            "a &lt; b &amp; c &gt; &quot;d&quot;"
        );
    }
    fn podcast() -> ItemCtx {
        let mut item = item();
        item.path = "items/podcast/episode-1".into();
        item.url = "items/podcast/episode-1/".into();
        item.title = "Episode 1".into();
        item.link = "https://podcast.test/episodes/1".into();
        item.authors = vec!["  ".into(), " Host ".into()];
        item.item_type = crate::site::item_type::ItemType::Podcast;
        item.extra.insert(
            "audio_url".into(),
            "https://cdn.podcast.test/episode-1.mp3?token=a&b=2".into(),
        );
        item.extra.insert("audio_type".into(), "audio/mpeg".into());
        item.extra.insert(
            "audio_length".into(),
            serde_yaml_ng::Value::from(12_345_u64),
        );
        item.extra.insert(
            "duration_seconds".into(),
            serde_yaml_ng::Value::from(1_671_u64),
        );
        item
    }

    fn sources() -> Vec<SourceCtx> {
        let source = |slug: &str, name: &str, engine: &str| SourceCtx {
            slug: slug.into(),
            name: name.into(),
            url: None,
            feed_url: None,
            site_url: None,
            language: None,
            category: None,
            engine: engine.into(),
            count: 1,
            latest: Some(at(8)),
            error: None,
            page: format!("sources/{slug}/"),
        };
        vec![
            SourceCtx {
                url: Some("https://blog.rust-lang.org/".into()),
                feed_url: Some("https://blog.rust-lang.org/feed.xml".into()),
                site_url: Some("https://blog.rust-lang.org/".into()),
                category: Some("Engineering".into()),
                ..source("rust", "Rust & Friends", "web")
            },
            SourceCtx {
                url: Some("https://feeds.podcast.test/show?token=abc".into()),
                ..source("podcast", "A \"Podcast\"", "web")
            },
            SourceCtx {
                url: Some("https://github.com/owner/reader".into()),
                ..source("mirror", "Mirror", "aggr")
            },
            source("orphan", "Retained only", "web"),
        ]
    }

    /// Drive a namespace-aware parser to the end so unbalanced tags, bad entities and undeclared
    /// prefixes fail the test with the offending byte offset.
    fn well_formed(xml: &str) {
        use quick_xml::events::Event;
        use quick_xml::name::ResolveResult;
        let mut reader = quick_xml::NsReader::from_str(xml);
        loop {
            match reader.read_resolved_event() {
                Ok((_, Event::Eof)) => break,
                Ok((ResolveResult::Unknown(prefix), _)) => panic!(
                    "undeclared namespace prefix {:?} at byte {}\n{xml}",
                    String::from_utf8_lossy(&prefix),
                    reader.buffer_position()
                ),
                Ok(_) => {}
                Err(error) => panic!(
                    "malformed XML at byte {}: {error}\n{xml}",
                    reader.error_position()
                ),
            }
        }
    }

    fn assert_matches_schema(value: &Value, schema: &Value) {
        let object = value.as_object().expect("schema objects are JSON objects");
        for required in schema["required"].as_array().into_iter().flatten() {
            let key = required.as_str().unwrap();
            assert!(object.contains_key(key), "missing required member {key}");
        }
        let properties = schema["properties"]
            .as_object()
            .expect("closed schemas declare their properties");
        for (key, member) in object {
            let declared = properties
                .get(key)
                .unwrap_or_else(|| panic!("member {key} is not declared by the schema"));
            if let Some(constant) = declared.get("const") {
                assert_eq!(member, constant, "{key}");
            }
            if declared.get("properties").is_some() {
                assert_matches_schema(member, declared);
            }
            if declared.get("$ref") == Some(&Value::String("#/$defs/links".into())) {
                for (name, link) in member.as_object().unwrap() {
                    assert!(link.is_string(), "{key}.{name} must be a URL reference");
                }
            }
        }
    }

    #[test]
    fn atom_round_trips_through_a_standard_parser() {
        let atom = atom_collection(
            &site(),
            &build(),
            "Engineering",
            "/categories/engineering",
            &[item(), podcast()],
        );
        well_formed(&atom);
        let feed = feed_rs::parser::parse(atom.as_bytes()).unwrap();

        assert_eq!(
            feed.id,
            "https://example.test/reads/categories/engineering/"
        );
        assert_eq!(feed.title.unwrap().content, "Engineering");
        assert_eq!(feed.updated, Some(at(9)));
        assert_eq!(
            feed.authors
                .iter()
                .map(|author| author.name.as_str())
                .collect::<Vec<_>>(),
            ["Example & Reader"]
        );
        assert_eq!(feed.entries.len(), 2);
        let entry = &feed.entries[0];
        assert_eq!(entry.id, item_uid(&item()));
        assert_eq!(
            entry.title.as_ref().unwrap().content,
            "A <safe> & useful story"
        );
        assert_eq!(entry.updated, Some(at(9)));
        let alternate = entry
            .links
            .iter()
            .find(|link| link.rel.as_deref() == Some("alternate"))
            .unwrap();
        assert_eq!(
            alternate.href,
            "https://example.test/reads/items/source/a-story/"
        );
        assert_eq!(
            entry
                .authors
                .iter()
                .map(|author| author.name.as_str())
                .collect::<Vec<_>>(),
            ["A & B"]
        );
    }

    #[test]
    fn rss_round_trips_through_a_standard_parser() {
        let rss = rss_collection(&site(), &build(), "Example", "", &[item(), podcast()]);
        well_formed(&rss);
        let feed = feed_rs::parser::parse(rss.as_bytes()).unwrap();

        assert!(!feed.id.is_empty());
        assert_eq!(feed.title.unwrap().content, "Example");
        assert_eq!(feed.updated, Some(at(9)));
        assert_eq!(feed.entries.len(), 2);
        let entry = &feed.entries[0];
        assert_eq!(entry.id, item_uid(&item()));
        assert_eq!(
            entry.title.as_ref().unwrap().content,
            "A <safe> & useful story"
        );
        assert_eq!(entry.published, Some(at(8)));
        let alternate = entry
            .links
            .iter()
            .find(|link| link.rel.as_deref().is_none_or(|rel| rel == "alternate"))
            .unwrap();
        assert_eq!(
            alternate.href,
            "https://example.test/reads/items/source/a-story/"
        );
        assert_eq!(
            entry
                .authors
                .iter()
                .map(|author| author.name.as_str())
                .collect::<Vec<_>>(),
            ["A & B"]
        );
    }

    #[test]
    fn json_feed_parses_with_unique_item_identities() {
        let mut second = item();
        second.path = "items/other/a-story".into();
        second.url = "items/other/a-story/".into();
        let feed: Value = serde_json::from_str(
            &json_collection(&site(), "Example", "", &[item(), second, podcast()]).unwrap(),
        )
        .unwrap();

        assert_eq!(feed["version"], "https://jsonfeed.org/version/1.1");
        assert_eq!(feed["title"], "Example");
        let ids = feed["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["id"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(ids.len(), 3);
        assert!(ids.iter().all(|id| !id.is_empty()));
        assert_eq!(ids.iter().collect::<BTreeSet<_>>().len(), ids.len());
    }

    #[test]
    fn sitemap_is_namespaced_absolute_and_uses_rfc3339_lastmod() {
        let output = sitemap(
            &[
                SitemapUrl::new("https://example.test/a?x=1&y=2", Some(at(8))),
                SitemapUrl::new("https://example.test/b", None),
            ],
            "https://example.test/sitemap.xml",
            SitemapLimits::default(),
        )
        .unwrap();
        let xml = output.root_xml();
        well_formed(xml);
        assert!(xml.contains("<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">"));
        let locs = xml
            .split("<loc>")
            .skip(1)
            .map(|rest| rest.split_once("</loc>").unwrap().0)
            .collect::<Vec<_>>();
        assert_eq!(locs.len(), 2);
        for loc in locs {
            let parsed = url::Url::parse(&loc.replace("&amp;", "&")).unwrap();
            assert!(matches!(parsed.scheme(), "http" | "https") && parsed.host_str().is_some());
        }
        let lastmods = xml
            .split("<lastmod>")
            .skip(1)
            .map(|rest| rest.split_once("</lastmod>").unwrap().0)
            .collect::<Vec<_>>();
        assert_eq!(lastmods, ["2026-09-02T08:30:00Z"]);
        assert!(
            DateTime::parse_from_rfc3339(lastmods[0]).is_ok(),
            "lastmod must be W3C datetime"
        );
    }

    #[test]
    fn instance_descriptor_matches_published_schema() {
        let schema: Value =
            serde_json::from_str(include_str!("../../docs/aggr-instance.schema.json")).unwrap();
        let mut release = site();
        release.repository = Some("owner/reader".into());
        release.config_url =
            Some("https://raw.githubusercontent.com/owner/reader/deadbeef/aggr.toml".into());
        let mut portable = site();
        portable.base_url = None;
        portable.config_url = None;
        portable.has_categories = false;
        for site in [release, portable] {
            let descriptor: Value =
                serde_json::from_str(&instance_descriptor(&site, &build(), at(9)).unwrap())
                    .unwrap();
            assert_matches_schema(&descriptor, &schema);
            assert_eq!(
                descriptor["discovery"]["opml"].as_str().unwrap(),
                if site.base_url.is_some() {
                    "https://example.test/reads/sources.opml"
                } else {
                    "sources.opml"
                }
            );
        }
    }

    #[test]
    fn enclosure_reads_capture_metadata_and_falls_back_to_the_extension() {
        assert_eq!(
            enclosure(&podcast()),
            Some(Enclosure {
                url: "https://cdn.podcast.test/episode-1.mp3?token=a&b=2".into(),
                mime: "audio/mpeg".into(),
                length: Some(12_345),
                duration: Some(1_671),
            })
        );
        assert_eq!(enclosure(&item()), None, "articles have no enclosure");

        let mut archived = podcast();
        archived.extra.remove("audio_type");
        archived.extra.remove("audio_length");
        archived.extra.insert(
            "audio_url".into(),
            "https://cdn.podcast.test/Episode-1.M4A".into(),
        );
        assert_eq!(
            enclosure(&archived),
            Some(Enclosure {
                url: "https://cdn.podcast.test/Episode-1.M4A".into(),
                mime: "audio/mp4".into(),
                length: None,
                duration: Some(1_671),
            })
        );
        for (file, mime) in [
            ("a.mp3", "audio/mpeg"),
            ("a.ogg", "audio/ogg"),
            ("a.oga", "audio/ogg"),
            ("a.opus", "audio/opus"),
            ("a.wav", "audio/wav"),
            ("a.flac", "audio/flac"),
            ("a.mp4", "video/mp4"),
            ("a.m4v", "video/mp4"),
            ("a.webm", "video/webm"),
            ("a.mov", "video/quicktime"),
        ] {
            assert_eq!(
                mime_for_extension(&format!("https://cdn.test/media/{file}?x=1")),
                Some(mime),
                "{file}"
            );
        }

        let mut unknown = archived.clone();
        unknown
            .extra
            .insert("audio_url".into(), "https://cdn.podcast.test/stream".into());
        assert_eq!(enclosure(&unknown), None, "no guessed media type");
        let mut relative = podcast();
        relative
            .extra
            .insert("audio_url".into(), "/episode-1.mp3".into());
        assert_eq!(enclosure(&relative), None, "enclosures must be absolute");

        let mut textual = podcast();
        textual.extra.insert("audio_length".into(), "200".into());
        textual.extra.insert("duration_seconds".into(), "0".into());
        let textual = enclosure(&textual).unwrap();
        assert_eq!((textual.length, textual.duration), (Some(200), None));
    }

    #[test]
    fn feeds_carry_the_episode_enclosure_in_each_format() {
        let atom = atom_feed(&site(), &build(), &[podcast()]);
        let feed = feed_rs::parser::parse(atom.as_bytes()).unwrap();
        let link = feed.entries[0]
            .links
            .iter()
            .find(|link| link.rel.as_deref() == Some("enclosure"))
            .expect("atom enclosure link");
        assert_eq!(
            link.href,
            "https://cdn.podcast.test/episode-1.mp3?token=a&b=2"
        );
        assert_eq!(link.media_type.as_deref(), Some("audio/mpeg"));
        assert_eq!(link.length, Some(12_345));

        let rss = rss_collection(&site(), &build(), "Example", "", &[podcast()]);
        let feed = feed_rs::parser::parse(rss.as_bytes()).unwrap();
        let content = feed.entries[0]
            .media
            .iter()
            .flat_map(|media| media.content.iter())
            .find(|content| content.url.is_some())
            .expect("rss enclosure");
        assert_eq!(
            content.url.as_ref().unwrap().as_str(),
            "https://cdn.podcast.test/episode-1.mp3?token=a&b=2"
        );
        assert_eq!(
            content.content_type.as_ref().unwrap().essence().to_string(),
            "audio/mpeg"
        );
        assert_eq!(content.size, Some(12_345));

        let mut unknown_size = podcast();
        unknown_size.extra.remove("audio_length");
        let rss = rss_collection(&site(), &build(), "Example", "", &[unknown_size]);
        assert!(
            rss.contains(
                "<enclosure url=\"https://cdn.podcast.test/episode-1.mp3?token=a&amp;b=2\" length=\"0\" type=\"audio/mpeg\"/>"
            ),
            "{rss}"
        );

        let json: Value =
            serde_json::from_str(&json_collection(&site(), "Example", "", &[podcast()]).unwrap())
                .unwrap();
        assert_eq!(
            json["items"][0]["attachments"],
            serde_json::json!([{
                "url": "https://cdn.podcast.test/episode-1.mp3?token=a&b=2",
                "mime_type": "audio/mpeg",
                "size_in_bytes": 12_345,
                "duration_in_seconds": 1_671,
            }])
        );
        let json: Value =
            serde_json::from_str(&json_collection(&site(), "Example", "", &[item()]).unwrap())
                .unwrap();
        assert!(json["items"][0].get("attachments").is_none());
    }

    #[test]
    fn feeds_drop_blank_authors_and_name_the_site_as_feed_author() {
        let mut anonymous = item();
        anonymous.authors = vec!["   ".into()];
        anonymous.source_name = " ".into();
        let atom = atom_feed(&site(), &build(), &[podcast(), anonymous.clone()]);
        well_formed(&atom);
        assert!(atom.contains("  <author>\n    <name>Example &amp; Reader</name>\n  </author>\n"));
        assert!(!atom.contains("<name></name>"));
        assert!(!atom.contains("<name>   </name>"));
        let feed = feed_rs::parser::parse(atom.as_bytes()).unwrap();
        assert_eq!(
            feed.entries[0]
                .authors
                .iter()
                .map(|author| author.name.as_str())
                .collect::<Vec<_>>(),
            ["Host"]
        );
        assert!(feed.entries[1].authors.is_empty());

        let rss = rss_collection(&site(), &build(), "Example", "", &[podcast(), anonymous]);
        well_formed(&rss);
        assert!(rss.contains("xmlns:dc=\"http://purl.org/dc/elements/1.1/\""));
        assert_eq!(rss.matches("<dc:creator>").count(), 1);
        assert!(rss.contains("<dc:creator>Host</dc:creator>"));

        let json: Value =
            serde_json::from_str(&json_collection(&site(), "Example", "", &[podcast()]).unwrap())
                .unwrap();
        assert_eq!(
            json["items"][0]["authors"],
            serde_json::json!([{"name": "Host"}])
        );
    }

    #[test]
    fn sources_opml_round_trips_through_the_subscription_importer() {
        let opml = sources_opml(&site(), at(12), &sources());
        well_formed(&opml);
        assert!(opml.starts_with(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<opml version=\"2.0\">\n  <head>\n    <title>Example &amp; Reader</title>\n    <dateCreated>Wed, 2 Sep 2026 12:30:00 +0000</dateCreated>\n  </head>\n"
        ));
        assert!(opml.contains("htmlUrl=\"https://blog.rust-lang.org/\""));
        assert!(
            !opml.contains("github.com/owner/reader"),
            "git mirrors are not feeds"
        );
        assert!(
            !opml.contains("Retained only"),
            "sources without a URL are skipped"
        );

        let imported = crate::config::Config::parse_source_document(
            opml.as_bytes(),
            &url::Url::parse("https://example.test/reads/sources.opml").unwrap(),
        )
        .unwrap();
        assert_eq!(
            imported
                .iter()
                .map(|source| {
                    (
                        source.url.as_deref().unwrap(),
                        source.name.as_deref().unwrap(),
                        source.category.as_deref(),
                    )
                })
                .collect::<Vec<_>>(),
            [
                (
                    "https://blog.rust-lang.org/feed.xml",
                    "Rust & Friends",
                    Some("Engineering")
                ),
                (
                    "https://feeds.podcast.test/show?token=abc",
                    "A \"Podcast\"",
                    None
                ),
            ]
        );
    }

    #[test]
    fn llms_inventory_and_descriptor_advertise_the_subscription_list() {
        assert!(
            llms_txt(&site())
                .contains("- [OPML subscriptions](https://example.test/reads/sources.opml)\n")
        );
        let mut portable = site();
        portable.base_url = None;
        assert!(llms_txt(&portable).contains("- [OPML subscriptions](sources.opml)\n"));
    }

    #[test]
    fn entries_declare_a_language_only_when_it_differs_from_the_feed() {
        let site = site();
        let mut french = item();
        french.language = Some("fr".into());
        let mut same = podcast();
        same.language = Some("en-gb".into());
        let unknown = item();
        assert_eq!(entry_language(&french, &site), Some("fr"));
        assert_eq!(entry_language(&same, &site), None, "case-insensitive");
        assert_eq!(entry_language(&unknown, &site), None);

        let items = [french, same, unknown];
        let atom = atom_collection(&site, &build(), "All", "", &items);
        well_formed(&atom);
        assert_eq!(atom.matches("<entry xml:lang=\"fr\">").count(), 1, "{atom}");
        assert_eq!(atom.matches("<entry>").count(), 2, "{atom}");
        // `xml:lang` is inherited by every child of the entry (RFC 4287 §2); feed-rs only reads it
        // from `<content>`, so the entry attribute is checked on the serialized document above.
        let parsed = feed_rs::parser::parse(atom.as_bytes()).unwrap();
        assert_eq!(parsed.language.as_deref(), Some("en-GB"));
        assert_eq!(parsed.entries.len(), 3);

        let json: serde_json::Value =
            serde_json::from_str(&json_collection(&site, "All", "", &items).unwrap()).unwrap();
        assert_eq!(json["language"], "en-GB");
        assert_eq!(json["items"][0]["language"], "fr");
        assert!(json["items"][1].get("language").is_none());
        assert!(json["items"][2].get("language").is_none());
    }

    #[test]
    fn atom_golden() {
        insta::assert_snapshot!("atom", atom_feed(&site(), &build(), &[item(), podcast()]));
    }

    #[test]
    fn rss_golden() {
        let site = site();
        insta::assert_snapshot!(
            "rss",
            rss_collection(&site, &build(), &site.title, "", &[item(), podcast()])
        );
    }

    #[test]
    fn json_feed_golden() {
        let site = site();
        insta::assert_snapshot!(
            "json_feed",
            json_collection(&site, &site.title, "", &[item(), podcast()]).unwrap()
        );
    }

    #[test]
    fn sitemap_golden() {
        let output = sitemap(
            &[
                SitemapUrl::new("https://example.test/reads/", Some(at(9))),
                SitemapUrl::new(
                    "https://example.test/reads/items/source/a-story/",
                    Some(at(9)),
                ),
                SitemapUrl::new("https://example.test/reads/browse/", None),
            ],
            "https://example.test/reads/sitemap.xml",
            SitemapLimits::default(),
        )
        .unwrap();
        insta::assert_snapshot!("sitemap", output.root_xml());
    }

    #[test]
    fn sources_opml_golden() {
        insta::assert_snapshot!("sources_opml", sources_opml(&site(), at(12), &sources()));
    }

    #[test]
    fn llms_txt_golden() {
        insta::assert_snapshot!("llms_txt", llms_txt(&site()));
    }

    #[test]
    fn aggr_json_golden() {
        let mut site = site();
        site.repository = Some("owner/reader".into());
        site.config_url =
            Some("https://raw.githubusercontent.com/owner/reader/deadbeef/aggr.toml".into());
        insta::assert_snapshot!(
            "aggr_json",
            instance_descriptor(&site, &build(), at(9)).unwrap()
        );
    }
}
