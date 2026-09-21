//! Item types shared by sources, the store, and the site, plus the pure rules that derive
//! identities from them: dedupe keys, normalized links, file names, git blob hashes.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

/// What a source engine produces for one entry, before the store decides whether it is new.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RawItem {
    pub id: Option<String>,
    pub title: String,
    pub link: String,
    pub published: Option<DateTime<Utc>>,
    pub updated: Option<DateTime<Utc>>,
    /// When the first aggr in a replication lineage retained this item.
    pub first_seen: Option<DateTime<Utc>>,
    pub authors: Vec<String>,
    pub labels: Vec<String>,
    /// Plain text.
    pub summary: Option<String>,
    /// HTML as delivered by the source.
    pub content_html: Option<String>,
    /// Explicit feed images, followed by article metadata and body fallbacks.
    pub preview_candidates: Vec<crate::preview::Candidate>,
    /// Already encoded local bytes; replicas carry these without contacting the publisher.
    pub preview: Option<crate::preview::Thumbnail>,
    /// Exact article image masters and verified lossless renditions for newly retained items.
    pub images: Vec<crate::media::Asset>,
    /// A bounded PDF retained alongside the article, including bytes carried by replicas.
    pub document: Option<crate::document::Asset>,
    pub extra: BTreeMap<String, serde_yaml_ng::Value>,
}

impl RawItem {
    /// Best available ordering time without conflating an aggr capture with publication.
    pub fn created_at(&self) -> Option<DateTime<Utc>> {
        self.published.or(self.updated).or(self.first_seen)
    }
}

pub fn normalize_category(category: &str) -> Option<String> {
    let normalized = category
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    (!normalized.is_empty()).then_some(normalized)
}

