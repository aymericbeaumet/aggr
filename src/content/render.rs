//! Markdown to safe HTML: comrak with raw HTML off, static syntax highlighting, substitution of
//! validated local images, inline video facades, heading anchors and table scroll regions,
//! behind [`PreparedMarkdown`].

use std::sync::OnceLock;

use scraper::{Html, Selector};
use url::Url;

use super::markdown::normalize_image_sources;
use super::resources::{ResourceLink, leading_resources};
use super::scan::{attribute_value, parse_tag, set_attribute};
use super::strip::{decode_entities, html_to_text, sanitize};
use super::{escape_html, highlight};

/// comrak with GFM extensions and raw HTML escaped rather than passed through, so the output is
/// safe by construction whatever the Markdown says.
pub fn render_markdown(markdown: &str) -> String {
    render_markdown_with_images(markdown, &[])
}

const READING_WORDS_PER_MINUTE: usize = 225;

/// Count visible Unicode words in Markdown and estimate reading time, rounded up to the next
/// minute. Empty documents deliberately report zero minutes so title-only entries can omit the
/// metric instead of promising a one-minute article.
#[cfg(test)]
pub fn reading_metrics(markdown: &str) -> (usize, usize) {
    use unicode_segmentation::UnicodeSegmentation as _;

    let html = render_markdown_html(markdown, &comrak::options::Plugins::default());
    let text = html_to_text(&html);
    let words = text.unicode_words().count();
    let minutes = words.div_ceil(READING_WORDS_PER_MINUTE);
    (words, minutes)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalImageVariant {
    pub url: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalImage {
    pub source: String,
    pub original: String,
    /// The publisher's `<img alt>` where the image was found, already normalised by the media
    /// pipeline; `None` for social cards and images that had none.
    pub alt: Option<String>,
    pub variants: Vec<LocalImageVariant>,
    pub width: u32,
    pub height: u32,
    pub color: String,
    pub placeholder: crate::media::placeholder::Placeholder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageDimensions {
    pub source: String,
    pub width: u32,
    pub height: u32,
}

/// Recover intrinsic publisher dimensions without downloading images or changing Markdown.
pub fn image_dimensions(html: &str, base: Option<&Url>) -> Vec<ImageDimensions> {
    let clean = sanitize(&normalize_image_sources(html), base);
    let document = Html::parse_fragment(&clean);
    let Ok(selector) = Selector::parse("img[src]") else {
        return Vec::new();
    };
    document
        .select(&selector)
        .filter_map(|image| {
            let source = image.value().attr("src")?;
            let (width, height) =
                image_size(image.value().attr("width"), image.value().attr("height"))?;
            Some(ImageDimensions {
                source: source.to_string(),
                width,
                height,
            })
        })
        .collect()
}

fn image_size(width: Option<&str>, height: Option<&str>) -> Option<(u32, u32)> {
    let positive = |value: &str| value.trim().parse::<u32>().ok().filter(|value| *value > 0);
    Some((positive(width?)?, positive(height?)?))
}

pub fn render_markdown_with_image_dimensions(
    markdown: &str,
    images: &[LocalImage],
    dimensions: &[ImageDimensions],
) -> String {
    PreparedMarkdown::new(markdown).with_images(images, dimensions)
}

/// A safe, highlighted Markdown rendering before output-specific image substitution. Keeping the
/// HTML private prevents callers from treating arbitrary stored HTML as an already-safe rendering.
pub struct PreparedMarkdown {
    html: String,
    resource_range: Option<std::ops::Range<usize>>,
    reader: OnceLock<String>,
    resources: Vec<ResourceLink>,
    text: OnceLock<String>,
    portable: OnceLock<String>,
}

impl PreparedMarkdown {
    pub fn new(markdown: &str) -> Self {
        let mut plugins = comrak::options::Plugins::default();
        plugins.render.codefence_syntax_highlighter = Some(&CodeHighlighter);
        let html = add_link_navigation_attributes(&render_markdown_html(markdown, &plugins));
        let (resources, resource_range) = leading_resources(&html)
            .map(|(resources, range)| (resources, Some(range)))
            .unwrap_or_default();
        Self {
            html,
            resource_range,
            reader: OnceLock::new(),
            resources,
            text: OnceLock::new(),
            portable: OnceLock::new(),
        }
    }

    pub fn plain_text(&self) -> &str {
        self.text.get_or_init(|| html_to_text(self.reader_html()))
    }

    pub(super) fn reader_html(&self) -> &str {
        let Some(range) = &self.resource_range else {
            return &self.html;
        };
        self.reader.get_or_init(|| {
            let mut html = String::with_capacity(self.html.len() - range.len());
            html.push_str(&self.html[..range.start]);
            html.push_str(&self.html[range.end..]);
            html
        })
    }

    pub fn resources(&self) -> &[ResourceLink] {
        &self.resources
    }

    /// Only the reader moves resource links into its header area. Portable representations keep
    /// the full original paragraph, so consumers never lose destinations when copying content.
    pub fn reader_html_with_images(
        &self,
        images: &[LocalImage],
        dimensions: &[ImageDimensions],
    ) -> String {
        let html = if self.resource_range.is_none() {
            self.with_images(images, dimensions)
        } else {
            enhance_rendered_images(self.reader_html(), images, dimensions)
        };
        wrap_scrollable_tables(&html)
    }

    pub fn reading_metrics(&self) -> (usize, usize) {
        use unicode_segmentation::UnicodeSegmentation as _;
        let words = self.plain_text().unicode_words().count();
        (words, words.div_ceil(READING_WORDS_PER_MINUTE))
    }

    pub fn excerpt(&self, max_chars: usize) -> String {
        text_excerpt(self.plain_text(), max_chars)
    }

    pub fn portable_html(&self) -> &str {
        self.portable
            .get_or_init(|| enhance_rendered_images(&self.html, &[], &[]))
    }

    pub fn with_images(&self, images: &[LocalImage], dimensions: &[ImageDimensions]) -> String {
        if images.is_empty() && dimensions.is_empty() {
            self.portable_html().to_string()
        } else {
            enhance_rendered_images(&self.html, images, dimensions)
        }
    }
}

/// Give reader headings stable ids and turn publisher self-links (`## [Title](#title)` or a link
/// back to the article's own page) into plain headings. Only the reader uses this; portable
/// representations keep the original links.
pub fn anchor_headings(html: &str, article_url: Option<&Url>) -> String {
    let mut out = String::with_capacity(html.len() + 64);
    let mut used = std::collections::BTreeSet::<String>::new();
    let mut position = 0;
    while let Some(offset) = html[position..].find("<h") {
        let start = position + offset;
        let Some(tag) = parse_tag(&html[start..]) else {
            out.push_str(&html[position..start + 2]);
            position = start + 2;
            continue;
        };
        let level = tag
            .name
            .strip_prefix('h')
            .filter(|rest| matches!(*rest, "1" | "2" | "3" | "4" | "5" | "6"));
        let (Some(_), false, Some(open_end)) = (level, tag.closing, tag.end) else {
            out.push_str(&html[position..start + 2]);
            position = start + 2;
            continue;
        };
        let closing = format!("</{}>", tag.name);
        let Some(inner_len) = html[start + open_end..].to_ascii_lowercase().find(&closing) else {
            out.push_str(&html[position..start + open_end]);
            position = start + open_end;
            continue;
        };
        let open = &html[start..start + open_end];
        let inner = &html[start + open_end..start + open_end + inner_len];
        let (inner, fragment) = unwrap_self_link(inner, article_url);
        let mut id = attribute_value(open, "id")
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .or(fragment)
            .unwrap_or_else(|| slug::slugify(html_to_text(inner)));
        if id.is_empty() {
            id = "section".to_string();
        }
        let mut unique = id.clone();
        let mut counter = 2;
        while !used.insert(unique.clone()) {
            unique = format!("{id}-{counter}");
            counter += 1;
        }
        out.push_str(&html[position..start]);
        let open = set_attribute(open, "id", &unique);
        // The page already has its title as the only `<h1>`; a publisher heading at that level
        // reads as a second document title, so assistive technology hears it as a section.
        if tag.name == "h1" {
            out.push_str(&set_attribute(&open, "aria-level", "2"));
        } else {
            out.push_str(&open);
        }
        out.push_str(inner);
        out.push_str(&closing);
        position = start + open_end + inner_len + closing.len();
    }
    out.push_str(&html[position..]);
    out
}

/// Turn a rendered paragraph that is only a link to a supported video into the same activation
/// facade the item page uses, so provider videos play inline instead of leaving the reader. The
/// poster is the linked picture when the paragraph had one, else the archived provider thumbnail;
/// nothing is requested from the provider before activation. Reader only; portable outputs keep
/// the link.
pub fn embed_body_videos(html: &str, images: &[LocalImage]) -> String {
    if !html.contains("<p><a ") {
        return html.to_string();
    }
    let mut out = String::with_capacity(html.len());
    let mut position = 0;
    while let Some(offset) = html[position..].find("<p><a ") {
        let start = position + offset;
        let Some(end) = html[start..].find("</p>").map(|index| start + index + 4) else {
            break;
        };
        out.push_str(&html[position..start]);
        let paragraph = &html[start..end];
        match video_facade(&paragraph[3..paragraph.len() - 4], images) {
            Some(facade) => out.push_str(&facade),
            None => out.push_str(paragraph),
        }
        position = end;
    }
    out.push_str(&html[position..]);
    out
}

fn video_facade(inner: &str, images: &[LocalImage]) -> Option<String> {
    let trimmed = inner.trim();
    let tag = parse_tag(trimmed)?;
    let (Some(open_end), "a", false) = (tag.end, tag.name.as_str(), tag.closing) else {
        return None;
    };
    let content = trimmed[open_end..].strip_suffix("</a>")?.trim();
    if content.contains("<a ") || content.contains("<a>") {
        return None;
    }
    let href = decode_entities(attribute_value(&trimmed[..open_end], "href")?);
    let href = href.trim();
    let video = crate::site::video::VideoCtx::from_url(href)?;
    let pictures = if content.contains("<picture") {
        content.matches("<picture").count()
    } else {
        content.matches("<img").count()
    };
    let text = html_to_text(content);
    let mut placeholder = String::new();
    let poster = if pictures == 1 && text.trim().is_empty() {
        content.to_string()
    } else if pictures == 0 && !text.contains('<') {
        let thumbnail = Url::parse(href)
            .ok()
            .and_then(|url| crate::sources::youtube::thumbnail(&url));
        match thumbnail.and_then(|url| images.iter().find(|image| image.source == url)) {
            Some(image) => {
                placeholder = format!(
                    " data-thumbhash=\"{}\" style=\"--image-placeholder: {}; --image-preview: url('{}')\"",
                    escape_html(&image.placeholder.hash),
                    escape_html(&image.color),
                    image.placeholder.data_url
                );
                format!(
                    "<img src=\"{}\" width=\"{}\" height=\"{}\" alt=\"\" loading=\"lazy\" decoding=\"async\">",
                    escape_html(&image.original),
                    image.width,
                    image.height
                )
            }
            None => format!(
                "<span class=\"video-preview-label\">{}</span>",
                escape_html(video.title)
            ),
        }
    } else {
        return None;
    };
    Some(format!(
        "<div class=\"video-player video-player-inline\" data-video-provider=\"{provider}\"{placeholder}><a class=\"video-preview\" data-video-embed=\"{embed}\"{parent} href=\"{href}\" title=\"{href}\" target=\"_blank\" rel=\"noopener noreferrer\" aria-label=\"Play video on {title}\">{poster}<span class=\"video-preview-play\" aria-hidden=\"true\"><svg viewBox=\"0 0 24 24\"><path d=\"M8 5v14l11-7z\"/></svg></span></a></div>",
        provider = video.provider,
        embed = escape_html(&video.embed_url),
        parent = if video.requires_parent {
            " data-video-parent"
        } else {
            ""
        },
        href = escape_html(href),
        title = escape_html(video.title),
    ))
}

/// A heading whose entire content is one link to its own anchor or article page.
fn unwrap_self_link<'a>(inner: &'a str, article_url: Option<&Url>) -> (&'a str, Option<String>) {
    let trimmed = inner.trim();
    let Some(tag) = parse_tag(trimmed) else {
        return (inner, None);
    };
    let (Some(open_end), "a", false) = (tag.end, tag.name.as_str(), tag.closing) else {
        return (inner, None);
    };
    let Some(content) = trimmed[open_end..].strip_suffix("</a>") else {
        return (inner, None);
    };
    if content.contains("<a ") || content.contains("<a>") {
        return (inner, None);
    }
    let Some(href) = attribute_value(&trimmed[..open_end], "href") else {
        return (inner, None);
    };
    let href = decode_entities(href);
    let href = href.trim();
    let fragment = if let Some(fragment) = href.strip_prefix('#') {
        Some(fragment.to_string())
    } else {
        let Some(article) = article_url else {
            return (inner, None);
        };
        let Ok(target) = Url::parse(href) else {
            return (inner, None);
        };
        let mut own = article.clone();
        own.set_fragment(None);
        let mut page = target.clone();
        page.set_fragment(None);
        if own != page {
            return (inner, None);
        }
        target.fragment().map(str::to_string)
    };
    (content, fragment.filter(|fragment| !fragment.is_empty()))
}

/// Render safe Markdown and replace only validated publisher images with immutable local assets.
/// The first image is allowed to become the LCP resource; later images use native lazy loading.
pub fn render_markdown_with_images(markdown: &str, images: &[LocalImage]) -> String {
    render_markdown_with_image_dimensions(markdown, images, &[])
}

fn render_markdown_html(markdown: &str, plugins: &comrak::options::Plugins<'_>) -> String {
    super::math::render_math_spans(&render_comrak_html(markdown, plugins))
}

fn render_comrak_html(markdown: &str, plugins: &comrak::options::Plugins<'_>) -> String {
    let options = markdown_options();
    if !markdown.contains("\\\n") && !markdown.contains("\\\r\n") {
        return comrak::markdown_to_html_with_plugins(markdown, &options, plugins);
    }
    // Parse breaks before autolinking, whose URL matcher otherwise consumes a trailing backslash.
    let mut parse_options = markdown_options();
    parse_options.extension.autolink = false;
    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, markdown, &parse_options);
    let mut lines = vec![0];
    lines.extend(markdown.match_indices('\n').map(|(index, _)| index + 1));
    let mut positions = root
        .descendants()
        .filter_map(|node| {
            let data = node.data.borrow();
            if !matches!(data.value, comrak::nodes::NodeValue::LineBreak) {
                return None;
            }
            let index = *lines.get(data.sourcepos.start.line.checked_sub(1)?)?
                + data.sourcepos.start.column.saturating_sub(1);
            (markdown.as_bytes().get(index) == Some(&b'\\')).then_some(index)
        })
        .collect::<Vec<_>>();
    positions.sort_unstable();
    positions.dedup();
    let mut prepared = markdown.to_string();
    for index in positions.into_iter().rev() {
        prepared.replace_range(index..index + 1, "  ");
    }
    comrak::markdown_to_html_with_plugins(&prepared, &options, plugins)
}

fn markdown_options() -> comrak::Options<'static> {
    let mut options = comrak::Options::default();
    options.extension.table = true;
    options.extension.strikethrough = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    options.extension.footnotes = true;
    options.extension.math_dollars = true;
    options.render.r#unsafe = false;
    options.render.escape = true;
    options
}

