//! Typed defaults for the reader's browser-local preferences.
//!
//! This module is the single source of truth for the reader's settings. One field table produces
//! both the validation rules used by the pre-paint bootstrap script in `base.html` and the grouped
//! form rendered on `/preferences/`, so a new setting cannot be declared in one place and forgotten
//! in the other.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields, rename_all(serialize = "kebab-case"))]
pub struct ReaderPreferences {
    pub theme: ReaderTheme,
    pub date_format: DateFormat,
    pub text_size: TextSize,
    pub reading_width: ReadingWidth,
    pub font_family: FontFamily,
    pub line_spacing: Spacing,
    pub paragraph_spacing: Spacing,
    pub paragraph_indent: bool,
    pub text_align: TextAlign,
    pub letter_spacing: TextSpacing,
    pub word_spacing: TextSpacing,
    pub density: Density,
    pub thumbnails: Thumbnails,
    pub feed_page_size: usize,
    pub motion: Motion,
    pub single_key_shortcuts: bool,
    /// Maximum lines moved by d/u, bounded by half the available viewport.
    pub scroll_amount: usize,
    /// Retired with the offline article archive. Still accepted so existing `aggr.toml` files keep
    /// parsing under `deny_unknown_fields`, but it no longer reaches the browser.
    #[serde(skip_serializing)]
    pub offline_items: usize,
}

/// One selectable value of a setting, as rendered in a `<select>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreferenceOption {
    pub value: String,
    pub label: String,
}

macro_rules! choices {
    ($name:ident { $($variant:ident => ($value:literal, $label:literal)),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
        pub enum $name {
            $(#[serde(rename = $value)] $variant),+
        }

        impl $name {
            fn options() -> Vec<PreferenceOption> {
                vec![$(PreferenceOption { value: $value.into(), label: $label.into() }),+]
            }
        }
    };
}

choices!(ReaderTheme {
    Auto => ("auto", "System"),
    Light => ("light", "Light"),
    Dark => ("dark", "Dark"),
    Sepia => ("sepia", "Sepia"),
});
choices!(DateFormat {
    Relative => ("relative", "Relative"),
    Iso => ("iso", "YYYY-MM-DD"),
    Local => ("local", "Local date"),
    LocalTime => ("local-time", "Local date & time"),
});
choices!(TextSize {
    Default => ("default", "Default"),
    Large => ("large", "Large"),
    Largest => ("largest", "Largest"),
});
choices!(ReadingWidth {
    Narrow => ("narrow", "Narrow"),
    Standard => ("standard", "Standard"),
    Wide => ("wide", "Wide"),
});
choices!(FontFamily {
    Sans => ("sans", "Sans serif"),
    Serif => ("serif", "Serif"),
    Mono => ("mono", "Monospace"),
});
choices!(Spacing {
    Compact => ("compact", "Compact"),
    Standard => ("standard", "Standard"),
    Relaxed => ("relaxed", "Relaxed"),
});
choices!(TextAlign {
    Left => ("left", "Start"),
    Justify => ("justify", "Justified"),
});
choices!(TextSpacing {
    Normal => ("normal", "Normal"),
    Wide => ("wide", "Wide"),
});
choices!(Density {
    Compact => ("compact", "Compact"),
    Comfortable => ("comfortable", "Comfortable"),
});
choices!(Thumbnails {
    Show => ("show", "Show when available"),
    Hide => ("hide", "Hide"),
});
choices!(Motion {
    Auto => ("auto", "Follow system"),
    Off => ("off", "Off"),
});

impl Default for ReaderPreferences {
    fn default() -> Self {
        Self {
            theme: ReaderTheme::Auto,
            date_format: DateFormat::Relative,
            text_size: TextSize::Default,
            reading_width: ReadingWidth::Standard,
            font_family: FontFamily::Sans,
            line_spacing: Spacing::Standard,
            paragraph_spacing: Spacing::Standard,
            paragraph_indent: false,
            text_align: TextAlign::Left,
            letter_spacing: TextSpacing::Normal,
            word_spacing: TextSpacing::Normal,
            density: Density::Compact,
            thumbnails: Thumbnails::Show,
            feed_page_size: 50,
            motion: Motion::Auto,
            single_key_shortcuts: true,
            scroll_amount: 10,
            offline_items: 30,
        }
    }
}

/// How `/preferences/` renders a setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Control {
    Select,
    Number,
    Checkbox,
}

/// The browser's validation rule for one setting, consumed by the pre-paint bootstrap.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BootstrapRule {
    pub initial: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<i64>,
    /// `documentElement.dataset` key this setting drives, when CSS reacts to it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attribute: Option<&'static str>,
}

