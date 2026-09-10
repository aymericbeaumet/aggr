//! Static site generation: the data tree + config + git facts in, a directory of files out.
//! The planning half (`plan`) is pure; `build` does the IO.

pub mod context;
mod display;
mod document;
pub(crate) mod interactive;
pub(crate) mod item_type;
mod native_media;
pub mod outputs;
mod pagefind;
mod parallel;
mod related;
pub mod render;
mod video;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use sha1::{Digest as _, Sha1};

use crate::config::{Config, SiteConfig, SiteIdentityKind, Source};
use crate::content;
use crate::model::Item;
use crate::store::{Status, Store};
use context::{
    ArticleLinkCtx, ArticlePreviewCtx, BuildCtx, CategoryCtx, GitHubLinks, ItemCtx, ItemOptions,
    PageCtx, PaginatorCtx, PreviewCtx, SiteCtx, SiteIdentityCtx, SourceCtx, SourceErrorCtx,
};
use render::{Layers, Renderer};

const EXCERPT_CHARS: usize = 240;
const RECOMMENDATION_CARD_COUNT: usize = 1;
const MARKER: &str = ".aggr-site";

/// Pick one archive page per article without changing retained data or breaking old page URLs.
fn visible_archive(items: Vec<Item>, sources: &[Source]) -> (Vec<Item>, Vec<(String, String)>) {
    let publisher_hosts: BTreeMap<_, _> = sources
        .iter()
        .filter_map(|source| {
            source.public_url.as_deref().and_then(|url| {
                let host = context::domain_of(url);
                (!host.is_empty()).then_some((source.slug.as_str(), host))
            })
        })
        .collect();
    let is_publisher = |item: &Item| {
        publisher_hosts
            .get(item.front.source.as_str())
            .is_some_and(|host| *host == context::domain_of(&item.front.link))
    };
    let mut groups = BTreeMap::<String, Vec<Item>>::new();
    let mut visible = Vec::new();
    for item in items {
        if item.front.hidden
            || url::Url::parse(&item.front.link)
                .is_ok_and(|url| crate::sources::youtube::is_short_url(&url))
        {
            continue;
        }
        let key = crate::model::normalize_link(&item.front.link);
        if key.is_empty() {
            visible.push(item);
        } else {
            groups.entry(key).or_default().push(item);
        }
    }
    let mut redirects = Vec::new();
    for mut group in groups.into_values() {
        group.sort_by(|a, b| {
            is_publisher(b)
                .cmp(&is_publisher(a))
                .then_with(|| b.body.len().cmp(&a.body.len()))
                .then_with(|| a.path.cmp(&b.path))
        });
        let mut group = group.into_iter();
        let winner = group.next().expect("nonempty article group");
        let target = context::item_url(&winner.path);
        for duplicate in group {
            let previous = context::item_url(&duplicate.path);
            if previous != target {
                redirects.push((previous, target.clone()));
            }
        }
        visible.push(winner);
    }
    (visible, redirects)
}

/// Facts about the build that do not come from the data tree.
pub struct BuildInfo {
    pub out: PathBuf,
    pub base_url: Option<String>,
    pub config_sha: Option<String>,
    /// Repository-relative path of the root config file.
    pub config_path: Option<String>,
    pub data_sha: Option<String>,
    pub generation: String,
    pub now: DateTime<Utc>,
    /// Production build: absolute URLs from `base_url`, CNAME for custom domains.
    pub release: bool,
    pub discussions: crate::discussions::ResolutionSet,
    /// Search-index cache shared across template-only rebuilds.
    pub pagefind_cache: Option<PathBuf>,
}

/// Which items belong on the recent home feed; archives render every retained item.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Window {
    pub rendered: Vec<usize>,
}