fn enhance_rendered_images(
    html: &str,
    images: &[LocalImage],
    dimensions: &[ImageDimensions],
) -> String {
    let mut out = String::with_capacity(html.len() + images.len() * 160);
    let mut position = 0;
    let mut image_index = 0;
    while let Some(start) = html[position..]
        .find("<img ")
        .map(|offset| position + offset)
    {
        out.push_str(&html[position..start]);
        let Some(tag) = parse_tag(&html[start..]) else {
            out.push('<');
            position = start + 1;
            continue;
        };
        let Some(end) = tag.end else {
            out.push_str(&html[start..]);
            return out;
        };
        let original = &html[start..start + end];
        out.push_str(&render_image_tag(original, image_index, images, dimensions));
        image_index += 1;
        position = start + end;
    }
    out.push_str(&html[position..]);
    group_captioned_figures(&out)
}

/// The reader's scroll container around a table: a labelled region that keyboard users can focus
/// and pan, wrapping a table that keeps its own layout box so header and body columns line up.
const TABLE_SCROLL_OPEN: &str =
    "<div class=\"table-scroll\" role=\"region\" tabindex=\"0\" aria-label=\"Table\">";

/// Wrap every top-level table in [`TABLE_SCROLL_OPEN`]; a table nested inside another table
/// scrolls with its parent and is left alone. Reader only: portable representations and stored
/// bodies keep the bare table.
fn wrap_scrollable_tables(html: &str) -> String {
    if !html.contains("<table") {
        return html.to_string();
    }
    let mut out = String::with_capacity(html.len() + 2 * TABLE_SCROLL_OPEN.len());
    let mut depth = 0usize;
    let mut position = 0;
    while let Some(offset) = html[position..].find("<") {
        let start = position + offset;
        out.push_str(&html[position..start]);
        let table = parse_tag(&html[start..]).filter(|tag| tag.name == "table");
        let Some(tag) = table else {
            out.push('<');
            position = start + 1;
            continue;
        };
        let end = tag.end.unwrap_or(html.len() - start);
        if tag.closing {
            depth = depth.saturating_sub(1);
            out.push_str(&html[start..start + end]);
            if depth == 0 {
                out.push_str("</div>");
            }
        } else {
            if depth == 0 {
                out.push_str(TABLE_SCROLL_OPEN);
            }
            depth += 1;
            out.push_str(&html[start..start + end]);
        }
        position = start + end;
    }
    out.push_str(&html[position..]);
    out
}

