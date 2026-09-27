use std::sync::LazyLock;

use regex::Regex;
use unicode_segmentation::UnicodeSegmentation;

static PICTOGRAPH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"[\p{Emoji_Presentation}[[\p{Extended_Pictographic}&&\p{So}]--[©®™\u{2190}-\u{21ff}]]]",
    )
    .expect("valid Unicode emoji properties")
});
static EMOJI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\p{Emoji}").expect("valid Unicode emoji property"));

/// Keep presentation cleanup outside the archive, and remove entire graphemes so modifiers,
/// joining characters and flag tags cannot survive their emoji. Text symbols retain their
/// ordinary meaning unless an explicit emoji selector requests pictographic presentation.
pub fn title(value: &str, fallback: &str) -> String {
    let plain: String = value
        .graphemes(true)
        .filter(|grapheme| {
            grapheme.contains('\u{fe0e}')
                || !(PICTOGRAPH.is_match(grapheme)
                    || ((grapheme.contains('\u{fe0f}') || grapheme.contains('\u{20e3}'))
                        && EMOJI.is_match(grapheme)))
        })
        .collect();
    let cleaned = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    let cleaned = unwrap_emphasis(&cleaned);
    if cleaned.is_empty() {
        fallback.to_string()
    } else {
        cleaned.to_string()
    }
}

/// A publisher that writes its posts in Markdown can emit the markers with the headline, and a
/// title emphasised from end to end is emphasising nothing: `**Know Who Spoke When**` is the
/// title, asterisks and all. Markers around part of a title are the author's own — `` `zig cc` ``
/// names a command, and `via ___ Supervision` is a blank to fill in — so only a pair wrapping the
/// whole of it comes off, and only when the rest carries no more of them.
fn unwrap_emphasis(title: &str) -> &str {
    for marker in ["**", "__", "*", "_", "`"] {
        let Some(inner) = title
            .strip_prefix(marker)
            .and_then(|rest| rest.strip_suffix(marker))
        else {
            continue;
        };
        if !inner.trim().is_empty() && !inner.contains(marker) {
            return inner;
        }
    }
    title
}