/// Split items (sorted newest first, hidden already removed) into the recent feed and its tail.
pub fn window(
    dates: &[DateTime<Utc>],
    now: DateTime<Utc>,
    max_items: usize,
    max_age_days: u32,
) -> Window {
    let cutoff = now - Duration::days(i64::from(max_age_days));
    let mut out = Window::default();
    for (index, date) in dates.iter().enumerate() {
        if out.rendered.len() < max_items && *date >= cutoff {
            out.rendered.push(index);
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pager {
    pub path: String,
    pub range: std::ops::Range<usize>,
    pub context: PaginatorCtx,
}

fn navigation_path(path: &str) -> String {
    if path.is_empty() {
        "./".to_string()
    } else {
        path.to_string()
    }
}

fn canonical_url(site: &SiteCtx, path: &str) -> Option<String> {
    site.base_url
        .as_ref()
        .map(|root| format!("{root}{}", path.trim_start_matches('/')))
}

/// Zola-style pagers for `total` entries: the first lives at `prefix`, then `page/N/`.
pub fn paginate(prefix: &str, total: usize, per_page: usize) -> Vec<Pager> {
    assert!(per_page > 0, "paginate_by must be positive");
    let number_pagers = total.div_ceil(per_page).max(1);
    let paths: Vec<_> = (1..=number_pagers)
        .map(|current_index| {
            if current_index == 1 {
                prefix.to_string()
            } else {
                format!("{prefix}page/{current_index}/")
            }
        })
        .collect();
    let first = navigation_path(&paths[0]);
    let last = navigation_path(paths.last().expect("at least one pager"));
    paths
        .iter()
        .enumerate()
        .map(|(index, path)| {
            let current_index = index + 1;
            let start = index * per_page;
            Pager {
                path: path.clone(),
                range: start..(start + per_page).min(total),
                context: PaginatorCtx {
                    paginate_by: per_page,
                    base_url: format!("{prefix}page/"),
                    number_pagers,
                    first: first.clone(),
                    last: last.clone(),
                    previous: index
                        .checked_sub(1)
                        .map(|previous| navigation_path(&paths[previous])),
                    next: paths.get(index + 1).map(|next| navigation_path(next)),
                    current_index,
                    total_items: total,
                    offset: start,
                },
            }
        })
        .collect()
}

/// Everything the service worker fetches at install, as site paths under `base`: shells, list
/// indexes, the assets needed to render them, the Pagefind bundle, and newest offline items.
pub fn precache_paths(
    base: &str,
    lists: impl IntoIterator<Item = String>,
    assets: &[String],
    item_urls: impl IntoIterator<Item = String>,
    offline_items: usize,
) -> Vec<String> {
    const SHELLS: [&str; 11] = [
        "",
        "aggr.json",
        "updates.json",
        "browse/",
        "categories/",
        "sources/",
        "tags/",
        "preferences/",
        "404.html",
        "offline.html",
        "manifest.webmanifest",
    ];
    let mut seen = BTreeSet::new();
    SHELLS
        .iter()
        .map(|path| path.to_string())
        .chain(lists)
        .chain(
            assets
                .iter()
                .filter(|name| {
                    name.ends_with(".css")
                        || name.ends_with(".js")
                        || name.starts_with("favicon-")
                        || name.starts_with("icon-")
                        || name.starts_with("apple-touch-icon-")
                })
                .map(|name| format!("assets/{name}")),
        )
        .chain(item_urls.into_iter().take(offline_items))
        .map(|path| format!("{base}{path}"))
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct PrecacheEntry {
    url: String,
    revision: String,
    required: bool,
}

/// Attach an exact content revision to every install-time resource. A new worker can copy
/// byte-identical responses from the previous precache instead of downloading the whole offline
/// archive after every feed commit.
fn precache_entries(root: &Path, urls: Vec<String>) -> Result<Vec<PrecacheEntry>> {
    urls.into_iter()
        .map(|url| {
            let relative = url.trim_start_matches('/');
            if relative.split('/').any(|part| part == "..") {
                bail!("invalid precache path {url:?}");
            }
            let file = if relative.is_empty() {
                root.join("index.html")
            } else if relative.ends_with('/') {
                root.join(relative).join("index.html")
            } else {
                root.join(relative)
            };
            let bytes = std::fs::read(&file)
                .with_context(|| format!("reading precache resource {}", file.display()))?;
            let required = relative.is_empty()
                || relative == "offline.html"
                || (relative.starts_with("items/") && relative.ends_with('/'))
                || (relative.starts_with("assets/")
                    && (relative.ends_with(".css") || relative.ends_with(".js")));
            Ok(PrecacheEntry {
                url,
                revision: crate::model::sha1_hex(&bytes),
                required,
            })
        })
        .collect()
}

fn publish_article_images(
    out: &Path,
    assets: Vec<crate::media::Asset>,
    written: &mut BTreeSet<String>,
) -> Result<Vec<content::LocalImage>> {
    let mut published = Vec::with_capacity(assets.len());
    for asset in assets {
        let mut publish = |extension: &str, hash: &str, bytes: &[u8]| -> Result<String> {
            let path = format!("assets/images/{hash}.{extension}");
            publish_asset(out, written, &path, bytes)?;
            Ok(path)
        };
        let original = publish(
            asset.master_extension,
            &asset.master_hash,
            &asset.master_bytes,
        )?;
        let mut variants = asset
            .renditions
            .iter()
            .map(|rendition| {
                Ok(content::LocalImageVariant {
                    url: publish(rendition.extension, &rendition.hash, &rendition.bytes)?,
                    width: rendition.width,
                    height: rendition.height,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        if asset.master_extension == "webp"
            && !variants.is_empty()
            && variants.last().map(|variant| variant.width) != Some(asset.width)
        {
            variants.push(content::LocalImageVariant {
                url: original.clone(),
                width: asset.width,
                height: asset.height,
            });
        }
        published.push(content::LocalImage {
            source: asset.source_url,
            original,
            variants,
            width: asset.width,
            height: asset.height,
            color: asset.dominant_color,
            placeholder: asset.placeholder,
        });
    }
    Ok(published)
}

fn preview_placeholder(
    bytes: &[u8],
    cache: &mut BTreeMap<String, crate::media::placeholder::Placeholder>,
) -> Result<crate::media::placeholder::Placeholder> {
    let hash = crate::model::sha1_hex(bytes);
    if let Some(placeholder) = cache.get(&hash) {
        return Ok(placeholder.clone());
    }
    let placeholder = crate::media::placeholder::from_bytes(bytes)?;
    cache.insert(hash, placeholder.clone());
    Ok(placeholder)
}

fn publish_asset(
    out: &Path,
    written: &mut BTreeSet<String>,
    path: &str,
    bytes: &[u8],
) -> Result<()> {
    if !written.contains(path) {
        write(&out.join(path), bytes)?;
        written.insert(path.to_string());
    }
    Ok(())
}

/// Names the precache from durable build inputs. A no-op rebuild therefore reuses its cache,
/// while a changed worker refreshes the same keys during installation.
pub fn cache_version(build: &BuildCtx) -> String {
    fn short(sha: Option<&str>) -> &str {
        sha.map_or("local", |sha| &sha[..sha.len().min(12)])
    }
    format!(
        "{}-{}-{}-{}",
        build.version,
        short(build.data_sha.as_deref()),
        short(build.config_sha.as_deref()),
        short(Some(&build.generation))
    )
}

/// Reader releases depend on shipped code, never on values substituted while rendering it.
fn app_version(binary_version: &str, layers: &Layers) -> Result<String> {
    fn field(hash: &mut Sha1, bytes: &[u8]) {
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    let mut hash = Sha1::new();
    field(&mut hash, b"aggr-app-v1");
    field(&mut hash, binary_version.as_bytes());
    for (kind, names) in [
        ("templates", layers.template_names()?),
        ("static", layers.static_names()?),
    ] {
        for name in names {
            let bytes = layers
                .read(kind, &name)?
                .with_context(|| format!("{kind}/{name} vanished during build"))?;
            field(&mut hash, format!("{kind}/{name}").as_bytes());
            field(&mut hash, &bytes);
        }
    }
    Ok(hex::encode(hash.finalize()))
}

/// Fingerprint of data that can affect rendered output, including age bands and the home-feed
/// cutoff. It changes only at a semantic boundary, so repeated builds stay instant while an
/// unchanged repository can never keep a stale 24-hour marker or expired river entry.
pub fn render_generation(items: &[Item], site: &SiteConfig, now: DateTime<Utc>) -> String {
    fn field(hash: &mut Sha1, bytes: &[u8]) {
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    let cutoff = now - Duration::days(i64::from(site.max_age_days));
    let mut ordered: Vec<_> = items.iter().filter(|item| !item.front.hidden).collect();
    ordered.sort_by(|a, b| a.path.cmp(&b.path));
    let mut hash = Sha1::new();
    for item in ordered {
        field(&mut hash, item.path.as_bytes());
        field(
            &mut hash,
            &serde_json::to_vec(&item.front).unwrap_or_default(),
        );
        field(&mut hash, item.body.as_bytes());
        field(
            &mut hash,
            context::age_band(now, item.created_at()).as_bytes(),
        );
        field(&mut hash, &[u8::from(item.created_at() >= cutoff)]);
    }
    hex::encode(hash.finalize())
}

/// `https://user.github.io/repo/` → `/repo/`; anything unparsable → `/`.
pub fn base_path(base_url: Option<&str>) -> String {
    let path = base_url
        .and_then(|url| url::Url::parse(url).ok())
        .map(|url| url.path().to_string())
        .unwrap_or_default();
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        "/".into()
    } else {
        format!("/{trimmed}/")
    }
}

/// Relative reference from a generated page back to the output root. Directory routes end in
/// `/`; standalone root files such as `offline.html` do not add a level.
pub fn relative_root(path: &str) -> String {
    let depth = path
        .trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .count()
        .saturating_sub(usize::from(!path.is_empty() && !path.ends_with('/')));
    if depth == 0 {
        "./".into()
    } else {
        "../".repeat(depth)
    }
}

#[derive(Serialize)]
struct SharedCtx {
    site: minijinja::Value,
    build: minijinja::Value,
    sources: minijinja::Value,
    categories: minijinja::Value,
    tags: minijinja::Value,
}

#[derive(Serialize)]
struct Ctx<'a> {
    #[serde(flatten)]
    shared: &'a SharedCtx,
    page: PageCtx,
    items: minijinja::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    item: Option<&'a ItemCtx>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<&'a SourceCtx>,
    #[serde(skip_serializing_if = "Option::is_none")]
    category: Option<&'a CategoryCtx>,
    #[serde(skip_serializing_if = "Option::is_none")]
    html: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    schema: Option<serde_json::Value>,
}

fn default_site_description(title: &str) -> String {
    format!(
        "Browse {title}, an independent, searchable archive of readable snapshots from followed feeds, preserved in Git with aggr."
    )
}

fn document_title(site_title: &str, page_title: &str, kind: &str, page_number: usize) -> String {
    let page_title = if matches!(kind, "item" | "source") {
        page_title.to_string()
    } else {
        page_title.to_lowercase()
    };
    match (kind, page_number) {
        ("river", 1) => format!("{site_title} | aggr"),
        ("river", page) => format!("{site_title} — page {page} | aggr"),
        (_, 1) => format!("{page_title} | {site_title} | aggr"),
        (_, page) => format!("{page_title} — page {page} | {site_title} | aggr"),
    }
}

fn item_description(site: &SiteCtx, item: &ItemCtx) -> String {
    let source = if item.domain.is_empty() {
        item.source_name.as_str()
    } else {
        item.domain.as_str()
    };
    let context = format!(
        "Archived readable snapshot of {} from {source}, first captured {} and preserved by {}.",
        item.title,
        item.first_seen.format("%Y-%m-%d"),
        site.title
    );
    if item.excerpt.is_empty() {
        context
    } else {
        content::excerpt(&format!("{context} {}", item.excerpt), 220)
    }
}

/// Sitemap freshness belongs to the local archive page. A newly captured old article must not
/// look stale merely because its upstream publication date is old.
fn archive_modified_at(item: &ItemCtx) -> DateTime<Utc> {
    item.published
        .into_iter()
        .chain(item.updated)
        .chain(std::iter::once(item.first_seen))
        .chain(item.replicated_at)
        .max()
        .unwrap_or(item.first_seen)
}

fn page_description(site: &SiteCtx, page_title: &str, kind: &str, page_number: usize) -> String {
    let description = match kind {
        "river" => site.description.clone(),
        "source" => format!(
            "Browse retained readable snapshots from {page_title} in {}, with original URLs and capture dates.",
            site.title
        ),
        "category" => format!(
            "Browse retained readable snapshots in the {page_title} category of {}.",
            site.title
        ),
        "tag" => format!(
            "Browse retained readable snapshots tagged {page_title} in {}.",
            site.title
        ),
        "browse" => format!(
            "Browse the sources, categories, and tags collected in {}, with links to every retained archive.",
            site.title
        ),
        "search" => format!("Search the articles and links collected in {}.", site.title),
        _ => site.description.clone(),
    };
    if page_number > 1 {
        format!("{} Page {page_number}.", description.trim_end_matches('.'))
    } else {
        description
    }
}

/// Schema.org metadata describes the independent archive page and its upstream provenance, so it
/// is emitted only when the build has a stable public URL. Internal navigation stays portable.
fn structured_data(
    site: &SiteCtx,
    page: &PageCtx,
    items: &[ItemCtx],
    item: Option<&ItemCtx>,
) -> Option<serde_json::Value> {
    let site_url = site.base_url.as_deref()?;
    let page_url = page.canonical_url.as_deref()?;
    let website_id = format!("{site_url}#website");
    let descriptor_url = format!("{site_url}aggr.json");
    let config_url = site
        .config_url
        .clone()
        .unwrap_or_else(|| format!("{site_url}aggr.toml"));
    let identity_id = site
        .identity
        .as_ref()
        .map(|_| format!("{site_url}#identity"));
    let mut website = serde_json::json!({
        "@type": "WebSite",
        "@id": website_id,
        "additionalType": outputs::AGGR_INSTANCE_TYPE,
        "url": site_url,
        "name": site.title,
        "description": site.description,
        "inLanguage": site.language,
        "isPartOf": {"@id": outputs::AGGR_NETWORK},
        "isBasedOn": config_url,
        "subjectOf": {
            "@type": "DigitalDocument",
            "url": descriptor_url,
            "encodingFormat": "application/json"
        },
        "potentialAction": {
            "@type": "SearchAction",
            "target": {
                "@type": "EntryPoint",
                "urlTemplate": format!("{site_url}?q={{search_term_string}}")
            },
            "query-input": "required name=search_term_string"
        }
    });
    if let Some(identity_id) = &identity_id {
        website["creator"] = serde_json::json!({"@id": identity_id});
        website["publisher"] = serde_json::json!({"@id": identity_id});
    }

    let (page_node, original_node) = if let Some(item) = item {
        let snapshot_id = format!("{page_url}#webpage");
        let authors: Vec<_> = item
            .authors
            .iter()
            .map(|name| serde_json::json!({"@type": "Person", "name": name}))
            .collect();
        let mut original = serde_json::json!({
            "@type": "CreativeWork",
            "@id": item.link,
            "url": item.link,
            "headline": item.title,
            "description": item.excerpt,
            "inLanguage": site.language,
            "archivedAt": {"@id": snapshot_id},
            "keywords": item.labels,
        });
        if item.word_count > 0 {
            original["wordCount"] = serde_json::json!(item.word_count);
            original["timeRequired"] =
                serde_json::Value::String(format!("PT{}M", item.reading_minutes));
        }
        if let Some(published) = item.published {
            original["datePublished"] = serde_json::Value::String(published.to_rfc3339());
        }
        if let Some(updated) = item.updated {
            original["dateModified"] = serde_json::Value::String(updated.to_rfc3339());
        }
        if let Some(category) = &item.category {
            original["genre"] = serde_json::Value::String(category.clone());
        }
        if !authors.is_empty() {
            original["author"] = serde_json::Value::Array(authors);
        }
        if let Some(preview) = &item.preview {
            original["image"] = serde_json::json!({
                "@type": "ImageObject",
                "url": format!("{site_url}{}", preview.url),
                "width": preview.width,
                "height": preview.height,
                "caption": preview.alt,
            });
        }
        (
            serde_json::json!({
                "@type": ["WebPage", "ArchiveComponent"],
                "@id": snapshot_id,
                "url": page_url,
                "name": format!("Archived snapshot: {}", item.title),
                "description": page.description,
                "dateCreated": item.replicated_at.unwrap_or(item.first_seen).to_rfc3339(),
                "temporalCoverage": item.first_seen.to_rfc3339(),
                "inLanguage": site.language,
                "isPartOf": {"@id": website_id},
                "isBasedOn": {"@id": item.link},
                "mainEntity": {"@id": item.link},
            }),
            Some(original),
        )
    } else {
        let page_type = match page.kind.as_str() {
            "search" => "SearchResultsPage",
            "river" | "source" | "category" | "tag" | "browse" => "CollectionPage",
            _ => "WebPage",
        };
        let mut node = serde_json::json!({
            "@type": page_type,
            "@id": format!("{page_url}#webpage"),
            "url": page_url,
            "name": page.title,
            "description": page.description,
            "inLanguage": site.language,
            "isPartOf": {"@id": website_id},
        });
        if page.paginator.is_some() {
            let offset = page
                .paginator
                .as_ref()
                .map_or(0, |paginator| paginator.offset);
            let elements: Vec<_> = items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    let url = format!("{site_url}{}", item.url);
                    serde_json::json!({
                        "@type": "DataFeedItem",
                        "position": offset + index + 1,
                        "dateCreated": item.replicated_at.unwrap_or(item.first_seen).to_rfc3339(),
                        "item": {
                            "@type": ["WebPage", "ArchiveComponent"],
                            "@id": format!("{url}#webpage"),
                            "url": url,
                            "name": format!("Archived snapshot: {}", item.title),
                            "dateCreated": item.replicated_at.unwrap_or(item.first_seen).to_rfc3339(),
                            "temporalCoverage": item.first_seen.to_rfc3339(),
                            "isBasedOn": {"@id": item.link},
                        }
                    })
                })
                .collect();
            node["mainEntity"] = serde_json::json!({
                "@type": "DataFeed",
                "@id": format!("{page_url}#feed"),
                "name": page.title,
                "dataFeedElement": elements,
            });
        }
        (node, None)
    };

    let network = serde_json::json!({
        "@type": "CreativeWorkSeries",
        "@id": outputs::AGGR_NETWORK,
        "name": "aggr network",
        "url": outputs::AGGR_REPOSITORY,
    });

    let identity = site
        .identity
        .as_ref()
        .zip(identity_id)
        .map(|(identity, id)| {
            let mut node = serde_json::json!({
                "@type": identity.kind,
                "@id": id,
                "name": identity.name,
            });
            if let Some(url) = &identity.url {
                node["url"] = serde_json::Value::String(url.clone());
            }
            if !identity.same_as.is_empty() {
                node["sameAs"] = serde_json::json!(identity.same_as);
            }
            node
        });

    let mut graph = vec![network];
    if let Some(identity) = identity {
        graph.push(identity);
    }
    graph.push(website);
    graph.push(page_node);
    if let Some(original_node) = original_node {
        graph.push(original_node);
    }

    Some(serde_json::json!({
        "@context": "https://schema.org",
        "@graph": graph,
    }))
}

#[derive(Serialize)]
struct OfflineArticle {
    url: String,
    title: String,
    resources: Vec<PrecacheEntry>,
}

fn offline_catalog(
    out: &Path,
    items: &[ItemCtx],
    images: &BTreeMap<String, Vec<content::LocalImage>>,
) -> Result<Vec<OfflineArticle>> {
    let mut revisions = BTreeMap::<String, PrecacheEntry>::new();
    items
        .iter()
        .take(1000)
        .map(|item| {
            let mut paths = BTreeSet::from([item.url.clone()]);
            if let Some(preview) = &item.preview {
                paths.insert(preview.url.clone());
            }
            for image in images.get(&item.path).into_iter().flatten() {
                paths.insert(image.original.clone());
                paths.extend(image.variants.iter().map(|variant| variant.url.clone()));
            }
            let resources = paths
                .into_iter()
                .map(|path| {
                    if let Some(entry) = revisions.get(&path) {
                        return Ok(entry.clone());
                    }
                    let entry = precache_entries(out, vec![path.clone()])?
                        .pop()
                        .context("offline resource revision")?;
                    revisions.insert(path, entry.clone());
                    Ok(entry)
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(OfflineArticle {
                url: item.url.clone(),
                title: item.title.clone(),
                resources,
            })
        })
        .collect()
}

/// What `sw.js` sees: the cache name and the revisioned install-time fetch list.
#[derive(Serialize)]
struct SwCtx<'a> {
    site: &'a SiteCtx,
    build: &'a BuildCtx,
    version: String,
    app_version: &'a str,
    content_version: &'a str,
    precache: Vec<PrecacheEntry>,
    offline_catalog: Vec<OfflineArticle>,
    offline_count: usize,
    search_manifest: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub pages: usize,
    pub items: usize,
    pub stubs: usize,
}

pub fn build(
    config: &Config,
    sources: &[Source],
    store: &Store,
    project_root: &Path,
    info: &BuildInfo,
) -> Result<Summary> {
    let started = std::time::Instant::now();
    let mut phase_started = started;
    let mut phase = |name: &str| {
        let now = std::time::Instant::now();
        log::debug!(
            "build {name}: {:.3}s",
            now.duration_since(phase_started).as_secs_f64()
        );
        phase_started = now;
    };
    let out = &info.out;
    prepare_out_dir(out)?;
    let config_name = info
        .config_path
        .as_deref()
        .and_then(|path| Path::new(path).file_name())
        .unwrap_or_else(|| std::ffi::OsStr::new("aggr.toml"));
    let config_source = project_root.join(config_name);
    if config_source.is_file() {
        write(&out.join("aggr.toml"), &std::fs::read(&config_source)?)?;
    }

    let base = base_path(info.base_url.as_deref());
    let repository = config.repository();
    let config_path = info.config_path.as_deref().unwrap_or("aggr.toml");
    let description = config
        .site
        .description
        .clone()
        .unwrap_or_else(|| default_site_description(&config.site.title));
    let discussion_shortcuts = context::discussion_shortcuts(&config.networks);
    let mut site = SiteCtx {
        preferences: config.site.preferences.browser_defaults()?,
        title: config.site.title.clone(),
        description,
        identity: config
            .site
            .identity
            .as_ref()
            .map(|identity| SiteIdentityCtx {
                kind: match identity.kind {
                    SiteIdentityKind::Person => "Person",
                    SiteIdentityKind::Organization => "Organization",
                },
                name: identity.name.clone(),
                url: identity.url.as_ref().map(ToString::to_string),
                same_as: identity.same_as.iter().map(ToString::to_string).collect(),
            }),
        language: config.site.language.clone(),
        base_path: base.clone(),
        base_url: info.base_url.clone().map(|url| ensure_trailing_slash(&url)),
        repository: repository.clone(),
        data_branch: config.store.branch.clone(),
        network_url: outputs::AGGR_NETWORK,
        instance_type_url: outputs::AGGR_INSTANCE_TYPE,
        pwa: config.site.pwa,
        config_page_url: repository.as_deref().and_then(|repository| {
            info.config_sha
                .as_deref()
                .map(|sha| format!("https://github.com/{repository}/blob/{sha}/{config_path}"))
        }),
        config_url: repository.as_deref().and_then(|repository| {
            info.config_sha.as_deref().map(|sha| {
                format!(
                    "https://raw.githubusercontent.com/{repository}/{sha}/{}",
                    config_path
                )
            })
        }),
        has_categories: false,
        discussions: config
            .networks
            .iter()
            .zip(discussion_shortcuts)
            .map(|(d, shortcut)| context::DiscussionLinkCtx {
                name: context::compact_name(&d.name),
                url: d.url.clone(),
                shortcut,
                found: false,
                score: None,
            })
            .collect(),
        entry_shortcuts: Vec::new(),
        params: config.site.params.clone(),
    };
    let layers = theme_layers(config, project_root)?;
    let mut build_ctx = BuildCtx {
        time: info.now,
        version: env!("CARGO_PKG_VERSION").to_string(),
        app_version: app_version(env!("CARGO_PKG_VERSION"), &layers)?,
        content_version: String::new(),
        config_sha: info.config_sha.clone(),
        data_sha: info.data_sha.clone(),
        generation: info.generation.clone(),
        release: info.release,
    };
    let links = repository.as_deref().map(|repository| GitHubLinks {
        repository,
        branch: &config.store.branch,
        data_sha: info.data_sha.as_deref(),
    });

    let renderer = Renderer::new(layers, &base)?;

    // Sources: config order, enriched with stored state and counts.
    let status = store.status()?;
    let (mut all_items, duplicate_redirects) = visible_archive(store.items()?, sources);
    for item in &mut all_items {
        item.body =
            content::strip_article_metadata(&item.body, item.front.published, &item.front.source);
    }
    all_items.sort_by(|a, b| {
        b.created_at()
            .cmp(&a.created_at())
            .then_with(|| b.path.cmp(&a.path))
    });
    let source_ctxs = source_contexts(sources, store, &status, &all_items)?;
    let source_by_slug: BTreeMap<&str, &SourceCtx> =
        source_ctxs.iter().map(|s| (s.slug.as_str(), s)).collect();

    let dates: Vec<_> = all_items.iter().map(Item::created_at).collect();
    let window = window(
        &dates,
        info.now,
        config.site.max_items,
        config.site.max_age_days,
    );
    phase("archive preparation");

    // The bounded window controls only the river. Source/category/tag pages, search, and clean
    // article pages are archives over the retained database; `[store]` retention is the explicit
    // knob for bounding those. This keeps old sources browsable without making the home feed stale.
    let prepared_bodies = parallel::map(&all_items, |item| {
        Ok(content::PreparedMarkdown::new(&item.body))
    })?;
    let prepared_by_path: BTreeMap<_, _> = all_items
        .iter()
        .zip(&prepared_bodies)
        .map(|(item, prepared)| (item.path.as_str(), prepared))
        .collect();
    let mut archive_items = Vec::with_capacity(all_items.len());
    let mut article_images = BTreeMap::<String, Vec<content::LocalImage>>::new();
    let mut media_previews = BTreeMap::new();
    let mut image_placeholders = BTreeMap::new();
    let mut written_assets = BTreeSet::new();
    for (item, prepared) in all_items.iter().zip(&prepared_bodies) {
        let (source_name, category) = source_by_slug
            .get(item.front.source.as_str())
            .map(|s| (s.name.as_str(), s.category.as_deref()))
            .unwrap_or((item.front.source.as_str(), None));
        let excerpt = item
            .front
            .summary
            .clone()
            .filter(|_| prepared.resources().is_empty())
            .map(|s| content::excerpt(&s, EXCERPT_CHARS))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| prepared.excerpt(EXCERPT_CHARS));
        let mut ctx = ItemCtx::from_item(
            item,
            ItemOptions {
                reading_metrics: prepared.reading_metrics(),
                source_name,
                category,
                links: links.as_ref(),
                excerpt,
                discussions: &config.networks,
                resolutions: &info.discussions,
                now: info.now,
            },
        );
        if let Some(source) = source_by_slug.get(item.front.source.as_str()) {
            ctx.set_source(source);
        }
        ctx.resources = prepared.resources().to_vec();
        if let Some(preview) = &item.front.preview
            && let Some(bytes) = store.read_preview(item)?
        {
            let extension = Path::new(&preview.file)
                .extension()
                .and_then(|value| value.to_str())
                .context("validated preview has a file extension")?;
            let path = format!(
                "assets/previews/{}.{}",
                crate::model::sha1_hex(&bytes),
                extension
            );
            publish_asset(out, &mut written_assets, &path, &bytes)?;
            ctx.preview = Some(PreviewCtx {
                url: path,
                width: preview.width,
                height: preview.height,
                alt: preview.alt.clone(),
                color: preview.color.clone(),
                placeholder: preview_placeholder(&bytes, &mut image_placeholders)?,
            });
        }
        let assets = store.read_image_assets(item)?;
        let previews_enabled = sources
            .iter()
            .find(|source| source.slug == item.front.source)
            .map_or(config.fetch.previews, |source| source.previews);
        if ctx.preview.is_none() && previews_enabled {
            for asset in assets
                .iter()
                .filter(|asset| !crate::media::is_status_badge(&asset.source_url))
                .take(12)
            {
                let retained = asset
                    .renditions
                    .iter()
                    .filter(|image| image.width >= 256 && image.height >= 32)
                    .map(|image| image.bytes.as_slice())
                    .chain(std::iter::once(asset.master_bytes.as_slice()));
                for bytes in retained.filter(|bytes| bytes.len() <= 5 * 1024 * 1024) {
                    let key = crate::model::sha1_hex(bytes);
                    let preview = media_previews.get(&key).cloned().unwrap_or_else(|| {
                        let preview = crate::preview::thumbnail(bytes, None).ok();
                        if media_previews.len() < 64 {
                            media_previews.insert(key, preview.clone());
                        }
                        preview
                    });
                    if let Some(preview) = preview {
                        let path = format!(
                            "assets/previews/{}.{}",
                            crate::model::sha1_hex(&preview.bytes),
                            preview.extension
                        );
                        publish_asset(out, &mut written_assets, &path, &preview.bytes)?;
                        ctx.preview = Some(PreviewCtx {
                            url: path,
                            width: preview.width,
                            height: preview.height,
                            alt: asset.alt.clone(),
                            color: Some(preview.color),
                            placeholder: preview_placeholder(
                                &preview.bytes,
                                &mut image_placeholders,
                            )?,
                        });
                        break;
                    }
                }
                if ctx.preview.is_some() {
                    break;
                }
            }
        }
        let local_images = publish_article_images(out, assets, &mut written_assets)?;
        if !local_images.is_empty() {
            article_images.insert(item.path.clone(), local_images);
        }
        archive_items.push(ctx);
    }
    phase("media and item metadata");

    let recommendation_text: Vec<_> = prepared_bodies
        .iter()
        .map(content::PreparedMarkdown::plain_text)
        .collect();
    let recommendations = related::resolve(&archive_items, &recommendation_text);
    let article_links: Vec<_> = archive_items.iter().map(ArticleLinkCtx::from).collect();
    for (item, recommendation) in archive_items.iter_mut().zip(recommendations) {
        let previous = recommendation.previous;
        let next = recommendation.next;
        item.previous_article = previous.map(|index| article_links[index].clone());
        item.next_article = next.map(|index| article_links[index].clone());
        item.recommended_articles = recommendation
            .articles
            .into_iter()
            .filter(|index| Some(*index) != previous && Some(*index) != next)
            .take(RECOMMENDATION_CARD_COUNT)
            .map(|index| article_links[index].clone())
            .collect();
    }

    let river_items: Vec<ItemCtx> = window
        .rendered
        .iter()
        .map(|&index| archive_items[index].clone())
        .collect();
    site.entry_shortcuts = river_items
        .iter()
        .take(9)
        .map(|item| item.url.clone())
        .collect();
    let source_members = source_members(&archive_items);
    let TaxonomyIndex {
        terms: categories,
        members: category_members,
    } = taxonomy_index(&archive_items, Taxonomy::Categories);
    site.has_categories = !categories.is_empty();
    let TaxonomyIndex {
        terms: tags,
        members: tag_members,
    } = taxonomy_index(&archive_items, Taxonomy::Tags);
    let mut pages = 0;
    let per_page = config.site.items_per_page;
    build_ctx.content_version = crate::model::sha1_hex(serde_json::to_vec(&(
        &build_ctx.generation,
        &build_ctx.data_sha,
        &build_ctx.config_sha,
        &site,
        &source_ctxs,
        &archive_items,
        per_page,
        config.site.max_items,
        config.site.max_age_days,
    ))?);
    write(
        &out.join("updates.json"),
        outputs::updates(&site, &build_ctx)?.as_bytes(),
    )?;
    // MiniJinja values retain their prepared representation across renders. Custom article
    // templates still receive the entire archive without serializing it again for every page.
    let template_shared = SharedCtx {
        site: minijinja::Value::from_serialize(&site),
        build: minijinja::Value::from_serialize(&build_ctx),
        sources: minijinja::Value::from_serialize(&source_ctxs),
        categories: minijinja::Value::from_serialize(&categories),
        tags: minijinja::Value::from_serialize(&tags),
    };
    let template_items = minijinja::Value::from_serialize(&archive_items);
    let mut sitemap_urls: Vec<outputs::SitemapUrl> = Vec::new();
    {
        let mut write_list = |kind: &str,
                              title: &str,
                              prefix: &str,
                              list: &[ItemCtx],
                              source: Option<&SourceCtx>,
                              category: Option<&CategoryCtx>|
         -> Result<()> {
            let pagination = paginate(prefix, list.len(), per_page);
            for pager in &pagination {
                let page_number = pager.context.current_index;
                let page = PageCtx {
                    kind: kind.to_string(),
                    title: title.to_string(),
                    document_title: document_title(&site.title, title, kind, page_number),
                    description: page_description(&site, title, kind, page_number),
                    indexable: true,
                    path: pager.path.clone(),
                    root: relative_root(&pager.path),
                    canonical_url: canonical_url(&site, &pager.path),
                    feed_path: Some(prefix.to_string()),
                    paginator: Some(pager.context.clone()),
                };
                let page_items = &list[pager.range.clone()];
                if let Some(url) = &page.canonical_url {
                    sitemap_urls.push(outputs::SitemapUrl::new(
                        url,
                        page_items.iter().map(archive_modified_at).max(),
                    ));
                }
                let schema = structured_data(&site, &page, page_items, None);
                let html = renderer.render(
                    "index.html",
                    Ctx {
                        shared: &template_shared,
                        page,
                        items: minijinja::Value::from_serialize(page_items),
                        item: None,
                        source,
                        category,
                        html: None,
                        schema,
                    },
                )?;
                write(&out.join(&pager.path).join("index.html"), html.as_bytes())?;
                pages += 1;
            }
            Ok(())
        };

        write_list("river", &config.site.title, "", &river_items, None, None)?;
        for source in &source_ctxs {
            let list = indexed_items(
                &archive_items,
                source_members.get(&source.slug).map(Vec::as_slice),
            );
            write_list(
                "source",
                &source.name,
                &source.page,
                &list,
                Some(source),
                None,
            )?;
            let feed_items = feed_items(&list, &prepared_by_path, per_page);
            write_collection_feeds(
                out,
                &site,
                &build_ctx,
                &source.name,
                &source.page,
                &feed_items,
            )?;
        }
        for category in &categories {
            let list = indexed_items(
                &archive_items,
                category_members.get(&category.slug).map(Vec::as_slice),
            );
            write_list(
                "category",
                &category.name,
                &category.page,
                &list,
                None,
                Some(category),
            )?;
            let feed_items = feed_items(&list, &prepared_by_path, per_page);
            write_collection_feeds(
                out,
                &site,
                &build_ctx,
                &category.name,
                &category.page,
                &feed_items,
            )?;
        }
        for tag in &tags {
            let list = indexed_items(
                &archive_items,
                tag_members.get(&tag.slug).map(Vec::as_slice),
            );
            write_list("tag", &tag.name, &tag.page, &list, None, Some(tag))?;
            let feed_items = feed_items(&list, &prepared_by_path, per_page);
            write_collection_feeds(out, &site, &build_ctx, &tag.name, &tag.page, &feed_items)?;
        }
    }

    let archive_updated = archive_items.iter().map(archive_modified_at).max();
    for path in ["browse/", "sources/", "categories/", "tags/"] {
        if let Some(url) = canonical_url(&site, path) {
            sitemap_urls.push(outputs::SitemapUrl::new(url, archive_updated));
        }
    }

    let simple = |kind: &str,
                  title: &str,
                  path: &str,
                  template: &str,
                  item: Option<&ItemCtx>,
                  html: Option<&str>,
                  page_items: Option<&[ItemCtx]>|
     -> Result<String> {
        let page_number = 1;
        let utility_fallback = matches!(kind, "404" | "offline");
        let page = PageCtx {
            kind: kind.to_string(),
            title: title.to_string(),
            document_title: document_title(&site.title, title, kind, page_number),
            description: item.map_or_else(
                || page_description(&site, title, kind, page_number),
                |item| item_description(&site, item),
            ),
            indexable: !matches!(kind, "search" | "preferences" | "404" | "offline"),
            path: path.to_string(),
            // These documents may be served for an arbitrarily deep failed navigation. An
            // explicit scoped root keeps every asset and menu link inside the installed app.
            root: if utility_fallback {
                site.base_path.clone()
            } else {
                relative_root(path)
            },
            canonical_url: (!utility_fallback)
                .then(|| canonical_url(&site, path))
                .flatten(),
            feed_path: None,
            paginator: None,
        };
        let items = page_items.unwrap_or(&archive_items);
        let schema = (!utility_fallback)
            .then(|| structured_data(&site, &page, items, item))
            .flatten();
        renderer.render(
            template,
            Ctx {
                shared: &template_shared,
                page,
                items: page_items
                    .map(minijinja::Value::from_serialize)
                    .unwrap_or_else(|| template_items.clone()),
                item,
                source: None,
                category: None,
                html,
                schema,
            },
        )
    };

    write(
        &out.join("browse/index.html"),
        simple(
            "browse",
            "Browse",
            "browse/",
            "browse.html",
            None,
            None,
            None,
        )?
        .as_bytes(),
    )?;
    for (kind, title) in [
        ("categories", "Categories"),
        ("sources", "Sources"),
        ("tags", "Tags"),
    ] {
        write(
            &out.join(kind).join("index.html"),
            simple(
                kind,
                title,
                &format!("{kind}/"),
                "browse.html",
                None,
                None,
                None,
            )?
            .as_bytes(),
        )?;
    }
    write(
        &out.join("preferences/index.html"),
        simple(
            "preferences",
            "Preferences",
            "preferences/",
            "preferences.html",
            None,
            None,
            None,
        )?
        .as_bytes(),
    )?;
    write(
        &out.join("404.html"),
        simple("404", "Not found", "404.html", "404.html", None, None, None)?.as_bytes(),
    )?;
    pages += 6;
    phase("feed and directory pages");

    let article_pairs: Vec<_> = archive_items.iter().zip(&all_items).collect();
    let article_sitemaps = parallel::map(&article_pairs, |&(ctx, item)| {
        let mut ctx = ctx.clone();
        ctx.video = video::VideoCtx::from_url(&item.front.link);
        ctx.interactive = interactive::InteractiveCtx::from_item(item);
        ctx.document = document::DocumentCtx::from_item(item);
        ctx.native_media = native_media::NativeMediaCtx::from_urls(
            &item.front.link,
            item.front
                .extra
                .get("audio_url")
                .and_then(|value| value.as_str()),
        );
        if ctx.is_youtube
            && let Ok(url) = url::Url::parse(&item.front.link)
            && let Some(image) = article_images.get(&item.path).and_then(|images| {
                let candidates = crate::sources::youtube::poster_candidates(&url);
                images.iter().find(|image| {
                    candidates
                        .iter()
                        .any(|candidate| candidate.url.as_str() == image.source)
                })
            })
        {
            ctx.article_preview = Some(ArticlePreviewCtx::from_image(image));
        }
        let prepared_markdown = prepared_by_path[ctx.path.as_str()];
        let portable_html = prepared_markdown.portable_html();
        if ctx.video.is_none()
            && ctx.document.is_none()
            && ctx.interactive.is_none()
            && let Ok(base) = url::Url::parse(&item.front.link)
        {
            ctx.article_preview = ArticlePreviewCtx::lead_image(
                portable_html,
                &base,
                article_images
                    .get(&item.path)
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
            );
        }
        let dimensions = if portable_html.contains("<img ") {
            let retained_html = store.read_html(item)?;
            let base = url::Url::parse(&item.front.link).ok();
            retained_html
                .as_deref()
                .map(|html| content::image_dimensions(html, base.as_ref()))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        ctx.body_html = Some(
            prepared_markdown.reader_html_with_images(
                article_images
                    .get(&item.path)
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
                &dimensions,
            ),
        );
        let dir = out.join(&ctx.url);
        let representation = out.join(ctx.url.trim_end_matches('/'));
        write(
            &dir.join("index.html"),
            simple(
                "item",
                &ctx.title,
                &ctx.url,
                "item.html",
                Some(&ctx),
                None,
                None,
            )?
            .as_bytes(),
        )?;
        // Alternate representations remain portable across hosts and mirrors. Only the rendered
        // archive page substitutes immutable site-local companions for publisher image URLs.
        ctx.body_html = Some(portable_html.to_string());
        let mut published_front = item.front.clone();
        published_front.title = ctx.title.clone();
        let markdown = crate::store::frontmatter::render(&published_front, &item.body)?;
        write(&representation.with_extension("md"), markdown.as_bytes())?;
        write(
            &representation.with_extension("txt"),
            outputs::text_item(&ctx).as_bytes(),
        )?;
        write(
            &representation.with_extension("rst"),
            outputs::rst_item(&ctx).as_bytes(),
        )?;
        write(
            &representation.with_extension("json"),
            outputs::item_json(&site, &build_ctx, &ctx, &item.body)?.as_bytes(),
        )?;
        Ok(canonical_url(&site, &ctx.url)
            .map(|url| outputs::SitemapUrl::new(url, Some(archive_modified_at(&ctx)))))
    })?;
    sitemap_urls.extend(article_sitemaps.into_iter().flatten());
    phase("article pages and representations");

    for (previous, target) in duplicate_redirects {
        let target = format!("{}{target}", site.base_path);
        write(
            &out.join(previous).join("index.html"),
            outputs::redirect_stub(&target).as_bytes(),
        )?;
    }

    let search_documents: Vec<_> = archive_items
        .iter()
        .zip(&prepared_bodies)
        .map(|(item, prepared)| pagefind::SearchDocument::new(item, prepared.plain_text()))
        .collect();

    let stubs = 4;

    let root_feed_items = feed_items(&river_items, &prepared_by_path, per_page);
    let atom = outputs::atom_feed(&site, &build_ctx, &root_feed_items);
    write(&out.join("feed.xml"), atom.as_bytes())?;
    write(&out.join("atom.xml"), atom.as_bytes())?;
    write(
        &out.join("rss.xml"),
        outputs::rss_collection(&site, &build_ctx, &site.title, "", &root_feed_items).as_bytes(),
    )?;
    write(
        &out.join("feed.json"),
        outputs::json_collection(&site, &site.title, "", &root_feed_items)?.as_bytes(),
    )?;
    write(
        &out.join("aggr.json"),
        outputs::instance_descriptor(&site, &build_ctx)?.as_bytes(),
    )?;
    write(&out.join("llms.txt"), outputs::llms_txt(&site).as_bytes())?;
    if let Some(root) = site.base_url.as_deref() {
        write(
            &out.join("linkset.json"),
            outputs::linkset_json(&site, &archive_items)?.as_bytes(),
        )?;
        let default_search_description = format!("Search {}", site.title);
        let search_description = if site.description.is_empty() {
            &default_search_description
        } else {
            &site.description
        };
        let search_url = format!("{root}?q={{searchTerms}}");
        let opensearch_url = format!("{root}opensearch.xml");
        let opensearch = outputs::opensearch_description(&outputs::OpenSearchDescription {
            short_name: &site.title,
            description: search_description,
            search_url: &search_url,
            self_url: Some(&opensearch_url),
        });
        write(&out.join("opensearch.xml"), opensearch.as_bytes())?;

        sitemap_urls.sort_by(|left, right| left.loc.cmp(&right.loc));
        sitemap_urls.dedup_by(|left, right| {
            if left.loc != right.loc {
                return false;
            }
            right.lastmod = left.lastmod.max(right.lastmod);
            true
        });
        let sitemap_url = format!("{root}sitemap.xml");
        let sitemap = outputs::sitemap(
            &sitemap_urls,
            &sitemap_url,
            outputs::SitemapLimits::default(),
        )?;
        write(&out.join("sitemap.xml"), sitemap.root_xml().as_bytes())?;
        for chunk in sitemap.chunks() {
            write(&out.join(&chunk.name), chunk.xml.as_bytes())?;
        }
        if site.base_path == "/" {
            write(
                &out.join("robots.txt"),
                format!("User-agent: *\nAllow: /\nSitemap: {sitemap_url}\n").as_bytes(),
            )?;
        }
    }
    write(&out.join(".nojekyll"), b"")?;
    write(&out.join(MARKER), env!("CARGO_PKG_VERSION").as_bytes())?;
    if let Some(domain) = cname(info) {
        write(&out.join("CNAME"), domain.as_bytes())?;
    }
    let assets = renderer.write_static(out)?;
    phase("feeds and static assets");

    let search_manifest = pagefind::build_cached(
        out,
        &search_documents,
        &site.language,
        info.pagefind_cache.as_deref(),
    )?;
    phase("search index");

    if config.site.pwa {
        let offline_items = &archive_items[..archive_items
            .len()
            .min(config.site.preferences.offline_items)];
        write(
            &out.join("offline.html"),
            simple(
                "offline",
                "Offline",
                "offline.html",
                "offline.html",
                None,
                None,
                Some(offline_items),
            )?
            .as_bytes(),
        )?;
        write(
            &out.join("manifest.webmanifest"),
            simple(
                "manifest",
                &config.site.title,
                "manifest.webmanifest",
                "manifest.webmanifest",
                None,
                None,
                None,
            )?
            .as_bytes(),
        )?;
        let scoped_lists = source_ctxs
            .iter()
            .map(|s| s.page.clone())
            .chain(categories.iter().map(|c| c.page.clone()))
            .chain(tags.iter().map(|tag| tag.page.clone()))
            .take(config.site.preferences.offline_items.clamp(32, 256));
        let lists = ["browse/".to_string()].into_iter().chain(scoped_lists);
        let paths = precache_paths("", lists, &assets, std::iter::empty(), 0);
        let mut sw_ctx = SwCtx {
            site: &site,
            build: &build_ctx,
            version: cache_version(&build_ctx),
            app_version: &build_ctx.app_version,
            content_version: &build_ctx.content_version,
            precache: precache_entries(out, paths)?,
            offline_catalog: offline_catalog(out, &archive_items, &article_images)?,
            offline_count: config.site.preferences.offline_items.min(1000),
            search_manifest: serde_json::json!({"version": search_manifest.version, "base": search_manifest.base}),
        };
        // Include the rendered worker and every resource revision so an installation never
        // deletes a live precache when only the theme or worker implementation changed.
        let worker = renderer.render("sw.js", &sw_ctx)?;
        sw_ctx.version = crate::model::sha1_hex(worker.as_bytes());
        let sw = renderer.render("sw.js", &sw_ctx)?;
        write(&out.join("sw.js"), sw.as_bytes())?;
        pages += 1;
    }

    phase("offline catalog and worker");
    log::debug!("build complete: {:.3}s", started.elapsed().as_secs_f64());
    Ok(Summary {
        pages,
        items: archive_items.len(),
        stubs,
    })
}

/// Template/static lookup order: the project's own `templates/`+`static/`, then the configured
/// theme directory, then the embedded default theme.
pub fn theme_layers(config: &Config, project_root: &Path) -> Result<Layers> {
    let mut layers = Layers::default();
    if project_root.join("templates").is_dir() || project_root.join("static").is_dir() {
        layers.dirs.push(project_root.to_path_buf());
    }
    match config.site.theme.as_str() {
        "default" => {
            // A development binary reads the shipped theme from its source tree so `aggr dev`
            // can rebuild template/CSS/JS edits without recompiling Rust. Release binaries stay
            // fully embedded and have no dependency on the build machine.
            #[cfg(debug_assertions)]
            {
                let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("themes/default");
                if source.is_dir() && !source.starts_with(project_root) {
                    layers.dirs.push(source);
                }
            }
        }
        theme => {
            let dir = project_root.join(theme);
            if !dir.is_dir() {
                bail!("theme {theme:?} is not a directory (git themes arrive in a later release)");
            }
            layers.dirs.push(dir);
        }
    }
    Ok(layers)
}

fn source_contexts(
    sources: &[Source],
    store: &Store,
    status: &Status,
    items: &[Item],
) -> Result<Vec<SourceCtx>> {
    let mut counts: BTreeMap<&str, (usize, Option<DateTime<Utc>>)> = BTreeMap::new();
    for item in items {
        let entry = counts.entry(item.front.source.as_str()).or_default();
        entry.0 += 1;
        entry.1 = entry.1.max(Some(item.created_at()));
    }
    let mut contexts = sources
        .iter()
        .map(|source| {
            let state = store.source_state(&source.slug)?;
            let (count, latest) = counts.remove(source.slug.as_str()).unwrap_or_default();
            let name = source
                .name
                .clone()
                .or_else(|| state.title.clone())
                .unwrap_or_else(|| source.slug.clone());
            let fallback = source
                .public_url
                .as_deref()
                .or(state.site_url.as_deref())
                .map(context::domain_of)
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "Unknown source".into());
            let name = display::title(&name, &fallback);
            Ok(SourceCtx {
                page: format!("sources/{}/", source.slug),
                slug: source.slug.clone(),
                name,
                url: source.public_url.clone(),
                site_url: state.site_url.clone(),
                category: source
                    .category
                    .as_deref()
                    .and_then(crate::model::normalize_category),
                engine: source.engine.name().to_string(),
                count,
                latest,
                error: status.errors.get(&source.slug).map(|error| SourceErrorCtx {
                    message: error.message.clone(),
                    since: error.since,
                }),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    for (slug, (count, latest)) in counts {
        let state = store.source_state(slug)?;
        let public_url = |value: Option<String>| {
            value
                .and_then(|value| url::Url::parse(&value).ok())
                .filter(|url| matches!(url.scheme(), "http" | "https"))
                .map(|url| crate::config::public_url(&url, false))
        };
        let site_url = public_url(state.site_url);
        let url = public_url(state.resolved_url).or_else(|| site_url.clone());
        let fallback = url
            .as_deref()
            .map(context::domain_of)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "Unknown source".into());
        let name = display::title(state.title.as_deref().unwrap_or(&fallback), &fallback);
        contexts.push(SourceCtx {
            page: format!("sources/{slug}/"),
            slug: slug.to_string(),
            name,
            url,
            site_url,
            category: None,
            engine: "web".into(),
            count,
            latest,
            error: None,
        });
    }
    contexts.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.slug.cmp(&b.slug))
    });
    Ok(contexts)
}

#[derive(Clone, Copy)]
enum Taxonomy {
    Categories,
    Tags,
}

struct TaxonomyIndex {
    terms: Vec<CategoryCtx>,
    members: BTreeMap<String, Vec<usize>>,
}

fn source_members(items: &[ItemCtx]) -> BTreeMap<String, Vec<usize>> {
    let mut members: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, item) in items.iter().enumerate() {
        members.entry(item.source.clone()).or_default().push(index);
    }
    members
}

fn taxonomy_index(items: &[ItemCtx], taxonomy: Taxonomy) -> TaxonomyIndex {
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    let mut members: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, item) in items.iter().enumerate() {
        let terms_for_item: Vec<&str> = match taxonomy {
            Taxonomy::Categories => item.category.as_deref().into_iter().collect(),
            Taxonomy::Tags => item.labels.iter().map(String::as_str).collect(),
        };
        let mut seen = std::collections::BTreeSet::new();
        for name in terms_for_item {
            let slug = context::category_slug(name);
            if slug.is_empty() || !seen.insert(slug.clone()) {
                continue;
            }
            names
                .entry(slug.clone())
                .or_insert_with(|| name.to_string());
            members.entry(slug).or_default().push(index);
        }
    }
    let root = match taxonomy {
        Taxonomy::Categories => "categories",
        Taxonomy::Tags => "tags",
    };
    let mut terms: Vec<_> = names
        .into_iter()
        .map(|(slug, name)| CategoryCtx {
            page: format!("{root}/{slug}/"),
            count: members.get(&slug).map_or(0, Vec::len),
            latest: members
                .get(&slug)
                .into_iter()
                .flatten()
                .map(|&index| items[index].date)
                .max(),
            name,
            slug,
        })
        .collect();
    terms.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.slug.cmp(&b.slug))
    });
    TaxonomyIndex { terms, members }
}

fn indexed_items(items: &[ItemCtx], indices: Option<&[usize]>) -> Vec<ItemCtx> {
    indices
        .into_iter()
        .flatten()
        .map(|&index| items[index].clone())
        .collect()
}

/// Prepare a bounded feed once, then hand the same identity/order/content to every serializer.
/// Article rendering is shared across every source, category, tag, and portable representation.
fn feed_items(
    items: &[ItemCtx],
    prepared_by_path: &BTreeMap<&str, &content::PreparedMarkdown>,
    limit: usize,
) -> Vec<ItemCtx> {
    items
        .iter()
        .take(limit)
        .cloned()
        .map(|mut item| {
            item.body_html = prepared_by_path
                .get(item.path.as_str())
                .map(|prepared| prepared.portable_html().to_string());
            item
        })
        .collect()
}

fn write_collection_feeds(
    out: &Path,
    site: &SiteCtx,
    build: &BuildCtx,
    title: &str,
    path: &str,
    items: &[ItemCtx],
) -> Result<()> {
    let dir = out.join(path);
    write(
        &dir.join("atom.xml"),
        outputs::atom_collection(site, build, title, path, items).as_bytes(),
    )?;
    write(
        &dir.join("rss.xml"),
        outputs::rss_collection(site, build, title, path, items).as_bytes(),
    )?;
    write(
        &dir.join("feed.json"),
        outputs::json_collection(site, title, path, items)?.as_bytes(),
    )
}

/// Wipe a previous build, refusing to touch a directory we did not create.
pub(crate) fn prepare_out_dir(out: &Path) -> Result<()> {
    let resolved = validate_replaceable_output(out)?;
    if resolved.exists() {
        std::fs::remove_dir_all(&resolved)
            .with_context(|| format!("clearing {}", resolved.display()))?;
    }
    std::fs::create_dir_all(&resolved).with_context(|| format!("creating {}", resolved.display()))
}

fn validate_replaceable_output(out: &Path) -> Result<PathBuf> {
    if out
        .components()
        .any(|component| component == std::path::Component::ParentDir)
    {
        bail!(
            "refusing output path with parent traversal: {}",
            out.display()
        );
    }
    if std::fs::symlink_metadata(out).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        bail!("refusing symlink output {}", out.display());
    }
    let absolute = resolve_output_path(out)?;
    if absolute.parent().is_none()
        || absolute.components().any(|component| {
            component
                .as_os_str()
                .to_str()
                .is_some_and(|name| name.eq_ignore_ascii_case(".git"))
        })
    {
        bail!("refusing protected output path {}", out.display());
    }

    let metadata = match std::fs::symlink_metadata(&absolute) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(absolute),
        Err(error) => return Err(error).with_context(|| format!("inspecting {}", out.display())),
    };
    if !metadata.is_dir() {
        bail!("refusing to clear {}: expected a directory", out.display());
    }
    if std::fs::read_dir(&absolute)?.next().is_none() {
        return Ok(absolute);
    }

    let marker = std::fs::symlink_metadata(absolute.join(MARKER)).map_err(|error| {
        anyhow::anyhow!(
            "refusing to clear {}: not an aggr output directory (no {MARKER} marker): {error}",
            out.display()
        )
    })?;
    if marker.file_type().is_symlink() || !marker.is_file() {
        bail!("refusing invalid output marker in {}", out.display());
    }
    for entry in walkdir::WalkDir::new(&absolute).follow_links(false) {
        let entry = entry?;
        if entry.file_type().is_symlink() {
            bail!(
                "refusing output containing symlink {}",
                entry.path().display()
            );
        }
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.eq_ignore_ascii_case(".git"))
        {
            bail!(
                "refusing generated output containing Git metadata: {}",
                entry.path().display()
            );
        }
    }
    Ok(absolute)
}

