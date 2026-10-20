use std::collections::BTreeMap;

use crate::model::{FrontMatter, Item, RawItem};
use chrono::NaiveDate;

type Identity = (String, NaiveDate);

#[derive(Default)]
pub(super) struct Archive(BTreeMap<String, BTreeMap<Identity, Option<String>>>);

pub(super) fn identity(
    title: &str,
    published: Option<chrono::DateTime<chrono::Utc>>,
) -> Option<Identity> {
    let title: String = crate::content::html_to_text(title)
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|c| c.is_alphanumeric())
        .collect();
    (!title.is_empty()).then_some((title, published?.date_naive()))
}

fn spotify_path(value: &str, kind: &str) -> bool {
    http_url(value).is_some_and(|url| {
        url.host_str() == Some("open.spotify.com")
            && url.path_segments().is_some_and(|mut parts| {
                parts.next() == Some(kind)
                    && parts.next().is_some_and(|id| !id.is_empty())
                    && parts.next().is_none()
            })
    })
}

pub(super) fn is_source(source: &crate::config::Source) -> bool {
    matches!(source.engine, crate::config::Engine::Feed { .. })
        && source
            .public_url
            .as_deref()
            .and_then(http_url)
            .is_some_and(|url| crate::sources::podcast::is_spotify_show(&url))
}

impl Archive {
    pub(super) fn insert(&mut self, item: &Item) {
        if !spotify_path(&item.front.link, "episode") {
            return;
        }
        let Some(key) = identity(&item.front.title, item.front.published) else {
            return;
        };
        self.0
            .entry(item.front.source.clone())
            .or_default()
            .entry(key)
            .and_modify(|path| *path = None)
            .or_insert_with(|| Some(item.path.clone()));
    }

    pub(super) fn path(&self, source: &str, raw: &RawItem) -> Option<&str> {
        self.0
            .get(source)?
            .get(&identity(&raw.title, raw.published)?)?
            .as_deref()
    }
}

fn http_url(value: &str) -> Option<url::Url> {
    let url = url::Url::parse(value).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none())
    .then_some(url)
}

// None rejects a match; Some(None) aliases an already identical enclosure without rewriting it.
pub(super) fn audio_change(front: &FrontMatter, raw: &RawItem) -> Option<Option<String>> {
    let incoming = raw.extra.get("audio_url")?.as_str()?;
    let incoming_url = http_url(incoming)?;
    match front.extra.get("audio_url") {
        None => Some(Some(incoming.to_string())),
        Some(existing) if http_url(existing.as_str()?)? == incoming_url => Some(None),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FrontMatter, Item, RawItem};

    fn archived(source: &str, path: &str) -> Item {
        Item {
            path: path.into(),
            front: FrontMatter {
                source: source.into(),
                title: "An Episode!".into(),
                link: "https://open.spotify.com/episode/old".into(),
                published: Some("2026-09-01T00:00:00Z".parse().unwrap()),
                ..Default::default()
            },
            body: "Preserved".into(),
        }
    }
    fn incoming() -> RawItem {
        RawItem {
            title: "AN episode!".into(),
            published: Some("2026-09-01T12:00:00Z".parse().unwrap()),
            extra: [("audio_url".into(), "https://cdn.example/episode.mp3".into())].into(),
            ..Default::default()
        }
    }
    #[test]
    fn localized_spotify_show_urls_use_the_shared_resolver_contract() {
        let mut source = super::super::tests::source();
        for url in [
            "https://open.spotify.com/show/1sz1NhoHqbpXbzNlpOnFoz",
            "https://open.spotify.com/intl-fr/show/1sz1NhoHqbpXbzNlpOnFoz?si=share",
        ] {
            source.public_url = Some(url.into());
            assert!(is_source(&source));
        }
        source.public_url = Some("https://notspotify.example/show/1sz1NhoHqbpXbzNlpOnFoz".into());
        assert!(!is_source(&source));
    }

    #[test]
    fn reconciliation_requires_one_title_and_day_match_in_the_same_source() {
        let mut archive = Archive::default();
        archive.insert(&archived("one", "items/one/old"));
        let raw = incoming();
        assert_eq!(archive.path("one", &raw), Some("items/one/old"));
        assert_eq!(archive.path("other", &raw), None);
        let different = RawItem {
            title: "Different".into(),
            ..raw.clone()
        };
        assert_eq!(archive.path("one", &different), None);
        archive.insert(&archived("one", "items/one/second"));
        assert_eq!(archive.path("one", &raw), None);
    }
    #[test]
    fn reconciliation_only_fills_missing_valid_audio_and_rejects_conflicting_enclosures() {
        let mut old = archived("one", "items/one/old");
        let raw = incoming();
        assert_eq!(
            audio_change(&old.front, &raw),
            Some(Some("https://cdn.example/episode.mp3".into()))
        );
        old.front.extra = raw.extra.clone();
        assert_eq!(audio_change(&old.front, &raw), Some(None));
        old.front
            .extra
            .insert("audio_url".into(), "https://cdn.example/other.mp3".into());
        assert_eq!(audio_change(&old.front, &raw), None);
        old.front.extra.clear();
        for url in [
            "javascript:alert(1)",
            "file:///tmp/audio",
            "https://user:secret@cdn.example/audio",
            "",
        ] {
            let invalid = RawItem {
                extra: [("audio_url".into(), url.into())].into(),
                ..raw.clone()
            };
            assert_eq!(audio_change(&old.front, &invalid), None);
        }
    }
}
