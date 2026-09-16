use std::fs;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, bail};
use url::Url;

use crate::cache::{ArticleCache, Namespace};
use crate::config::Source;
use crate::http::{self, Request, Response};
use crate::model::RawItem;
use crate::site::item_type::ItemType;

const RETRY_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// Whether an item plays a recording: a podcast episode, an audio file, or a video page.
pub(super) fn is_recording(link: &str, audio: Option<&str>) -> bool {
    matches!(
        ItemType::from_urls(link, audio),
        ItemType::Podcast | ItemType::Audio | ItemType::Video
    )
}

pub(super) async fn infer(
    raw: &RawItem,
    source: &Source,
    client: &http::Client,
    cache_dir: &Path,
    failures: &super::ArticleFailures,
) -> Result<Option<u64>> {
    infer_at(raw, source, client, cache_dir, failures, SystemTime::now()).await
}

async fn infer_at(
    raw: &RawItem,
    source: &Source,
    client: &http::Client,
    cache_dir: &Path,
    failures: &super::ArticleFailures,
    now: SystemTime,
) -> Result<Option<u64>> {
    if raw.extra.get("duration_seconds").is_some_and(|value| {
        value.as_u64().is_some_and(|seconds| seconds > 0)
            || value
                .as_str()
                .and_then(crate::media_duration::parse)
                .is_some()
    }) {
        return Ok(None);
    }
    let audio = raw
        .extra
        .get("audio_url")
        .and_then(|value| value.as_str())
        .and_then(safe_url);
    if !is_recording(&raw.link, audio.as_ref().map(Url::as_str)) {
        return Ok(None);
    }
    let Some(url) = safe_url(&raw.link) else {
        return Ok(None);
    };
    // Recording metadata comes from a page, not from downloading an entire media enclosure.
    if direct_media(&url) {
        return Ok(None);
    }
    let headers = http::source_headers(source, &url);
    let cache = ArticleCache::new(cache_dir);
    let cached = cache.load(&url, headers)?;
    let extract = |page: &str, final_url: &Url| {
        if crate::sources::youtube::is_video_url(&url) {
            crate::sources::youtube::duration_seconds(page, &url)
        } else {
            crate::media_duration::from_html(page, final_url, audio.as_slice())
        }
    };
    if let Some(response) = &cached
        && let Some(seconds) = extract(&response.html_text(), &response.final_url)
    {
        return Ok(Some(seconds));
    }
    if failures.blocked(&url) {
        return Ok(None);
    }
    let probe = Probe(Namespace::RecordingDuration.dir(cache_dir).join(format!(
        "{}.probe",
        crate::model::sha1_hex(format!("{}\0{:?}\0{:?}", url, headers, audio))
    )));
    let previous = probe.read()?;
    if previous
        .is_some_and(|previous| now.duration_since(previous).unwrap_or_default() < RETRY_AFTER)
    {
        return Ok(None);
    }
    if cached.is_some() && previous.is_none() {
        probe.record(now)?;
        return Ok(None);
    }
    let request = client.get(Request {
        url: &url,
        headers,
        etag: cached
            .as_ref()
            .and_then(|response| response.etag.as_deref()),
        last_modified: cached
            .as_ref()
            .and_then(|response| response.last_modified.as_deref()),
    });
    let response = match tokio::time::timeout(Duration::from_secs(8), request).await {
        Ok(Ok(Response::Ok(body)))
            if http::is_html_content_type(body.content_type.as_deref())
                && body.bytes.len() <= crate::cache::MAX_ARTICLE_BODY_BYTES =>
        {
            cache.store(&url, headers, &body)?
        }
        Ok(Ok(Response::Ok(_))) => {
            probe.record(now)?;
            return Ok(None);
        }
        Ok(Ok(Response::NotModified)) => match cached {
            Some(response) => response,
            None => {
                log::debug!(
                    "{}: recording metadata returned 304 without a cached page",
                    source.slug
                );
                return Ok(None);
            }
        },
        Ok(Err(error)) => {
            failures.record(&url, http::status_code(&error));
            log::debug!("{}: recording metadata unavailable: {error:#}", source.slug);
            return Ok(None);
        }
        Err(_) => {
            log::debug!("{}: recording metadata request timed out", source.slug);
            return Ok(None);
        }
    };
    let duration = extract(&response.html_text(), &response.final_url);
    if duration.is_none() {
        probe.record(now)?;
    }
    Ok(duration)
}

