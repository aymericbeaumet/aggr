//! Primary reading format, inferred at build time without modifying retained item metadata.

use serde::Serialize;
use url::Url;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemType {
    #[default]
    Article,
    Podcast,
    Video,
    Audio,
    Image,
    Document,
}

impl ItemType {
    pub fn from_urls(link: &str, audio_url: Option<&str>) -> Self {
        if super::video::VideoCtx::from_url(link).is_some() {
            return Self::Video;
        }
        if super::document::DocumentCtx::from_url(link).is_some() {
            return Self::Document;
        }
        let enclosure = audio_url.and_then(public_url);
        if let Some(media) = super::native_media::NativeMediaCtx::from_urls(
            link,
            enclosure.as_ref().map(Url::as_str),
        ) {
            return if media.kind == "video" {
                Self::Video
            } else if enclosure.is_some() {
                Self::Podcast
            } else {
                Self::Audio
            };
        }
        let Some(url) = public_url(link) else {
            return Self::Article;
        };
        let parts = url.path().trim_matches('/').split('/').collect::<Vec<_>>();
        let episode = match parts.as_slice() {
            ["episode", id] => {
                !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
            }
            [locale, "episode", id] if locale.starts_with("intl-") => {
                !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
            }
            _ => false,
        };
        // One host normalizer for the whole codebase, so an alias like `spotify.com` is
        // recognised the same way `open.spotify.com` is.
        let host = crate::platform::host(&url);
        if (host == Some("spotify.com") && episode)
            || (host == Some("podcasts.apple.com")
                && parts.contains(&"podcast")
                && parts.iter().any(|part| {
                    part.strip_prefix("id").is_some_and(|id| {
                        !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit())
                    })
                }))
        {
            return Self::Podcast;
        }
        let image = |path: &str| {
            path.to_ascii_lowercase()
                .replace("%2e", ".")
                .rsplit_once('.')
                .is_some_and(|(_, extension)| {
                    matches!(
                        extension,
                        "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif" | "svg"
                    )
                })
        };
        if image(url.path())
            || url
                .query_pairs()
                .any(|(key, value)| key.eq_ignore_ascii_case("filename") && image(&value))
        {
            return Self::Image;
        }
        Self::Article
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Article => "article",
            Self::Podcast => "podcast",
            Self::Video => "video",
            Self::Audio => "audio",
            Self::Image => "image",
            Self::Document => "document",
        }
    }
}

fn public_url(value: &str) -> Option<Url> {
    let url = Url::parse(value).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none())
    .then_some(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_resource_type_follows_validated_urls_and_podcast_enclosures() {
        for (link, enclosure, expected) in [
            ("https://publisher.test/story", None, ItemType::Article),
            (
                "https://publisher.test/episode",
                Some("https://cdn.test/play?id=1"),
                ItemType::Podcast,
            ),
            (
                "https://cdn.test/episode.mp3",
                Some("https://cdn.test/episode.mp3"),
                ItemType::Podcast,
            ),
            (
                "https://cdn.test/recording.MP3?token=x",
                None,
                ItemType::Audio,
            ),
            ("https://cdn.test/movie.webm", None, ItemType::Video),
            ("https://youtu.be/dQw4w9WgXcQ", None, ItemType::Video),
            (
                "https://example.test/download?filename=paper.PDF",
                None,
                ItemType::Document,
            ),
            (
                "https://example.test/diagram%2Epng?token=x",
                None,
                ItemType::Image,
            ),
            (
                "https://open.spotify.com/episode/abc123",
                None,
                ItemType::Podcast,
            ),
            (
                "https://podcasts.apple.com/us/podcast/show/id123?i=456",
                None,
                ItemType::Podcast,
            ),
            (
                "https://publisher.test/movie.mp4",
                Some("https://cdn.test/audio.mp3"),
                ItemType::Video,
            ),
        ] {
            assert_eq!(ItemType::from_urls(link, enclosure), expected, "{link}");
        }
    }

    #[test]
    fn articles_are_not_reclassified_by_incidental_or_unsafe_media_mentions() {
        for link in [
            "https://publisher.test/story?image=diagram.png",
            "https://publisher.test/story#episode.mp3",
            "https://publisher.test/photo.png/notes",
            "https://youtube.com/@channel",
            "https://open.spotify.com/track/abc123",
            "https://example.test/podcast/episode",
            "https://user:secret@example.test/photo.png",
            "javascript:alert('movie.mp4')",
        ] {
            assert_eq!(ItemType::from_urls(link, None), ItemType::Article, "{link}");
        }
        assert_eq!(
            ItemType::from_urls("https://publisher.test/story", Some("file:///audio.mp3")),
            ItemType::Article
        );
    }
}