/// An image followed by a hard break and a short line is the figure/caption pair that Markdown
/// cannot express. Restore the semantics so the caption reads as a caption instead of body prose.
fn group_captioned_figures(html: &str) -> String {
    static FIGURE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let figure = FIGURE.get_or_init(|| {
        regex::Regex::new(
            r"(?s)<p>((?:<a [^>]*>)?<picture\b.*?</picture>(?:</a>)?)<br />\s*(.*?)</p>",
        )
        .expect("valid figure pattern")
    });
    figure
        .replace_all(
            html,
            "<figure class=\"article-figure\">$1<figcaption>$2</figcaption></figure>",
        )
        .into_owned()
}

fn render_image_tag(
    tag: &str,
    index: usize,
    images: &[LocalImage],
    hints: &[ImageDimensions],
) -> String {
    let fragment = Html::parse_fragment(tag);
    let Ok(selector) = Selector::parse("img") else {
        return tag.to_string();
    };
    let Some(image) = fragment.select(&selector).next() else {
        return tag.to_string();
    };
    let Some(source) = image.value().attr("src") else {
        return tag.to_string();
    };
    let badge = crate::media::is_status_badge(source);
    let badge_class = if badge { " article-badge" } else { "" };
    let alt = image.value().attr("alt").unwrap_or_default();
    let title = image.value().attr("title");
    let key = normalized_image_url(source);
    let local = images
        .iter()
        .find(|candidate| normalized_image_url(&candidate.source) == key)
        .filter(|candidate| candidate.width > 0 && candidate.height > 0);
    let loading = if index == 0 { "eager" } else { "lazy" };
    let priority = if index == 0 { "high" } else { "low" };
    let intrinsic = local
        .map(|local| (local.width, local.height))
        .or_else(|| image_size(image.value().attr("width"), image.value().attr("height")))
        .or_else(|| {
            hints
                .iter()
                .find(|hint| {
                    normalized_image_url(&hint.source) == key && hint.width > 0 && hint.height > 0
                })
                .map(|hint| (hint.width, hint.height))
        })
        .or_else(|| badge.then_some((160, 24)));
    let dimensions = intrinsic
        .map(|(width, height)| format!(" width=\"{width}\" height=\"{height}\""))
        .unwrap_or_default();
    let src = local.map_or(source, |local| local.original.as_str());
    let title = title
        .map(|title| format!(" title=\"{}\"", escape_html(title)))
        .unwrap_or_default();
    let responsive = local.map_or_else(String::new, |local| {
        let mut candidates = local
            .variants
            .iter()
            .filter(|variant| variant.width >= 320 && variant.width < local.width)
            .map(|variant| (variant.width, variant.url.as_str()))
            .collect::<std::collections::BTreeMap<_, _>>();
        if candidates.is_empty() {
            return String::new();
        }
        candidates.insert(local.width, local.original.as_str());
        let srcset = candidates
            .into_iter()
            .map(|(width, url)| format!("{} {width}w", escape_html(url)))
            .collect::<Vec<_>>()
            .join(", ");
        format!(" srcset=\"{srcset}\" sizes=\"(max-width: 56rem) calc(100vw - 2rem), 52rem\"")
    });
    let tag = format!(
        "<img src=\"{}\"{} alt=\"{}\"{}{responsive} loading=\"{}\" decoding=\"async\" fetchpriority=\"{}\" referrerpolicy=\"no-referrer\" class=\"progressive-image\">",
        escape_html(src),
        dimensions,
        escape_html(alt),
        title,
        loading,
        priority
    );
    let Some(local) = local else {
        return match intrinsic {
            Some((width, height)) => format!(
                "<picture class=\"article-picture{badge_class}\" style=\"--image-width:{width}px;--image-ratio:{width} / {height}\">{tag}</picture>"
            ),
            None => format!(
                "<picture class=\"article-picture article-picture-fallback\" style=\"--image-ratio:16 / 9\">{tag}</picture>"
            ),
        };
    };
    let source = if let Some(full) = local
        .variants
        .last()
        .filter(|variant| variant.width == local.width && variant.height == local.height)
    {
        let srcset = local
            .variants
            .iter()
            .map(|variant| format!("{} {}w", escape_html(&variant.url), variant.width))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "<source type=\"image/webp\" width=\"{}\" height=\"{}\" srcset=\"{srcset}\" sizes=\"(max-width: 56rem) calc(100vw - 2rem), 52rem\">",
            full.width, full.height
        )
    } else {
        String::new()
    };
    format!(
        "<picture class=\"article-picture{badge_class}\" data-thumbhash=\"{}\" style=\"--image-width:{}px;--image-ratio:{} / {};--image-placeholder:{};--image-preview:url('{}')\">{source}{tag}</picture>",
        escape_html(&local.placeholder.hash),
        local.width,
        local.width,
        local.height,
        escape_html(&local.color),
        escape_html(&local.placeholder.data_url)
    )
}