/// One rendered control on `/preferences/`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PreferenceField {
    pub key: &'static str,
    pub label: &'static str,
    pub control: Control,
    pub value: serde_json::Value,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<PreferenceOption>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PreferenceGroup {
    pub id: &'static str,
    pub title: &'static str,
    pub fields: Vec<PreferenceField>,
}

/// Both browser-facing views of the settings, derived from one field table.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PreferenceSchema {
    /// Keyed by setting name, for the inline validation bootstrap.
    pub bootstrap: std::collections::BTreeMap<&'static str, BootstrapRule>,
    /// Ordered groups, for the static form.
    pub groups: Vec<PreferenceGroup>,
}

/// A setting before it is split into a bootstrap rule and a form field.
struct Setting {
    key: &'static str,
    label: &'static str,
    control: Control,
    value: serde_json::Value,
    options: Vec<PreferenceOption>,
    min: Option<i64>,
    max: Option<i64>,
    attribute: Option<&'static str>,
    help: Option<&'static str>,
}

fn select(
    key: &'static str,
    label: &'static str,
    attribute: &'static str,
    options: Vec<PreferenceOption>,
    value: impl Serialize,
) -> Setting {
    Setting {
        key,
        label,
        control: Control::Select,
        value: serde_json::to_value(value).unwrap_or(serde_json::Value::Null),
        options,
        min: None,
        max: None,
        attribute: Some(attribute),
        help: None,
    }
}

fn toggle(key: &'static str, label: &'static str, value: bool) -> Setting {
    Setting {
        key,
        label,
        control: Control::Checkbox,
        value: value.into(),
        options: Vec::new(),
        min: None,
        max: None,
        attribute: None,
        help: None,
    }
}

fn number(
    key: &'static str,
    label: &'static str,
    min: i64,
    max: i64,
    value: usize,
    help: &'static str,
) -> Setting {
    Setting {
        key,
        label,
        control: Control::Number,
        value: value.into(),
        options: Vec::new(),
        min: Some(min),
        max: Some(max),
        attribute: None,
        help: Some(help),
    }
}

impl ReaderPreferences {
    pub(super) fn validate(&self) -> Result<()> {
        if ![10, 25, 50].contains(&self.feed_page_size) {
            bail!("[site.preferences] feed_page_size must be 10, 25, or 50");
        }
        if !(1..=100).contains(&self.scroll_amount) {
            bail!("[site.preferences] scroll_amount must be between 1 and 100 lines");
        }
        if self.offline_items != Self::default().offline_items {
            log::warn!(
                "[site.preferences] offline_items is no longer used: the reader caches pages it visits instead of downloading an archive"
            );
        }
        Ok(())
    }

