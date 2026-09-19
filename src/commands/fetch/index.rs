//! One pass over the archive feeds every later decision: the URLs already stored, where each
//! source's items live, and which items still want a recording duration, an image repair, or a
//! capture of their original page.

use std::collections::{BTreeMap, HashSet};

use super::{podcast, recording};
use crate::media;
use crate::model::{RawItem, dedupe_keys, normalize_link};

#[derive(Default)]
pub(super) struct ExistingPaths {
    pub(super) items: BTreeMap<String, BTreeMap<String, String>>,
    pub(super) podcasts: podcast::Archive,
    pub(super) images: BTreeMap<String, Vec<ArchivedImages>>,
    pub(super) recordings: BTreeMap<String, Vec<(String, RawItem)>>,
    /// Items still carrying only feed content because the original page was unavailable, and
    /// captures that archived a script's loading placeholder instead of an article.
    pub(super) captures: BTreeMap<String, Vec<(String, RawItem)>>,
}

pub(super) struct ArchivedImages {
    pub(super) path: String,
    pub(super) candidates: Vec<media::Candidate>,
    pub(super) images: Vec<crate::model::ArticleImage>,
    pub(super) preview: Option<crate::model::Preview>,
}

pub(super) fn index_archive(
    items: impl IntoIterator<Item = crate::model::Item>,
) -> (HashSet<String>, ExistingPaths) {
    let mut links = HashSet::new();
    let mut paths = ExistingPaths::default();
    for item in items {
        let audio = item
            .front
            .extra
            .get("audio_url")
            .and_then(serde_yaml_ng::Value::as_str);
        if item
            .front
            .extra
            .get("duration_seconds")
            .and_then(serde_yaml_ng::Value::as_u64)
            .is_none_or(|value| value == 0)
            && recording::is_recording(&item.front.link, audio)
        {
            paths
                .recordings
                .entry(item.front.source.clone())
                .or_default()
                .push((
                    item.path.clone(),
                    RawItem {
                        title: item.front.title.clone(),
                        link: item.front.link.clone(),
                        extra: item.front.extra.clone(),
                        ..Default::default()
                    },
                ));
        }
        // Binary links are final as feed content: heavy extraction never requests them.
        let wants_capture = match item.front.content {
            crate::model::ContentKind::Feed | crate::model::ContentKind::None => true,
            crate::model::ContentKind::Extracted => crate::content::is_placeholder_body(&item.body),
        };
        if wants_capture
            && item.front.replicated_at.is_none()
            && url::Url::parse(&item.front.link).is_ok_and(|url| {
                matches!(url.scheme(), "http" | "https") && !super::is_binary_link(&url)
            })
        {
            paths
                .captures
                .entry(item.front.source.clone())
                .or_default()
                .push((
                    item.path.clone(),
                    RawItem {
                        title: item.front.title.clone(),
                        link: item.front.link.clone(),
                        published: item.front.published,
                        updated: item.front.updated,
                        first_seen: Some(item.front.first_seen),
                        authors: item.front.authors.clone(),
                        labels: item.front.labels.clone(),
                        summary: item.front.summary.clone(),
                        extra: item.front.extra.clone(),
                        ..Default::default()
                    },
                ));
        }
        if let Ok(base) = url::Url::parse(&item.front.link) {
            let mut candidates = media::markdown_candidates(&item.body, &base);
            for image in &item.front.images {
                if let Ok(url) = url::Url::parse(&image.source)
                    && matches!(url.scheme(), "http" | "https")
                    && url.username().is_empty()
                    && url.password().is_none()
                    && !candidates.iter().any(|candidate| candidate.url == url)
                {
                    candidates.push(media::Candidate { url, alt: None });
                }
            }
            if !candidates.is_empty() {
                paths
                    .images
                    .entry(item.front.source.clone())
                    .or_default()
                    .push(ArchivedImages {
                        path: item.path.clone(),
                        candidates,
                        images: item.front.images.clone(),
                        preview: item.front.preview.clone(),
                    });
            }
        }
        paths.podcasts.insert(&item);
        let link = normalize_link(&item.front.link);
        if !link.is_empty() {
            links.insert(link);
        }
        let entries = paths.items.entry(item.front.source).or_default();
        let raw = RawItem {
            title: item.front.title,
            link: item.front.link,
            published: item.front.published,
            updated: item.front.updated,
            ..Default::default()
        };
        for key in dedupe_keys(&raw) {
            entries.entry(key).or_insert_with(|| item.path.clone());
        }
    }
    (links, paths)
}