fn normalized_image_url(value: &str) -> String {
    let Ok(mut url) = Url::parse(value) else {
        return value.to_string();
    };
    url.set_fragment(None);
    url.to_string()
}

struct CodeHighlighter;

impl comrak::adapters::SyntaxHighlighterAdapter for CodeHighlighter {
    fn write_highlighted(
        &self,
        output: &mut dyn std::fmt::Write,
        language: Option<&str>,
        code: &str,
    ) -> std::fmt::Result {
        highlight::write(output, language, code)
    }

    fn write_pre_tag(
        &self,
        output: &mut dyn std::fmt::Write,
        attributes: std::collections::HashMap<&'static str, std::borrow::Cow<'_, str>>,
    ) -> std::fmt::Result {
        write_code_tag(output, "pre", attributes)
    }

    fn write_code_tag(
        &self,
        output: &mut dyn std::fmt::Write,
        attributes: std::collections::HashMap<&'static str, std::borrow::Cow<'_, str>>,
    ) -> std::fmt::Result {
        write_code_tag(output, "code", attributes)
    }
}

fn write_code_tag(
    output: &mut dyn std::fmt::Write,
    tag: &str,
    attributes: std::collections::HashMap<&'static str, std::borrow::Cow<'_, str>>,
) -> std::fmt::Result {
    write!(output, "<{tag}")?;
    let mut attributes = attributes.into_iter().collect::<Vec<_>>();
    attributes.sort_by_key(|(name, _)| *name);
    for (name, value) in attributes {
        write!(output, " {name}=\"{}\"", escape_html(&value))?;
    }
    output.write_char('>')
}