/// The display-only contract shared by static metadata and search result components.
/// Existing item fields remain available to custom themes.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Metadata {
    pub original: String,
    pub date: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated: Option<String>,
    pub source_slug: String,
    pub source_query: String,
    pub source_display: String,
    pub source_title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<Category>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feed_display: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub feed_sources: Vec<super::context::SourceMembershipCtx>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_aggregated: bool,
    pub word_count: usize,
    pub reading_minutes: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consumption: Option<Consumption>,
    pub discussions: Vec<Discussion>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Category {
    pub name: String,
    pub slug: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Discussion {
    pub name: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Consumption {
    pub action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minutes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub words: Option<usize>,
}

impl Consumption {
    fn new(
        kind: super::item_type::ItemType,
        words: usize,
        minutes: usize,
        seconds: Option<u64>,
    ) -> Option<Self> {
        use super::item_type::ItemType;
        let action = match kind {
            ItemType::Podcast | ItemType::Audio => "listen",
            ItemType::Video => "watch",
            ItemType::Article | ItemType::Document if words > 0 => "read",
            _ => return None,
        };
        let seconds = seconds.filter(|seconds| *seconds > 0 && *seconds <= 9_007_199_254_740_991);
        Some(if action == "read" {
            Self {
                action,
                minutes: Some(minutes as u64),
                seconds: None,
                words: Some(words),
            }
        } else {
            Self {
                action,
                minutes: seconds.map(|seconds| seconds.div_ceil(60)),
                seconds,
                words: None,
            }
        })
    }
}

fn public_url(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
}

fn count(value: Option<&serde_yaml_ng::Value>) -> Option<u64> {
    value.and_then(|value| value.as_u64().or_else(|| value.as_str()?.parse().ok()))
}

impl From<&super::context::ItemCtx> for Metadata {
    fn from(item: &super::context::ItemCtx) -> Self {
        Self {
            original: item.link.clone(),
            date: item.date.to_rfc3339(),
            updated: item
                .updated
                .filter(|date| *date != item.date)
                .map(|date| date.to_rfc3339()),
            source_slug: item.publisher_source.clone(),
            source_query: item
                .source_memberships
                .iter()
                .find(|source| source.slug == item.publisher_source)
                .map(|source| source.query_value.clone())
                .unwrap_or_else(|| item.publisher_source.clone()),
            source_display: item.source_display.clone(),
            source_title: item.source_title.clone(),
            category: item.category.as_ref().map(|name| Category {
                name: name.clone(),
                slug: super::context::category_slug(name),
            }),
            feed_display: item.is_aggregated.then(|| item.feed_display.clone()),
            feed_sources: item
                .source_memberships
                .iter()
                .filter(|source| source.slug != item.publisher_source)
                .cloned()
                .collect(),
            is_aggregated: item.is_aggregated,
            word_count: item.word_count,
            reading_minutes: item.reading_minutes,
            consumption: Consumption::new(
                item.item_type,
                item.word_count,
                item.reading_minutes,
                count(item.extra.get("duration_seconds")),
            ),
            discussions: item
                .discussions
                .iter()
                .filter(|discussion| discussion.found && public_url(&discussion.url))
                .map(|discussion| Discussion {
                    name: discussion.name.clone(),
                    url: discussion.url.clone(),
                    score: discussion.score,
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::title;

    #[test]
    fn consumption_uses_playback_duration_and_never_estimates_media_from_prose() {
        use super::Consumption;
        use crate::site::item_type::ItemType;
        for (kind, action) in [
            (ItemType::Audio, "listen"),
            (ItemType::Podcast, "listen"),
            (ItemType::Video, "watch"),
        ] {
            let known = Consumption::new(kind, 4500, 20, Some(3601)).unwrap();
            assert_eq!(known.action, action);
            assert_eq!(known.minutes, Some(61));
            assert_eq!(known.seconds, Some(3601));
            assert_eq!(known.words, None);
            assert_eq!(
                Consumption::new(kind, 4500, 20, Some(1)).unwrap().minutes,
                Some(1)
            );
            for seconds in [None, Some(0), Some(u64::MAX)] {
                let unknown = Consumption::new(kind, 4500, 20, seconds).unwrap();
                assert_eq!(unknown.minutes, None);
                assert_eq!(unknown.seconds, None);
            }
        }
        let article = Consumption::new(ItemType::Article, 450, 2, Some(3601)).unwrap();
        assert_eq!(article.action, "read");
        assert_eq!(article.minutes, Some(2));
        assert_eq!(article.words, Some(450));
        assert_eq!(article.seconds, None);
        assert!(Consumption::new(ItemType::Image, 450, 2, None).is_none());
        assert!(Consumption::new(ItemType::Article, 0, 0, None).is_none());
    }

    #[test]
    fn removes_complete_emoji_sequences_and_cleans_spacing() {
        for value in [
            "🚀  New 🧑🏽‍💻 ideas 🇫🇷",
            "New\t❤️ ideas 🏳️‍🌈",
            "☀ New ❤ ideas ✨",
            "1️⃣ New #️⃣ ideas *️⃣",
            "New 1\u{20e3} ideas 🏴\u{e0067}\u{e0062}\u{e0065}\u{e006e}\u{e0067}\u{e007f}",
        ] {
            assert_eq!(title(value, "Untitled"), "New ideas", "{value}");
        }
        assert_eq!(title("Before🚀After", "Untitled"), "BeforeAfter");
    }

    #[test]
    fn preserves_languages_punctuation_and_text_presentation() {
        let value = "Café 日本語 العربية हिन्दी #1 * 2 + 3 = 5 ∑ ∞ ↔ ↕ © ® ™ ❤︎";
        assert_eq!(title(value, "Untitled"), value);
    }

    #[test]
    fn a_title_emphasised_end_to_end_keeps_only_its_words() {
        // huggingface.co: the post is written in Markdown and the headline arrives with it.
        assert_eq!(
            title(
                "**Know Who Spoke When: Build Real-Time, Multi-Speaker AI with NVIDIA Nemotron 3 Diarization**",
                "Untitled",
            ),
            "Know Who Spoke When: Build Real-Time, Multi-Speaker AI with NVIDIA Nemotron 3 Diarization"
        );
        for (value, expected) in [
            ("__Bold all through__", "Bold all through"),
            ("*Whole thing*", "Whole thing"),
            ("`one command`", "one command"),
            // The author's own markers: a command mid-title, a blank to fill in, emphasis on two
            // separate words, and a title that is nothing but markers.
            (
                "`zig cc`: a Powerful Drop-In Replacement for GCC/Clang",
                "`zig cc`: a Powerful Drop-In Replacement for GCC/Clang",
            ),
            (
                "Bootstrapping Labels via ___ Supervision",
                "Bootstrapping Labels via ___ Supervision",
            ),
            ("*This* and *that*", "*This* and *that*"),
            ("Using `make` to compile", "Using `make` to compile"),
            ("**", "**"),
            ("****", "****"),
        ] {
            assert_eq!(title(value, "Untitled"), expected, "{value}");
        }
    }

    #[test]
    fn empty_and_emoji_only_titles_have_readable_fallbacks() {
        assert_eq!(title(" 🚀🧑🏽‍💻🇫🇷 ", "Untitled"), "Untitled");
        assert_eq!(title("\n\t", "example.com"), "example.com");
    }
}