fn resolve_output_path(path: &Path) -> Result<PathBuf> {
    let mut existing = std::path::absolute(path)?;
    let mut missing = Vec::new();
    while !existing.exists() {
        missing.push(
            existing
                .file_name()
                .context("output path has an existing ancestor")?
                .to_os_string(),
        );
        existing.pop();
    }
    let mut resolved = existing
        .canonicalize()
        .with_context(|| format!("resolving output ancestor {}", existing.display()))?;
    for component in missing.into_iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

/// The host `CNAME` should carry: a release build on a domain that is not GitHub's own.
pub fn cname(info: &BuildInfo) -> Option<String> {
    if !info.release {
        return None;
    }
    let url = url::Url::parse(info.base_url.as_deref()?).ok()?;
    let host = url.host_str()?;
    (!host.ends_with(".github.io") && host != "localhost").then(|| host.to_string())
}

fn ensure_trailing_slash(url: &str) -> String {
    if url.ends_with('/') {
        url.to_string()
    } else {
        format!("{url}/")
    }
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn shared_immutable_assets_are_written_once_per_build() {
        let root = tempfile::tempdir().unwrap();
        let mut written = BTreeSet::new();
        let path = "assets/images/known.png";
        publish_asset(root.path(), &mut written, path, b"exact source bytes").unwrap();
        let timestamp = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
        std::fs::File::options()
            .write(true)
            .open(root.path().join(path))
            .unwrap()
            .set_modified(timestamp)
            .unwrap();
        publish_asset(root.path(), &mut written, path, b"exact source bytes").unwrap();
        assert_eq!(
            std::fs::metadata(root.path().join(path))
                .unwrap()
                .modified()
                .unwrap(),
            timestamp
        );
        assert_eq!(
            std::fs::read(root.path().join(path)).unwrap(),
            b"exact source bytes"
        );
        assert_eq!(written.len(), 1);
    }

    #[test]
    fn prepared_context_preserves_custom_theme_archive_access_without_reserializing() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Counted<'a>(&'a AtomicUsize);
        impl Serialize for Counted<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                self.0.fetch_add(1, Ordering::Relaxed);
                serde_json::json!({"title":"A <story>","preview":{"width":320}})
                    .serialize(serializer)
            }
        }
        let visits = AtomicUsize::new(0);
        let items = minijinja::Value::from_serialize([Counted(&visits), Counted(&visits)]);
        let shared = SharedCtx {
            site: minijinja::context!(title => "Reader"),
            build: minijinja::context!(version => "1"),
            sources: minijinja::Value::from_serialize([serde_json::json!({"name":"Publisher"})]),
            categories: minijinja::Value::from_serialize([serde_json::json!({"name":"news"})]),
            tags: minijinja::Value::from_serialize(["rust"]),
        };
        let mut env = minijinja::Environment::new();
        env.add_template("custom.html", "{{ site.title }} {{ build.version }} {{ sources[0].name }} {{ categories[0].name }} {{ tags[0] }} {{ items|length }} {{ items[0].title }} {{ items[1].preview.width }} {{ page.title }} {{ item is defined }}").unwrap();
        for _ in 0..20 {
            let page = PageCtx {
                kind: "item".into(),
                title: "Page".into(),
                document_title: "Page".into(),
                description: String::new(),
                indexable: true,
                path: "items/test/".into(),
                root: "../../".into(),
                canonical_url: None,
                feed_path: None,
                paginator: None,
            };
            let ctx = Ctx {
                shared: &shared,
                page,
                items: items.clone(),
                item: None,
                source: None,
                category: None,
                html: None,
                schema: None,
            };
            assert_eq!(
                env.get_template("custom.html")
                    .unwrap()
                    .render(ctx)
                    .unwrap(),
                "Reader 1 Publisher news rust 2 A &lt;story&gt; 320 Page False"
            );
        }
        assert_eq!(visits.load(Ordering::Relaxed), 2);
    }

    #[test]
    #[ignore = "manual template preparation performance comparison"]
    fn benchmark_prepared_template_context() {
        let records: Vec<_> = (0..1000)
            .map(|index| {
                serde_json::json!({
                    "title":format!("Article {index}"), "source":"publisher", "date":"2026-09-01",
                    "url":format!("items/publisher/article-{index}/"), "labels":["reading","rust"],
                    "preview":{"url":"assets/preview.jpg","width":320,"height":180},
                    "next_article":{"title":"Next article","url":"items/next/"},
                })
            })
            .collect();
        #[derive(Serialize)]
        struct Raw<'a> {
            items: &'a [serde_json::Value],
        }
        #[derive(Serialize)]
        struct Prepared {
            items: minijinja::Value,
        }
        let mut env = minijinja::Environment::new();
        env.add_template(
            "item.html",
            "{{ items|length }} {{ items[0].title }} {{ items[999].title }}",
        )
        .unwrap();
        let template = env.get_template("item.html").unwrap();
        let started = std::time::Instant::now();
        for _ in 0..1000 {
            assert_eq!(
                template.render(Raw { items: &records }).unwrap(),
                "1000 Article 0 Article 999"
            );
        }
        let repeated = started.elapsed();
        let started = std::time::Instant::now();
        let prepared = Prepared {
            items: minijinja::Value::from_serialize(&records),
        };
        for _ in 0..1000 {
            assert_eq!(
                template.render(&prepared).unwrap(),
                "1000 Article 0 Article 999"
            );
        }
        println!(
            "1000 pages × 1000 archive entries: repeated={repeated:?}, prepared={:?}",
            started.elapsed()
        );
    }

    #[test]
    fn publishing_reconstructed_article_images_does_not_decode_again() {
        let pixels = image::RgbaImage::from_fn(400, 240, |x, y| {
            image::Rgba([
                ((x * 37 + y * 11) % 256) as u8,
                ((x * 13 + y * 41) % 256) as u8,
                ((x * 29 + y * 23) % 256) as u8,
                255,
            ])
        });
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(pixels)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let asset = crate::media::prepare_asset(
            &crate::media::Candidate {
                url: url::Url::parse("https://publisher.example/diagram.png").unwrap(),
                alt: Some("Diagram".into()),
            },
            encoded.into_inner(),
            &crate::media::MediaLimits::default(),
        )
        .unwrap();
        assert!(!asset.renditions.is_empty());
        let out = tempfile::tempdir().unwrap();

        crate::media::reset_stored_decode_count();
        let published =
            publish_article_images(out.path(), vec![asset], &mut BTreeSet::new()).unwrap();

        assert_eq!(crate::media::stored_decode_count(), 0);
        assert_eq!(published.len(), 1);
        assert!(out.path().join(&published[0].original).is_file());
        assert!(
            published[0]
                .variants
                .iter()
                .all(|variant| out.path().join(&variant.url).is_file())
        );
    }

    fn day(d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, d, 0, 0, 0).unwrap()
    }

    #[test]
    fn retained_sources_keep_metadata_and_archive_pages_after_config_removal() {
        let dir = tempfile::tempdir().unwrap();
        let (config, _, store) = fixture(dir.path(), 2, "max_age_days = 30\npwa = false\n");
        store
            .write_source_state(
                "blog",
                &crate::store::SourceState {
                    title: Some("Hacker News: Best".into()),
                    site_url: Some("https://news.ycombinator.com/".into()),
                    resolved_url: Some("https://hnrss.org/best".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let items = store.items().unwrap();
        let contexts = source_contexts(&[], &store, &Status::default(), &items).unwrap();
        assert_eq!(contexts.len(), 1);
        let source = &contexts[0];
        assert_eq!(source.slug, "blog");
        assert_eq!(source.name, "Hacker News: Best");
        assert_eq!(source.url.as_deref(), Some("https://hnrss.org/best"));
        assert_eq!(
            source.site_url.as_deref(),
            Some("https://news.ycombinator.com/")
        );
        assert_eq!(source.count, 2);
        assert_eq!(source.latest, Some(day(2)));
        assert_eq!(source.page, "sources/blog/");

        let out = dir.path().join("out");
        build(&config, &[], &store, dir.path(), &info(out.clone())).unwrap();
        let archive = std::fs::read_to_string(out.join("sources/blog/index.html")).unwrap();
        assert!(archive.contains("Post 0"));
        assert!(archive.contains("Post 1"));
        assert!(archive.contains("Hacker News: Best"));
        let article =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0/index.html")).unwrap();
        assert!(
            article.contains("href=\"./?q=source%3A%22blog%22\""),
            "{article}"
        );
        assert!(article.contains("hnrss.org/best"));
        assert!(article.contains("blog.example · via Hacker News: Best"));
        let archive_document = scraper::Html::parse_document(&archive);
        assert!(
            archive_document
                .select(&scraper::Selector::parse(".row .tags").unwrap())
                .next()
                .is_none()
        );
        for html in [&archive, &article] {
            let document = scraper::Html::parse_document(html);
            let selector = scraper::Selector::parse(".meta .domain").unwrap();
            let source_link = document.select(&selector).next().unwrap();
            assert_eq!(
                source_link.value().attr("href"),
                Some("./?q=source%3A%22blog%22")
            );
            assert_eq!(
                source_link.text().collect::<String>().trim(),
                "blog.example via hnrss.org/best"
            );
            let emphasis = scraper::Selector::parse("em").unwrap();
            assert_eq!(
                source_link
                    .select(&emphasis)
                    .next()
                    .unwrap()
                    .text()
                    .collect::<String>(),
                "via hnrss.org/best"
            );
        }
    }

    #[test]
    fn feed_and_article_share_metadata_including_sources() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "");
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        let parse = |path: &str| {
            scraper::Html::parse_document(&std::fs::read_to_string(out.join(path)).unwrap())
        };
        let feed = parse("index.html");
        let article = parse("items/blog/2026-09-01-post-0/index.html");
        let fields = scraper::Selector::parse(".meta > .meta-field").unwrap();
        let source = scraper::Selector::parse(".domain").unwrap();
        let shared = |document: &scraper::Html| {
            document
                .select(&fields)
                .map(|field| field.inner_html())
                .collect::<Vec<_>>()
        };
        assert_eq!(shared(&feed), shared(&article));
        assert_eq!(feed.select(&source).count(), 1);
        assert_eq!(article.select(&source).count(), 1);
        assert!(
            feed.select(&scraper::Selector::parse(".reading-stats").unwrap())
                .next()
                .is_some()
        );
    }

    #[test]
    fn retained_sources_without_state_keep_archives_without_invented_urls() {
        let dir = tempfile::tempdir().unwrap();
        let (_, _, store) = fixture(dir.path(), 1, "");
        let contexts =
            source_contexts(&[], &store, &Status::default(), &store.items().unwrap()).unwrap();
        assert_eq!(contexts.len(), 1);
        assert_eq!(contexts[0].name, "Unknown source");
        assert!(contexts[0].url.is_none());
        assert!(contexts[0].site_url.is_none());
    }

    #[test]
    fn document_titles_keep_page_site_and_aggr_identity_consistent() {
        for (kind, page, title, expected) in [
            ("river", 1, "Reader", "Reader | aggr"),
            ("river", 2, "Reader", "Reader — page 2 | aggr"),
            ("search", 1, "Search", "search | Reader | aggr"),
            ("browse", 1, "Browse", "browse | Reader | aggr"),
            (
                "preferences",
                1,
                "Preferences",
                "preferences | Reader | aggr",
            ),
            ("source", 3, "Example", "Example — page 3 | Reader | aggr"),
            ("category", 1, "Engineering", "engineering | Reader | aggr"),
            ("tag", 1, "Rust", "rust | Reader | aggr"),
            ("item", 1, "Article Title", "Article Title | Reader | aggr"),
            ("404", 1, "Not found", "not found | Reader | aggr"),
            ("offline", 1, "Offline", "offline | Reader | aggr"),
        ] {
            assert_eq!(
                document_title("Reader", title, kind, page),
                expected,
                "{kind}"
            );
        }
    }

    #[test]
    fn window_respects_count_and_age() {
        let dates = vec![day(10), day(9), day(8), day(1)];
        let w = window(&dates, day(10), 2, 30);
        assert_eq!(
            w,
            Window {
                rendered: vec![0, 1],
            }
        );
        let w = window(&dates, day(10), 10, 5);
        assert_eq!(
            w,
            Window {
                rendered: vec![0, 1, 2],
            }
        );
    }

    #[test]
    fn pagination_paths() {
        let pages = paginate("", 0, 10);
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].path, "");
        assert_eq!(pages[0].range, 0..0);
        assert_eq!(pages[0].context.first, "./");
        assert_eq!(pages[0].context.last, "./");
        assert_eq!(pages[0].context.total_items, 0);
        let pages = paginate("sources/x/", 25, 10);
        assert_eq!(
            pages.iter().map(|p| p.path.as_str()).collect::<Vec<_>>(),
            ["sources/x/", "sources/x/page/2/", "sources/x/page/3/"]
        );
        assert_eq!(pages[2].range, 20..25);
    }

    #[test]
    fn paginator_context_uses_stable_edge_and_adjacent_routes() {
        let pages = paginate("", 35, 10);
        let first = &pages[0].context;
        assert_eq!(first.current_index, 1);
        assert_eq!(first.number_pagers, 4);
        assert_eq!(first.paginate_by, 10);
        assert_eq!(first.total_items, 35);
        assert_eq!(first.first, "./");
        assert_eq!(first.last, "page/4/");
        assert_eq!(first.previous, None);
        assert_eq!(first.next.as_deref(), Some("page/2/"));

        let middle = &pages[2].context;
        assert_eq!(middle.first, "./");
        assert_eq!(middle.last, "page/4/");
        assert_eq!(middle.previous.as_deref(), Some("page/2/"));
        assert_eq!(middle.next.as_deref(), Some("page/4/"));
        assert_eq!(middle.offset, 20);

        let last = &pages[3].context;
        assert_eq!(last.previous.as_deref(), Some("page/3/"));
        assert_eq!(last.next, None);

        let nested = paginate("sources/x/", 11, 10);
        assert_eq!(nested[1].context.first, "sources/x/");
        assert_eq!(nested[1].context.previous.as_deref(), Some("sources/x/"));
        assert_eq!(nested[1].context.last, "sources/x/page/2/");
    }

    #[test]
    fn categories_without_rendered_items_are_omitted() {
        assert!(taxonomy_index(&[], Taxonomy::Categories).terms.is_empty());
    }

    #[test]
    fn base_paths() {
        assert_eq!(base_path(None), "/");
        assert_eq!(base_path(Some("https://u.github.io/")), "/");
        assert_eq!(base_path(Some("https://u.github.io/repo")), "/repo/");
        assert_eq!(base_path(Some("https://u.github.io/a/b/")), "/a/b/");
    }

    #[test]
    fn relative_roots_cover_files_and_nested_routes() {
        assert_eq!(relative_root(""), "./");
        assert_eq!(relative_root("404.html"), "./");
        assert_eq!(relative_root("search/"), "../");
        assert_eq!(relative_root("categories/rust/"), "../../");
        assert_eq!(relative_root("sources/rust/page/2/"), "../../../../");
    }

    #[test]
    fn refuses_to_clear_foreign_directories() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("precious"), "x").unwrap();
        assert!(prepare_out_dir(dir.path()).is_err());
        std::fs::write(dir.path().join(MARKER), "").unwrap();
        prepare_out_dir(dir.path()).unwrap();
        assert!(!dir.path().join("precious").exists());
    }

    #[test]
    fn refuses_git_metadata_inside_owned_output() {
        for name in [".git", ".GIT"] {
            let root = tempfile::tempdir().unwrap();
            let out = root.path().join("site");
            std::fs::create_dir_all(out.join("nested").join(name)).unwrap();
            std::fs::write(out.join(MARKER), "1").unwrap();
            std::fs::write(out.join("nested").join(name).join("proof"), "keep").unwrap();
            let error = prepare_out_dir(&out).unwrap_err();
            assert!(format!("{error:#}").contains("Git metadata"));
            assert!(out.join("nested").join(name).join("proof").exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinks_inside_owned_output() {
        let root = tempfile::tempdir().unwrap();
        let out = root.path().join("site");
        let outside = root.path().join("outside");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(out.join(MARKER), "1").unwrap();
        std::fs::write(outside.join("proof"), "keep").unwrap();
        std::os::unix::fs::symlink(&outside, out.join("linked")).unwrap();

        let error = prepare_out_dir(&out).unwrap_err();
        assert!(format!("{error:#}").contains("symlink"));
        assert_eq!(
            std::fs::read_to_string(outside.join("proof")).unwrap(),
            "keep"
        );
    }

    #[test]
    fn precache_lists_shells_lists_assets_and_the_newest_items() {
        let paths = precache_paths(
            "/repo/",
            ["sources/a/".to_string(), "sources/a/".to_string()],
            &["style.css".to_string()],
            ["items/a/1/".to_string(), "items/a/2/".to_string()],
            1,
        );
        assert_eq!(paths[0], "/repo/");
        assert!(paths.contains(&"/repo/aggr.json".to_string()));
        assert!(paths.contains(&"/repo/offline.html".to_string()));
        assert!(paths.contains(&"/repo/browse/".to_string()));
        assert!(paths.contains(&"/repo/preferences/".to_string()));
        assert!(!paths.contains(&"/repo/settings/".to_string()));
        assert!(paths.contains(&"/repo/sources/a/".to_string()));
        assert!(paths.contains(&"/repo/assets/style.css".to_string()));
        assert!(paths.contains(&"/repo/items/a/1/".to_string()));
        assert!(!paths.contains(&"/repo/items/a/2/".to_string()));
        assert_eq!(
            paths
                .iter()
                .filter(|path| *path == "/repo/sources/a/")
                .count(),
            1
        );
    }

    #[test]
    fn precache_entries_revision_every_resource_and_require_the_app_shell() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("assets")).unwrap();
        std::fs::create_dir_all(dir.path().join("items/a/1")).unwrap();
        std::fs::write(dir.path().join("index.html"), "home").unwrap();
        std::fs::write(dir.path().join("offline.html"), "offline").unwrap();
        std::fs::write(dir.path().join("assets/style-a.css"), "css").unwrap();
        std::fs::write(dir.path().join("items/a/1/index.html"), "item").unwrap();

        let entries = precache_entries(
            dir.path(),
            vec![
                "".into(),
                "offline.html".into(),
                "assets/style-a.css".into(),
                "items/a/1/".into(),
            ],
        )
        .unwrap();
        assert!(entries[0].required);
        assert!(entries[1].required);
        assert!(entries[2].required);
        assert!(entries[3].required);
        assert_eq!(entries[0].revision, crate::model::sha1_hex(b"home"));
        assert_eq!(entries[3].revision, crate::model::sha1_hex(b"item"));
    }

    #[test]
    fn cache_version_changes_with_content_but_not_rebuild_time() {
        let build = BuildCtx {
            time: day(2),
            version: "0".into(),
            app_version: "app".into(),
            content_version: "content".into(),
            config_sha: Some("c".repeat(40)),
            data_sha: None,
            generation: "g".repeat(40),
            release: false,
        };
        assert_eq!(
            cache_version(&build),
            format!("0-local-{}-{}", "c".repeat(12), "g".repeat(12))
        );
        let later = BuildCtx {
            data_sha: Some("d".repeat(40)),
            time: day(3),
            ..build.clone()
        };
        assert_ne!(cache_version(&later), cache_version(&build));
        let rebuilt = BuildCtx {
            time: day(3),
            ..build.clone()
        };
        assert_eq!(cache_version(&rebuilt), cache_version(&build));
    }

    #[test]
    fn render_generation_changes_only_at_visible_time_boundaries() {
        let at = day(3);
        let item = |path: &str, published| Item {
            path: path.into(),
            front: crate::model::FrontMatter {
                title: path.into(),
                link: format!("https://example.com/{path}"),
                source: "example".into(),
                published: Some(published),
                first_seen: published,
                ..Default::default()
            },
            body: "body".into(),
        };
        let site = SiteConfig {
            max_age_days: 2,
            ..Default::default()
        };

        let stable = item("stable", at - Duration::hours(30));
        assert_eq!(
            render_generation(std::slice::from_ref(&stable), &site, at),
            render_generation(&[stable], &site, at + Duration::minutes(10))
        );

        let age_boundary = item("age", at - Duration::minutes(59));
        assert_ne!(
            render_generation(std::slice::from_ref(&age_boundary), &site, at),
            render_generation(&[age_boundary], &site, at + Duration::minutes(2))
        );

        let cutoff = item("cutoff", at - Duration::hours(47) - Duration::minutes(59));
        assert_ne!(
            render_generation(std::slice::from_ref(&cutoff), &site, at),
            render_generation(&[cutoff], &site, at + Duration::minutes(2))
        );
    }

    /// A store with `count` items of one source, newest last, and a matching config.
    fn fixture(root: &Path, count: usize, extra: &str) -> (Config, Vec<Source>, Store) {
        use crate::model::{FrontMatter, file_stem, item_dir};
        use crate::store::NewItem;

        let config = Config::parse(&format!(
            "[site]\ntitle = \"Demo <site>\"\n{extra}\n[[sources]]\nslug = \"blog\"\nurl = \"https://blog.example/feed\"\n"
        ))
        .unwrap();
        let sources = config.resolve_sources(&|_| None).unwrap();
        let store = Store::open(root.join("data"));
        for i in 0..count {
            let date = day(1 + i as u32);
            let front = FrontMatter {
                title: format!("Post {i}"),
                link: format!("https://blog.example/{i}"),
                source: "blog".into(),
                published: Some(date),
                first_seen: date,
                ..Default::default()
            };
            store
                .write_item(NewItem {
                    dir: &item_dir("blog", date),
                    stem: &file_stem(date, &front.title),
                    front: &front,
                    body: "Hello",
                    html: None,
                    preview: None,
                    images: &[],
                })
                .unwrap();
        }
        (config, sources, store)
    }

    fn info(out: PathBuf) -> BuildInfo {
        BuildInfo {
            out,
            base_url: Some("https://u.github.io/repo/".into()),
            config_sha: Some("c".repeat(40)),
            config_path: Some("aggr.toml".into()),
            data_sha: Some("d".repeat(40)),
            generation: "fixture".into(),
            now: day(20),
            release: false,
            discussions: crate::discussions::ResolutionSet::default(),
            pagefind_cache: None,
        }
    }

    #[test]
    fn archive_selection_is_order_independent_and_keeps_anonymous_items() {
        let item = |path: &str, link: &str, body: &str| Item {
            path: path.into(),
            front: crate::model::FrontMatter {
                link: link.into(),
                ..Default::default()
            },
            body: body.into(),
        };
        let items = vec![
            item("items/hn/a", "https://example.test/article", "Summary"),
            item(
                "items/hn/b",
                "https://example.test/article#top",
                "Full article content",
            ),
            item(
                "items/hn/c",
                "https://example.test/article?id=1",
                "Distinct query",
            ),
            item("items/hn/d", "", "Anonymous first"),
            item("items/hn/e", "", "Anonymous second"),
        ];
        let (selected, redirects) = visible_archive(items.clone(), &[]);
        assert_eq!(selected.len(), 4);
        assert!(selected.iter().any(|item| item.path == "items/hn/b"));
        assert_eq!(
            redirects,
            vec![("items/hn/a/".into(), "items/hn/b/".into())]
        );
        let (mut reversed, reverse_redirects) =
            visible_archive(items.into_iter().rev().collect(), &[]);
        let mut selected = selected;
        selected.sort_by(|a, b| a.path.cmp(&b.path));
        reversed.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(selected, reversed);
        assert_eq!(redirects, reverse_redirects);
    }

    #[test]
    fn archived_duplicates_and_shorts_are_removed_from_all_collections() {
        use crate::model::{FrontMatter, file_stem, item_dir};
        use crate::store::NewItem;

        let dir = tempfile::tempdir().unwrap();
        let (_, _, store) = fixture(dir.path(), 0, "");
        let config = Config::parse(
            r#"[site]
max_age_days = 365
pwa = true
[[sources]]
slug = "hn"
url = "https://hnrss.org/frontpage"
category = "News"
[[sources]]
slug = "blog"
url = "https://blog.example/feed"
category = "Science"
"#,
        )
        .unwrap();
        let sources = config.resolve_sources(&|_| None).unwrap();
        let mut paths = Vec::new();
        for (source, title, link, hidden, labels) in [
            (
                "hn",
                "Duplicate headline",
                "http://www.blog.example/article/?utm_source=hn#top",
                false,
                vec!["duplicate-only"],
            ),
            (
                "blog",
                "Publisher headline",
                "https://blog.example/article",
                false,
                vec!["science"],
            ),
            (
                "hn",
                "Excluded short",
                "https://www.youtube.com/shorts/abc123",
                false,
                vec!["short-only"],
            ),
            (
                "hn",
                "Hidden headline",
                "https://blog.example/hidden",
                true,
                vec!["hidden-only"],
            ),
            (
                "hn",
                "Distinct article",
                "https://blog.example/article?id=2",
                false,
                vec![],
            ),
        ] {
            let date = day(19);
            let front = FrontMatter {
                title: title.into(),
                link: link.into(),
                source: source.into(),
                published: Some(date),
                first_seen: date,
                hidden,
                labels: labels.into_iter().map(str::to_owned).collect(),
                ..Default::default()
            };
            let item_dir = item_dir(source, date);
            let stem = file_stem(date, title);
            store
                .write_item(NewItem {
                    dir: &item_dir,
                    stem: &stem,
                    front: &front,
                    body: title,
                    html: None,
                    preview: None,
                    images: &[],
                })
                .unwrap();
            paths.push(context::item_url(&format!("{item_dir}/{stem}")));
        }
        let before = store.items().unwrap();
        let out = dir.path().join("out");
        let mut build_info = info(out.clone());
        build_info.release = true;
        let summary = build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        assert_eq!(summary.items, 2);
        assert_eq!(
            store.items().unwrap(),
            before,
            "rendering leaves the archive intact"
        );

        for path in [
            "index.html",
            "sources/hn/index.html",
            "sources/blog/index.html",
            "categories/news/index.html",
            "categories/science/index.html",
            "browse/index.html",
            "feed.xml",
            "atom.xml",
            "rss.xml",
            "feed.json",
            "linkset.json",
            "sitemap.xml",
            "sources/hn/feed.json",
            "sources/blog/atom.xml",
            "categories/news/rss.xml",
            "sw.js",
        ] {
            let text = std::fs::read_to_string(out.join(path)).unwrap();
            for excluded in ["Duplicate headline", "Excluded short", "Hidden headline"] {
                assert!(!text.contains(excluded), "{path} includes {excluded}");
            }
            for path in [&paths[0], &paths[2], &paths[3]] {
                assert!(!text.contains(path), "collection links to excluded {path}");
            }
        }
        assert!(!out.join("tags/short-only").exists());
        assert!(!out.join("tags/duplicate-only").exists());
        assert!(!out.join(&paths[2]).exists());
        assert!(!out.join(&paths[3]).exists());
        let redirect = std::fs::read_to_string(out.join(&paths[0]).join("index.html")).unwrap();
        assert!(redirect.contains(&paths[1]));
        assert!(redirect.contains("noindex"));
        assert!(out.join(&paths[1]).join("index.html").is_file());
    }

    #[test]
    fn old_items_stay_browsable_in_source_archives() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "max_age_days = 5\npwa = false\n");
        let out = dir.path().join("out");
        let summary = build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        assert_eq!(summary.items, 1);
        assert_eq!(summary.stubs, 4);

        let feed = std::fs::read_to_string(out.join("index.html")).unwrap();
        assert!(
            !feed.contains("Post 0"),
            "old items should not crowd the recent feed"
        );
        let source = std::fs::read_to_string(out.join("sources/blog/index.html")).unwrap();
        assert!(source.contains("Post 0"), "{source}");
        assert!(
            out.join("items/blog/2026-09-01-post-0/index.html")
                .is_file()
        );
    }

    #[test]
    fn rendered_worker_is_stable_and_versions_theme_and_worker_changes() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "pwa = true\n");
        let out = dir.path().join("out");
        let mut build_info = info(out.clone());
        build_info.release = true;
        build_info.pagefind_cache = Some(dir.path().join("pagefind-cache"));
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        let original = std::fs::read_to_string(out.join("sw.js")).unwrap();
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        assert_eq!(
            std::fs::read_to_string(out.join("sw.js")).unwrap(),
            original
        );

        std::fs::create_dir(dir.path().join("static")).unwrap();
        std::fs::write(dir.path().join("static/style.css"), "body { color: teal; }").unwrap();
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        let styled = std::fs::read_to_string(out.join("sw.js")).unwrap();
        let version = |worker: &str| {
            worker
                .lines()
                .find(|line| line.starts_with("var VERSION ="))
                .unwrap()
                .to_owned()
        };
        assert_ne!(version(&styled), version(&original));

        std::fs::create_dir(dir.path().join("templates")).unwrap();
        std::fs::write(
            dir.path().join("templates/sw.js"),
            format!(
                "{}\n// Worker revision test.\n",
                include_str!("../../themes/default/templates/sw.js")
            ),
        )
        .unwrap();
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        let revised = std::fs::read_to_string(out.join("sw.js")).unwrap();
        assert_ne!(version(&revised), version(&styled));
    }

    #[test]
    fn application_fingerprint_tracks_effective_assets_templates_and_binary() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        let theme = dir.path().join("theme");
        std::fs::create_dir_all(project.join("static")).unwrap();
        std::fs::create_dir_all(theme.join("static")).unwrap();
        std::fs::create_dir_all(project.join("templates")).unwrap();
        std::fs::write(project.join("static/style.css"), "body { color: teal; }").unwrap();
        std::fs::write(theme.join("static/style.css"), "body { color: red; }").unwrap();
        let layers = Layers {
            dirs: vec![project.clone(), theme.clone()],
        };
        let original = app_version("1.2.3", &layers).unwrap();
        assert_ne!(original, app_version("1.2.4", &layers).unwrap());

        std::fs::write(theme.join("static/style.css"), "body { color: blue; }").unwrap();
        assert_eq!(
            original,
            app_version("1.2.3", &layers).unwrap(),
            "shadowed layer bytes do not ship"
        );
        std::fs::write(project.join("static/style.css"), "body { color: green; }").unwrap();
        let styled = app_version("1.2.3", &layers).unwrap();
        assert_ne!(original, styled);
        std::fs::write(
            project.join("templates/custom-include.html"),
            "<p>Custom include</p>",
        )
        .unwrap();
        let template = app_version("1.2.3", &layers).unwrap();
        assert_ne!(styled, template);
        std::fs::write(
            project.join("templates/sw.js"),
            "/* worker implementation */",
        )
        .unwrap();
        assert_ne!(template, app_version("1.2.3", &layers).unwrap());
    }

    #[test]
    fn update_manifest_distinguishes_content_from_application_releases_without_pwa() {
        let dir = tempfile::tempdir().unwrap();
        let (mut config, mut sources, store) = fixture(dir.path(), 1, "pwa = false\n");
        let out = dir.path().join("out");
        let mut build_info = info(out.clone());
        let manifest = || -> serde_json::Value {
            serde_json::from_slice(&std::fs::read(out.join("updates.json")).unwrap()).unwrap()
        };
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        let original = manifest();
        assert_eq!(
            original["entries"],
            serde_json::json!(["items/blog/2026-09-01-post-0/"])
        );

        build_info.now += Duration::minutes(1);
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        assert_eq!(manifest(), original, "rebuild time is not a release");

        config.site.title = "A new reader title".into();
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        let titled = manifest();
        assert_eq!(titled["app_version"], original["app_version"]);
        assert_ne!(titled["content_version"], original["content_version"]);

        sources[0].name = Some("Updated publisher".into());
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        let publisher = manifest();
        assert_eq!(publisher["app_version"], titled["app_version"]);
        assert_ne!(publisher["content_version"], titled["content_version"]);

        let front = crate::model::FrontMatter {
            title: "Newest article".into(),
            link: "https://blog.example/new".into(),
            source: "blog".into(),
            first_seen: day(19),
            published: Some(day(19)),
            ..Default::default()
        };
        store
            .write_item(crate::store::NewItem {
                dir: "items/blog/2026/09",
                stem: "2026-09-19-newest-article",
                front: &front,
                body: "New content",
                html: None,
                preview: None,
                images: &[],
            })
            .unwrap();
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        let article = manifest();
        assert_eq!(article["app_version"], publisher["app_version"]);
        assert_ne!(article["content_version"], publisher["content_version"]);
        assert_eq!(
            article["entries"][0],
            "items/blog/2026-09-19-newest-article/"
        );

        std::fs::create_dir(dir.path().join("static")).unwrap();
        std::fs::write(dir.path().join("static/style.css"), "body { color: teal; }").unwrap();
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        let application = manifest();
        assert_ne!(application["app_version"], article["app_version"]);
        assert_eq!(application["content_version"], article["content_version"]);
    }

    #[test]
    fn item_pages_show_next_then_one_distinct_recommendation() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 6, "max_age_days = 30\npwa = false\n");
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();

        let newest =
            std::fs::read_to_string(out.join("items/blog/2026-09-06-post-5/index.html")).unwrap();
        assert!(
            newest.contains("data-next-url=\"items/blog/2026-09-05-post-4/\""),
            "{newest}"
        );
        assert!(!newest.contains("data-previous-url="), "{newest}");
        assert_eq!(newest.matches("class=\"article-more-link").count(), 2);
        assert_eq!(newest.matches("article-more-neighbor").count(), 0);
        assert!(!newest.contains("article-navigation-link"), "{newest}");
        assert!(!newest.contains("article-more-label"), "{newest}");

        let second =
            std::fs::read_to_string(out.join("items/blog/2026-09-05-post-4/index.html")).unwrap();
        assert!(second.contains("data-previous-url=\"items/blog/2026-09-06-post-5/\""));
        assert!(second.contains("data-next-url=\"items/blog/2026-09-04-post-3/\""));
        assert_eq!(second.matches("class=\"article-more-link").count(), 2);
        assert_eq!(second.matches("article-more-neighbor").count(), 0);
        assert!(!second.contains("article-navigation-link"), "{second}");

        let second_footer = second
            .split_once("<footer class=\"article-footer\"")
            .unwrap()
            .1;
        assert!(!second_footer.contains("rel=\"prev\""), "{second_footer}");
        let first_card_end = second_footer.find("</a>").unwrap();
        let first_card = &second_footer[..first_card_end];
        assert!(first_card.contains("rel=\"next\""), "{first_card}");
        assert!(first_card.contains(">Coming next</h2>"), "{first_card}");
        assert!(
            first_card.contains("items/blog/2026-09-04-post-3/"),
            "{first_card}"
        );
        let recommendation = &second_footer[first_card_end + "</a>".len()..];
        assert!(
            recommendation.contains("class=\"article-more-link title\""),
            "{recommendation}"
        );
        assert!(
            recommendation.contains(">Discover more</h2>"),
            "{recommendation}"
        );
        assert!(
            !recommendation.contains("items/blog/2026-09-04-post-3/"),
            "{recommendation}"
        );

        for day in 1..=6 {
            let page = std::fs::read_to_string(out.join(format!(
                "items/blog/2026-09-{day:02}-post-{}/index.html",
                day - 1
            )))
            .unwrap();
            let expected = if day == 1 { 1 } else { 2 };
            assert_eq!(page.matches("class=\"article-more-link").count(), expected);
            assert!(
                page.matches("class=\"article-more-link").count() <= 2,
                "day {day} exceeds the continuation-card limit"
            );
        }
    }

    #[test]
    fn continuation_omits_an_empty_footer_when_only_the_previous_article_exists() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 2, "max_age_days = 30\npwa = false\n");
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();

        let newest =
            std::fs::read_to_string(out.join("items/blog/2026-09-02-post-1/index.html")).unwrap();
        assert_eq!(newest.matches("class=\"article-more-link").count(), 1);
        assert!(newest.contains("rel=\"next\""), "{newest}");

        let oldest =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0/index.html")).unwrap();
        assert!(oldest.contains("data-previous-url="), "{oldest}");
        assert!(
            !oldest.contains("<footer class=\"article-footer\""),
            "{oldest}"
        );
    }

    #[test]
    fn recommendations_reuse_feed_metadata_and_reserved_previews() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 2, "pwa = false\n");
        let mut item = store.items().unwrap().remove(0);
        let (directory, stem) = item.path.rsplit_once('/').unwrap();
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 75)
            .encode_image(&image::DynamicImage::new_rgb8(160, 80))
            .unwrap();
        let hash = crate::model::sha1_hex(&bytes);
        item.front.preview = Some(crate::model::Preview {
            file: format!("{stem}.preview-{}.jpg", &hash[..12]),
            width: 160,
            height: 80,
            alt: Some("Preview".into()),
            color: None,
        });
        store
            .write_item(crate::store::NewItem {
                dir: directory,
                stem,
                front: &item.front,
                body: &item.body,
                html: None,
                preview: Some(&bytes),
                images: &[],
            })
            .unwrap();
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        let feed = scraper::Html::parse_document(
            &std::fs::read_to_string(out.join("index.html")).unwrap(),
        );
        let page = scraper::Html::parse_document(
            &std::fs::read_to_string(out.join("items/blog/2026-09-02-post-1/index.html")).unwrap(),
        );
        let row = feed
            .select(
                &scraper::Selector::parse(".row[data-url='items/blog/2026-09-01-post-0/']")
                    .unwrap(),
            )
            .next()
            .unwrap();
        let card = page
            .select(&scraper::Selector::parse(".article-more-card").unwrap())
            .next()
            .unwrap();
        let metadata = scraper::Selector::parse(".meta").unwrap();
        assert_eq!(
            row.select(&metadata).next().unwrap().inner_html(),
            card.select(&metadata).next().unwrap().inner_html()
        );
        let preview = scraper::Selector::parse(".preview-image").unwrap();
        let feed_image = row.select(&preview).next().unwrap();
        let card_image = card.select(&preview).next().unwrap();
        for attribute in ["src", "width", "height", "loading"] {
            assert_eq!(
                feed_image.value().attr(attribute),
                card_image.value().attr(attribute)
            );
        }
    }

    #[test]
    fn article_resources_move_below_header_without_changing_tags_or_portable_links() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "pwa = false\n");
        let mut item = store.items().unwrap().remove(0);
        item.front.labels = vec!["Interpretability".into()];
        item.front.summary =
            Some("HUGGING FACE MODELSCOPE TECHNICAL REPORT A publisher teaser.".into());
        item.body = "![Qwen-Scope main image](https://qianwen-res.oss-accelerate.aliyuncs.com/qwen-scope/Figures/overview.png)\n\n[HUGGING FACE](https://huggingface.co/collections/Qwen/qwen-scope) [MODELSCOPE & DATA](https://modelscope.cn/collections/Qwen/Qwen-Scope) [TECHNICAL REPORT](https://arxiv.org/abs/2605.11887?download=1&mode=full)\n\nInterpretability helps us understand how models work.\n".into();
        let (directory, stem) = item.path.rsplit_once('/').unwrap();
        store
            .write_item(crate::store::NewItem {
                dir: directory,
                stem,
                front: &item.front,
                body: &item.body,
                html: None,
                preview: None,
                images: &[],
            })
            .unwrap();
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        let article =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0/index.html")).unwrap();
        let document = scraper::Html::parse_document(&article);
        let resources = document
            .select(&scraper::Selector::parse(".itemhead + .article-resources a").unwrap())
            .collect::<Vec<_>>();
        assert_eq!(resources.len(), 3);
        assert_eq!(
            resources[1].text().collect::<String>().trim(),
            "MODELSCOPE & DATA ↗"
        );
        let destinations = [
            "https://huggingface.co/collections/Qwen/qwen-scope",
            "https://modelscope.cn/collections/Qwen/Qwen-Scope",
            "https://arxiv.org/abs/2605.11887?download=1&mode=full",
        ];
        for (resource, destination) in resources.iter().zip(destinations) {
            assert_eq!(resource.value().attr("href"), Some(destination));
        }
        let body = document
            .select(&scraper::Selector::parse(".body").unwrap())
            .next()
            .unwrap();
        assert!(
            body.text()
                .collect::<String>()
                .trim()
                .starts_with("Interpretability")
        );
        assert!(!body.html().contains("TECHNICAL REPORT"));
        assert!(body.html().contains("qwen-scope/Figures/overview.png"));
        assert!(body.html().contains("Qwen-Scope main image"));
        let tags = document
            .select(&scraper::Selector::parse(".item-tags .tag").unwrap())
            .map(|tag| tag.text().collect::<String>())
            .collect::<Vec<_>>();
        assert_eq!(tags, ["#interpretability"]);
        assert!(article.contains("MODELSCOPE &amp; DATA"));
        let description = document
            .select(&scraper::Selector::parse("meta[name='description']").unwrap())
            .next()
            .unwrap()
            .value()
            .attr("content")
            .unwrap();
        assert!(description.contains("Interpretability helps"));
        assert!(!description.contains("HUGGING FACE"));
        for path in [
            "atom.xml",
            "rss.xml",
            "feed.json",
            "items/blog/2026-09-01-post-0.md",
            "items/blog/2026-09-01-post-0.txt",
            "items/blog/2026-09-01-post-0.rst",
            "items/blog/2026-09-01-post-0.json",
        ] {
            let output = std::fs::read_to_string(out.join(path)).unwrap();
            assert!(output.contains("HUGGING FACE"), "lost resources in {path}");
            assert!(
                output.contains("huggingface.co/collections/Qwen/qwen-scope"),
                "lost destination in {path}"
            );
            assert!(
                output.contains("arxiv.org/abs/2605.11887"),
                "lost report in {path}"
            );
        }
        assert!(
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0.txt"))
                .unwrap()
                .contains("TECHNICAL REPORT")
        );
        let retained = store.items().unwrap().remove(0);
        assert_eq!(retained.body, item.body);
        assert_eq!(retained.front.labels, item.front.labels);
        assert_eq!(retained.front.summary, item.front.summary);
    }

    #[test]
    fn published_titles_share_cleanup_across_all_representations() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "pwa = false\n");
        let mut item = store.items().unwrap().remove(0);
        item.front.title = "🚀 Café #1 🧑🏽‍💻".into();
        item.body = "Body emoji stays 🚀.\n".into();
        let (directory, stem) = item.path.rsplit_once('/').unwrap();
        store
            .write_item(crate::store::NewItem {
                dir: directory,
                stem,
                front: &item.front,
                body: &item.body,
                html: None,
                preview: None,
                images: &[],
            })
            .unwrap();
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        for path in [
            "index.html",
            "atom.xml",
            "rss.xml",
            "feed.json",
            "sources/blog/atom.xml",
            "sources/blog/rss.xml",
            "sources/blog/feed.json",
            "items/blog/2026-09-01-post-0/index.html",
            "items/blog/2026-09-01-post-0.md",
            "items/blog/2026-09-01-post-0.txt",
            "items/blog/2026-09-01-post-0.rst",
            "items/blog/2026-09-01-post-0.json",
        ] {
            let output = std::fs::read_to_string(out.join(path)).unwrap();
            assert!(output.contains("Café #1"), "missing title in {path}");
            assert!(!output.contains(&item.front.title), "raw title in {path}");
        }
        let markdown =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0.md")).unwrap();
        assert!(markdown.contains("Body emoji stays 🚀."));
        assert_eq!(store.items().unwrap()[0].front.title, item.front.title);
    }

    #[test]
    fn build_cleans_leading_publication_dates_from_existing_items() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "max_age_days = 30\npwa = false\n");
        let mut item = store.items().unwrap().remove(0);
        item.body = "1st September 2026\n\nActual opening.\n".into();
        let stem = item.path.rsplit('/').next().unwrap();
        let dir_path = item.path.rsplit_once('/').unwrap().0;
        store
            .write_item(crate::store::NewItem {
                dir: dir_path,
                stem,
                front: &item.front,
                body: &item.body,
                html: None,
                preview: None,
                images: &[],
            })
            .unwrap();
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();

        let html =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0/index.html")).unwrap();
        assert!(!html.contains("1st September 2026"), "{html}");
        assert!(html.contains("Actual opening."), "{html}");
        let markdown =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0.md")).unwrap();
        assert!(!markdown.contains("1st September 2026"), "{markdown}");
    }

    #[test]
    fn build_cleans_existing_boundary_controls_without_rewriting_archive() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "pwa = false\n");
        let mut item = store.items().unwrap().remove(0);
        item.body =
            "Advertisement\n\n•\n\nActual article.\n\n\\[[0 comments](https://publisher.example/#comment-form)\\]\n"
                .into();
        let (directory, stem) = item.path.rsplit_once('/').unwrap();
        store
            .write_item(crate::store::NewItem {
                dir: directory,
                stem,
                front: &item.front,
                body: &item.body,
                html: None,
                preview: None,
                images: &[],
            })
            .unwrap();
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        let html =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0/index.html")).unwrap();
        let markdown =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0.md")).unwrap();
        assert!(!html.contains("0 comments"), "{html}");
        assert!(!markdown.contains("0 comments"), "{markdown}");
        assert!(html.contains("Actual article."), "{html}");
        for rendered in [&html, &markdown] {
            assert!(!rendered.contains("Advertisement"));
            assert!(!rendered.contains('•'));
        }
        for file in ["rss.xml", "atom.xml", "feed.json"] {
            let feed = std::fs::read_to_string(out.join(file)).unwrap();
            assert!(!feed.contains("Advertisement"), "{file}");
            assert!(!feed.contains('•'), "{file}");
            assert!(feed.contains("Actual article."), "{file}");
        }
        assert_eq!(store.items().unwrap()[0].body, item.body);
    }

    #[test]
    fn build_keeps_stored_markdown_authoritative_over_retained_html() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "pwa = false\n");
        let mut item = store.items().unwrap().remove(0);
        let (directory, stem) = item.path.rsplit_once('/').unwrap();
        let body = "```\n$ z dotfiles$ pwd/private/dotfiles\n```\n";
        let html = "<pre><code data-lang=\"bash\"><span><span>$ z dotfiles\n</span></span><span><span>$ <span>pwd</span>\n</span></span><span><span>/private/dotfiles\n</span></span></code></pre>";
        item.front.html = Some(format!("{stem}.html"));
        store
            .write_item(crate::store::NewItem {
                dir: directory,
                stem,
                front: &item.front,
                body,
                html: Some(html),
                preview: None,
                images: &[],
            })
            .unwrap();
        let stored = std::fs::read(dir.path().join("data").join(item.md_path())).unwrap();
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("data").join(item.md_path())).unwrap(),
            stored
        );
        let markdown =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0.md")).unwrap();
        assert!(markdown.contains(body), "{markdown}");
        let json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(out.join("items/blog/2026-09-01-post-0.json")).unwrap(),
        )
        .unwrap();
        assert!(
            json["content_markdown"]
                .as_str()
                .unwrap()
                .contains(body.trim())
        );
    }

    #[test]
    fn build_copies_valid_previews_into_optional_offline_assets() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "pwa = true\n");
        let mut item = store.items().unwrap().remove(0);
        let (directory, stem) = item.path.rsplit_once('/').unwrap();
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 75)
            .encode_image(&image::DynamicImage::new_rgb8(160, 80))
            .unwrap();
        let hash = crate::model::sha1_hex(&bytes);
        item.front.preview = Some(crate::model::Preview {
            file: format!("{stem}.preview-{}.jpg", &hash[..12]),
            width: 160,
            height: 80,
            alt: Some("A local preview".into()),
            color: None,
        });
        store
            .write_item(crate::store::NewItem {
                dir: directory,
                stem,
                front: &item.front,
                body: &item.body,
                html: None,
                preview: Some(&bytes),
                images: &[],
            })
            .unwrap();
        let out = dir.path().join("out");
        let mut build_info = info(out.clone());
        build_info.release = true;
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();
        let path = format!("assets/previews/{hash}.jpg");
        assert_eq!(std::fs::read(out.join(&path)).unwrap(), bytes);
        assert!(!precache_entries(&out, vec![path.clone()]).unwrap()[0].required);
        let worker = std::fs::read_to_string(out.join("sw.js")).unwrap();
        assert!(worker.contains(&path));
        let page =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0/index.html")).unwrap();
        assert!(page.contains(&format!("https://u.github.io/repo/{path}")));
        let feed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(out.join("feed.json")).unwrap()).unwrap();
        assert_eq!(
            feed["items"][0]["image"],
            format!("https://u.github.io/repo/{path}")
        );
    }

    #[test]
    fn youtube_article_poster_keeps_feed_thumbnail_small_and_local() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "");
        let mut item = store.items().unwrap().remove(0);
        let (directory, stem) = item.path.rsplit_once('/').unwrap();
        item.front.link = "https://www.youtube.com/watch?v=video123".into();
        item.body = "Video description.\n".into();
        let pixels = image::RgbaImage::from_pixel(1280, 720, image::Rgba([21, 82, 109, 255]));
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(pixels)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let asset = crate::media::prepare_asset(
            &crate::sources::youtube::poster_candidates(
                &url::Url::parse(&item.front.link).unwrap(),
            )[0],
            encoded.into_inner(),
            &crate::media::MediaLimits::default(),
        )
        .unwrap();
        let preview = crate::preview::thumbnail(&asset.master_bytes, None).unwrap();
        item.front.images = vec![asset.metadata(stem)];
        item.front.preview = Some(preview.metadata(stem));
        store
            .write_item(crate::store::NewItem {
                dir: directory,
                stem,
                front: &item.front,
                body: &item.body,
                html: None,
                preview: Some(&preview.bytes),
                images: std::slice::from_ref(&asset),
            })
            .unwrap();
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        let page =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0/index.html")).unwrap();
        assert!(page.contains("video-preview"), "{page}");
        assert!(page.contains("width=\"1280\" height=\"720\""), "{page}");
        assert!(page.contains("640w"), "{page}");
        assert!(page.contains("1280w"), "{page}");
        assert!(!page.contains("src=\"https://i.ytimg.com"), "{page}");
        let feed = std::fs::read_to_string(out.join("index.html")).unwrap();
        assert!(!feed.contains("assets/images/"), "{feed}");
        assert!(feed.contains("width=\"256\" height=\"144\""), "{feed}");
        let export: serde_json::Value =
            serde_json::from_slice(&std::fs::read(out.join("feed.json")).unwrap()).unwrap();
        assert!(
            export["items"][0]["image"]
                .as_str()
                .unwrap()
                .contains("/assets/previews/")
        );
        assert_eq!(
            store
                .read_preview(&store.items().unwrap()[0])
                .unwrap()
                .unwrap(),
            preview.bytes
        );
    }

    #[test]
    fn build_publishes_lossless_article_images_without_changing_portable_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "pwa = true\n");
        let mut item = store.items().unwrap().remove(0);
        let (directory, stem) = item.path.rsplit_once('/').unwrap();
        let source = "https://blog.example/article-diagram.png";
        item.body = format!("Before.\n\n![Article diagram]({source})\n\nAfter.\n");
        let pixels = image::RgbaImage::from_pixel(640, 320, image::Rgba([21, 82, 109, 255]));
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(pixels)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let asset = crate::media::prepare_asset(
            &crate::media::Candidate {
                url: url::Url::parse(source).unwrap(),
                alt: Some("Article diagram".into()),
            },
            encoded.into_inner(),
            &crate::media::MediaLimits::default(),
        )
        .unwrap();
        item.front.images = vec![asset.metadata(stem)];
        store
            .write_item(crate::store::NewItem {
                dir: directory,
                stem,
                front: &item.front,
                body: &item.body,
                html: None,
                preview: None,
                images: std::slice::from_ref(&asset),
            })
            .unwrap();

        let out = dir.path().join("out");
        let mut build_info = info(out.clone());
        build_info.release = true;
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();

        let feed = std::fs::read_to_string(out.join("index.html")).unwrap();
        assert!(
            feed.contains("assets/previews/"),
            "archived media supplies missing preview: {feed}"
        );
        assert!(store.items().unwrap()[0].front.preview.is_none());

        let master = format!("assets/images/{}.png", asset.master_hash);
        assert_eq!(
            std::fs::read(out.join(&master)).unwrap(),
            asset.master_bytes
        );
        let page =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0/index.html")).unwrap();
        assert!(
            page.contains("<picture class=\"article-picture\""),
            "{page}"
        );
        assert!(page.contains(&format!("src=\"{master}\"")), "{page}");
        assert!(page.contains("type=\"image/webp\""), "{page}");
        assert!(page.contains("width=\"640\" height=\"320\""), "{page}");
        assert!(page.contains("fetchpriority=\"high\""), "{page}");
        for rendition in &asset.renditions {
            let path = format!("assets/images/{}.webp", rendition.hash);
            assert_eq!(std::fs::read(out.join(&path)).unwrap(), rendition.bytes);
            assert!(page.contains(&path), "{page}");
        }

        let markdown =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0.md")).unwrap();
        assert!(markdown.contains(source), "{markdown}");
        let json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(out.join("items/blog/2026-09-01-post-0.json")).unwrap(),
        )
        .unwrap();
        assert!(json["content_html"].as_str().unwrap().contains(source));
        assert!(!json["content_html"].as_str().unwrap().contains(&master));

        let worker = std::fs::read_to_string(out.join("sw.js")).unwrap();
        assert!(worker.contains(&master), "{worker}");
        for rendition in &asset.renditions {
            assert!(
                worker.contains(&format!("assets/images/{}.webp", rendition.hash)),
                "{worker}"
            );
        }
    }

    #[test]
    fn middle_pages_link_to_adjacent_and_edge_pages() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 5, "items_per_page = 1\npwa = false\n");
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();

        let middle = std::fs::read_to_string(out.join("page/3/index.html")).unwrap();
        assert!(middle.contains("href=\"./\">first</a>"), "{middle}");
        assert!(middle.contains("href=\"page/2/\">newer</a>"), "{middle}");
        assert!(middle.contains("href=\"page/4/\">older</a>"), "{middle}");
        assert!(middle.contains("href=\"page/5/\">last</a>"), "{middle}");
        assert!(
            middle.contains("<link rel=\"canonical\" href=\"https://u.github.io/repo/page/3/\">"),
            "{middle}"
        );
        assert!(
            middle.contains("<link rel=\"first\" href=\"./\">"),
            "{middle}"
        );
        assert!(
            middle.contains("<link rel=\"prev\" href=\"page/2/\">"),
            "{middle}"
        );
        assert!(
            middle.contains("<link rel=\"next\" href=\"page/4/\">"),
            "{middle}"
        );
        assert!(
            middle.contains("<link rel=\"last\" href=\"page/5/\">"),
            "{middle}"
        );

        let second = std::fs::read_to_string(out.join("page/2/index.html")).unwrap();
        assert!(second.contains("href=\"./\">first</a>"), "{second}");
        assert!(second.contains("href=\"./\">newer</a>"), "{second}");
    }

    #[test]
    fn pwa_outputs_cover_the_shells_and_the_newest_items() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 3, "preferences.offline_items = 2\n");
        let out = dir.path().join("out");
        let summary = build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        assert_eq!(summary.items, 3);
        // River, the source archive, six utility/directory pages, and the offline page.
        assert_eq!(summary.pages, 1 + 1 + 6 + 1);
        assert!(!out.join("categories/blog").exists());
        let home = std::fs::read_to_string(out.join("index.html")).unwrap();
        assert!(home.contains("href=\"browse/\""), "{home}");
        assert!(home.contains(">browse</"), "{home}");
        let document = scraper::Html::parse_document(&home);
        let primary = scraper::Selector::parse(".nav-primary a").unwrap();
        assert_eq!(
            document
                .select(&primary)
                .filter_map(|link| link.value().attr("href"))
                .collect::<Vec<_>>(),
            ["", "browse/", "preferences/"]
        );
        let actions = scraper::Selector::parse(".nav-actions a").unwrap();
        assert_eq!(document.select(&actions).count(), 1);
        assert!(!home.contains("<dd>Categories</dd>"), "{home}");
        let browse = std::fs::read_to_string(out.join("browse/index.html")).unwrap();
        assert!(browse.contains("id=\"sources\""), "{browse}");
        assert!(browse.contains("id=\"tags\""), "{browse}");
        assert!(browse.contains("id=\"categories\""), "{browse}");
        assert!(browse.contains("0 categories."), "{browse}");
        assert!(browse.contains("0 tags."), "{browse}");
        assert!(browse.contains("in <code>aggr.toml</code>"), "{browse}");
        assert!(
            browse.contains("href=\"./?q=source%3A%22blog%22\""),
            "{browse}"
        );
        assert!(!browse.contains("explore-nav"), "{browse}");
        assert!(!browse.contains("directory-count"), "{browse}");
        assert!(!browse.contains("directory-status"), "{browse}");
        for removed in ["explore", "library", "search"] {
            assert!(!out.join(removed).exists());
        }

        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(out.join("manifest.webmanifest")).unwrap())
                .unwrap();
        assert_eq!(manifest["name"], "Demo <site>");
        assert_eq!(manifest["short_name"], "aggr");
        assert_eq!(
            manifest["description"],
            "Browse Demo <site>, an independent, searchable archive of readable snapshots from followed feeds, preserved in Git with aggr."
        );
        assert_eq!(manifest["start_url"], "./");
        assert_eq!(manifest["scope"], "./");
        let browse_shortcut = manifest["shortcuts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|shortcut| shortcut["name"] == "Browse")
            .unwrap();
        assert_eq!(browse_shortcut["url"], "browse/");
        assert_eq!(manifest["display"], "standalone");
        assert_eq!(
            manifest["display_override"],
            serde_json::json!(["standalone", "minimal-ui"])
        );
        assert_eq!(manifest["background_color"], "#f5f6fa");
        assert_eq!(manifest["icons"].as_array().unwrap().len(), 4);
        assert!(
            manifest["icons"][0]["src"]
                .as_str()
                .unwrap()
                .starts_with("assets/icon-192-")
        );
        for icon in [
            "icon-192-",
            "icon-512-",
            "icon-maskable-512-",
            "apple-touch-icon-",
        ] {
            let path = std::fs::read_dir(out.join("assets"))
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| {
                    path.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with(icon)
                })
                .unwrap();
            let png = std::fs::read(path).unwrap();
            assert!(png.starts_with(b"\x89PNG"), "{icon} is not a PNG");
            assert_eq!(png.get(25), Some(&2), "{icon} must be an opaque RGB PNG");
        }

        let sw = std::fs::read_to_string(out.join("sw.js")).unwrap();
        let version = sw
            .split("var VERSION = \"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        assert_eq!(version.len(), 40);
        assert!(version.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(sw.contains("new URL(\"./\", self.registration.scope)"));
        assert!(sw.contains("\"assets/style-"));
        assert!(!sw.contains("\"pagefind/pagefind.js\""));
        assert!(!out.join("search-meta.json").exists());
        assert!(
            !sw.contains("assets/logo-"),
            "the multi-megabyte source icon is not precached"
        );
        assert!(sw.contains("navigationPreload.enable"));
        assert!(sw.contains("x-requested-with"));
        assert!(sw.contains("function firstCached(request, choices, index)"));
        assert!(!sw.contains("caches.match("));
        assert!(sw.contains("var REQUIRED_URLS ="));
        assert!(sw.contains("var REVISIONS = CACHE_NAMESPACE + \"revisions-\" + VERSION"));
        assert!(sw.contains("if (!response || !response.ok)"));
        assert!(sw.contains("migratePrecacheAssets"));
        assert!(sw.contains("var ASSETS = CACHE_NAMESPACE + \"assets\""));
        assert!(sw.contains("feed|aggr|linkset"), "{sw}");
        assert!(sw.contains("\"offline.html\""));
        assert!(sw.contains("\"browse/\""));
        assert!(sw.contains("\"sources/blog/\""));
        let catalog: serde_json::Value = serde_json::from_str(
            sw.split("var OFFLINE_CATALOG = ")
                .nth(1)
                .unwrap()
                .split(";\n")
                .next()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(catalog.as_array().unwrap().len(), 3);
        assert_eq!(catalog[0]["url"], "items/blog/2026-09-03-post-2/");
        assert!(sw.contains("var OFFLINE_COUNT = 2;"));
        assert!(
            catalog[0]["resources"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["url"] == catalog[0]["url"])
        );

        let offline = std::fs::read_to_string(out.join("offline.html")).unwrap();
        assert!(offline.contains("id=\"offline-articles\""));
        assert!(offline.contains("id=\"offline-download-status\""));
        assert!(offline.contains("<link rel=\"manifest\" href=\"manifest.webmanifest\">"));
        assert!(offline.contains("pwa: true"));
        assert!(offline.contains("<base id=\"aggr-base\" href=\"/repo/\">"));
        assert!(!offline.contains("rel=\"canonical\""));
        assert!(!offline.contains("application/ld+json"));
        let not_found = std::fs::read_to_string(out.join("404.html")).unwrap();
        assert!(not_found.contains("<base id=\"aggr-base\" href=\"/repo/\">"));
        assert!(!not_found.contains("rel=\"canonical\""));
        assert!(!not_found.contains("property=\"og:url\""));
        assert!(!not_found.contains("application/ld+json"));
        let river = std::fs::read_to_string(out.join("index.html")).unwrap();
        assert!(
            river.contains("<title>Demo &lt;site&gt; | aggr</title>"),
            "{river}"
        );
        assert!(
            river.contains("<meta name=\"application-name\" content=\"Demo &lt;site&gt;\">"),
            "{river}"
        );
        assert!(
            river.contains(
                "<meta name=\"apple-mobile-web-app-title\" content=\"Demo &lt;site&gt;\">"
            ),
            "{river}"
        );
        assert!(
            river.contains(
                "<meta name=\"description\" content=\"Browse Demo &lt;site&gt;, an independent, searchable archive of readable snapshots from followed feeds, preserved in Git with aggr.\">"
            ),
            "{river}"
        );
        assert!(river.contains("type=\"application/ld+json\""), "{river}");
        assert!(!river.contains("#identity"), "{river}");
        assert!(river.contains("https://schema.org"), "{river}");
        assert!(river.contains("rel=\"search\""), "{river}");
        assert!(river.contains("rel=\"service-meta\""), "{river}");
        assert!(river.contains("rel=\"linkset\""), "{river}");
        assert!(river.contains("rel=\"type\""), "{river}");
        assert!(river.contains("name=\"aggr:network\""), "{river}");
        assert!(
            river.contains("name=\"robots\" content=\"index,follow,"),
            "{river}"
        );
        assert!(river.contains("rel=\"apple-touch-icon\""));
        assert!(river.contains("name=\"theme-color\""));
        assert!(river.contains("href=\"browse/\""), "{river}");
        assert!(river.contains("href=\"preferences/\""), "{river}");
        assert!(river.contains("aggr.toml ↗</a>"), "{river}");
        assert!(out.join("preferences/index.html").is_file());
        assert!(!out.join("settings/index.html").exists());
        assert!(!out.join("pagefind/pagefind.js").exists());
        for path in ["preferences/index.html", "offline.html", "404.html"] {
            let utility = std::fs::read_to_string(out.join(path)).unwrap();
            assert!(
                utility.contains("name=\"robots\" content=\"noindex,follow\""),
                "{path}: {utility}"
            );
        }
        let descriptor: serde_json::Value =
            serde_json::from_slice(&std::fs::read(out.join("aggr.json")).unwrap()).unwrap();
        assert_eq!(descriptor["type"], "aggr-instance");
        assert_eq!(
            descriptor["network"],
            "https://github.com/aymericbeaumet/aggr#network"
        );
        assert_eq!(descriptor["url"], "https://u.github.io/repo/");
        assert!(
            descriptor["source"]["config"]
                .as_str()
                .is_some_and(|url| url.ends_with("/aggr.toml")),
            "{descriptor}"
        );
        assert_eq!(
            descriptor["feeds"]["json"],
            "https://u.github.io/repo/feed.json"
        );
        assert_eq!(
            descriptor["discovery"]["linkset"],
            "https://u.github.io/repo/linkset.json"
        );
        let linkset: serde_json::Value =
            serde_json::from_slice(&std::fs::read(out.join("linkset.json")).unwrap()).unwrap();
        let original = linkset["linkset"]
            .as_array()
            .unwrap()
            .iter()
            .find(|context| context["anchor"] == "https://blog.example/2")
            .unwrap();
        assert_eq!(
            original["https://schema.org/archivedAt"][0]["href"],
            "https://u.github.io/repo/items/blog/2026-09-03-post-2/"
        );
        assert!(out.join("sources/blog/atom.xml").is_file());
        assert!(out.join("sources/blog/rss.xml").is_file());
        assert!(out.join("sources/blog/feed.json").is_file());
        let atom = std::fs::read_to_string(out.join("sources/blog/atom.xml")).unwrap();
        assert!(atom.contains("xml:lang=\"en\""), "{atom}");
        assert!(atom.contains("<id>urn:aggr:item:"), "{atom}");
        assert!(atom.contains("<link rel=\"via\""), "{atom}");
        assert!(atom.contains("<content type=\"html\">"), "{atom}");
        let opensearch = std::fs::read_to_string(out.join("opensearch.xml")).unwrap();
        assert!(
            opensearch.contains("https://u.github.io/repo/?q={searchTerms}"),
            "{opensearch}"
        );
        let sitemap = std::fs::read_to_string(out.join("sitemap.xml")).unwrap();
        assert!(
            sitemap.contains("https://u.github.io/repo/browse/"),
            "{sitemap}"
        );
        assert!(
            sitemap.contains("<loc>https://u.github.io/repo/sources/</loc>"),
            "{sitemap}"
        );
        assert!(
            sitemap.contains("https://u.github.io/repo/items/blog/2026-09-03-post-2/"),
            "{sitemap}"
        );
        assert!(
            !out.join("robots.txt").exists(),
            "a robots file under a project subpath cannot govern the origin"
        );
        let item =
            std::fs::read_to_string(out.join("items/blog/2026-09-03-post-2/index.html")).unwrap();
        assert!(
            item.contains("<link rel=\"canonical\" href=\"https://u.github.io/repo/items/blog/2026-09-03-post-2/\">"),
            "{item}"
        );
        assert!(
            item.contains("<link rel=\"via\" href=\"https://blog.example/2\">"),
            "{item}"
        );
        assert!(
            item.contains("<link rel=\"original\" href=\"https://blog.example/2\">"),
            "{item}"
        );
        assert!(!item.contains("class=\"archive-note\""), "{item}");
        assert!(!item.contains("Git record <code>"), "{item}");
        assert!(item.contains("class=\"article-more\""), "{item}");
        assert!(item.contains("isBasedOn"), "{item}");
        assert!(item.contains("ArchiveComponent"), "{item}");
        assert!(item.contains("archivedAt"), "{item}");
        assert!(!item.contains("BlogPosting"), "{item}");
        let schema = item
            .split_once("<script type=\"application/ld+json\">")
            .and_then(|(_, rest)| rest.split_once("</script>"))
            .map(|(json, _)| serde_json::from_str::<serde_json::Value>(json).unwrap())
            .unwrap();
        let html_snapshot = schema["@graph"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| {
                node["@type"]
                    .as_array()
                    .is_some_and(|types| types.iter().any(|kind| kind == "ArchiveComponent"))
            })
            .unwrap();
        let html_original = schema["@graph"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["@id"] == "https://blog.example/2")
            .unwrap();
        assert!(
            html_original["wordCount"]
                .as_u64()
                .is_some_and(|count| count > 0)
        );
        assert!(
            html_original["timeRequired"]
                .as_str()
                .is_some_and(|duration| duration.starts_with("PT") && duration.ends_with('M'))
        );
        let json_snapshot: serde_json::Value = serde_json::from_slice(
            &std::fs::read(out.join("items/blog/2026-09-03-post-2.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(html_snapshot["@id"], json_snapshot["@id"]);
        assert_eq!(html_original["wordCount"], json_snapshot["word_count"]);
        assert_eq!(
            html_original["timeRequired"],
            json_snapshot["mainEntity"]["timeRequired"]
        );
        assert_eq!(
            json_snapshot["@id"],
            "https://u.github.io/repo/items/blog/2026-09-03-post-2/#webpage"
        );
    }

    #[test]
    fn rendered_titles_preserve_article_and_source_case_with_lowercase_topics() {
        let dir = tempfile::tempdir().unwrap();
        let (config, mut sources, store) = fixture(dir.path(), 1, "");
        sources[0].name = Some("The Example Blog".into());
        sources[0].category = Some("Computer SCIENCE".into());
        let mut item = store.items().unwrap().remove(0);
        item.front.labels = vec!["#Rust".into(), "RUST".into(), "Generative AI".into()];
        store
            .write_item(crate::store::NewItem {
                dir: "items/blog/2026/09",
                stem: "2026-09-01-post-0",
                front: &item.front,
                body: &item.body,
                html: None,
                preview: None,
                images: &[],
            })
            .unwrap();
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        for (path, title) in [
            ("items/blog/2026-09-01-post-0/index.html", "Post 0"),
            ("sources/blog/index.html", "The Example Blog"),
            ("categories/computer-science/index.html", "computer science"),
            ("tags/rust/index.html", "rust"),
            ("tags/generative-ai/index.html", "generative ai"),
            ("browse/index.html", "browse"),
        ] {
            let html = std::fs::read_to_string(out.join(path)).unwrap();
            assert!(
                html.contains(&format!(
                    "<title>{title} | Demo &lt;site&gt; | aggr</title>"
                )),
                "{path}: {html}"
            );
            assert!(!html.contains("Computer SCIENCE"), "{path}");
            assert!(!html.contains("Generative AI"), "{path}");
            assert!(!html.contains(">#Rust<"), "{path}");
        }
        let article =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0/index.html")).unwrap();
        assert!(article.contains("content=\"computer science\""));
        assert!(article.contains(">#rust</a>"));
        assert!(article.contains(">#generative ai</a>"));
    }

    #[test]
    fn seo_metadata_is_configurable_unique_and_identifies_the_publisher() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(
            dir.path(),
            2,
            r#"description = "Independent reading notes and useful links."
items_per_page = 1
repository = "owner/reader"

[site.identity]
type = "person"
name = "Ada Example"
url = "https://example.com/ada"
same_as = ["https://social.example/@ada"]
"#,
        );
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();

        let home = std::fs::read_to_string(out.join("index.html")).unwrap();
        assert!(
            home.contains("<title>Demo &lt;site&gt; | aggr</title>"),
            "{home}"
        );
        assert!(
            home.contains(
                "<meta name=\"description\" content=\"Independent reading notes and useful links.\">"
            ),
            "{home}"
        );

        let second = std::fs::read_to_string(out.join("page/2/index.html")).unwrap();
        assert!(
            second.contains("<title>Demo &lt;site&gt; — page 2 | aggr</title>"),
            "{second}"
        );
        assert!(
            second.contains(
                "<meta name=\"description\" content=\"Independent reading notes and useful links Page 2.\">"
            ),
            "{second}"
        );
        let source = std::fs::read_to_string(out.join("sources/blog/index.html")).unwrap();
        assert!(
            source.contains(
                "<meta name=\"description\" content=\"Browse retained readable snapshots from blog in Demo &lt;site&gt;, with original URLs and capture dates.\">"
            ),
            "{source}"
        );
        let item =
            std::fs::read_to_string(out.join("items/blog/2026-09-02-post-1/index.html")).unwrap();
        assert!(
            item.contains("<title>Post 1 | Demo &lt;site&gt; | aggr</title>"),
            "{item}"
        );
        assert!(
            item.contains("Archived readable snapshot of Post 1 from blog.example, first captured 2026-09-02 and preserved by Demo &lt;site&gt;."),
            "{item}"
        );

        let schema_start = "<script type=\"application/ld+json\">";
        let schema = home
            .split_once(schema_start)
            .and_then(|(_, rest)| rest.split_once("</script>"))
            .map(|(json, _)| serde_json::from_str::<serde_json::Value>(json).unwrap())
            .unwrap();
        let graph = schema["@graph"].as_array().unwrap();
        let identity = graph.iter().find(|node| node["@type"] == "Person").unwrap();
        assert_eq!(identity["name"], "Ada Example");
        assert_eq!(identity["url"], "https://example.com/ada");
        assert_eq!(identity["sameAs"][0], "https://social.example/@ada");
        let website = graph
            .iter()
            .find(|node| node["@type"] == "WebSite")
            .unwrap();
        assert_eq!(website["creator"]["@id"], identity["@id"]);
        assert_eq!(website["publisher"]["@id"], identity["@id"]);

        let llms = std::fs::read_to_string(out.join("llms.txt")).unwrap();
        assert!(llms.starts_with("# Demo <site>\n\n> Independent reading notes"));
        assert!(llms.contains("[Instance metadata](https://u.github.io/repo/aggr.json)"));
        assert!(llms.contains("[Sitemap](https://u.github.io/repo/sitemap.xml)"));
        assert!(
            llms.contains(
                "[Original URL to snapshot linkset](https://u.github.io/repo/linkset.json)"
            )
        );
        assert!(llms.contains("[Source configuration](https://raw.githubusercontent.com/"));
        assert!(!llms.contains("[Categories]"), "{llms}");
    }

    #[test]
    fn sitemap_uses_local_capture_time_for_a_newly_archived_old_article() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "pwa = false\n");
        let mut item = store.items().unwrap().remove(0);
        item.front.replicated_at = Some(day(15));
        let stem = item.path.rsplit('/').next().unwrap();
        let item_dir = item.path.rsplit_once('/').unwrap().0;
        store
            .write_item(crate::store::NewItem {
                dir: item_dir,
                stem,
                front: &item.front,
                body: &item.body,
                html: None,
                preview: None,
                images: &[],
            })
            .unwrap();

        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        let sitemap = std::fs::read_to_string(out.join("sitemap.xml")).unwrap();
        let entry = sitemap
            .split("<url>")
            .find(|entry| entry.contains("items/blog/2026-09-01-post-0/"))
            .unwrap();
        assert!(
            entry.contains("<lastmod>2026-09-15T00:00:00Z</lastmod>"),
            "{entry}"
        );
        let page =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0/index.html")).unwrap();
        assert!(page.contains("first captured"), "{page}");
        assert!(
            page.contains("\"dateCreated\":\"2026-09-15T00:00:00+00:00\""),
            "{page}"
        );
        assert!(!page.contains("replicated here"), "{page}");
        assert!(!page.contains("class=\"archive-note\""), "{page}");
    }

    #[test]
    fn config_navigation_uses_github_page_while_metadata_keeps_raw_url() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(
            dir.path(),
            1,
            "repository = \"owner/reader\"\npwa = false\n",
        );
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();

        let home = std::fs::read_to_string(out.join("index.html")).unwrap();
        let sha = "c".repeat(40);
        assert!(
            home.contains(&format!(
                "href=\"https://github.com/owner/reader/blob/{sha}/aggr.toml\""
            )),
            "{home}"
        );
        assert!(
            home.contains(&format!(
                "name=\"aggr:source\" content=\"https://raw.githubusercontent.com/owner/reader/{sha}/aggr.toml\""
            )),
            "{home}"
        );
    }

    #[test]
    fn origin_root_build_advertises_its_sitemap_in_robots() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "pwa = false\n");
        let out = dir.path().join("out");
        let mut build_info = info(out.clone());
        build_info.base_url = Some("https://reads.example/".into());
        build(&config, &sources, &store, dir.path(), &build_info).unwrap();

        let robots = std::fs::read_to_string(out.join("robots.txt")).unwrap();
        assert_eq!(
            robots,
            "User-agent: *\nAllow: /\nSitemap: https://reads.example/sitemap.xml\n"
        );
    }

    #[test]
    fn pwa_off_writes_no_manifest_worker_or_offline_page() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "pwa = false\n");
        let out = dir.path().join("out");
        let summary = build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        assert_eq!(summary.pages, 1 + 1 + 6);
        for name in ["manifest.webmanifest", "sw.js", "offline.html"] {
            assert!(!out.join(name).exists(), "{name} was written");
        }
        let river = std::fs::read_to_string(out.join("index.html")).unwrap();
        assert!(!river.contains("rel=\"manifest\""));
        assert!(!river.contains("mobile-web-app-capable"));
        assert!(!river.contains("apple-mobile-web-app-capable"));
        assert!(river.contains("pwa: false"));
    }

    #[test]
    fn interactive_originals_keep_fallbacks_and_document_alternates_keep_original_identity() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 2, "pwa = false\n");
        let mut items = store.items().unwrap();
        items.sort_by(|left, right| left.path.cmp(&right.path));
        items[0]
            .front
            .extra
            .insert("content:interactive".into(), true.into());
        items[1].front.extra.insert(
            "document_url".into(),
            "https://papers.test/verified.pdf".into(),
        );
        for item in &items {
            let (directory, stem) = item.path.rsplit_once('/').unwrap();
            store
                .write_item(crate::store::NewItem {
                    dir: directory,
                    stem,
                    front: &item.front,
                    body: &item.body,
                    html: None,
                    preview: None,
                    images: &[],
                })
                .unwrap();
        }
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        let interactive =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0/index.html")).unwrap();
        assert!(
            interactive.contains("data-interactive-embed=\"https://blog.example/0\""),
            "{interactive}"
        );
        assert!(!interactive.contains("<iframe"));
        assert!(interactive.contains("if the interactive view is unavailable"));
        let document =
            std::fs::read_to_string(out.join("items/blog/2026-09-02-post-1/index.html")).unwrap();
        assert!(document.contains("src=\"https://papers.test/verified.pdf\""));
        assert!(document.contains("href=\"https://blog.example/1\""));
        let feed = std::fs::read_to_string(out.join("index.html")).unwrap();
        assert!(!feed.contains("data-interactive-embed"));
        assert!(!feed.contains("<embed"));
    }

    #[test]
    fn pdf_articles_embed_documents_with_fallback_without_loading_them_in_feeds() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "pwa = false\n");
        let mut item = store.items().unwrap().remove(0);
        let (directory, stem) = item.path.rsplit_once('/').unwrap();
        item.front.link =
            "https://example.com/paper.PDF?token=one&filename=paper.pdf#page=2".into();
        item.front.content = crate::model::ContentKind::None;
        store
            .write_item(crate::store::NewItem {
                dir: directory,
                stem,
                front: &item.front,
                body: "",
                html: None,
                preview: None,
                images: &[],
            })
            .unwrap();
        let out = dir.path().join("out");
        build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        let page =
            std::fs::read_to_string(out.join("items/blog/2026-09-01-post-0/index.html")).unwrap();
        assert!(page.contains("<embed class=\"document-viewer\""), "{page}");
        assert!(page.contains("type=\"application/pdf\""), "{page}");
        assert!(
            page.contains(
                "src=\"https://example.com/paper.PDF?token=one&amp;filename=paper.pdf#page=2\""
            ),
            "{page}"
        );
        assert!(page.contains("Open PDF"), "{page}");
        assert!(page.contains("if the viewer is unavailable"), "{page}");
        assert!(
            !page.contains("This source publishes titles only"),
            "{page}"
        );
        let river = std::fs::read_to_string(out.join("index.html")).unwrap();
        assert!(!river.contains("<embed"), "{river}");
        assert!(!river.contains("<object"), "{river}");
    }
}