    /// Every setting, in the order `/preferences/` presents them.
    fn settings(&self) -> Vec<(&'static str, &'static str, Vec<Setting>)> {
        vec![
            (
                "appearance",
                "Appearance",
                vec![
                    select(
                        "theme",
                        "Theme",
                        "theme",
                        ReaderTheme::options(),
                        self.theme,
                    ),
                    select(
                        "motion",
                        "Animations",
                        "motion",
                        Motion::options(),
                        self.motion,
                    ),
                ],
            ),
            (
                "reading",
                "Reading",
                vec![
                    select(
                        "font-family",
                        "Typeface",
                        "fontFamily",
                        FontFamily::options(),
                        self.font_family,
                    ),
                    select(
                        "text-size",
                        "Article text",
                        "textSize",
                        TextSize::options(),
                        self.text_size,
                    ),
                    select(
                        "reading-width",
                        "Line width",
                        "readingWidth",
                        ReadingWidth::options(),
                        self.reading_width,
                    ),
                    select(
                        "line-spacing",
                        "Line spacing",
                        "lineSpacing",
                        Spacing::options(),
                        self.line_spacing,
                    ),
                    select(
                        "paragraph-spacing",
                        "Paragraph spacing",
                        "paragraphSpacing",
                        Spacing::options(),
                        self.paragraph_spacing,
                    ),
                    toggle(
                        "paragraph-indent",
                        "Indent paragraphs",
                        self.paragraph_indent,
                    ),
                    select(
                        "text-align",
                        "Text alignment",
                        "textAlign",
                        TextAlign::options(),
                        self.text_align,
                    ),
                    select(
                        "letter-spacing",
                        "Letter spacing",
                        "letterSpacing",
                        TextSpacing::options(),
                        self.letter_spacing,
                    ),
                    select(
                        "word-spacing",
                        "Word spacing",
                        "wordSpacing",
                        TextSpacing::options(),
                        self.word_spacing,
                    ),
                ],
            ),
            (
                "feed",
                "Feed",
                vec![
                    select(
                        "density",
                        "Density",
                        "density",
                        Density::options(),
                        self.density,
                    ),
                    select(
                        "thumbnails",
                        "Preview images",
                        "thumbnails",
                        Thumbnails::options(),
                        self.thumbnails,
                    ),
                    // Browser page slicing is CSS-driven, so this one needs a dataset attribute
                    // even though it has no typed enum.
                    select(
                        "feed-page-size",
                        "Maximum per page",
                        "feedPageSize",
                        ["10", "25", "50"]
                            .into_iter()
                            .map(|value| PreferenceOption {
                                value: value.into(),
                                label: value.into(),
                            })
                            .collect(),
                        self.feed_page_size.to_string(),
                    ),
                    select(
                        "date-format",
                        "Dates",
                        "dateFormat",
                        DateFormat::options(),
                        self.date_format,
                    ),
                ],
            ),
            (
                "keyboard",
                "Keyboard",
                vec![
                    number(
                        "scroll-amount",
                        "d/u scroll distance (lines)",
                        1,
                        100,
                        self.scroll_amount,
                        "Moves up to this many lines, limited to half the visible article.",
                    ),
                    toggle(
                        "single-key-shortcuts",
                        "Enable keyboard shortcuts",
                        self.single_key_shortcuts,
                    ),
                ],
            ),
        ]
    }

    /// The bootstrap rules and the form groups, derived from one table so they cannot drift.
    pub fn schema(&self) -> PreferenceSchema {
        let mut bootstrap = std::collections::BTreeMap::new();
        let mut groups = Vec::new();
        for (id, title, settings) in self.settings() {
            let mut fields = Vec::new();
            for setting in settings {
                bootstrap.insert(
                    setting.key,
                    BootstrapRule {
                        initial: setting.value.clone(),
                        values: match setting.control {
                            Control::Select => Some(
                                setting
                                    .options
                                    .iter()
                                    .map(|option| option.value.clone().into())
                                    .collect(),
                            ),
                            Control::Checkbox => Some(vec![false.into(), true.into()]),
                            Control::Number => None,
                        },
                        min: setting.min,
                        max: setting.max,
                        attribute: setting.attribute,
                    },
                );
                fields.push(PreferenceField {
                    key: setting.key,
                    label: setting.label,
                    control: setting.control,
                    value: setting.value,
                    options: setting.options,
                    min: setting.min,
                    max: setting.max,
                    help: setting.help,
                });
            }
            groups.push(PreferenceGroup { id, title, fields });
        }
        PreferenceSchema { bootstrap, groups }
    }

