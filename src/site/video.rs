use serde::Serialize;
use url::Url;

#[derive(Debug, Clone, Serialize)]
pub struct VideoCtx {
    pub provider: &'static str,
    pub title: &'static str,
    pub embed_url: String,
    pub requires_parent: bool,
}

impl VideoCtx {
    pub fn from_url(link: &str) -> Option<Self> {
        let url = Url::parse(link).ok()?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
        {
            return None;
        }
        match url.host_str()? {
            "youtube.com"
            | "www.youtube.com"
            | "m.youtube.com"
            | "music.youtube.com"
            | "youtu.be"
            | "www.youtu.be"
            | "youtube-nocookie.com"
            | "www.youtube-nocookie.com" => youtube(&url),
            "twitch.tv" | "www.twitch.tv" | "m.twitch.tv" | "clips.twitch.tv" => twitch(&url),
            "vimeo.com" | "www.vimeo.com" | "player.vimeo.com" => vimeo(&url),
            _ => None,
        }
    }
}

fn youtube(url: &Url) -> Option<VideoCtx> {
    let parts = url.path().trim_matches('/').split('/').collect::<Vec<_>>();
    let supported_path = if matches!(url.host_str(), Some("youtu.be" | "www.youtu.be")) {
        parts.len() == 1
    } else {
        matches!(
            parts.as_slice(),
            ["watch"] | ["embed" | "live" | "shorts" | "v", _]
        )
    };
    if !supported_path {
        return None;
    }
    let id = crate::sources::youtube::video_id(url)?;
    let mut embed = Url::parse(&format!("https://www.youtube-nocookie.com/embed/{id}")).ok()?;
    embed.query_pairs_mut().extend_pairs([
        ("autoplay", "0"),
        ("rel", "0"),
        ("playsinline", "1"),
        ("iv_load_policy", "3"),
    ]);
    if let Some(start) = start_seconds(url) {
        embed
            .query_pairs_mut()
            .append_pair("start", &start.to_string());
    }
    Some(VideoCtx {
        provider: "youtube",
        title: "YouTube",
        embed_url: embed.into(),
        requires_parent: false,
    })
}

fn twitch(url: &Url) -> Option<VideoCtx> {
    let parts = url.path().trim_matches('/').split('/').collect::<Vec<_>>();
    let (key, value, clips) = if url.host_str() == Some("clips.twitch.tv") {
        let clip = match parts.as_slice() {
            ["embed"] => url
                .query_pairs()
                .find(|(key, _)| key == "clip")?
                .1
                .into_owned(),
            [clip] => clip.to_string(),
            _ => return None,
        };
        ("clip", clip, true)
    } else {
        match parts.as_slice() {
            ["videos", id] if numeric_id(id) => ("video", format!("v{id}"), false),
            [channel, "clip", clip] if channel_id(channel) => ("clip", clip.to_string(), true),
            [channel] if channel_id(channel) => ("channel", channel.to_ascii_lowercase(), false),
            _ => return None,
        }
    };
    if clips && !token(&value, 128) {
        return None;
    }
    let mut embed = Url::parse(if clips {
        "https://clips.twitch.tv/embed"
    } else {
        "https://player.twitch.tv/"
    })
    .ok()?;
    embed
        .query_pairs_mut()
        .append_pair(key, &value)
        .append_pair("autoplay", "false");
    if key == "video"
        && let Some(start) = start_seconds(url)
    {
        embed
            .query_pairs_mut()
            .append_pair("time", &format!("{start}s"));
    }
    Some(VideoCtx {
        provider: "twitch",
        title: "Twitch",
        embed_url: embed.into(),
        requires_parent: true,
    })
}

