//! `type = "aggr"`: another aggr repository as a source. Its data branch is mirrored with a
//! depth-1 clone (no history needed) and its retained items are re-published here, optionally
//! bounded to the newest entries. Items keep their original link, so a feed both sites follow
//! still dedupes to one entry.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use serde_yaml_ng::Value;
use url::Url;

use super::{Context, Fetch, SourceMeta, Validators};
use crate::config::Source;
use crate::content;
use crate::git;
use crate::model::{Item, RawItem, sha1_hex};
use crate::store::Store;

type MirrorMutex = Mutex<()>;
type MirrorLockRegistry = Mutex<HashMap<PathBuf, Weak<MirrorMutex>>>;

static MIRROR_LOCKS: OnceLock<MirrorLockRegistry> = OnceLock::new();

pub async fn fetch(
    url: &Url,
    branch: &str,
    only: &[String],
    limit: Option<usize>,
    _source: &Source,
    ctx: &Context<'_>,
) -> Result<Fetch> {
    let remote = url.to_string();
    let advertised_tip = {
        let (r, b) = (remote.clone(), branch.to_string());
        tokio::task::spawn_blocking(move || git::remote_tip(&r, &b))
            .await
            .context("git task")??
            .with_context(|| format!("{remote} has no {branch:?} branch"))?
    };
    if ctx.state.body_hash.as_deref() == Some(advertised_tip.as_str()) {
        return Ok(Fetch::Unchanged {
            validators: Validators {
                body_hash: Some(advertised_tip),
                ..Default::default()
            },
        });
    }

    let dir = ctx
        .cache_dir
        .join("mirrors")
        .join(mirror_key(&remote, branch));
    let via = human_url(url);
    let (checked_out_tip, items) = {
        let mirror_dir = dir.clone();
        let (remote, branch, via, only) = (
            remote.clone(),
            branch.to_string(),
            via.clone(),
            only.to_vec(),
        );
        with_mirror_lock(dir, move || -> Result<_> {
            let checked_out_tip = git::mirror(&remote, &branch, &mirror_dir)?;
            let items = read_items(&Store::open(&mirror_dir), &mirror_dir, &via, &only, limit)?;
            Ok((checked_out_tip, items))
        })
        .await?
    };

    Ok(changed_fetch(url, via, checked_out_tip, items))
}

fn mirror_lock(dir: &Path) -> Arc<MirrorMutex> {
    let mut locks = MIRROR_LOCKS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(dir).and_then(Weak::upgrade) {
        return lock;
    }

    let lock = Arc::new(Mutex::new(()));
    locks.insert(dir.to_path_buf(), Arc::downgrade(&lock));
    lock
}

async fn with_mirror_lock<T, F>(dir: PathBuf, task: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    let lock = mirror_lock(&dir);
    tokio::task::spawn_blocking(move || {
        let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        task()
    })
    .await
    .context("git task")?
}

fn changed_fetch(url: &Url, via: String, checked_out_tip: String, items: Vec<RawItem>) -> Fetch {
    let title = url
        .path()
        .trim_matches('/')
        .trim_end_matches(".git")
        .to_string();
    Fetch::Changed {
        validators: Validators {
            body_hash: Some(checked_out_tip),
            ..Default::default()
        },
        meta: SourceMeta {
            title: (!title.is_empty()).then_some(title),
            site_url: Some(via),
        },
        items,
    }
}

/// Retained items in the mirrored tree, optionally restricted by source and newest-first limit.
pub fn read_items(
    store: &Store,
    store_root: &Path,
    via: &str,
    only: &[String],
    limit: Option<usize>,
) -> Result<Vec<RawItem>> {
    let mut items = store.items()?;
    items.retain(|item| {
        !item.front.hidden && (only.is_empty() || only.contains(&item.front.source))
    });
    items.sort_by(|a, b| {
        b.created_at()
            .cmp(&a.created_at())
            .then_with(|| b.path.cmp(&a.path))
    });
    if let Some(limit) = limit {
        items.truncate(limit);
    }
    items
        .iter()
        .map(|item| {
            let html = store.read_html(item)?;
            let mut raw = convert(item, html, via);
            attach_companion_locator(&mut raw, store_root, &item.path)?;
            Ok(raw)
        })
        .collect()
}

// `RawItem` deliberately has no source-engine state. Keep this opaque value transient and remove
// it before front matter is planned; it only bridges mirror enumeration and post-dedupe hydration.
const COMPANION_LOCATOR_KEY: &str = "\u{e000}aggr:mirror-companions";

#[derive(Debug, Serialize, Deserialize)]
struct CompanionLocator {
    store_root: PathBuf,
    item_path: String,
}

pub(crate) fn attach_companion_locator(
    raw: &mut RawItem,
    store_root: &Path,
    item_path: &str,
) -> Result<()> {
    let locator = CompanionLocator {
        store_root: store_root.to_path_buf(),
        item_path: item_path.to_string(),
    };
    raw.extra.insert(
        COMPANION_LOCATOR_KEY.into(),
        serde_yaml_ng::to_value(locator).context("encoding mirror companion locator")?,
    );
    Ok(())
}

