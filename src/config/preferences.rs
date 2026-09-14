//! Typed defaults for the reader's browser-local preferences.

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
    /// Newest article pages and retained images downloaded ahead of time.
    pub offline_items: usize,
}

macro_rules! choices {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
        pub enum $name {
            $(#[serde(rename = $value)] $variant),+
        }
    };
}

choices!(ReaderTheme { Auto => "auto", Light => "light", Dark => "dark", Sepia => "sepia" });
choices!(DateFormat { Relative => "relative", Iso => "iso", Local => "local", LocalTime => "local-time" });
choices!(TextSize { Default => "default", Large => "large", Largest => "largest" });
choices!(ReadingWidth { Narrow => "narrow", Standard => "standard", Wide => "wide" });
choices!(FontFamily { Sans => "sans", Serif => "serif", Mono => "mono" });
choices!(Spacing { Compact => "compact", Standard => "standard", Relaxed => "relaxed" });
choices!(TextAlign { Left => "left", Justify => "justify" });
choices!(TextSpacing { Normal => "normal", Wide => "wide" });
choices!(Density { Compact => "compact", Comfortable => "comfortable" });
choices!(Thumbnails { Show => "show", Hide => "hide" });
choices!(Motion { Auto => "auto", Off => "off" });

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

impl ReaderPreferences {
    pub(super) fn validate(&self) -> Result<()> {
        if ![10, 25, 50].contains(&self.feed_page_size) {
            bail!("[site.preferences] feed_page_size must be 10, 25, or 50");
        }
        if !(1..=100).contains(&self.scroll_amount) {
            bail!("[site.preferences] scroll_amount must be between 1 and 100 lines");
        }
        if self.offline_items > 1000 {
            bail!("[site.preferences] offline_items must be between 0 and 1000");
        }
        Ok(())
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
        let config = Config::parse("[site.preferences]\noffline_items=7\n").unwrap();
        let values = config.site.preferences.browser_defaults().unwrap();
        assert_eq!(values["paragraph-indent"], false);
        assert_eq!(values["offline-items"], 7);
        assert_eq!(values["feed-page-size"], "50");
        assert_eq!(values["scroll-amount"], 10);
        assert_eq!(values.as_object().unwrap().len(), 18);
    }

    #[test]
    fn reader_defaults_accept_typed_overrides_and_reject_invalid_values() {
        let config = Config::parse("[site.preferences]\nparagraph_indent=true\nfont_family='serif'\nscroll_amount=15\noffline_items=0\ntheme='sepia'\n").unwrap();
        let values = config.site.preferences.browser_defaults().unwrap();
        assert_eq!(values["paragraph-indent"], true);
        assert_eq!(values["font-family"], "serif");
        assert_eq!(values["offline-items"], 0);
        for setting in [
            "scroll_amount=0",
            "scroll_amount=101",
            "offline_items=1001",
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
}