fn vimeo(url: &Url) -> Option<VideoCtx> {
    let parts = url.path().trim_matches('/').split('/').collect::<Vec<_>>();
    let (id, path_hash) = if url.host_str() == Some("player.vimeo.com") {
        match parts.as_slice() {
            ["video", id] => (*id, None),
            _ => return None,
        }
    } else {
        match parts.as_slice() {
            [id] => (*id, None),
            [id, hash] => (*id, Some(*hash)),
            ["channels", channel, id] if token(channel, 128) => (*id, None),
            ["groups", group, "videos", id] if token(group, 128) => (*id, None),
            _ => return None,
        }
    };
    if !numeric_id(id) {
        return None;
    }
    let query_hash = url
        .query_pairs()
        .find(|(key, _)| key == "h")
        .map(|(_, value)| value.into_owned());
    let hash = query_hash.as_deref().or(path_hash);
    if hash.is_some_and(|hash| !token(hash, 128)) {
        return None;
    }
    let mut embed = Url::parse(&format!("https://player.vimeo.com/video/{id}")).ok()?;
    if let Some(hash) = hash {
        embed.query_pairs_mut().append_pair("h", hash);
    }
    embed.query_pairs_mut().extend_pairs([
        ("autoplay", "0"),
        ("dnt", "1"),
        ("playsinline", "1"),
        ("title", "0"),
        ("byline", "0"),
        ("portrait", "0"),
    ]);
    if let Some(start) = start_seconds(url) {
        embed.set_fragment(Some(&format!("t={start}s")));
    }
    Some(VideoCtx {
        provider: "vimeo",
        title: "Vimeo",
        embed_url: embed.into(),
        requires_parent: false,
    })
}

