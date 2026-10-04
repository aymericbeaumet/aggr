//! Independent capture policies and the per-feed retained corpus.

use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{ContentMode, DocumentPolicy, ImagePolicy, PreviewPolicy};

#[derive(Debug, Default, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Defaults {
    pub content: ContentMode,
    pub media: MediaPolicy,
    #[serde(flatten)]
    pub limits: Limits,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct Limits {
    pub max_items: usize,
    pub max_age_days: u32,
    pub max_bytes: u64,
    #[serde(default, deserialize_with = "deserialize_since")]
    pub since: Option<NaiveDate>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_items: 250,
            max_age_days: 730,
            max_bytes: 1_000_000_000,
            since: None,
        }
    }
}

impl Limits {
    /// Both age restrictions apply; zero days removes only the relative-age restriction.
    pub fn cutoff(self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let age = (self.max_age_days != 0)
            .then(|| now.checked_sub_signed(Duration::days(i64::from(self.max_age_days))))
            .flatten();
        let since = self
            .since
            .and_then(|date| date.and_hms_opt(0, 0, 0))
            .map(|date| date.and_utc());
        age.into_iter().chain(since).max()
    }
}

#[derive(Debug, Default, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct LimitOverrides {
    pub max_items: Option<usize>,
    pub max_age_days: Option<u32>,
    pub max_bytes: Option<u64>,
    pub since: Option<Since>,
}

impl LimitOverrides {
    pub fn inherit(self, parent: Self) -> Self {
        Self {
            max_items: self.max_items.or(parent.max_items),
            max_age_days: self.max_age_days.or(parent.max_age_days),
            max_bytes: self.max_bytes.or(parent.max_bytes),
            since: self.since.or(parent.since),
        }
    }

    pub fn resolve(self, defaults: Limits) -> Limits {
        Limits {
            max_items: self.max_items.unwrap_or(defaults.max_items),
            max_age_days: self.max_age_days.unwrap_or(defaults.max_age_days),
            max_bytes: self.max_bytes.unwrap_or(defaults.max_bytes),
            since: self.since.map(Since::date).unwrap_or(defaults.since),
        }
    }
}

/// Unlike an omitted value, `false` removes an inherited absolute date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Since {
    Disabled,
    Date(NaiveDate),
}

impl Since {
    fn date(self) -> Option<NaiveDate> {
        match self {
            Self::Disabled => None,
            Self::Date(date) => Some(date),
        }
    }
}

impl Serialize for Since {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Disabled => serializer.serialize_bool(false),
            Self::Date(date) => serializer.serialize_str(&date.to_string()),
        }
    }
}

impl<'de> Deserialize<'de> for Since {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Value {
            Boolean(bool),
            Text(String),
            Date(toml::value::Datetime),
        }
        let date = match Value::deserialize(deserializer)? {
            Value::Boolean(false) => return Ok(Self::Disabled),
            Value::Text(value) => value,
            Value::Date(value) if value.time.is_none() && value.offset.is_none() => {
                value.to_string()
            }
            _ => {
                return Err(serde::de::Error::custom(
                    "since must be an ISO date (YYYY-MM-DD) or false",
                ));
            }
        };
        if date.len() != 10
            || date.as_bytes().get(4) != Some(&b'-')
            || date.as_bytes().get(7) != Some(&b'-')
        {
            return Err(serde::de::Error::custom(
                "since must be an ISO date (YYYY-MM-DD) or false",
            ));
        }
        NaiveDate::parse_from_str(&date, "%Y-%m-%d")
            .map(Self::Date)
            .map_err(serde::de::Error::custom)
    }
}

