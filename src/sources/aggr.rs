//! `type = "aggr"`: another aggr repository as a source. Its data branch is mirrored with a
//! depth-1 clone (no history needed) and its newest items are re-published here. Items keep
//! their original link, so a feed both sites follow still dedupes to one entry.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde_yaml_ng::Value;
use url::Url;

use super::{Context, Fetch, SourceMeta, Validators};
use crate::config::Source;
use crate::content;
use crate::git;
use crate::model::{Item, RawItem, sha1_hex};
use crate::store::Store;

pub async fn fetch(
    url: &Url,
    branch: &str,
    only: &[String],
    limit: usize,
    _source: &Source,
    ctx: &Context<'_>,
) -> Result<Fetch> {
    let remote = url.to_string();
    let tip = {
        let (r, b) = (remote.clone(), branch.to_string());
        tokio::task::spawn_blocking(move || git::remote_tip(&r, &b))
            .await
            .context("git task")??
            .with_context(|| format!("{remote} has no {branch:?} branch"))?
    };
    let validators = Validators {
        body_hash: Some(tip.clone()),
        ..Default::default()
    };
    if ctx.state.body_hash.as_deref() == Some(tip.as_str()) {
        return Ok(Fetch::Unchanged { validators });
    }

    let dir = ctx
        .cache_dir
        .join("mirrors")
        .join(mirror_key(&remote, branch));
    let via = human_url(url);
    let items = {
        let (remote, branch, via, only) = (
            remote.clone(),
            branch.to_string(),
            via.clone(),
            only.to_vec(),
        );
        tokio::task::spawn_blocking(move || -> Result<Vec<RawItem>> {
            git::mirror(&remote, &branch, &dir)?;
            read_items(&Store::open(&dir), &via, &only, limit)
        })
        .await
        .context("git task")??
    };

    let title = url
        .path()
        .trim_matches('/')
        .trim_end_matches(".git")
        .to_string();
    Ok(Fetch::Changed {
        validators,
        meta: SourceMeta {
            title: (!title.is_empty()).then_some(title),
            site_url: Some(via),
        },
        items,
    })
}

/// Newest `limit` items of the mirrored tree, optionally restricted to some of its sources.
pub fn read_items(store: &Store, via: &str, only: &[String], limit: usize) -> Result<Vec<RawItem>> {
    let mut items = store.items()?;
    items.retain(|item| {
        !item.front.hidden && (only.is_empty() || only.contains(&item.front.source))
    });
    items.sort_by(|a, b| {
        b.created_at()
            .cmp(&a.created_at())
            .then_with(|| b.path.cmp(&a.path))
    });
    items.truncate(limit);
    items
        .iter()
        .map(|item| {
            let html = store.read_html(&item.path)?;
            Ok(convert(item, html, via))
        })
        .collect()
}

/// A stored item as a fresh one, with the raw HTML when the other side kept it and a rendering
/// of its Markdown otherwise.
pub fn convert(item: &Item, html: Option<String>, via: &str) -> RawItem {
    let front = &item.front;
    let content_html = html.or_else(|| {
        let body = item.body.trim();
        (!body.is_empty()).then(|| content::render_markdown(body))
    });
    let mut extra = front.extra.clone();
    extra.insert("via".into(), Value::String(via.to_string()));
    extra.insert("via_source".into(), Value::String(front.source.clone()));
    RawItem {
        id: Some(format!("{via}#{}", item.path)),
        title: front.title.clone(),
        link: front.link.clone(),
        published: front.published.or(Some(front.first_seen)),
        updated: front.updated,
        authors: front.authors.clone(),
        labels: front.labels.clone(),
        summary: front.summary.clone(),
        content_html,
        extra,
    }
}

/// `https://github.com/o/r.git` → `https://github.com/o/r`, without any credentials.
pub fn human_url(url: &Url) -> String {
    let mut clean = url.clone();
    let _ = clean.set_username("");
    let _ = clean.set_password(None);
    clean.set_query(None);
    clean.set_fragment(None);
    let text = clean.to_string();
    text.trim_end_matches('/')
        .trim_end_matches(".git")
        .to_string()
}