#[cfg(test)]
mod tests {
    use super::super::plan::{plan, use_existing_path};
    use super::super::tests::{options, source};
    use super::*;
    use crate::model::{ContentKind, FrontMatter, file_stem, unique_stem};
    use chrono::{TimeZone, Utc};

    fn index_existing_item_paths(
        items: impl IntoIterator<Item = crate::model::Item>,
        source_slug: &str,
    ) -> BTreeMap<String, String> {
        index_archive(items)
            .1
            .items
            .remove(source_slug)
            .unwrap_or_default()
    }

    #[test]
    fn placeholder_captures_are_retried_like_feed_only_items() {
        let item = |content: ContentKind, body: &str| crate::model::Item {
            path: format!("items/blog/2026/09/{}", body.len()),
            front: FrontMatter {
                title: "Post".into(),
                link: format!("https://example.com/{}", body.len()),
                source: "blog".into(),
                content,
                ..Default::default()
            },
            body: body.into(),
        };
        let (_, paths) = index_archive([
            item(ContentKind::Extracted, "loading…\n"),
            item(
                ContentKind::Extracted,
                "overview metrics about\n\nreconnecting…\n",
            ),
            item(
                ContentKind::Extracted,
                "A real article body that mentions loading… once.\n",
            ),
            item(ContentKind::Feed, "Feed summary.\n"),
        ]);
        let captures = paths.captures.get("blog").unwrap();
        let bodies = captures
            .iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            bodies,
            [
                "items/blog/2026/09/11",
                "items/blog/2026/09/40",
                "items/blog/2026/09/14"
            ]
        );
    }

    #[test]
    fn archive_index_scans_items_once_and_keeps_metadata_paths_per_source() {
        let visited = std::cell::Cell::new(0);
        let items = ["first", "second"].map(|source| crate::model::Item {
            path: format!("items/{source}/story"),
            front: FrontMatter {
                source: source.into(),
                title: "Same title".into(),
                link: "https://example.com/shared#fragment".into(),
                ..Default::default()
            },
            body: String::new(),
        });
        let (links, paths) = index_archive(
            items
                .clone()
                .into_iter()
                .inspect(|_| visited.set(visited.get() + 1)),
        );
        assert_eq!(visited.get(), 2);
        assert_eq!(links.len(), 1);
        assert_eq!(paths.items.len(), 2);
        for (source, entries) in &paths.items {
            assert!(!entries.is_empty());
            assert!(
                entries
                    .values()
                    .all(|path| path == &format!("items/{source}/story"))
            );
        }
    }

    #[test]
    fn refresh_reuses_the_exact_collision_suffixed_item_path() {
        let published = Utc.with_ymd_and_hms(2026, 9, 1, 8, 0, 0).unwrap();
        let first = RawItem {
            title: "Same title".into(),
            link: "https://example.com/first".into(),
            published: Some(published),
            ..Default::default()
        };
        let second = RawItem {
            link: "https://example.com/second".into(),
            ..first.clone()
        };
        let base_stem = file_stem(published, &first.title);
        let second_keys = dedupe_keys(&second);
        let second_stem = unique_stem(&base_stem, &second_keys[0], |stem| stem == base_stem);
        let item = |path: String, raw: &RawItem| crate::model::Item {
            path,
            front: FrontMatter {
                title: raw.title.clone(),
                link: raw.link.clone(),
                source: "blog".into(),
                published: raw.published,
                first_seen: published,
                ..Default::default()
            },
            body: String::new(),
        };
        let indexed = index_existing_item_paths(
            [
                item(format!("items/blog/2026/09/{base_stem}"), &first),
                item(format!("items/blog/2026/09/{second_stem}"), &second),
            ],
            "blog",
        );
        let existing = second_keys.iter().find_map(|key| indexed.get(key)).unwrap();
        let mut planned = plan(&second, &source(), &options(), ContentKind::Feed);

        use_existing_path(&mut planned, existing).unwrap();

        assert_eq!(planned.stem, second_stem);
        assert_ne!(planned.stem, base_stem);
    }
}