fn deserialize_since<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<NaiveDate>, D::Error> {
    Option::<Since>::deserialize(deserializer).map(|value| value.and_then(Since::date))
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum MediaPolicy {
    #[default]
    Remote,
    Compressed(crate::media::CompactPolicy),
    Local,
}

impl MediaPolicy {
    pub fn images(self) -> ImagePolicy {
        match self {
            Self::Remote => ImagePolicy::Remote,
            Self::Compressed(policy) => ImagePolicy::Compact(policy),
            Self::Local => ImagePolicy::Original,
        }
    }
    pub fn previews(self) -> PreviewPolicy {
        match self {
            Self::Remote => PreviewPolicy::Remote,
            Self::Compressed(_) | Self::Local => PreviewPolicy::Local,
        }
    }
    pub fn documents(self) -> DocumentPolicy {
        match self {
            Self::Remote => DocumentPolicy::Remote,
            Self::Compressed(_) | Self::Local => DocumentPolicy::Original,
        }
    }
}

impl Serialize for MediaPolicy {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Remote => serializer.serialize_str("remote"),
            Self::Local => serializer.serialize_str("local"),
            Self::Compressed(policy) => {
                use serde::ser::SerializeStruct as _;
                let mut value = serializer.serialize_struct("MediaPolicy", 3)?;
                value.serialize_field("mode", "compressed")?;
                value.serialize_field("quality", &policy.jpeg_quality)?;
                value.serialize_field("max_axis", &policy.max_axis)?;
                value.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for MediaPolicy {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Tuned {
            mode: String,
            quality: Option<u8>,
            max_axis: Option<u32>,
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Value {
            Name(String),
            Tuned(Tuned),
        }
        let (mode, quality, max_axis) = match Value::deserialize(deserializer)? {
            Value::Name(mode) => (mode, None, None),
            Value::Tuned(value) => (value.mode, value.quality, value.max_axis),
        };
        if mode != "compressed" && (quality.is_some() || max_axis.is_some()) {
            return Err(serde::de::Error::custom(
                "quality and max_axis apply only to compressed media",
            ));
        }
        match mode.as_str() {
            "remote" => Ok(Self::Remote),
            "local" => Ok(Self::Local),
            "compressed" => {
                let defaults = crate::media::CompactPolicy::archive();
                let jpeg_quality = quality.unwrap_or(defaults.jpeg_quality);
                let max_axis = max_axis.unwrap_or(defaults.max_axis);
                if !(1..=100).contains(&jpeg_quality)
                    || !(super::MIN_COMPACT_AXIS..=super::MAX_COMPACT_AXIS).contains(&max_axis)
                {
                    return Err(serde::de::Error::custom(
                        "compressed media quality must be 1-100 and max_axis 320-8192",
                    ));
                }
                Ok(Self::Compressed(crate::media::CompactPolicy {
                    jpeg_quality,
                    max_axis,
                }))
            }
            _ => Err(serde::de::Error::custom(
                "media must be remote, compressed, or local",
            )),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct DocumentDefaults {
    pub content: Option<ContentMode>,
    pub media: Option<MediaPolicy>,
    #[serde(flatten)]
    pub limits: LimitOverrides,
}

impl DocumentDefaults {
    pub fn inherit(self, parent: Self) -> Self {
        Self {
            content: self.content.or(parent.content),
            media: self.media.or(parent.media),
            limits: self.limits.inherit(parent.limits),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn default_policy_bounds_each_feed_and_keeps_media_remote() {
        let config = Config::parse("[[sources]]\nurl = 'https://example.com/feed'\n").unwrap();
        let source = config.sources().unwrap().remove(0);
        assert_eq!(source.limits, Limits::default());
        assert_eq!(source.content, ContentMode::Light);
        assert_eq!(source.images, ImagePolicy::Remote);
    }

    #[test]
    fn source_limits_inherit_individually_and_can_clear_each_constraint() {
        let config = Config::parse("[defaults]\ncontent = 'heavy'\nmedia = 'local'\nmax_items = 42\nmax_age_days = 12\nmax_bytes = 345\nsince = 2026-01-02\n[[sources]]\nurl = 'https://example.com/feed'\nmax_items = 0\nsince = false\ncontent = 'light'\nmedia = 'compressed'\n").unwrap();
        let source = config.sources().unwrap().remove(0);
        assert_eq!(
            source.limits,
            Limits {
                max_items: 0,
                max_age_days: 12,
                max_bytes: 345,
                since: None
            }
        );
        assert_eq!(source.content, ContentMode::Light);
        assert_eq!(
            source.images,
            ImagePolicy::Compact(crate::media::CompactPolicy::archive())
        );
        assert_eq!(source.documents, DocumentPolicy::Original);
    }

    #[test]
    fn strict_flat_schema_rejects_unknown_keys_and_old_policies() {
        for text in [
            "[defaults]\nmax_item = 2",
            "[defaults.limits]\nmax_items = 2",
            "[fetch]\ncontent = 'heavy'",
            "[fetch]\nmax_items_per_source = 2",
            "[store]\nmax_items = 2",
            "[[sources]]\nurl = 'https://example.com'\nimages = 'remote'",
            "[[sources]]\nurl = 'https://example.com'\nlimit = 2",
            "[[sources]]\nurl = 'https://example.com'\nmax_item = 2",
        ] {
            assert!(Config::parse(text).is_err(), "accepted {text}");
        }
    }

    #[test]
    fn since_accepts_only_dates_and_false_and_uses_the_newest_cutoff() {
        for value in ["2026-01-02", "'2026-01-02'"] {
            let config = Config::parse(&format!("[defaults]\nsince = {value}\n")).unwrap();
            assert_eq!(
                config.defaults.limits.since.unwrap().to_string(),
                "2026-01-02"
            );
        }
        for value in ["true", "'2026-02-30'", "2026-01-02T01:00:00Z", "'2026-1-2'"] {
            assert!(Config::parse(&format!("[defaults]\nsince = {value}\n")).is_err());
        }
        let now = "2026-01-12T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let mut limits = Limits {
            max_age_days: 3,
            since: Some("2026-01-02".parse().unwrap()),
            ..Limits::default()
        };
        assert_eq!(limits.cutoff(now).unwrap(), now - Duration::days(3));
        limits.max_age_days = 0;
        assert_eq!(
            limits.cutoff(now).unwrap().to_rfc3339(),
            "2026-01-02T00:00:00+00:00"
        );
    }

    #[test]
    fn media_modes_are_independent_and_only_compression_is_tunable() {
        let config = Config::parse(
            "[defaults]\nmedia = { mode = 'compressed', quality = 60, max_axis = 1200 }\n",
        )
        .unwrap();
        assert_eq!(
            config.defaults.media,
            MediaPolicy::Compressed(crate::media::CompactPolicy {
                jpeg_quality: 60,
                max_axis: 1200
            })
        );
        for value in [
            "true",
            "'original'",
            "'compact'",
            "{ mode = 'local', quality = 50 }",
            "{ mode = 'compressed', quality = 0 }",
            "{ mode = 'compressed', max_axis = 1 }",
            "{ mode = 'compressed', typo = 1 }",
        ] {
            assert!(
                Config::parse(&format!("[defaults]\nmedia = {value}")).is_err(),
                "{value}"
            );
        }
    }
}