fn numeric_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 20 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn token(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn channel_id(value: &str) -> bool {
    token(value, 25)
        && !value.contains('-')
        && ![
            "directory",
            "downloads",
            "settings",
            "search",
            "login",
            "signup",
            "subscriptions",
            "inventory",
            "following",
            "prime",
            "wallet",
            "products",
            "store",
            "turbo",
            "team",
            "teams",
            "about",
            "jobs",
            "p",
            "videos",
            "embed",
        ]
        .contains(&value.to_ascii_lowercase().as_str())
}

fn start_seconds(url: &Url) -> Option<u32> {
    ["start", "t"]
        .into_iter()
        .find_map(|key| {
            url.query_pairs()
                .find(|(name, _)| name == key)
                .and_then(|(_, value)| duration_seconds(&value))
        })
        .or_else(|| {
            url.fragment()
                .and_then(|fragment| fragment.strip_prefix("t="))
                .and_then(duration_seconds)
        })
}

fn duration_seconds(value: &str) -> Option<u32> {
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return value.parse::<u32>().ok();
    }
    let mut rest = value;
    let mut total = 0_u32;
    let mut previous = u32::MAX;
    while !rest.is_empty() {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        let number = rest[..digits].parse::<u32>().ok()?;
        let multiplier = match rest.as_bytes().get(digits)? {
            b'h' => 3600,
            b'm' => 60,
            b's' => 1,
            _ => return None,
        };
        if multiplier >= previous {
            return None;
        }
        total = total.checked_add(number.checked_mul(multiplier)?)?;
        previous = multiplier;
        rest = &rest[digits + 1..];
    }
    (!value.is_empty()).then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn embed(link: &str) -> Url {
        Url::parse(&VideoCtx::from_url(link).unwrap().embed_url).unwrap()
    }

    fn parameter(url: &Url, key: &str) -> Option<String> {
        url.query_pairs()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.into_owned())
    }

    #[test]
    fn youtube_uses_privacy_domain_and_preserves_start_time() {
        for link in [
            "https://www.youtube.com/watch?v=abc_DEF-123&t=90",
            "https://youtu.be/abc_DEF-123?t=1m30s",
            "https://www.youtube.com/live/abc_DEF-123?start=90",
        ] {
            let video = VideoCtx::from_url(link).unwrap();
            let url = embed(link);
            assert_eq!(video.provider, "youtube");
            assert!(!video.requires_parent);
            assert_eq!(url.host_str(), Some("www.youtube-nocookie.com"));
            assert_eq!(url.path(), "/embed/abc_DEF-123");
            assert_eq!(parameter(&url, "start").as_deref(), Some("90"));
            assert_eq!(parameter(&url, "autoplay").as_deref(), Some("0"));
            assert_eq!(parameter(&url, "playsinline").as_deref(), Some("1"));
        }
    }

    #[test]
    fn twitch_channels_videos_and_clips_require_current_parent() {
        for (link, host, key, value) in [
            (
                "https://www.twitch.tv/example_channel",
                "player.twitch.tv",
                "channel",
                "example_channel",
            ),
            (
                "https://www.twitch.tv/videos/123456?t=1h2m3s",
                "player.twitch.tv",
                "video",
                "v123456",
            ),
            (
                "https://clips.twitch.tv/InterestingClip-Ab12",
                "clips.twitch.tv",
                "clip",
                "InterestingClip-Ab12",
            ),
            (
                "https://www.twitch.tv/example/clip/InterestingClip-Ab12",
                "clips.twitch.tv",
                "clip",
                "InterestingClip-Ab12",
            ),
        ] {
            let video = VideoCtx::from_url(link).unwrap();
            let url = embed(link);
            assert_eq!(video.provider, "twitch");
            assert!(video.requires_parent);
            assert_eq!(url.host_str(), Some(host));
            assert_eq!(parameter(&url, key).as_deref(), Some(value));
            assert_eq!(parameter(&url, "parent"), None);
            assert_eq!(parameter(&url, "autoplay").as_deref(), Some("false"));
        }
        assert_eq!(
            parameter(
                &embed("https://www.twitch.tv/videos/123456?t=1h2m3s"),
                "time"
            )
            .as_deref(),
            Some("3723s")
        );
    }

    #[test]
    fn vimeo_preserves_unlisted_hash_without_forwarding_tracking_parameters() {
        for link in [
            "https://vimeo.com/123456789/abc123def4?tracking=secret",
            "https://player.vimeo.com/video/123456789?h=abc123def4&tracking=secret",
        ] {
            let video = VideoCtx::from_url(link).unwrap();
            let url = embed(link);
            assert_eq!(video.provider, "vimeo");
            assert_eq!(url.host_str(), Some("player.vimeo.com"));
            assert_eq!(url.path(), "/video/123456789");
            assert_eq!(parameter(&url, "h").as_deref(), Some("abc123def4"));
            assert_eq!(parameter(&url, "dnt").as_deref(), Some("1"));
            assert_eq!(parameter(&url, "tracking"), None);
        }
        assert_eq!(
            embed("https://vimeo.com/123456789#t=90s").fragment(),
            Some("t=90s")
        );
    }

    #[test]
    fn unsupported_and_untrusted_urls_never_generate_players() {
        for link in [
            "javascript:alert(1)",
            "ftp://www.youtube.com/watch?v=video",
            "https://youtube.com.evil.test/watch?v=video",
            "https://user:secret@www.youtube.com/watch?v=video",
            "https://www.youtube.com:8080/watch?v=video",
            "https://youtu.be/%3Cscript%3E",
            "https://youtu.be/video/extra",
            "https://www.youtube.com/embed/video/extra",
            "https://www.youtube.com/@channel",
            "https://www.youtube.com/watch?list=playlist",
            "https://www.twitch.tv/directory",
            "https://www.twitch.tv/videos/not-a-number",
            "https://clips.twitch.tv/%3Cscript%3E",
            "https://vimeo.com/channels",
            "https://vimeo.com/123/invalid%3Chash",
            "https://player.vimeo.com/video/123?h=bad%26hash",
            "https://example.com/video/123",
        ] {
            assert!(VideoCtx::from_url(link).is_none(), "{link}");
        }
    }

    #[test]
    fn invalid_times_are_ignored_without_overflow() {
        for time in ["-1", "1.5", "99999999999999999999999999", "1h2h", "abc"] {
            assert_eq!(
                parameter(&embed(&format!("https://youtu.be/video?t={time}")), "start"),
                None
            );
        }
    }
}