fn safe_url(value: &str) -> Option<Url> {
    Url::parse(value).ok().filter(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.username().is_empty()
            && url.password().is_none()
    })
}

fn direct_media(url: &Url) -> bool {
    url.path()
        .to_ascii_lowercase()
        .rsplit_once('.')
        .is_some_and(|(_, extension)| {
            matches!(
                extension,
                "mp3"
                    | "m4a"
                    | "aac"
                    | "ogg"
                    | "oga"
                    | "opus"
                    | "wav"
                    | "flac"
                    | "mp4"
                    | "m4v"
                    | "webm"
                    | "mov"
                    | "m3u8"
            )
        })
}

struct Probe(PathBuf);

impl Probe {
    fn read(&self) -> Result<Option<SystemTime>> {
        let metadata = match fs::symlink_metadata(&self.0) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("inspecting recording duration probe"),
        };
        if !metadata.file_type().is_file() {
            bail!(
                "recording duration probe is not a regular file: {}",
                self.0.display()
            );
        }
        if metadata.len() > 32 {
            return Ok(None);
        }
        let mut value = String::new();
        fs::File::open(&self.0)?
            .take(33)
            .read_to_string(&mut value)
            .context("reading recording duration probe")?;
        Ok(value
            .trim()
            .parse::<u64>()
            .ok()
            .and_then(|seconds| UNIX_EPOCH.checked_add(Duration::from_secs(seconds))))
    }

    fn record(&self, now: SystemTime) -> Result<()> {
        let parent = self
            .0
            .parent()
            .context("recording duration probe has no parent")?;
        fs::create_dir_all(parent).context("creating recording duration probe directory")?;
        let mut temporary =
            tempfile::NamedTempFile::new_in(parent).context("creating recording duration probe")?;
        write!(
            temporary,
            "{}",
            now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
        )
        .context("writing recording duration probe")?;
        temporary
            .persist(&self.0)
            .context("publishing recording duration probe")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    fn episode(url: String, audio: String) -> RawItem {
        let mut raw = RawItem {
            link: url,
            ..Default::default()
        };
        raw.extra.insert("audio_url".into(), audio.into());
        raw
    }

    fn page(audio: &str) -> String {
        format!(
            r#"<script type="application/ld+json">{{"@type":"AudioObject","contentUrl":"{audio}","duration":"PT27M51S"}}</script>"#
        )
    }

    #[test]
    fn recordings_are_video_pages_audio_files_and_podcast_episodes() {
        assert!(is_recording(
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            None
        ));
        assert!(is_recording("https://cdn.example/episode.mp3", None));
        assert!(is_recording(
            "https://publisher.example/episode",
            Some("https://cdn.example/episode.mp3")
        ));
        assert!(!is_recording("https://publisher.example/article", None));
        assert!(!is_recording(
            "https://publisher.example/article",
            Some("not a url")
        ));
        assert!(!is_recording("https://publisher.example/paper.pdf", None));
    }

    #[tokio::test]
    async fn cached_primary_media_duration_needs_no_request_and_known_or_nonmedia_items_are_skipped()
     {
        crate::http::install_crypto_provider();
        let server = MockServer::start_async().await;
        let endpoint = server
            .mock_async(|when, then| {
                when.path("/episode");
                then.status(500);
            })
            .await;
        let directory = tempfile::tempdir().unwrap();
        let source = super::super::tests::source();
        let client = http::Client::new(&crate::config::FetchConfig::default()).unwrap();
        let mut raw = episode(server.url("/episode"), server.url("/audio.mp3"));
        let url = Url::parse(&raw.link).unwrap();
        ArticleCache::new(directory.path())
            .store(
                &url,
                &[],
                &http::Body {
                    bytes: page(raw.extra["audio_url"].as_str().unwrap()).into_bytes(),
                    final_url: url.clone(),
                    etag: None,
                    last_modified: None,
                    content_type: Some("text/html".into()),
                },
            )
            .unwrap();
        let failures = super::super::ArticleFailures::default();
        failures.record(&url, Some(429));
        assert_eq!(
            infer(&raw, &source, &client, directory.path(), &failures)
                .await
                .unwrap(),
            Some(1671)
        );
        let paused = server
            .mock_async(|when, then| {
                when.path("/uncached");
                then.status(500);
            })
            .await;
        let uncached = RawItem {
            link: server.url("/uncached"),
            ..raw.clone()
        };
        assert_eq!(
            infer(&uncached, &source, &client, directory.path(), &failures)
                .await
                .unwrap(),
            None
        );
        paused.assert_calls_async(0).await;
        raw.extra.insert("duration_seconds".into(), 99.into());
        assert_eq!(
            infer(
                &raw,
                &source,
                &client,
                directory.path(),
                &Default::default()
            )
            .await
            .unwrap(),
            None
        );
        raw.extra.clear();
        assert_eq!(
            infer(
                &raw,
                &source,
                &client,
                directory.path(),
                &Default::default()
            )
            .await
            .unwrap(),
            None
        );
        endpoint.assert_calls_async(0).await;
    }

    #[tokio::test]
    async fn unknown_recordings_retry_after_a_day_without_repeating_requests_in_between() {
        crate::http::install_crypto_provider();
        let server = MockServer::start_async().await;
        let empty = server
            .mock_async(|when, then| {
                when.path("/episode");
                then.status(200)
                    .header("content-type", "text/html")
                    .body("<p>Live recording has no final duration yet.</p>");
            })
            .await;
        let directory = tempfile::tempdir().unwrap();
        let source = super::super::tests::source();
        let client = http::Client::new(&crate::config::FetchConfig::default()).unwrap();
        let raw = episode(server.url("/episode"), server.url("/audio.mp3"));
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);
        assert_eq!(
            infer_at(
                &raw,
                &source,
                &client,
                directory.path(),
                &Default::default(),
                now
            )
            .await
            .unwrap(),
            None
        );
        assert_eq!(
            infer_at(
                &raw,
                &source,
                &client,
                directory.path(),
                &Default::default(),
                now + Duration::from_secs(60)
            )
            .await
            .unwrap(),
            None
        );
        empty.assert_calls_async(1).await;
        empty.delete_async().await;
        let ready = server
            .mock_async(|when, then| {
                when.path("/episode");
                then.status(200)
                    .header("content-type", "text/html")
                    .body(page(raw.extra["audio_url"].as_str().unwrap()));
            })
            .await;
        assert_eq!(
            infer_at(
                &raw,
                &source,
                &client,
                directory.path(),
                &Default::default(),
                now + RETRY_AFTER + Duration::from_secs(1)
            )
            .await
            .unwrap(),
            Some(1671)
        );
        ready.assert_calls_async(1).await;
    }

    #[tokio::test]
    async fn upstream_failure_does_not_mark_a_probe_successful() {
        crate::http::install_crypto_provider();
        let server = MockServer::start_async().await;
        let failed = server
            .mock_async(|when, then| {
                when.path("/episode");
                then.status(404);
            })
            .await;
        let directory = tempfile::tempdir().unwrap();
        let source = super::super::tests::source();
        let client = http::Client::new(&crate::config::FetchConfig::default()).unwrap();
        let raw = episode(server.url("/episode"), server.url("/audio.mp3"));
        assert_eq!(
            infer(
                &raw,
                &source,
                &client,
                directory.path(),
                &Default::default()
            )
            .await
            .unwrap(),
            None
        );
        assert!(!Namespace::RecordingDuration.dir(directory.path()).exists());
        failed.delete_async().await;
        let ready = server
            .mock_async(|when, then| {
                when.path("/episode");
                then.status(200)
                    .header("content-type", "text/html")
                    .body(page(raw.extra["audio_url"].as_str().unwrap()));
            })
            .await;
        assert_eq!(
            infer(
                &raw,
                &source,
                &client,
                directory.path(),
                &Default::default()
            )
            .await
            .unwrap(),
            Some(1671)
        );
        ready.assert_calls_async(1).await;
    }
}