fn take_companion_locator(raw: &mut RawItem) -> Option<CompanionLocator> {
    raw.extra
        .remove(COMPANION_LOCATOR_KEY)
        .and_then(|value| serde_yaml_ng::from_value(value).ok())
}

pub(crate) fn discard_companion_locator(raw: &mut RawItem) {
    raw.extra.remove(COMPANION_LOCATOR_KEY);
}

pub(crate) fn persistent_extra(raw: &RawItem) -> std::collections::BTreeMap<String, Value> {
    let mut extra = raw.extra.clone();
    extra.remove(COMPANION_LOCATOR_KEY);
    extra
}

/// Load optional mirror companions only after the caller has decided this item will be written.
/// All failures remain best-effort: the retained Markdown still points at publisher images.
pub(crate) async fn hydrate_companions(mut raw: RawItem) -> RawItem {
    let Some(locator) = take_companion_locator(&mut raw) else {
        return raw;
    };
    let item_path = locator.item_path.clone();
    let loaded = tokio::task::spawn_blocking(move || -> Result<_> {
        let store = Store::open(locator.store_root);
        let item = store.read_item(&locator.item_path)?;
        let preview = match (&item.front.preview, store.read_preview(&item)?) {
            (Some(metadata), Some(bytes)) => Some(crate::preview::Thumbnail {
                extension: if metadata.file.ends_with(".webp") {
                    "webp"
                } else {
                    "jpg"
                },
                bytes,
                width: metadata.width,
                height: metadata.height,
                alt: metadata.alt.clone(),
                color: metadata.color.clone().unwrap_or_else(|| "#d4d4d8".into()),
            }),
            _ => None,
        };
        let images = store.read_image_assets(&item)?;
        Ok((preview, images))
    })
    .await;
    match loaded {
        Ok(Ok((preview, images))) => {
            raw.preview = preview;
            raw.images = images;
        }
        Ok(Err(error)) => {
            log::debug!("ignoring unavailable mirrored media for {item_path}: {error:#}");
        }
        Err(error) => {
            log::debug!("mirror media task failed for {item_path}: {error}");
        }
    }
    raw
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
    let origin = extra
        .get("via")
        .cloned()
        .unwrap_or_else(|| Value::String(via.to_string()));
    let origin_source = extra
        .get("via_source")
        .cloned()
        .unwrap_or_else(|| Value::String(front.source.clone()));
    extra.entry("origin".into()).or_insert(origin);
    extra.entry("origin_source".into()).or_insert(origin_source);
    extra.insert("via".into(), Value::String(via.to_string()));
    extra.insert("via_source".into(), Value::String(front.source.clone()));
    RawItem {
        id: Some(format!("{via}#{}", item.path)),
        title: front.title.clone(),
        link: front.link.clone(),
        published: front.published,
        updated: front.updated,
        first_seen: Some(front.first_seen),
        authors: front.authors.clone(),
        labels: front.labels.clone(),
        summary: front.summary.clone(),
        content_html,
        preview_candidates: Vec::new(),
        preview: None,
        images: Vec::new(),
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
        assert_eq!(raw.published, None);
        assert_eq!(raw.first_seen, Some(item().front.first_seen));
        assert!(raw.content_html.unwrap().contains("<strong>bold</strong>"));
        assert_eq!(
            raw.extra.get("origin"),
            Some(&Value::String("https://github.com/friend/reads".into()))
        );
        assert_eq!(
            raw.extra.get("origin_source"),
            Some(&Value::String("hn".into()))
        );
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
    fn repeated_imports_keep_the_first_origin_and_capture_time() {
        let first_via = "https://git.example/first/reads";
        let second_via = "https://git.example/second/reads";
        let first = convert(&item(), None, first_via);
        let replicated = Item {
            path: "items/friends/2026/09/2026-09-02-hello".into(),
            front: FrontMatter {
                title: first.title,
                link: first.link,
                source: "friends".into(),
                published: first.published,
                updated: first.updated,
                first_seen: first.first_seen.unwrap(),
                extra: first.extra,
                ..Default::default()
            },
            body: "Body".into(),
        };

        let second = convert(&replicated, None, second_via);
        assert_eq!(second.first_seen, Some(item().front.first_seen));
        assert_eq!(
            second.extra.get("origin"),
            Some(&Value::String(first_via.into()))
        );
        assert_eq!(
            second.extra.get("origin_source"),
            Some(&Value::String("hn".into()))
        );
        assert_eq!(
            second.extra.get("via"),
            Some(&Value::String(second_via.into()))
        );
        assert_eq!(
            second.extra.get("via_source"),
            Some(&Value::String("friends".into()))
        );
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
    fn changed_fetch_uses_the_checked_out_snapshot_tip() {
        let url = Url::parse("https://github.com/friend/reads.git").unwrap();
        let advertised_tip = "tip-before-fetch";
        let checked_out_tip = "tip-returned-by-mirror";
        let fetch = changed_fetch(
            &url,
            "https://github.com/friend/reads".into(),
            checked_out_tip.into(),
            vec![],
        );

        let Fetch::Changed { validators, .. } = fetch else {
            panic!("expected a changed fetch");
        };
        assert_eq!(validators.body_hash.as_deref(), Some(checked_out_tip));
        assert_ne!(validators.body_hash.as_deref(), Some(advertised_tip));
    }

    #[tokio::test]
    async fn shared_mirror_directory_serializes_distinct_filtered_reads() {
        let tmp = tempfile::tempdir().unwrap();
        let mirror_dir = tmp.path().join("shared-mirror");
        let (first_entered_tx, first_entered_rx) = tokio::sync::oneshot::channel();
        let (release_first_tx, release_first_rx) = std::sync::mpsc::channel();

        let first = tokio::spawn(with_mirror_lock(mirror_dir.clone(), move || {
            first_entered_tx.send(()).unwrap();
            release_first_rx.recv().unwrap();
            Ok((vec!["source-a".to_string()], Some(3_usize)))
        }));
        first_entered_rx.await.unwrap();

        let (second_entered_tx, mut second_entered_rx) = tokio::sync::oneshot::channel();
        let second = tokio::spawn(with_mirror_lock(mirror_dir, move || {
            second_entered_tx.send(()).unwrap();
            Ok((vec!["source-b".to_string()], Some(7_usize)))
        }));

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut second_entered_rx,)
                .await
                .is_err(),
            "the second read entered while the shared mirror was in use"
        );

        release_first_tx.send(()).unwrap();
        let first_filter = first.await.unwrap().unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), &mut second_entered_rx)
            .await
            .unwrap()
            .unwrap();
        let second_filter = second.await.unwrap().unwrap();

        assert_eq!(first_filter, (vec!["source-a".to_string()], Some(3)));
        assert_eq!(second_filter, (vec!["source-b".to_string()], Some(7)));
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
                    preview: None,
                    images: &[],
                })
                .unwrap();
        }
        let all = read_items(&store, tmp.path(), "v", &[], None).unwrap();
        assert_eq!(all.len(), 3);
        assert!(all[0].id.as_deref().unwrap().ends_with("2026-09-03-x"));
        let only_b = read_items(&store, tmp.path(), "v", &["b".into()], None).unwrap();
        assert_eq!(only_b.len(), 1);
        assert_eq!(
            read_items(&store, tmp.path(), "v", &[], Some(2))
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn mirror_media_stays_lazy_then_hydrates_and_degrades_on_demand() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path());
        let image = image::DynamicImage::new_rgb8(640, 400);
        let mut encoded = std::io::Cursor::new(Vec::new());
        image
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let asset = crate::media::prepare_asset(
            &crate::media::Candidate {
                url: Url::parse("https://publisher.invalid/diagram.png").unwrap(),
                alt: Some("Diagram".into()),
            },
            encoded.into_inner(),
            &crate::media::MediaLimits::default(),
        )
        .unwrap();
        let mut front = item().front;
        front.images = vec![asset.metadata("article")];
        store
            .write_item(crate::store::NewItem {
                dir: "items/hn/2026/09",
                stem: "article",
                front: &front,
                body: "![Diagram](https://publisher.invalid/diagram.png)",
                html: None,
                preview: None,
                images: std::slice::from_ref(&asset),
            })
            .unwrap();

        let mirrored = read_items(&store, tmp.path(), "https://reader.invalid", &[], None).unwrap();
        assert_eq!(mirrored.len(), 1);
        assert!(mirrored[0].images.is_empty());
        let hydrated = hydrate_companions(mirrored.into_iter().next().unwrap()).await;
        assert_eq!(hydrated.images.len(), 1);
        assert_eq!(hydrated.images[0].source_url, asset.source_url);
        assert_eq!(hydrated.images[0].master_bytes, asset.master_bytes);
        assert_eq!(hydrated.images[0].renditions, asset.renditions);

        let original = tmp
            .path()
            .join("items/hn/2026/09")
            .join(&front.images[0].original.file);
        std::fs::remove_file(&original).unwrap();
        let degraded = read_items(&store, tmp.path(), "https://reader.invalid", &[], None).unwrap();
        assert_eq!(degraded.len(), 1);
        assert!(
            hydrate_companions(degraded.into_iter().next().unwrap())
                .await
                .images
                .is_empty()
        );

        std::fs::write(original, b"not an image").unwrap();
        let degraded = read_items(&store, tmp.path(), "https://reader.invalid", &[], None).unwrap();
        assert_eq!(degraded.len(), 1);
        assert!(
            hydrate_companions(degraded.into_iter().next().unwrap())
                .await
                .images
                .is_empty()
        );
    }
}