    pub fn browser_defaults(&self) -> Result<serde_json::Value> {
        let mut values = serde_json::to_value(self)?;
        // Select controls use string values; numeric inputs retain their numeric types.
        values["feed-page-size"] = self.feed_page_size.to_string().into();
        Ok(values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn reader_defaults_use_preferences_and_disable_indentation() {
        let config = Config::parse("[site.preferences]\nscroll_amount=7\n").unwrap();
        let values = config.site.preferences.browser_defaults().unwrap();
        assert_eq!(values["paragraph-indent"], false);
        assert_eq!(values["scroll-amount"], 7);
        assert_eq!(values["feed-page-size"], "50");
        assert_eq!(values.as_object().unwrap().len(), 17);
    }

    #[test]
    fn retired_offline_items_parses_without_reaching_the_browser() {
        let config = Config::parse("[site.preferences]\noffline_items=7\n").unwrap();
        let values = config.site.preferences.browser_defaults().unwrap();
        assert!(values.get("offline-items").is_none());
        assert!(
            !config
                .site
                .preferences
                .schema()
                .bootstrap
                .contains_key("offline-items")
        );
    }

    #[test]
    fn reader_defaults_accept_typed_overrides_and_reject_invalid_values() {
        let config = Config::parse("[site.preferences]\nparagraph_indent=true\nfont_family='serif'\nscroll_amount=15\ntheme='sepia'\n").unwrap();
        let values = config.site.preferences.browser_defaults().unwrap();
        assert_eq!(values["paragraph-indent"], true);
        assert_eq!(values["font-family"], "serif");
        assert_eq!(values["scroll-amount"], 15);
        for setting in [
            "scroll_amount=0",
            "scroll_amount=101",
            "offline_items=-1",
            "paragraph_indent='true'",
            "theme='purple'",
            "feed_page_size=20",
            "unknown=true",
        ] {
            assert!(
                Config::parse(&format!("[site.preferences]\n{setting}\n")).is_err(),
                "{setting}"
            );
        }
    }

    #[test]
    fn documented_reader_defaults_match_compiled_defaults() {
        let config = Config::parse(crate::config::DEFAULTS).unwrap();
        assert_eq!(config.site.preferences, ReaderPreferences::default());
    }

    #[test]
    fn schema_covers_every_browser_setting_exactly_once() {
        let preferences = ReaderPreferences::default();
        let schema = preferences.schema();
        let exported = preferences.browser_defaults().unwrap();
        let exported = exported.as_object().unwrap();

        let fields: Vec<_> = schema
            .groups
            .iter()
            .flat_map(|group| group.fields.iter())
            .collect();
        assert_eq!(fields.len(), exported.len());
        assert_eq!(schema.bootstrap.len(), exported.len());
        for field in &fields {
            let rule = schema
                .bootstrap
                .get(field.key)
                .unwrap_or_else(|| panic!("{} is missing a bootstrap rule", field.key));
            assert_eq!(rule.initial, field.value, "{}", field.key);
            let default = exported
                .get(field.key)
                .unwrap_or_else(|| panic!("{} is not exported to the browser", field.key));
            assert_eq!(default, &field.value, "{}", field.key);
            match field.control {
                Control::Select => assert!(
                    rule.values
                        .as_ref()
                        .is_some_and(|values| values.contains(&field.value)),
                    "{} default is not one of its options",
                    field.key
                ),
                Control::Number => assert!(rule.min.is_some() && rule.max.is_some()),
                Control::Checkbox => assert!(field.value.is_boolean()),
            }
        }
    }

    #[test]
    fn site_defaults_flow_into_both_schema_views() {
        let config =
            Config::parse("[site.preferences]\ntheme='dark'\nfeed_page_size=25\n").unwrap();
        let schema = config.site.preferences.schema();
        assert_eq!(schema.bootstrap["theme"].initial, "dark");
        assert_eq!(schema.bootstrap["feed-page-size"].initial, "25");
        let feed = schema
            .groups
            .iter()
            .find(|group| group.id == "feed")
            .unwrap();
        let size = feed
            .fields
            .iter()
            .find(|field| field.key == "feed-page-size")
            .unwrap();
        assert_eq!(size.value, "25");
        assert_eq!(size.options.len(), 3);
    }
}