/// Canonical labels used in front matter, indexes, feeds and templates. Providers disagree on
/// casing and occasionally include a presentation `#`; normalizing at the model boundary keeps
/// one stable taxonomy while still accepting hand-written historical items at render time.
pub fn normalize_labels(labels: impl IntoIterator<Item = impl AsRef<str>>) -> Vec<String> {
    labels
        .into_iter()
        .filter_map(|label| {
            let collapsed = label
                .as_ref()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let normalized = collapsed.trim_start_matches('#').trim().to_lowercase();
            (!normalized.is_empty()).then_some(normalized)
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Where the Markdown body came from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContentKind {
    Feed,
    Extracted,
    #[default]
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preview {
    pub file: String,
    pub width: u32,
    pub height: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

fn deserialize_preview<'de, D>(deserializer: D) -> Result<Option<Preview>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_yaml_ng::Value::deserialize(deserializer)?;
    Ok(serde_yaml_ng::from_value(value).ok())
}

impl Preview {
    /// Restrict companions to their owning item, including on Windows and untrusted mirrors.
    pub fn is_valid_for(&self, stem: &str) -> bool {
        let Some(suffix) = self.file.strip_prefix(&format!("{stem}.preview-")) else {
            return false;
        };
        let Some((hash, extension)) = suffix.rsplit_once('.') else {
            return false;
        };
        !stem.is_empty()
            && !self.file.contains(['/', '\\', ':'])
            && !self.file.starts_with('.')
            && hash.len() == 12
            && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            && matches!(extension, "jpg" | "webp")
            && self.width > 0
            && self.height > 0
            && self.width <= 320
            && self.height <= 320
            && self.color.as_deref().is_none_or(|color| {
                color.len() == 7
                    && color.starts_with('#')
                    && color[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageFile {
    pub file: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArticleImage {
    /// Absolute publisher URL kept in Markdown and used when a local companion is unavailable.
    pub source: String,
    /// Exact publisher bytes. Lossless renditions are optional enhancements, never replacements.
    pub original: ImageFile,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<ImageFile>,
    /// Immediate paint beneath the image while its bytes load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

fn deserialize_images<'de, D>(deserializer: D) -> Result<Vec<ArticleImage>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_yaml_ng::Value::deserialize(deserializer)?;
    Ok(value
        .as_sequence()
        .into_iter()
        .flatten()
        .filter_map(|image| serde_yaml_ng::from_value(image.clone()).ok())
        .collect())
}

impl ArticleImage {
    /// Restrict every companion to its owning item and to browser-safe raster formats.
    pub fn is_valid_for(&self, stem: &str) -> bool {
        static LIMITS: std::sync::LazyLock<crate::media::MediaLimits> =
            std::sync::LazyLock::new(crate::media::MediaLimits::default);
        let valid_source = url::Url::parse(&self.source).is_ok_and(|url| {
            matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.fragment().is_none()
        });
        let valid_file = |image: &ImageFile, variants_only: bool| {
            let Some(suffix) = image.file.strip_prefix(&format!("{stem}.image-")) else {
                return false;
            };
            let Some((hash, extension)) = suffix.rsplit_once('.') else {
                return false;
            };
            !stem.is_empty()
                && !image.file.contains(['/', '\\', ':'])
                && hash.len() == 12
                && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
                && if variants_only {
                    extension == "webp"
                } else {
                    matches!(extension, "jpg" | "png" | "gif" | "webp")
                }
                && image.width > 0
                && image.height > 0
                && image.width <= LIMITS.max_axis
                && image.height <= LIMITS.max_axis
                && u64::from(image.width) * u64::from(image.height) <= LIMITS.max_pixels
        };
        let valid_color = self.color.as_deref().is_none_or(|color| {
            color.len() == 7
                && color.starts_with('#')
                && color[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
        });
        let mut previous_width = 0;
        let mut files = BTreeSet::new();
        valid_source
            && valid_file(&self.original, false)
            && files.insert(&self.original.file)
            && valid_color
            && self.variants.iter().all(|variant| {
                let ordered = variant.width > previous_width;
                previous_width = variant.width;
                valid_file(variant, true)
                    && ordered
                    && variant.width <= self.original.width
                    && variant.height <= self.original.height
                    && files.insert(&variant.file)
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    pub file: String,
    pub source: String,
}

impl Document {
    /// Restrict the companion to this item and a credential-free HTTP(S) publisher URL.
    pub fn is_valid_for(&self, stem: &str) -> bool {
        let Some(hash) = self
            .file
            .strip_prefix(&format!("{stem}.document-"))
            .and_then(|suffix| suffix.strip_suffix(".pdf"))
        else {
            return false;
        };
        !stem.is_empty()
            && !self.file.starts_with('.')
            && !self.file.contains(['/', '\\', ':'])
            && hash.len() == 12
            && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            && url::Url::parse(&self.source).is_ok_and(|url| {
                matches!(url.scheme(), "http" | "https")
                    && url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.fragment().is_none()
            })
    }
}

fn deserialize_document<'de, D>(deserializer: D) -> Result<Option<Document>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_yaml_ng::Value::deserialize(deserializer)?;
    Ok(serde_yaml_ng::from_value(value).ok())
}

/// The YAML block at the top of every item file. Defaults are skipped on write so the table
/// GitHub renders stays short.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FrontMatter {
    pub title: String,
    pub link: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated: Option<DateTime<Utc>>,
    /// First capture anywhere in an aggr replication lineage.
    pub first_seen: DateTime<Utc>,
    /// Time this copy entered the current repository, for replicated aggr items.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replicated_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "is_default")]
    pub content: ContentKind,
    /// File name of the raw HTML sibling, when one was written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_document",
        skip_serializing_if = "Option::is_none"
    )]
    pub document: Option<Document>,
    #[serde(
        default,
        deserialize_with = "deserialize_preview",
        skip_serializing_if = "Option::is_none"
    )]
    pub preview: Option<Preview>,
    /// Exact publisher image companions plus optional verified lossless responsive renditions.
    #[serde(
        default,
        deserialize_with = "deserialize_images",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub images: Vec<ArticleImage>,
    #[serde(skip_serializing_if = "is_default")]
    pub html_truncated: bool,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, serde_yaml_ng::Value>,
    #[serde(skip_serializing_if = "is_default")]
    pub hidden: bool,
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// An item as read back from the store.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// `items/<slug>/<yyyy>/<mm>/<stem>` — the identity used by the site and localStorage.
    pub path: String,
    pub front: FrontMatter,
    /// Markdown body.
    pub body: String,
}

impl Item {
    /// Creation time supplied by the source, falling back to when aggr first saw the item.
    pub fn created_at(&self) -> DateTime<Utc> {
        self.front
            .published
            .or(self.front.updated)
            .unwrap_or(self.front.first_seen)
    }

    pub fn md_path(&self) -> String {
        format!("{}.md", self.path)
    }
}

/// Keys recorded in `seen.txt`. Any match means the entry was already stored (or deleted by hand).
pub fn dedupe_keys(item: &RawItem) -> Vec<String> {
    let mut keys = Vec::with_capacity(3);
    if let Some(id) = item
        .id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        keys.push(sha1_hex(format!("id:{id}")));
    }
    let link = normalize_link(&item.link);
    if !link.is_empty() {
        keys.push(sha1_hex(format!("link:{link}")));
    }
    if let Some(published) = item.published {
        let title = item.title.trim().to_lowercase();
        if !title.is_empty() {
            keys.push(sha1_hex(format!(
                "title:{title}|{}",
                published.to_rfc3339()
            )));
        }
    }
    keys.dedup();
    keys
}

const TRACKING_PARAMS: &[&str] = &[
    "fbclid", "gclid", "mc_cid", "mc_eid", "ref", "ref_src", "source", "yclid", "_hsenc", "_hsmi",
];

/// Canonical form for dedupe: default-port HTTP becomes HTTPS; nondefault ports keep their
/// scheme. Drop `www.`, tracking parameters, fragments and trailing slashes; sort query pairs.
pub fn normalize_link(link: &str) -> String {
    let Ok(mut url) = url::Url::parse(link.trim()) else {
        return link.trim().to_string();
    };
    // Upgrading HTTP on port 443 would erase that nondefault endpoint as HTTPS's default.
    if url.scheme() == "http" && url.port().is_none() {
        let _ = url.set_scheme("https");
    }
    if let Some(host) = url.host_str() {
        let host = host.to_ascii_lowercase();
        let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
        let _ = url.set_host(Some(&host));
    }
    url.set_fragment(None);
    let mut pairs: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(name, _)| !name.starts_with("utm_") && !TRACKING_PARAMS.contains(&name.as_ref()))
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    pairs.sort();
    if pairs.is_empty() {
        url.set_query(None);
    } else {
        let query = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(pairs)
            .finish();
        url.set_query(Some(&query));
    }
    let mut out = url.to_string();
    let path_end = out.len() - url.query().map_or(0, |q| q.len() + 1);
    if out[..path_end].ends_with('/') && url.path() != "/" {
        out.remove(path_end - 1);
    }
    out
}

const MAX_STEM_SLUG: usize = 60;

/// Names Windows refuses regardless of extension.
const WINDOWS_RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// `<yyyy-mm-dd>-<slug>` for an item file: lowercase ASCII, cut on a word boundary, safe on
/// every OS. Collisions inside a directory are resolved by [`unique_stem`].
pub fn file_stem(date: DateTime<Utc>, title: &str) -> String {
    let mut slug = slug::slugify(title);
    if slug.len() > MAX_STEM_SLUG {
        let cut = slug[..MAX_STEM_SLUG].rfind('-').unwrap_or(MAX_STEM_SLUG);
        slug.truncate(cut);
    }
    let slug = slug.trim_matches('-');
    let slug = if slug.is_empty() || WINDOWS_RESERVED.contains(&slug) {
        "item"
    } else {
        slug
    };
    format!("{}-{slug}", date.format("%Y-%m-%d"))
}

/// Resolve a same-day/title collision with a stable item-derived suffix. Numeric allocation makes
/// concurrent syncs choose paths according to arrival order; content-derived names converge.
pub fn unique_stem(stem: &str, identity: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(stem) {
        return stem.to_string();
    }
    let hash = sha1_hex(identity);
    for width in [8, 12, 16, hash.len()] {
        let candidate = format!("{stem}-{}", &hash[..width]);
        if !taken(&candidate) {
            return candidate;
        }
    }
    (2..)
        .map(|n| format!("{stem}-{hash}-{n}"))
        .find(|candidate| !taken(candidate))
        .expect("unbounded")
}

/// `items/<slug>/<yyyy>/<mm>` for an item dated `date`.
pub fn item_dir(source_slug: &str, date: DateTime<Utc>) -> String {
    format!("items/{source_slug}/{:04}/{:02}", date.year(), date.month())
}

pub fn sha1_hex(input: impl AsRef<[u8]>) -> String {
    hex::encode(Sha1::digest(input.as_ref()))
}

/// The alternative text an image carries, at capture and into the reader: publisher whitespace
/// runs collapsed, `None` when it was empty, and capped so a pasted paragraph does not become a
/// screen-reader monologue.
pub fn image_alt(alt: &str) -> Option<String> {
    const MAX_CHARS: usize = 300;
    let collapsed = alt.split_whitespace().collect::<Vec<_>>().join(" ");
    (!collapsed.is_empty()).then(|| collapsed.chars().take(MAX_CHARS).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn created_at_has_one_ordering_rule_before_and_after_storage() {
        let published = Utc.with_ymd_and_hms(2026, 9, 1, 9, 0, 0).unwrap();
        let updated = Utc.with_ymd_and_hms(2026, 9, 2, 9, 0, 0).unwrap();
        let seen = Utc.with_ymd_and_hms(2026, 9, 3, 9, 0, 0).unwrap();
        let raw = RawItem {
            published: Some(published),
            updated: Some(updated),
            ..Default::default()
        };
        assert_eq!(raw.created_at(), Some(published));
        let updated_only = RawItem {
            published: None,
            ..raw.clone()
        };
        assert_eq!(updated_only.created_at(), Some(updated));
        let seen_only = RawItem {
            updated: None,
            first_seen: Some(seen),
            ..updated_only
        };
        assert_eq!(seen_only.created_at(), Some(seen));

        let item = Item {
            path: "items/x/a".into(),
            front: FrontMatter {
                published: None,
                updated: Some(updated),
                first_seen: seen,
                ..Default::default()
            },
            body: String::new(),
        };
        assert_eq!(item.created_at(), updated);
        let undated = Item {
            front: FrontMatter {
                updated: None,
                ..item.front.clone()
            },
            ..item
        };
        assert_eq!(undated.created_at(), seen);
    }

    #[test]
    fn labels_are_lowercase_trimmed_sorted_and_unique() {
        assert_eq!(
            normalize_labels([" AI ", "#Rust", "rust", "Generative   AI", "", "  #  "]),
            ["ai", "generative ai", "rust"]
        );
    }

    fn date(y: i32, m: u32, d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, 12, 0, 0).unwrap()
    }

    #[test]
    fn normalizes_links() {
        assert_eq!(
            normalize_link("http://www.Example.com/a/b/?utm_source=x&b=2&a=1#frag"),
            "https://example.com/a/b?a=1&b=2"
        );
        assert_eq!(
            normalize_link("https://example.com/"),
            "https://example.com/"
        );
        assert_eq!(
            normalize_link("https://example.com:443/x/"),
            "https://example.com/x"
        );
        assert_eq!(
            normalize_link("https://example.com/x?ref=hn&fbclid=1"),
            "https://example.com/x"
        );
        assert_eq!(normalize_link("  not a url "), "not a url");
    }

    #[test]
    fn nondefault_ports_keep_distinct_link_identities() {
        for (link, expected) in [
            ("http://example.com:80/post/", "https://example.com/post"),
            ("https://example.com:443/post/", "https://example.com/post"),
            (
                "https://example.com:8443/post/",
                "https://example.com:8443/post",
            ),
            (
                "http://example.com:8080/post/",
                "http://example.com:8080/post",
            ),
            (
                "http://example.com:443/post/",
                "http://example.com:443/post",
            ),
        ] {
            assert_eq!(normalize_link(link), expected, "{link}");
        }
        let links = [
            "https://example.com/post",
            "https://example.com:8080/post",
            "https://example.com:9090/post",
            "http://example.com:443/post",
        ];
        let keys = links.map(|link| {
            dedupe_keys(&RawItem {
                link: link.into(),
                ..Default::default()
            })
        });
        for (index, key) in keys.iter().enumerate() {
            assert_eq!(key.len(), 1);
            for other in &keys[index + 1..] {
                assert_ne!(key, other);
            }
        }
    }

    #[test]
    fn dedupe_keys_cover_id_link_and_title() {
        let item = RawItem {
            id: Some("guid-1".into()),
            title: "Hello".into(),
            link: "https://example.com/post".into(),
            published: Some(date(2026, 9, 2)),
            ..Default::default()
        };
        let keys = dedupe_keys(&item);
        assert_eq!(keys.len(), 3);
        assert!(keys.iter().all(|k| k.len() == 40));

        let same_link_other_guid = RawItem {
            id: Some("guid-2".into()),
            link: "http://www.example.com/post/".into(),
            ..item.clone()
        };
        let other = dedupe_keys(&same_link_other_guid);
        assert!(
            other.iter().any(|k| keys.contains(k)),
            "link key must match"
        );

        let no_id = RawItem {
            id: None,
            published: None,
            ..item
        };
        assert_eq!(dedupe_keys(&no_id).len(), 1);
    }

    #[test]
    fn file_stems_are_safe_everywhere() {
        let d = date(2026, 9, 2);
        assert_eq!(
            file_stem(d, "Announcing Rust 1.96!"),
            "2026-09-02-announcing-rust-1-96"
        );
        assert_eq!(file_stem(d, "CON"), "2026-09-02-item");
        assert_eq!(file_stem(d, "nul."), "2026-09-02-item");
        assert_eq!(file_stem(d, "🎉🎉"), "2026-09-02-tada-tada");
        assert_eq!(file_stem(d, "!!! ???"), "2026-09-02-item");
        assert_eq!(file_stem(d, ""), "2026-09-02-item");
        assert_eq!(file_stem(d, "Ünïcödé Tïtle"), "2026-09-02-unicode-title");
        let long = "word ".repeat(100);
        let stem = file_stem(d, &long);
        assert!(stem.len() <= 11 + MAX_STEM_SLUG, "{stem}");
        assert!(!stem.ends_with('-'));
    }

    #[test]
    fn unique_stems_suffix_on_collision() {
        let taken = ["a", "a-2"];
        assert_eq!(unique_stem("b", "item-b", |s| taken.contains(&s)), "b");
        assert_eq!(
            unique_stem("a", "item-a", |s| taken.contains(&s)),
            format!("a-{}", &sha1_hex("item-a")[..8])
        );
    }

    #[test]
    fn item_dir_uses_year_and_month() {
        assert_eq!(item_dir("rust", date(2026, 9, 2)), "items/rust/2026/09");
    }

    #[test]
    fn article_image_companions_are_bound_to_their_item() {
        let stem = "2026-09-02-post";
        let image = ArticleImage {
            source: "https://example.com/image.png".into(),
            original: ImageFile {
                file: format!("{stem}.image-0123456789ab.png"),
                width: 1200,
                height: 800,
            },
            variants: vec![ImageFile {
                file: format!("{stem}.image-fedcba987654.webp"),
                width: 480,
                height: 320,
            }],
            color: Some("#285a8c".into()),
        };
        assert!(image.is_valid_for(stem));

        let mut unsafe_image = image.clone();
        unsafe_image.original.file = "../outside.png".into();
        assert!(!unsafe_image.is_valid_for(stem));

        let mut wrong_type = image.clone();
        wrong_type.variants[0].file = format!("{stem}.image-fedcba987654.svg");
        assert!(!wrong_type.is_valid_for(stem));

        let mut upscale = image;
        upscale.variants[0].width = 1600;
        assert!(!upscale.is_valid_for(stem));
    }

    #[test]
    fn article_image_dimensions_match_archive_limits() {
        let image = |width, height| ArticleImage {
            source: "https://example.com/figure.jpg".into(),
            original: ImageFile {
                file: "article.image-0123456789ab.jpg".into(),
                width,
                height,
            },
            variants: Vec::new(),
            color: None,
        };
        assert!(image(15_197, 8_488).is_valid_for("article"));
        assert!(image(17_277, 11_171).is_valid_for("article"));
        assert!(image(20_000, 10_000).is_valid_for("article"));
        assert!(image(24_000, 100).is_valid_for("article"));
        assert!(!image(24_001, 100).is_valid_for("article"));
        assert!(!image(20_000, 10_001).is_valid_for("article"));
        assert!(!image(0, 100).is_valid_for("article"));
        assert!(!image(u32::MAX, u32::MAX).is_valid_for("article"));
    }

    #[test]
    fn front_matter_uses_labels_without_interpreting_obsolete_tags() {
        let front: FrontMatter =
            serde_yaml_ng::from_str("labels: [current]\ntags: [obsolete]\n").unwrap();
        assert_eq!(front.labels, ["current"]);
        let obsolete: FrontMatter = serde_yaml_ng::from_str("tags: [obsolete]\n").unwrap();
        assert!(obsolete.labels.is_empty());
    }

    #[test]
    fn malformed_optional_image_metadata_does_not_hide_an_article() {
        let front: FrontMatter = serde_yaml_ng::from_str(
            r##"
title: Still readable
images:
  - source: [not, a, url]
  - source: https://example.com/image.png
    original:
      file: post.image-0123456789ab.png
      width: 640
      height: 320
    color: "#285a8c"
"##,
        )
        .unwrap();
        assert_eq!(front.title, "Still readable");
        assert_eq!(front.images.len(), 1);
        assert_eq!(front.images[0].source, "https://example.com/image.png");

        let malformed: FrontMatter =
            serde_yaml_ng::from_str("title: Still readable\nimages: broken\n").unwrap();
        assert!(malformed.images.is_empty());
    }

    #[test]
    fn image_alt_collapses_whitespace_drops_empty_text_and_caps_length() {
        let long = "é".repeat(400);
        for (alt, expected) in [
            (
                "  A   diagram\n of the\tpipeline ",
                Some("A diagram of the pipeline".to_string()),
            ),
            ("   ", None),
            ("", None),
            ("Chart", Some("Chart".to_string())),
            (long.as_str(), Some("é".repeat(300))),
        ] {
            assert_eq!(image_alt(alt), expected, "{alt:?}");
        }
    }
}
