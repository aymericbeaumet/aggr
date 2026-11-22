//! Strict recording durations shared by feeds and public player metadata.

pub fn parse(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() || value.len() > 80 {
        return None;
    }
    let seconds = if let Some(iso) = value.strip_prefix('P') {
        let mut total = 0_u64;
        let mut start = 0;
        let mut time = false;
        let mut previous = 0;
        let mut components = 0;
        for (index, ch) in iso.char_indices() {
            if ch == 'T' {
                if time || index != start {
                    return None;
                }
                time = true;
                start = index + 1;
                continue;
            }
            if ch.is_ascii_digit() || ch == '.' {
                continue;
            }
            let (order, multiplier) = match (time, ch) {
                (false, 'D') => (1, 86_400),
                (true, 'H') => (2, 3_600),
                (true, 'M') => (3, 60),
                (true, 'S') => (4, 1),
                _ => return None,
            };
            if order <= previous {
                return None;
            }
            let number = &iso[start..index];
            if multiplier != 1 && number.contains('.') {
                return None;
            }
            total = total.checked_add(seconds(number)?.checked_mul(multiplier)?)?;
            previous = order;
            components += 1;
            start = index + 1;
        }
        if components == 0 || start != iso.len() || iso.ends_with('T') {
            return None;
        }
        total
    } else if value.contains(':') {
        let parts: Vec<_> = value.split(':').collect();
        if !(2..=3).contains(&parts.len()) {
            return None;
        }
        let mut total = 0_u64;
        for (index, part) in parts.iter().enumerate() {
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let number: u64 = part.parse().ok()?;
            if index > 0 && number >= 60 {
                return None;
            }
            total = total.checked_mul(60)?.checked_add(number)?;
        }
        total
    } else {
        seconds(value)?
    };
    (seconds > 0).then_some(seconds)
}

fn seconds(value: &str) -> Option<u64> {
    let (whole, fraction) = value
        .split_once('.')
        .map_or((value, None), |(a, b)| (a, Some(b)));
    if whole.is_empty() || !whole.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let whole: u64 = whole.parse().ok()?;
    match fraction {
        Some(fraction) if fraction.is_empty() || !fraction.bytes().all(|b| b.is_ascii_digit()) => {
            None
        }
        Some(fraction) if fraction.bytes().any(|b| b != b'0') => whole.checked_add(1),
        _ => Some(whole),
    }
}

pub(crate) enum RecordingDuration {
    Recorded(u64),
    Unfinished,
}

/// Only accept structured media attached to this page or an explicitly identified primary player.
pub fn from_html(page: &str, page_url: &url::Url, media_urls: &[url::Url]) -> Option<u64> {
    match from_page(
        page,
        &scraper::Html::parse_document(page),
        page_url,
        media_urls,
    ) {
        Some(RecordingDuration::Recorded(seconds)) => Some(seconds),
        _ => None,
    }
}

/// Structured data first; failing that, the player's own JSON embedded in the page.
pub(crate) fn from_page(
    page: &str,
    document: &scraper::Html,
    page_url: &url::Url,
    media_urls: &[url::Url],
) -> Option<RecordingDuration> {
    from_document(document, page_url, media_urls)
        .or_else(|| embedded(page, page_url, media_urls).map(RecordingDuration::Recorded))
}

/// Pages without structured data often carry their player's state as JSON (Substack keeps
/// `podcast_duration` beside the post's `canonical_url`): a numeric `…duration` key in the same
/// JSON object as the page's own URL or the exact media URL, the nearest when there are several.
/// Only JSON string values count (`:"…"`), not links or attributes mentioning the URL; every such
/// mention must point at the same value; previews and drafts are not the recording.
pub(crate) fn embedded(page: &str, page_url: &url::Url, media_urls: &[url::Url]) -> Option<u64> {
    use std::sync::LazyLock;
    static DURATION: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r#"\\?"((?:[a-z]+_)*duration)\\?"\s*:\s*([0-9]+(?:\.[0-9]+)?)"#)
            .expect("valid duration key pattern")
    });
    let json_value = |index: usize| {
        let before = page[..index].trim_end_matches(' ');
        before.ends_with(":\"") || before.ends_with(":\\\"")
    };
    let mut found = None;
    for anchor in std::iter::once(page_url).chain(media_urls) {
        let plain = anchor.as_str().trim_end_matches('/');
        let escaped = plain.replace('/', "\\/");
        for needle in [plain, escaped.as_str()] {
            for (index, _) in page.match_indices(needle).take(16) {
                if !json_value(index) {
                    continue;
                }
                let (start, end) = enclosing_object(page, index, index + needle.len());
                let nearest = DURATION
                    .captures_iter(&page[start..end])
                    .filter(|capture| {
                        let key = &capture[1];
                        !key.contains("preview") && !key.contains("draft")
                    })
                    .filter_map(|capture| {
                        let at = start + capture.get(0)?.start();
                        seconds(&capture[2])
                            .filter(|value| *value > 0)
                            .map(|value| (at.abs_diff(index), value))
                    })
                    .min_by_key(|(distance, _)| *distance);
                let Some((_, value)) = nearest else {
                    continue;
                };
                if found.is_some_and(|previous| previous != value) {
                    return None;
                }
                found = Some(value);
            }
        }
    }
    found
}

