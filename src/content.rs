//! HTML in, safe Markdown/HTML/text out. Three views of the same content:
//! the raw `.html` copy (stripped of active content, capped), the sanitized HTML view, and the
//! Markdown body derived from it. Rendering Markdown back to HTML never emits raw HTML.
//!
//! The pipeline is split along its seams: [`scan`] is the tag scanner every pass builds on,
//! [`strip`] the storage and sanitizer safety layer, [`extract`] Readability extraction,
//! [`markdown`] HTML to Markdown, [`cleanup`] the Markdown cleanup shared by capture and build,
//! [`math`] dollar-delimited TeX as readable text, [`render`] Markdown to reader HTML and
//! [`resources`] the leading resource-link detection.

mod cleanup;
mod extract;
#[path = "content_highlight.rs"]
mod highlight;
mod markdown;
mod math;
mod module;
mod render;
mod resources;
mod scan;
mod strip;

pub use cleanup::strip_article_metadata;
pub(crate) use extract::balanced_json_object;
pub use extract::{
    ExtractedArticle, extract_article_async, extraction_misses_feed_content, feed_content_on_page,
    is_placeholder_body,
};
pub(crate) use markdown::normalize_image_sources;
pub use markdown::to_markdown;
pub use module::{article_from_module, is_script_shell, module_scripts};
#[cfg(test)]
pub use render::reading_metrics;
pub use render::{
    LocalImage, LocalImageVariant, PreparedMarkdown, anchor_headings, embed_body_videos, excerpt,
    image_dimensions, render_markdown,
};
pub use resources::ResourceLink;
pub use strip::{html_to_text, sanitize, storage_html};

pub use crate::model::image_alt;

/// `& < > " '` replaced by their entities, for text nodes and attribute values alike.
pub(crate) fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
