//! Item types shared by sources, the store, and the site, plus the pure rules that derive
//! identities from them: dedupe keys, normalized links, file names, git blob hashes.

use std::collections::BTreeMap;

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
    pub authors: Vec<String>,
    pub tags: Vec<String>,
    /// Plain text.
    pub summary: Option<String>,
    /// HTML as delivered by the source.
    pub content_html: Option<String>,
    pub extra: BTreeMap<String, serde_yaml_ng::Value>,
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
    pub first_seen: DateTime<Utc>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "is_default")]
    pub content: ContentKind,
    /// File name of the raw HTML sibling, when one was written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    #[serde(skip_serializing_if = "is_default")]
    pub html_truncated: bool,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, serde_yaml_ng::Value>,
    /// User-owned flags, only ever set by editing the file.
    #[serde(skip_serializing_if = "is_default")]
    pub starred: bool,
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
    pub fn sort_date(&self) -> DateTime<Utc> {
        self.front.published.unwrap_or(self.front.first_seen)
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

/// Canonical form of a link for dedupe: https, no `www.`, tracking parameters and fragment
/// dropped, sorted query, no trailing slash.
pub fn normalize_link(link: &str) -> String {
    let Ok(mut url) = url::Url::parse(link.trim()) else {
        return link.trim().to_string();
    };
    if url.scheme() == "http" {
        let _ = url.set_scheme("https");
    }
    if let Some(host) = url.host_str() {
        let host = host.to_ascii_lowercase();
        let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
        let _ = url.set_host(Some(&host));
    }
    let _ = url.set_port(None);
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

/// Append `-2`, `-3`, … until `taken` does not contain the stem.
pub fn unique_stem(stem: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(stem) {
        return stem.to_string();
    }
    (2..)
        .map(|n| format!("{stem}-{n}"))
        .find(|candidate| !taken(candidate))
        .expect("unbounded")
}

/// `items/<slug>/<yyyy>/<mm>` for an item dated `date`.
pub fn item_dir(source_slug: &str, date: DateTime<Utc>) -> String {
    format!("items/{source_slug}/{:04}/{:02}", date.year(), date.month())
}

/// What `git hash-object` prints for these bytes.
pub fn git_blob_sha(bytes: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(format!("blob {}\0", bytes.len()).as_bytes());
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

pub fn sha1_hex(input: impl AsRef<[u8]>) -> String {
    hex::encode(Sha1::digest(input.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

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
        assert_eq!(unique_stem("b", |s| taken.contains(&s)), "b");
        assert_eq!(unique_stem("a", |s| taken.contains(&s)), "a-3");
    }

    #[test]
    fn item_dir_uses_year_and_month() {
        assert_eq!(item_dir("rust", date(2026, 9, 2)), "items/rust/2026/09");
    }

    #[test]
    fn git_blob_sha_matches_git() {
        // `printf 'hello\n' | git hash-object --stdin`
        assert_eq!(
            git_blob_sha(b"hello\n"),
            "ce013625030ba8dba906f756967f9e9ca394464a"
        );
        assert_eq!(
            git_blob_sha(b""),
            "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391"
        );
    }
}