/// The JSON object around `start..end`, by brace nesting, bounded so a huge document is never
/// scanned whole. Braces inside strings are rare in player state and only shrink the object.
fn enclosing_object(page: &str, start: usize, end: usize) -> (usize, usize) {
    const REACH: usize = 1_200;
    let bytes = page.as_bytes();
    let floor = start.saturating_sub(REACH);
    let mut depth = 0_i32;
    let mut object_start = floor;
    for (offset, byte) in bytes[floor..start].iter().enumerate().rev() {
        match byte {
            b'}' => depth += 1,
            b'{' if depth == 0 => {
                object_start = floor + offset;
                break;
            }
            b'{' => depth -= 1,
            _ => {}
        }
    }
    let ceiling = (end + REACH).min(page.len());
    depth = 0;
    let mut object_end = ceiling;
    for (offset, byte) in bytes[end..ceiling].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' if depth == 0 => {
                object_end = end + offset + 1;
                break;
            }
            b'}' => depth -= 1,
            _ => {}
        }
    }
    let boundary = |mut index: usize, down: bool| {
        while !page.is_char_boundary(index) {
            index = if down { index - 1 } else { index + 1 };
        }
        index
    };
    (boundary(object_start, true), boundary(object_end, false))
}

pub(crate) fn from_document(
    document: &scraper::Html,
    page_url: &url::Url,
    media_urls: &[url::Url],
) -> Option<RecordingDuration> {
    use serde_json::Value;
    let selector = scraper::Selector::parse("script[type='application/ld+json']").ok()?;
    let mut duration = None;
    for script in document.select(&selector).take(16) {
        let Ok(value) = serde_json::from_str::<Value>(&script.inner_html()) else {
            continue;
        };
        let mut pending = vec![&value];
        for _ in 0..512 {
            let Some(value) = pending.pop() else { break };
            if let Some(values) = value.as_array() {
                pending.extend(values.iter().take(128));
                continue;
            }
            pending.extend(
                ["@graph", "mainEntity", "video", "audio"]
                    .iter()
                    .filter_map(|key| value.get(key)),
            );
            let Some(kind) = value.get("@type") else {
                continue;
            };
            let media_kind =
                |value: &Value| matches!(value.as_str(), Some("VideoObject" | "AudioObject"));
            if !media_kind(kind)
                && !kind
                    .as_array()
                    .is_some_and(|values| values.iter().any(media_kind))
            {
                continue;
            }
            let identity_matches = ["url", "@id", "mainEntityOfPage", "embedUrl", "contentUrl"]
                .iter()
                .filter_map(|key| value.get(key))
                .filter_map(|value| value.as_str().or_else(|| value.get("@id")?.as_str()))
                .any(|value| matches_primary(value, page_url, media_urls));
            if !identity_matches {
                continue;
            }
            let live =
                |value: &Value| value.get("isLiveBroadcast").and_then(Value::as_bool) == Some(true);
            if live(value)
                || value.get("publication").is_some_and(|value| {
                    live(value)
                        || value
                            .as_array()
                            .is_some_and(|values| values.iter().any(live))
                })
            {
                return Some(RecordingDuration::Unfinished);
            }
            let Some(seconds) = value
                .get("duration")
                .and_then(Value::as_str)
                .and_then(parse)
            else {
                continue;
            };
            if duration.is_some_and(|previous| previous != seconds) {
                return None;
            }
            duration = Some(seconds);
        }
    }
    duration.map(RecordingDuration::Recorded)
}