/// Article links open separately, while fragment links such as footnote references and backrefs
/// must navigate within the current document.
fn add_link_navigation_attributes(html: &str) -> String {
    const LINK_OPEN: &str = "<a href=\"";
    const EXTERNAL_LINK_OPEN: &str = "<a target=\"_blank\" rel=\"noopener noreferrer\" href=\"";

    let mut out = String::with_capacity(html.len());
    let mut remaining = html;
    while let Some(index) = remaining.find(LINK_OPEN) {
        out.push_str(&remaining[..index]);
        remaining = &remaining[index + LINK_OPEN.len()..];
        out.push_str(if remaining.starts_with('#') {
            LINK_OPEN
        } else {
            EXTERNAL_LINK_OPEN
        });
    }
    out.push_str(remaining);
    out
}

/// First `max_chars` chars of the Markdown's plain text, cut on a word boundary with `…`.
pub fn excerpt(markdown: &str, max_chars: usize) -> String {
    PreparedMarkdown::new(markdown).excerpt(max_chars)
}

fn text_excerpt(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let window: String = text.chars().take(max_chars).collect();
    let cut = window
        .rfind(char::is_whitespace)
        .filter(|&pos| pos > 0)
        .unwrap_or(window.len());
    let mut out = window[..cut].trim_end().to_string();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::to_markdown;

    #[test]
    fn standalone_video_links_become_inline_facades() {
        let placeholder =
            crate::media::placeholder::from_image(&image::DynamicImage::new_rgb8(4, 4)).unwrap();
        let thumbnail = LocalImage {
            source: "https://i.ytimg.com/vi/xyz987_-ABC/hqdefault.jpg".into(),
            original: "assets/images/poster.jpg".into(),
            alt: None,
            variants: vec![],
            width: 480,
            height: 360,
            color: "#112233".into(),
            placeholder,
        };
        let html = "<p><a target=\"_blank\" href=\"https://youtu.be/abcDEF12345?t=90\"><picture class=\"article-picture\"><img src=\"assets/images/thumb.png\" alt=\"\"></picture></a></p><p><a href=\"https://www.youtube.com/watch?v=xyz987_-ABC\">https://www.youtube.com/watch?v=xyz987_-ABC</a></p><p><a href=\"https://vimeo.com/76979871\">Watch the talk</a></p><p><a href=\"https://youtu.be/short12345\">Watch</a> and <a href=\"https://example.com\">more</a></p><p>See <a href=\"https://youtu.be/inline1234\">this</a>.</p><p><a href=\"https://example.com/video\">Not a provider</a></p>";
        let embedded = embed_body_videos(html, std::slice::from_ref(&thumbnail));
        assert_eq!(
            embedded.matches("video-player-inline").count(),
            3,
            "{embedded}"
        );
        assert!(
            embedded
                .contains("data-video-embed=\"https://www.youtube-nocookie.com/embed/abcDEF12345?"),
            "{embedded}"
        );
        assert!(embedded.contains("<a class=\"video-preview\" data-video-embed=\"https://www.youtube-nocookie.com/embed/abcDEF12345?autoplay=0&amp;rel=0&amp;playsinline=1&amp;iv_load_policy=3&amp;start=90\" href=\"https://youtu.be/abcDEF12345?t=90\""), "{embedded}");
        assert!(embedded.contains("<picture class=\"article-picture\"><img src=\"assets/images/thumb.png\" alt=\"\"></picture><span class=\"video-preview-play\""), "{embedded}");
        assert!(
            embedded.contains("<img src=\"assets/images/poster.jpg\" width=\"480\" height=\"360\""),
            "{embedded}"
        );
        assert!(
            embedded.contains("--image-placeholder: #112233"),
            "{embedded}"
        );
        assert!(
            embedded.contains("data-video-provider=\"vimeo\""),
            "{embedded}"
        );
        assert!(
            embedded.contains("<span class=\"video-preview-label\">Vimeo</span>"),
            "{embedded}"
        );
        assert!(
            embedded.contains("<p><a href=\"https://youtu.be/short12345\">Watch</a> and"),
            "{embedded}"
        );
        assert!(
            embedded.contains("<p>See <a href=\"https://youtu.be/inline1234\">this</a>.</p>"),
            "{embedded}"
        );
        assert!(
            embedded.contains("<p><a href=\"https://example.com/video\">Not a provider</a></p>"),
            "{embedded}"
        );
        assert!(
            !embedded.contains("youtube.com/watch?v=xyz987_-ABC\">https://"),
            "{embedded}"
        );
        assert_eq!(embed_body_videos("<p>plain</p>", &[]), "<p>plain</p>");
    }

    #[test]
    fn reader_headings_become_anchors_without_links() {
        let article = Url::parse("https://cognition.com/blog/factoring-rsa-260").unwrap();
        let html = "<h2><a href=\"https://cognition.com/blog/factoring-rsa-260#how-did-this-happen\">How did this happen?</a></h2><p>Text</p><h3><a href=\"#cost\">Cost <em>estimates</em></a></h3><h2>Cost estimates</h2><h2>Cost estimates</h2><h2><a href=\"https://example.com/paper\">External paper</a></h2><h2>See <a href=\"#x\">partial</a> link</h2><h4 id=\"keep\">Kept id</h4><h2></h2>";
        let anchored = anchor_headings(html, Some(&article));
        assert_eq!(
            anchored,
            "<h2 id=\"how-did-this-happen\">How did this happen?</h2><p>Text</p><h3 id=\"cost\">Cost <em>estimates</em></h3><h2 id=\"cost-estimates\">Cost estimates</h2><h2 id=\"cost-estimates-2\">Cost estimates</h2><h2 id=\"external-paper\"><a href=\"https://example.com/paper\">External paper</a></h2><h2 id=\"see-partial-link\">See <a href=\"#x\">partial</a> link</h2><h4 id=\"keep\">Kept id</h4><h2 id=\"section\"></h2>"
        );
        assert_eq!(
            anchor_headings("<p>no headings</p><hr>", None),
            "<p>no headings</p><hr>"
        );
    }

    #[test]
    fn a_body_h1_reads_as_a_section_heading_without_changing_its_tag() {
        // Feed-supplied bodies sometimes repeat the article title as their own `<h1>`; the page
        // title is the document's only first-level heading.
        assert_eq!(
            anchor_headings(
                "<h1>Title again</h1><h2>Next</h2><h1 id=\"x\">Kept</h1><h3>Deep</h3>",
                None
            ),
            "<h1 id=\"title-again\" aria-level=\"2\">Title again</h1><h2 id=\"next\">Next</h2><h1 id=\"x\" aria-level=\"2\">Kept</h1><h3 id=\"deep\">Deep</h3>"
        );
    }

    #[test]
    fn prepared_article_shares_visible_text_and_preserves_rendering_contracts() {
        for markdown in [
            "# Title\n\nText with **bold** and [link](https://example.org/target).",
            "```rust\nfn main() { println!(\"hi\"); }\n```",
            "![image words](https://example.org/image.jpg)\n\n日本語 café 👋 words.",
            "A footnote[^1].\n\n[^1]: Note here.\n",
            "<script>unsafe()</script>\n\n&amp; literal",
            "",
        ] {
            let prepared = PreparedMarkdown::new(markdown);
            assert_eq!(prepared.reading_metrics(), reading_metrics(markdown));
            assert_eq!(
                prepared.plain_text(),
                html_to_text(prepared.portable_html())
            );
            assert_eq!(prepared.with_images(&[], &[]), render_markdown(markdown));
            assert_eq!(
                prepared.excerpt(20),
                text_excerpt(prepared.plain_text(), 20)
            );
            assert!(std::ptr::eq(prepared.plain_text(), prepared.plain_text()));
        }
    }

    #[test]
    fn article_images_receive_loading_and_privacy_attributes_after_rendering() {
        let html = render_markdown(
            "![Hero](https://example.com/hero.webp)\n\n![Later](https://example.com/later.webp)",
        );
        assert!(html.contains("loading=\"eager\""), "{html}");
        assert!(html.contains("fetchpriority=\"high\""), "{html}");
        assert!(html.contains("loading=\"lazy\""), "{html}");
        assert!(html.contains("fetchpriority=\"low\""), "{html}");
        assert!(html.contains("decoding=\"async\""), "{html}");
        assert!(html.contains("referrerpolicy=\"no-referrer\""), "{html}");
        assert!(html.contains("alt=\"Hero\""), "{html}");
    }

    #[test]
    fn publisher_images_have_stable_fallback_frames_before_loading() {
        let html = render_markdown("![Portrait](https://publisher.example/portrait.png)");
        assert!(
            html.contains("class=\"article-picture article-picture-fallback\""),
            "{html}"
        );
        assert!(html.contains("--image-ratio:16 / 9"), "{html}");
        assert!(html.contains("class=\"progressive-image\""), "{html}");
        assert!(
            !html.contains("width=\"16\""),
            "fallback ratios are not intrinsic dimensions"
        );
    }

    #[test]
    fn preserved_publisher_dimensions_match_resolved_image_urls() {
        let base = Url::parse("https://publisher.example/article/").unwrap();
        let hints = image_dimensions(
            r#"<img data-src="../portrait.png" width="600" height="900"><img src="bad.png" width="100%" height="400"><img src="zero.png" width="0" height="100"><img src="javascript:alert(1)" width="10" height="10">"#,
            Some(&base),
        );
        assert_eq!(
            hints,
            [ImageDimensions {
                source: "https://publisher.example/portrait.png".into(),
                width: 600,
                height: 900
            }]
        );
        let html = render_markdown_with_image_dimensions(
            "![Portrait](https://publisher.example/portrait.png#original)",
            &[],
            &hints,
        );
        assert!(html.contains("width=\"600\" height=\"900\""), "{html}");
        assert!(
            html.contains("--image-width:600px;--image-ratio:600 / 900"),
            "{html}"
        );
        assert!(!html.contains("article-picture-fallback"), "{html}");
    }

    #[test]
    fn linked_status_badges_keep_compact_geometry_and_link_semantics() {
        let html = render_markdown_with_image_dimensions(
            "[![Build](https://github.com/user/repo/actions/workflows/build.yml/badge.svg)](https://github.com/user/repo/actions)",
            &[],
            &[],
        );
        assert!(html.contains("article-picture article-badge"), "{html}");
        assert!(html.contains("width=\"160\" height=\"24\""), "{html}");
        assert!(html.contains("alt=\"Build\""), "{html}");
        assert!(
            html.contains("href=\"https://github.com/user/repo/actions\""),
            "{html}"
        );
        assert!(!html.contains("article-picture-fallback"), "{html}");
    }

    #[test]
    fn local_article_images_use_lossless_sources_and_an_exact_fallback() {
        let image = LocalImage {
            source: "https://publisher.example/diagram.png".into(),
            original: "assets/images/original.png".into(),
            alt: None,
            variants: vec![
                LocalImageVariant {
                    url: "assets/images/small.webp".into(),
                    width: 320,
                    height: 213,
                },
                LocalImageVariant {
                    url: "assets/images/large.webp".into(),
                    width: 640,
                    height: 427,
                },
                LocalImageVariant {
                    url: "assets/images/full.webp".into(),
                    width: 1200,
                    height: 800,
                },
            ],
            width: 1200,
            height: 800,
            color: "#285a8c".into(),
            placeholder: crate::media::placeholder::from_image(&image::DynamicImage::new_rgb8(
                4, 4,
            ))
            .unwrap(),
        };
        let html = render_markdown_with_images(
            "![Useful diagram](https://publisher.example/diagram.png \"Details\")",
            std::slice::from_ref(&image),
        );
        assert!(
            html.contains("<picture class=\"article-picture\""),
            "{html}"
        );
        assert!(
            html.contains(&format!("data-thumbhash=\"{}\"", image.placeholder.hash)),
            "{html}"
        );
        assert!(
            html.contains(
                "style=\"--image-width:1200px;--image-ratio:1200 / 800;--image-placeholder:#285a8c;--image-preview:url('data:image/png;base64,"
            ),
            "{html}"
        );
        assert!(html.contains("type=\"image/webp\""), "{html}");
        assert!(
            html.contains("<source type=\"image/webp\" width=\"1200\" height=\"800\""),
            "{html}"
        );
        assert!(
            html.contains(
                "small.webp 320w, assets/images/large.webp 640w, assets/images/full.webp 1200w"
            ),
            "{html}"
        );
        assert!(
            html.contains("src=\"assets/images/original.png\""),
            "{html}"
        );
        assert!(html.contains("width=\"1200\" height=\"800\""), "{html}");
        assert!(html.contains("alt=\"Useful diagram\""), "{html}");
        assert!(html.contains("title=\"Details\""), "{html}");
        assert!(html.contains("--image-placeholder:#285a8c"), "{html}");

        let hinted = render_markdown_with_image_dimensions(
            "![Useful diagram](https://publisher.example/diagram.png)",
            std::slice::from_ref(&image),
            &[ImageDimensions {
                source: image.source.clone(),
                width: 400,
                height: 900,
            }],
        );
        assert!(
            hinted.contains("width=\"1200\" height=\"800\""),
            "validated image dimensions override publisher hints: {hinted}"
        );
        assert!(!hinted.contains("--image-ratio:400 / 900"), "{hinted}");

        let without_variants = LocalImage {
            variants: Vec::new(),
            ..image.clone()
        };
        let html = render_markdown_with_images(
            "![Useful diagram](https://publisher.example/diagram.png)",
            &[without_variants],
        );
        assert!(
            html.contains("<picture class=\"article-picture\""),
            "{html}"
        );
        assert!(!html.contains("<source "), "{html}");

        let partial = LocalImage {
            variants: image.variants[..2].to_vec(),
            ..image.clone()
        };
        let partial_html = render_markdown_with_images(
            "![Diagram](https://publisher.example/diagram.png)",
            &[partial],
        );
        assert!(partial_html.contains("srcset=\"assets/images/small.webp 320w, assets/images/large.webp 640w, assets/images/original.png 1200w\""), "large masters retain responsive choices without a full-width WebP: {partial_html}");
        assert!(!partial_html.contains("<source "), "{partial_html}");

        let later = render_markdown_with_images(
            "![Remote](https://publisher.example/first.png)\n\n![Useful diagram](https://publisher.example/diagram.png)",
            &[image],
        );
        assert!(
            later.contains("sizes=\"(max-width: 56rem) calc(100vw - 2rem), 52rem\""),
            "{later}"
        );
        assert!(!later.contains("sizes=\"auto,"), "{later}");
    }

    #[test]
    fn bare_urls_before_hard_breaks_do_not_include_backslashes() {
        let markdown = to_markdown(
            "<p>Watch https://example.com/video<br>Second line.<br>More.</p>",
            None,
        );
        let html = render_markdown(&markdown);
        assert!(
            html.contains("href=\"https://example.com/video\""),
            "{markdown:?}\n{html}"
        );
        assert!(
            html.contains("</a><br />\nSecond line.<br />\nMore."),
            "{html}"
        );
        assert!(!html.contains('\\'), "{html}");
        let unicode = render_markdown("Étude https://example.com/video\\\r\nSecond line.\r\n");
        assert!(
            unicode.contains("href=\"https://example.com/video\""),
            "{unicode}"
        );
        assert!(unicode.contains("</a><br />\nSecond line."), "{unicode}");
    }

    #[test]
    fn markdown_break_repairs_preserve_literal_backslashes_and_code() {
        let markdown = "A literal \\\\ character.\n\n`https://example.com/\\`\n\n```text\n# heading\\\nhttps://example.com/\\\n```\n";
        let html = render_markdown(markdown);
        assert!(html.contains("A literal \\ character."), "{html}");
        assert!(
            html.contains("<code>https://example.com/\\</code>"),
            "{html}"
        );
        let fragment = Html::parse_fragment(&html);
        let code = fragment
            .select(&Selector::parse("pre code").unwrap())
            .next()
            .unwrap();
        assert_eq!(
            code.text().collect::<String>(),
            "# heading\\\nhttps://example.com/\\\n"
        );
    }

    #[test]
    fn literal_markdown_heading_breaks_remain_literal() {
        assert!(
            render_markdown("# Literal\\\nFollowing paragraph.\n")
                .contains("<h1>Literal\\</h1>\n<p>Following paragraph.</p>")
        );
        assert!(
            render_markdown("> # Literal\\\n> Following paragraph.\n")
                .contains("<h1>Literal\\</h1>")
        );
    }

    #[test]
    fn code_labels_respect_explicit_languages_and_plain_fallbacks() {
        for (language, label) in [("JS", "JavaScript"), ("py", "Python"), ("rs", "Rust")] {
            let html = render_markdown(&format!("```{language}\nlet value = 1;\n```\n"));
            assert!(
                html.contains(&format!("data-language=\"{label}\"")),
                "{html}"
            );
            assert!(html.contains("syntax-"), "{html}");
        }
        // A language Sublime has no grammar for still shows the name the publisher used.
        for (language, label) in [("dockerfile", "Dockerfile"), ("zig", "Zig")] {
            let html = render_markdown(&format!("```{language}\nFROM scratch\n```\n"));
            assert!(
                html.contains(&format!("data-language=\"{label}\"")),
                "{html}"
            );
            assert!(!html.contains("syntax-"), "{html}");
        }
        for language in ["text", "plaintext", "evil\"<img/src=x>"] {
            let html = render_markdown(&format!("```{language}\nfn main() {{}}\n```\n"));
            assert!(!html.contains("data-language"), "{html}");
            assert!(!html.contains("syntax-"), "{html}");
            assert!(!html.contains("<img"), "{html}");
        }
    }

    #[test]
    fn unlabelled_code_uses_distinctive_signatures_without_guessing_prose() {
        for (code, label) in [
            ("fn main() {\n    println!(\"hello\");\n}", "Rust"),
            ("def greet(name):\n    return name", "Python"),
            ("#!/usr/bin/env bash\necho hello", "Shell"),
            ("{\"ready\": true, \"count\": 2}", "JSON"),
            ("SELECT title FROM articles WHERE id = 1;", "SQL"),
            ("package main\nfunc main() {}", "Go"),
            ("function greet(name) { return name; }", "JavaScript"),
        ] {
            let html = render_markdown(&format!("```\n{code}\n```\n"));
            assert!(
                html.contains(&format!("data-language=\"{label}\"")),
                "{html}"
            );
            assert!(html.contains("syntax-"), "{html}");
        }
        for code in [
            "the function returns a value",
            "select a book from the shelf",
            "let x = 1",
            "[an example]",
            "{}",
            "42",
            "List:       openbsd-tech\nSubject:    GEFS preview\nFrom:       ori\nDate:       2026-09-15",
        ] {
            let html = render_markdown(&format!("```\n{code}\n```\n"));
            assert!(!html.contains("data-language"), "{html}");
            assert!(!html.contains("syntax-"), "{html}");
        }
    }

    #[test]
    fn code_labels_do_not_change_copied_code_or_unbounded_fallback() {
        let code = "<script>alert(1)</script>\n";
        let html = render_markdown(&format!("```html\n{code}```\n"));
        let fragment = Html::parse_fragment(&html);
        let selected = fragment
            .select(&Selector::parse("pre code").unwrap())
            .next()
            .unwrap();
        assert_eq!(selected.text().collect::<String>(), code);
        assert!(html.contains("data-language=\"HTML\""), "{html}");
        let long = "x".repeat(2_001);
        let html = render_markdown(&format!("```rust\n{long}\n```\n"));
        assert!(!html.contains("syntax-"), "{html}");
        assert!(html.contains("data-language=\"Rust\""), "{html}");
    }

    #[test]
    fn syntax_highlighting_is_static_safe_and_optional() {
        let html = render_markdown("```rust\nlet name = \"<script>\";\n```\n");
        assert!(html.contains("syntax-"), "{html}");
        assert!(!html.contains("<script>"), "{html}");
        let plain = render_markdown("```unknown-language\n<script>\n```\n");
        assert!(!plain.contains("syntax-"), "{plain}");
        assert!(plain.contains("&lt;script&gt;"), "{plain}");
    }

    #[test]
    fn reader_tables_become_focusable_scroll_regions_and_portable_outputs_keep_bare_tables() {
        // The table needs one layout box, or its header and body columns stop lining up; the
        // region around it is what a keyboard user focuses and pans.
        let markdown = "Intro.\n\n| Register | Action |\n| --- | --- |\n| `triangleCMD` | Start rendering. |\n\nBetween.\n\n| A | B |\n| --- | --- |\n| 1 | 2 |\n";
        let prepared = PreparedMarkdown::new(markdown);
        let reader = prepared.reader_html_with_images(&[], &[]);
        assert_eq!(reader.matches(TABLE_SCROLL_OPEN).count(), 2, "{reader}");
        assert_eq!(reader.matches("</table></div>").count(), 2, "{reader}");
        assert_eq!(reader.matches("<table>").count(), 2, "{reader}");
        assert!(
            reader.contains(&format!("{TABLE_SCROLL_OPEN}<table>")),
            "{reader}"
        );
        // Stored bodies, feeds and the text/plain, reStructuredText and JSON representations
        // all derive from the portable rendering, which stays a bare table.
        for portable in [
            prepared.portable_html().to_string(),
            render_markdown(markdown),
        ] {
            assert!(!portable.contains("table-scroll"), "{portable}");
            assert!(!portable.contains("role=\"region\""), "{portable}");
            assert_eq!(portable.matches("<table>").count(), 2, "{portable}");
        }
        assert_eq!(
            prepared.plain_text(),
            html_to_text(prepared.portable_html())
        );
        assert!(!prepared.plain_text().contains("Table"));
        assert_eq!(
            to_markdown(&reader, None),
            to_markdown(prepared.portable_html(), None)
        );
    }

    #[test]
    fn nested_tables_scroll_with_their_parent() {
        let html = "<p>a</p><table><tbody><tr><td><table><tr><td>inner</td></tr></table></td></tr></tbody></table><p>b</p><table class=\"x\"><tr><td>2</td></tr></table><tablet>";
        assert_eq!(
            wrap_scrollable_tables(html),
            format!(
                "<p>a</p>{TABLE_SCROLL_OPEN}<table><tbody><tr><td><table><tr><td>inner</td></tr></table></td></tr></tbody></table></div><p>b</p>{TABLE_SCROLL_OPEN}<table class=\"x\"><tr><td>2</td></tr></table></div><tablet>"
            )
        );
        assert_eq!(wrap_scrollable_tables("<p>no table</p>"), "<p>no table</p>");
        assert_eq!(
            wrap_scrollable_tables("<table><tr><td>open"),
            format!("{TABLE_SCROLL_OPEN}<table><tr><td>open")
        );
    }

    #[test]
    fn render_markdown_supports_gfm_and_scrubs_html() {
        let html = render_markdown("| a | b |\n|---|---|\n| 1 | 2 |\n\n- [x] done\n- [ ] todo\n");
        assert!(html.contains("<table>"), "{html}");
        assert!(html.contains(r#"type="checkbox""#), "{html}");

        let html =
            render_markdown("<script>alert(1)</script>\n\n[x](javascript:alert(1))\n\nVec<T>\n");
        assert!(!html.contains("<script"), "{html}");
        assert!(!html.contains("javascript:"), "{html}");
        assert!(html.contains("Vec&lt;T&gt;"), "{html}");

        let html = render_markdown("[external](https://example.com)");
        assert!(html.contains(r#"target="_blank""#), "{html}");
        assert!(html.contains(r#"rel="noopener noreferrer""#), "{html}");

        let html = render_markdown("Body[^1]\n\n[^1]: Footnote.\n");
        assert!(html.contains("<a href=\"#fn-1\""), "{html}");
        assert!(html.contains("<a href=\"#fnref-1\""), "{html}");
        assert!(!html.contains(r##"target="_blank" rel="noopener noreferrer" href="#"##));
    }

    #[test]
    fn reading_metrics_count_visible_unicode_words_and_round_up() {
        let markdown = "# One two\n\nThree **four** five. `six`\n\n```text\nseven eight\n```\n";
        assert_eq!(reading_metrics(markdown), (8, 1));
        assert_eq!(reading_metrics(""), (0, 0));

        let long = std::iter::repeat_n("word", READING_WORDS_PER_MINUTE + 1)
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(reading_metrics(&long), (READING_WORDS_PER_MINUTE + 1, 2));
        assert_eq!(reading_metrics("你好世界").0, 4);
    }

    #[test]
    fn excerpt_cuts_on_word_boundary() {
        assert_eq!(excerpt("Short **text**.", 100), "Short text.");
        assert_eq!(
            excerpt("The quick brown fox jumps over the lazy dog", 16),
            "The quick brown…"
        );
        assert_eq!(excerpt("Supercalifragilistic", 5), "Super…");
        assert_eq!(
            excerpt("# Head\n\n[link](https://x.y) and `code`\n", 100),
            "Head link and code"
        );
    }
}
