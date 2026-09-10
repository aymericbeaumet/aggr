use scraper::{Html, Selector};
use serde_json::Value;
use url::Url;

use crate::media_duration::RecordingDuration;

/// Read public metadata from the already fetched page, never from a recommended video's player.
pub fn duration_seconds(page: &str, video: &Url) -> Option<u64> {
    let id = super::video_id(video)?;
    let mut duration = None;
    for player in player_responses(page) {
        if player
            .pointer("/videoDetails/videoId")
            .and_then(Value::as_str)
            != Some(&id)
        {
            continue;
        }
        if unfinished_broadcast(&player) {
            return None;
        }
        duration = duration.or_else(|| {
            [
                "/videoDetails/lengthSeconds",
                "/microformat/playerMicroformatRenderer/lengthSeconds",
            ]
            .iter()
            .find_map(|path| {
                let value = player.pointer(path)?;
                value
                    .as_u64()
                    .or_else(|| value.as_str()?.parse::<u64>().ok())
                    .filter(|seconds| *seconds > 0)
            })
        });
    }
    duration.or_else(|| public_duration(page, video, &id))
}

pub(super) fn player_responses(page: &str) -> impl Iterator<Item = Value> + '_ {
    const MARKER: &str = "ytInitialPlayerResponse";
    page.match_indices(MARKER)
        .take(16)
        .filter_map(|(start, _)| {
            let tail = &page[start + MARKER.len()..];
            let opening = tail.find('{')?;
            let prefix = &tail[..opening];
            if prefix.len() > 32
                || !prefix.contains(['=', ':'])
                || !prefix
                    .chars()
                    .all(|c| c.is_ascii_whitespace() || matches!(c, '"' | '\'' | ']' | '=' | ':'))
            {
                return None;
            }
            serde_json::Deserializer::from_str(&tail[opening..])
                .into_iter::<Value>()
                .next()?
                .ok()
        })
}

fn unfinished_broadcast(player: &Value) -> bool {
    let flag = |path| player.pointer(path).and_then(Value::as_bool) == Some(true);
    if flag("/videoDetails/isLive")
        || flag("/videoDetails/isUpcoming")
        || flag("/microformat/playerMicroformatRenderer/liveBroadcastDetails/isLiveNow")
        || player
            .pointer("/playabilityStatus/status")
            .and_then(Value::as_str)
            == Some("LIVE_STREAM_OFFLINE")
    {
        return true;
    }
    let broadcast = player.pointer("/microformat/playerMicroformatRenderer/liveBroadcastDetails");
    if !flag("/videoDetails/isLiveContent") && broadcast.is_none() {
        return false;
    }
    // A rolling live duration is not the recording's final duration. Require an explicit end.
    let ended = broadcast
        .and_then(|value| value.get("endTimestamp"))
        .and_then(Value::as_str)
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok());
    ended.is_none()
}

fn matches_video(value: &str, id: &str) -> bool {
    Url::parse(value)
        .ok()
        .and_then(|url| super::video_id(&url))
        .as_deref()
        == Some(id)
}