fn matches_primary(value: &str, page_url: &url::Url, media_urls: &[url::Url]) -> bool {
    let Ok(mut candidate) = page_url.join(value) else {
        return false;
    };
    if !matches!(candidate.scheme(), "http" | "https")
        || !candidate.username().is_empty()
        || candidate.password().is_some()
    {
        return false;
    }
    if candidate == *page_url {
        return true;
    }
    candidate.set_fragment(None);
    media_urls.iter().any(|media| {
        if let (Some(candidate_id), Some(media_id)) = (
            crate::sources::youtube::video_id(&candidate),
            crate::sources::youtube::video_id(media),
        ) {
            return candidate_id == media_id;
        }
        let mut media = media.clone();
        media.set_fragment(None);
        candidate == media
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_player_json_names_the_recording_by_the_page_or_media_url() {
        let audio = url::Url::parse("https://api.substack.com/feed/podcast/1/abc.mp3").unwrap();
        let page_url = url::Url::parse("https://example.com/p/episode").unwrap();
        // Substack: the page's preloaded state, a JSON string holding escaped JSON, where the
        // post names its page and its length, and the player URL is not the feed's enclosure.
        let page = r#"<link rel="canonical" href="https://example.com/p/episode"><script>window._preloads = JSON.parse("{\"post\":{\"id\":1,\"canonical_url\":\"https:\/\/example.com\/p\/episode\",\"podcast_duration\":2352.013,\"preview_duration\":30,\"draft_free_podcast_duration\":null,\"podcast_url\":\"https:\/\/api.substack.com\/api\/v1\/audio\/upload\/x\/src\"},\"other\":{\"canonical_url\":\"https:\/\/example.com\/p\/other\",\"podcast_duration\":99}}")</script>"#;
        assert_eq!(
            super::embedded(page, &page_url, std::slice::from_ref(&audio)),
            Some(2353)
        );
        assert_eq!(
            super::from_html(page, &page_url, std::slice::from_ref(&audio)),
            Some(2353)
        );
        // The enclosure itself anchors too; a link or attribute mentioning the URL does not.
        let plain = r#"<a href="https://api.substack.com/feed/podcast/1/abc.mp3">play</a><script>{"duration":600,"url":"https://api.substack.com/feed/podcast/1/abc.mp3"}</script>"#;
        assert_eq!(
            super::embedded(plain, &page_url, std::slice::from_ref(&audio)),
            Some(600)
        );
        assert_eq!(super::embedded(plain, &page_url, &[]), None);
        // Players that disagree about the same recording say nothing.
        assert_eq!(
            super::embedded(
                r#"{"duration":600,"url":"https://api.substack.com/feed/podcast/1/abc.mp3"} {"url":"https://api.substack.com/feed/podcast/1/abc.mp3","duration":601}"#,
                &page_url,
                std::slice::from_ref(&audio)
            ),
            None
        );
        assert_eq!(
            super::embedded(
                r#"{"duration":600,"url":"https://elsewhere.example/x.mp3"}"#,
                &page_url,
                std::slice::from_ref(&audio)
            ),
            None
        );
    }

    #[test]
    fn recording_duration_formats_are_strict_and_checked() {
        for (input, expected) in [
            ("27:51", 1671),
            ("1:02:03", 3723),
            ("PT1H2M3S", 3723),
            ("P1DT2H", 93600),
            ("PT0.2S", 1),
            ("3601.001", 3602),
            (" 42 ", 42),
        ] {
            assert_eq!(parse(input), Some(expected), "{input}");
        }
        for input in [
            "",
            "unknown42",
            "42junk",
            "0",
            "-1",
            "NaN",
            "1:60",
            "1::2",
            "PT",
            "PT1M2H",
            "P1DT",
            "PT1.5M",
            "18446744073709551616",
            "PT18446744073709551615H",
        ] {
            assert_eq!(parse(input), None, "{input}");
        }
    }

    #[test]
    fn structured_duration_requires_primary_page_or_player_identity() {
        let page = url::Url::parse("https://example.test/article/").unwrap();
        let player = url::Url::parse("https://cdn.test/episode.mp3").unwrap();
        let html = r##"<script type="application/ld+json">{"@graph":[
            {"@type":"VideoObject","url":"https://example.test/sidebar","duration":"PT99M"},
            {"@type":"VideoObject","@id":"#related","duration":"PT88M"},
            {"@type":"AudioObject","contentUrl":"https://cdn.test/episode.mp3","duration":"PT27M51S"}
        ]}</script>"##;
        assert_eq!(from_html(html, &page, &[]), None);
        assert_eq!(from_html(html, &page, &[player]), Some(1671));
        let page_media = r#"<script type="application/ld+json">{"@type":["Thing","VideoObject"],"mainEntityOfPage":{"@id":"https://example.test/article/"},"duration":"PT42S"}</script>"#;
        assert_eq!(from_html(page_media, &page, &[]), Some(42));
    }

    #[test]
    fn structured_duration_rejects_live_and_ambiguous_primary_recordings() {
        let page = url::Url::parse("https://example.test/article/").unwrap();
        let object =
            serde_json::json!({"@type":"VideoObject","url":page.as_str(),"duration":"PT1M"});
        let mut other = object.clone();
        other["duration"] = "PT2M".into();
        let html = format!(
            "<script type=\"application/ld+json\">{}</script>",
            serde_json::json!([object.clone(), other])
        );
        assert_eq!(from_html(&html, &page, &[]), None);
        let mut live = object;
        live["publication"] = serde_json::json!([{"isLiveBroadcast":true}]);
        let html = format!("<script type=\"application/ld+json\">{live}</script>");
        assert_eq!(from_html(&html, &page, &[]), None);
    }
}
