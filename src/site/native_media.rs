use serde::Serialize;
use url::Url;

#[derive(Debug, Clone, Serialize)]
pub struct NativeMediaCtx {
    pub kind: &'static str,
    pub url: String,
}

impl NativeMediaCtx {
    pub fn from_urls(link: &str, audio_url: Option<&str>) -> Option<Self> {
        if let Some(url) = safe_url(link) {
            let kind = media_kind(url.path()).or_else(|| {
                url.query_pairs().find_map(|(key, value)| {
                    key.eq_ignore_ascii_case("filename")
                        .then(|| media_kind(&value))
                        .flatten()
                })
            });
            if let Some(kind) = kind {
                return Some(Self {
                    kind,
                    url: url.into(),
                });
            }
        }
        audio_url.and_then(safe_url).map(|url| Self {
            kind: "audio",
            url: url.into(),
        })
    }
}

fn safe_url(value: &str) -> Option<Url> {
    let url = Url::parse(value).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none())
    .then_some(url)
}

fn media_kind(path: &str) -> Option<&'static str> {
    let path = path.to_ascii_lowercase().replace("%2e", ".");
    match path.rsplit_once('.')?.1 {
        "mp3" | "m4a" | "aac" | "ogg" | "oga" | "opus" | "wav" | "flac" => Some("audio"),
        "mp4" | "m4v" | "webm" | "ogv" | "mov" => Some("video"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_media_urls_preserve_signed_queries_and_fragments() {
        for (link, kind) in [
            ("https://example.com/episode.MP3?token=one#t=30", "audio"),
            ("http://localhost:3000/recording.webm", "video"),
            (
                "https://example.com/download?filename=Episode%20One.m4a",
                "audio",
            ),
            (
                "https://example.com/movie.mp4?signature=one&part=2",
                "video",
            ),
        ] {
            let media = NativeMediaCtx::from_urls(link, None).unwrap();
            assert_eq!(media.kind, kind);
            assert_eq!(media.url, link);
        }
    }

    #[test]
    fn podcast_enclosures_can_have_extensionless_urls() {
        let audio = "https://cdn.example.com/episodes/123?token=one";
        let media =
            NativeMediaCtx::from_urls("https://podcast.example/episode", Some(audio)).unwrap();
        assert_eq!(media.kind, "audio");
        assert_eq!(media.url, audio);
        let video =
            NativeMediaCtx::from_urls("https://example.com/movie.mp4", Some(audio)).unwrap();
        assert_eq!(video.kind, "video");
    }

    #[test]
    fn native_media_rejects_unsafe_urls_and_misleading_extensions() {
        for link in [
            "javascript:alert('audio.mp3')",
            "data:audio/mpeg;base64,AAAA",
            "file:///episode.mp3",
            "//example.com/episode.mp3",
            "https://user:password@example.com/episode.mp3",
        ] {
            assert!(NativeMediaCtx::from_urls(link, None).is_none(), "{link}");
            assert!(
                NativeMediaCtx::from_urls("https://podcast.example/episode", Some(link)).is_none(),
                "{link}"
            );
        }
        for link in [
            "https://example.com/movie.mp4.html",
            "https://example.com/article#episode.mp3",
            "https://example.com/article?url=https://other.example/movie.mp4",
            "https://example.com/movie.mp4/notes",
        ] {
            assert!(NativeMediaCtx::from_urls(link, None).is_none(), "{link}");
        }
    }
}