fn public_duration(page: &str, video: &Url, id: &str) -> Option<u64> {
    let document = Html::parse_document(page);
    match crate::media_duration::from_document(&document, video, std::slice::from_ref(video)) {
        Some(RecordingDuration::Recorded(seconds)) => return Some(seconds),
        Some(RecordingDuration::Unfinished) => return None,
        None => {}
    }
    let identity =
        Selector::parse("link[rel='canonical'], meta[property='og:url'], link[itemprop='url']")
            .ok()?;
    if !document.select(&identity).any(|element| {
        element
            .value()
            .attr("href")
            .or_else(|| element.value().attr("content"))
            .is_some_and(|value| matches_video(value, id))
    }) {
        return None;
    }
    let live = Selector::parse("meta[itemprop='isLiveBroadcast']").ok()?;
    if document.select(&live).any(|element| {
        element
            .value()
            .attr("content")
            .is_some_and(|value| value.eq_ignore_ascii_case("true"))
    }) {
        return None;
    }
    let durations = Selector::parse("meta[itemprop='duration']").ok()?;
    document
        .select(&durations)
        .find_map(|element| crate::media_duration::parse(element.value().attr("content")?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn video() -> Url {
        Url::parse("https://youtu.be/requested?t=30").unwrap()
    }

    fn player(value: Value) -> String {
        format!("<script>var ytInitialPlayerResponse = {value};</script>")
    }

    #[test]
    fn youtube_duration_matches_requested_video_and_ignores_recommendations() {
        let page = player(json!({"videoDetails":{"videoId":"other","lengthSeconds":"999"}}))
            + &player(json!({"videoDetails":{"videoId":"requested","lengthSeconds":"3601"}}));
        assert_eq!(duration_seconds(&page, &video()), Some(3601));
        assert_eq!(
            duration_seconds(
                &page,
                &Url::parse("https://example.com/watch?v=requested").unwrap()
            ),
            None
        );
        assert_eq!(
            duration_seconds(
                &player(json!({"videoDetails":{"lengthSeconds":"42"}})),
                &video()
            ),
            None
        );
    }

    #[test]
    fn youtube_duration_uses_matching_microformat_and_checked_seconds() {
        for length in [
            json!(0),
            json!("0"),
            json!(-5),
            json!(1.5),
            json!("NaN"),
            json!("18446744073709551616"),
        ] {
            assert_eq!(
                duration_seconds(
                    &player(json!({"videoDetails":{"videoId":"requested","lengthSeconds":length}})),
                    &video()
                ),
                None
            );
        }
        assert_eq!(
            duration_seconds(
                &player(json!({"videoDetails":{"videoId":"requested","lengthSeconds":92}})),
                &video()
            ),
            Some(92)
        );
        assert_eq!(
            duration_seconds(
                &player(
                    json!({"videoDetails":{"videoId":"requested"},"microformat":{"playerMicroformatRenderer":{"lengthSeconds":"95"}}})
                ),
                &video()
            ),
            Some(95)
        );
    }

    #[test]
    fn youtube_duration_keeps_live_and_upcoming_unknown_but_accepts_finished_recordings() {
        for detail in [
            json!({"isLive":true}),
            json!({"isUpcoming":true}),
            json!({"isLiveContent":true}),
        ] {
            let mut data = json!({"videoDetails":{"videoId":"requested","lengthSeconds":"999"}});
            data["videoDetails"]
                .as_object_mut()
                .unwrap()
                .extend(detail.as_object().unwrap().clone());
            assert_eq!(duration_seconds(&player(data), &video()), None);
        }
        let mut data = json!({"videoDetails":{"videoId":"requested","lengthSeconds":"3660","isLiveContent":true},"microformat":{"playerMicroformatRenderer":{"liveBroadcastDetails":{"isLiveNow":false,"startTimestamp":"2026-01-01T10:00:00Z","endTimestamp":"2026-01-01T11:01:00Z"}}}});
        assert_eq!(
            duration_seconds(&player(data.clone()), &video()),
            Some(3660)
        );
        data["microformat"]["playerMicroformatRenderer"]["liveBroadcastDetails"]["isLiveNow"] =
            json!(true);
        assert_eq!(duration_seconds(&player(data), &video()), None);
    }

    #[test]
    fn youtube_duration_public_metadata_requires_matching_identity_and_does_not_override_live() {
        let meta = r#"<link rel="canonical" href="https://www.youtube.com/watch?v=requested"><meta itemprop="duration" content="PT1H2M3S">"#;
        assert_eq!(duration_seconds(meta, &video()), Some(3723));
        assert_eq!(
            duration_seconds(&meta.replace("requested", "other"), &video()),
            None
        );
        let live = player(
            json!({"videoDetails":{"videoId":"requested","lengthSeconds":"999","isLive":true}}),
        );
        assert_eq!(duration_seconds(&(live + meta), &video()), None);
        let graph = r#"<script type="application/ld+json">{"@graph":[{"@type":"VideoObject","embedUrl":"https://youtube.com/embed/other","duration":"PT99M"},{"@type":"VideoObject","embedUrl":"https://www.youtube-nocookie.com/embed/requested","duration":"PT2M4S"}]}</script>"#;
        assert_eq!(duration_seconds(graph, &video()), Some(124));
        let live_graph = graph.replace(
            "\"duration\":\"PT2M4S\"",
            "\"duration\":\"PT2M4S\",\"publication\":[{\"isLiveBroadcast\":true}]",
        );
        assert_eq!(duration_seconds(&(live_graph + meta), &video()), None);
    }

    #[test]
    fn youtube_duration_skips_malformed_assignments_without_parsing_unrelated_data() {
        let data = json!({"videoDetails":{"videoId":"requested","lengthSeconds":"25"}});
        let noise = format!("ytInitialPlayerResponse; unrelated = {data};");
        assert_eq!(duration_seconds(&noise, &video()), None);
        let valid = format!("<script>window[\"ytInitialPlayerResponse\"] = {data};</script>");
        assert_eq!(duration_seconds(&(noise + &valid), &video()), Some(25));
        let unavailable = player(
            json!({"videoDetails":{"videoId":"requested","lengthSeconds":"25"},"playabilityStatus":{"status":"LIVE_STREAM_OFFLINE"}}),
        );
        assert_eq!(duration_seconds(&unavailable, &video()), None);
    }
}