/// Directory name for a mirror: readable prefix plus a hash so two URLs never collide.
pub fn mirror_key(remote: &str, branch: &str) -> PathBuf {
    let public = Url::parse(remote)
        .ok()
        .map(|url| human_url(&url))
        .unwrap_or_else(|| "repo".into());
    let readable: String = public
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("repo")
        .trim_end_matches(".git")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(32)
        .collect();
    let hash = sha1_hex(format!("{remote}\n{branch}").as_bytes());
    Path::new(&format!("{readable}-{}", &hash[..12])).to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FrontMatter;
    use chrono::{TimeZone, Utc};

    fn item() -> Item {
        Item {
            path: "items/hn/2026/09/2026-09-02-hello".into(),
            front: FrontMatter {
                title: "Hello".into(),
                link: "https://example.com/hello".into(),
                source: "hn".into(),
                first_seen: Utc.with_ymd_and_hms(2026, 9, 2, 10, 0, 0).unwrap(),
                ..Default::default()
            },
            body: "# Hi\n\nBody **bold**.\n".into(),
        }
    }

    #[test]
    fn converts_items_keeping_link_and_marking_origin() {
        let raw = convert(&item(), None, "https://github.com/friend/reads");
        assert_eq!(
            raw.id.as_deref(),
            Some("https://github.com/friend/reads#items/hn/2026/09/2026-09-02-hello")
        );
        assert_eq!(raw.link, "https://example.com/hello");
        assert_eq!(raw.published, Some(item().front.first_seen));
        assert!(raw.content_html.unwrap().contains("<strong>bold</strong>"));
        assert_eq!(
            raw.extra.get("via"),
            Some(&Value::String("https://github.com/friend/reads".into()))
        );
        assert_eq!(
            raw.extra.get("via_source"),
            Some(&Value::String("hn".into()))
        );

        let with_html = convert(&item(), Some("<p>raw</p>".into()), "x");
        assert_eq!(with_html.content_html.as_deref(), Some("<p>raw</p>"));
    }

    #[test]
    fn human_urls_drop_credentials_and_git_suffix() {
        let url = Url::parse("https://token@github.com/o/r.git").unwrap();
        assert_eq!(human_url(&url), "https://github.com/o/r");
        let url = Url::parse("https://github.com/o/r").unwrap();
        assert_eq!(human_url(&url), "https://github.com/o/r");
    }

    #[test]
    fn mirror_keys_are_readable_and_distinct() {
        let a = mirror_key("https://github.com/o/reads.git", "aggr");
        let b = mirror_key("https://github.com/o/reads.git", "other");
        assert!(a.to_string_lossy().starts_with("reads-"), "{a:?}");
        assert_ne!(a, b);
        assert_eq!(a, mirror_key("https://github.com/o/reads.git", "aggr"));
    }

    #[test]
    fn reads_newest_items_from_a_mirror() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path());
        for (n, source) in [(1, "a"), (2, "b"), (3, "a")] {
            let mut item = item();
            item.path = format!("items/{source}/2026/09/2026-09-0{n}-x");
            item.front.source = source.into();
            item.front.published = Some(Utc.with_ymd_and_hms(2026, 9, n, 0, 0, 0).unwrap());
            store
                .write_item(crate::store::NewItem {
                    dir: &format!("items/{source}/2026/09"),
                    stem: &format!("2026-09-0{n}-x"),
                    front: &item.front,
                    body: "body",
                    html: None,
                })
                .unwrap();
        }
        let all = read_items(&store, "v", &[], 10).unwrap();
        assert_eq!(all.len(), 3);
        assert!(all[0].id.as_deref().unwrap().ends_with("2026-09-03-x"));
        let only_b = read_items(&store, "v", &["b".into()], 10).unwrap();
        assert_eq!(only_b.len(), 1);
        assert_eq!(read_items(&store, "v", &[], 2).unwrap().len(), 2);
    }
}
