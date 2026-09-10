//! HTML in, safe Markdown/HTML/text out. Three views of the same content:
//! the raw `.html` copy (stripped of active content, capped), the sanitized HTML view, and the
//! Markdown body derived from it. Rendering Markdown back to HTML never emits raw HTML.

use std::collections::HashSet;
use std::sync::OnceLock;

use ammonia::UrlRelative;
use anyhow::{Context, Result, bail};
use chrono::{DateTime, NaiveDate, Utc};
use dom_smoothie::Readability;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use url::Url;

#[path = "content_highlight.rs"]
mod highlight;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedArticle {
    pub html: String,
    pub image: Option<String>,
}

/// Elements whose content is executable, styled, or embedded: dropped whole.
const DROP_ELEMENTS: &[&str] = &["script", "style", "svg", "iframe", "object", "embed"];
/// Raw-text elements: their content ends at the first matching close tag, no nesting.
const RAW_TEXT_ELEMENTS: &[&str] = &["script", "style"];
/// Elements that cannot have content; only the tag itself is dropped.
const VOID_ELEMENTS: &[&str] = &["embed"];
/// Elements that separate words when flattened to text; inline tags do not.
const BLOCK_ELEMENTS: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "br",
    "dd",
    "details",
    "div",
    "dl",
    "dt",
    "figcaption",
    "figure",
    "footer",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hr",
    "li",
    "main",
    "nav",
    "ol",
    "p",
    "pre",
    "section",
    "summary",
    "table",
    "td",
    "th",
    "tr",
    "ul",
];
/// Inline wrappers commonly used as CSS layout children. Readability keeps these elements but not
/// the CSS `gap` or grid columns that visually separated directly adjacent siblings.
const LAYOUT_INLINE_ELEMENTS: &[&str] = &["a", "label", "span", "time"];
const FOOTNOTE_REF_START: char = '\u{e000}';
const FOOTNOTE_REF_END: char = '\u{e001}';
/// Attributes that carry URLs and therefore may smuggle `data:` payloads.
const URL_ATTRIBUTES: &[&str] = &[
    "src",
    "href",
    "srcset",
    "poster",
    "data",
    "action",
    "formaction",
    "background",
    "xlink:href",
];

/// Extract the primary article from a complete origin page. Readability intentionally does not
/// sanitize its output, so callers must still pass this HTML through the normal storage and
/// Markdown safety pipeline.
pub fn extract_article(page: &str, url: &Url) -> Result<ExtractedArticle> {
    let config = dom_smoothie::Config {
        max_elements_to_parse: 100_000,
        ..Default::default()
    };
    let mut readability = Readability::new(page, Some(url.as_str()), Some(config))
        .context("parsing the original article page")?;
    // Figure filenames such as `replies.png` can resemble comment widgets to Readability.
    // Explicit image-and-caption structure supplies stronger evidence than those incidental IDs.
    readability.doc.select(
        "figure:has(img):has(figcaption), div.figure:has(img):has(figcaption, .photoCaption, .caption)",
    ).add_class("readability-content");
    // Keep the semantic article above equally scored figure siblings. Otherwise a score tie
    // can select a figure and discard unscored neighboring figures.
    readability
        .doc
        .select("article:has(figure img), article:has(div.figure img)")
        .add_class("readability-content");
    let article = readability
        .parse()
        .context("extracting readable article content")?;
    let html = article.content.to_string();
    if !has_meaningful_extracted_content(&html, url) {
        bail!("extracted article is empty");
    }
    Ok(ExtractedArticle {
        html,
        image: article.image,
    })
}

fn has_meaningful_extracted_content(html: &str, base: &Url) -> bool {
    if !html_to_text(html).trim().is_empty() {
        return true;
    }
    let normalized = normalize_image_sources(html);
    let passive = strip_active_content(&normalized);
    let clean = sanitize(&passive, Some(base));
    let Ok(images) = Selector::parse("img[src]") else {
        return false;
    };
    Html::parse_fragment(&clean).select(&images).any(|image| {
        image.value().attr("src").is_some_and(|src| {
            Url::parse(src).is_ok_and(|url| {
                matches!(url.scheme(), "http" | "https") && url.host_str().is_some()
            })
        })
    })
}

pub async fn extract_article_async(page: String, url: Url) -> Result<ExtractedArticle> {
    use std::sync::Arc;
    use tokio::sync::Semaphore;
    static LIMIT: OnceLock<Arc<Semaphore>> = OnceLock::new();
    limited_extraction(
        LIMIT.get_or_init(|| Arc::new(Semaphore::new(2))).clone(),
        move || extract_article(&page, &url),
    )
    .await
}

async fn limited_extraction<T: Send + 'static>(
    limit: std::sync::Arc<tokio::sync::Semaphore>,
    operation: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    let permit = limit
        .acquire_owned()
        .await
        .context("waiting for article extraction")?;
    tokio::task::spawn_blocking(move || {
        // Cancellation cannot interrupt CPU work; keep its slot until the closure really exits.
        let _permit = permit;
        operation()
    })
    .await
    .context("article extraction task")?
}

/// HTML prepared for storage as the `.html` sibling: `<script>`, `<style>`, inline `<svg>`,
/// `<iframe>`/`<object>`/`<embed>`, HTML comments removed; `on*` handlers and `data:` /
/// `javascript:` URL attributes removed; then capped at `max_bytes` on a char boundary (cut at
/// the last `>` before the limit when possible). Returns `(html, truncated)`. Everything else is
/// kept verbatim: this is the raw copy, and it is never served unsanitized.
pub fn storage_html(raw: &str, max_bytes: usize) -> (String, bool) {
    cap(strip_active_content(raw), max_bytes)
}

fn strip_active_content(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let Some(lt) = raw[i..].find('<').map(|pos| i + pos) else {
            out.push_str(&raw[i..]);
            break;
        };
        out.push_str(&raw[i..lt]);
        let rest = &raw[lt..];
        if let Some(after) = comment_end(rest) {
            i = lt + after;
            continue;
        }
        let Some(tag) = parse_tag(rest) else {
            out.push('<');
            i = lt + 1;
            continue;
        };
        let Some(end) = tag.end else {
            // Unterminated tag: nothing after it can render, drop the tail.
            i = raw.len();
            continue;
        };
        let after_tag = lt + end;
        if DROP_ELEMENTS.contains(&tag.name.as_str()) {
            i = if tag.closing || tag.self_closing || VOID_ELEMENTS.contains(&tag.name.as_str()) {
                after_tag
            } else {
                skip_element(raw, after_tag, &tag.name)
            };
            continue;
        }
        out.push_str(&without_active_attributes(&rest[..end]));
        i = after_tag;
    }
    out
}

/// Length of the comment starting `s`, if any. An unterminated comment swallows the rest of the
/// document, as in HTML itself.
fn comment_end(s: &str) -> Option<usize> {
    let body = s.strip_prefix("<!--")?;
    Some(body.find("-->").map_or(s.len(), |end| 4 + end + 3))
}

struct Tag {
    name: String,
    closing: bool,
    self_closing: bool,
    /// Byte offset just past `>`, `None` when the input ends inside the tag.
    end: Option<usize>,
}

/// Parse `<name …>` / `</name …>` at the start of `s`. `None` when `<` does not start a tag.
fn parse_tag(s: &str) -> Option<Tag> {
    let bytes = s.as_bytes();
    let mut pos = 1;
    let closing = bytes.get(pos) == Some(&b'/');
    if closing {
        pos += 1;
    }
    let name_start = pos;
    while pos < bytes.len() && is_name_byte(bytes[pos]) {
        pos += 1;
    }
    if pos == name_start || !bytes[name_start].is_ascii_alphabetic() {
        return None;
    }
    let name = s[name_start..pos].to_ascii_lowercase();
    let end = tag_end(s, pos);
    let self_closing = end.is_some_and(|end| s[..end].trim_end_matches('>').ends_with('/'));
    Some(Tag {
        name,
        closing,
        self_closing,
        end,
    })
}

fn is_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b':'
}

/// Offset just past the `>` closing the tag that starts at 0, honouring quoted attribute values.
fn tag_end(s: &str, from: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut pos = from;
    while pos < bytes.len() {
        match bytes[pos] {
            b'>' => return Some(pos + 1),
            quote @ (b'"' | b'\'') => {
                pos += 1;
                while pos < bytes.len() && bytes[pos] != quote {
                    pos += 1;
                }
                pos += 1;
            }
            _ => pos += 1,
        }
    }
    None
}

/// Offset just past the close tag matching an element opened before `from`. Raw-text elements
/// end at the first close tag; others nest.
fn skip_element(raw: &str, from: usize, name: &str) -> usize {
    let raw_text = RAW_TEXT_ELEMENTS.contains(&name);
    let mut depth = 1;
    let mut i = from;
    while i < raw.len() {
        let Some(lt) = raw[i..].find('<').map(|pos| i + pos) else {
            break;
        };
        match parse_tag(&raw[lt..]) {
            Some(tag) if tag.name == name => {
                let after = tag.end.map_or(raw.len(), |end| lt + end);
                if tag.closing {
                    depth -= 1;
                    if depth == 0 {
                        return after;
                    }
                } else if !raw_text && !tag.self_closing {
                    depth += 1;
                }
                i = after;
            }
            _ => i = lt + 1,
        }
    }
    raw.len()
}

/// The tag text without `on*` handlers and without URL attributes carrying `data:` or
/// `javascript:` values.
fn without_active_attributes(tag: &str) -> String {
    let bytes = tag.as_bytes();
    let mut out = String::with_capacity(tag.len());
    let mut kept_until = 0;
    let mut pos = 1;
    // Skip the tag name (and a leading `/`).
    while pos < bytes.len() && (is_name_byte(bytes[pos]) || bytes[pos] == b'/') {
        pos += 1;
    }
    while pos < bytes.len() {
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        let attr_start = pos;
        while pos < bytes.len()
            && !bytes[pos].is_ascii_whitespace()
            && !b"=>/".contains(&bytes[pos])
        {
            pos += 1;
        }
        if pos == attr_start {
            pos += 1;
            continue;
        }
        let name = tag[attr_start..pos].to_ascii_lowercase();
        let mut probe = pos;
        while probe < bytes.len() && bytes[probe].is_ascii_whitespace() {
            probe += 1;
        }
        let mut value = "";
        if bytes.get(probe) == Some(&b'=') {
            probe += 1;
            while probe < bytes.len() && bytes[probe].is_ascii_whitespace() {
                probe += 1;
            }
            let value_start = probe;
            match bytes.get(probe) {
                Some(quote @ (b'"' | b'\'')) => {
                    probe += 1;
                    while probe < bytes.len() && bytes[probe] != *quote {
                        probe += 1;
                    }
                    value = &tag[value_start + 1..probe.min(bytes.len())];
                    probe = (probe + 1).min(bytes.len());
                }
                _ => {
                    while probe < bytes.len()
                        && !bytes[probe].is_ascii_whitespace()
                        && bytes[probe] != b'>'
                    {
                        probe += 1;
                    }
                    value = &tag[value_start..probe];
                }
            }
            pos = probe;
        }
        let active = name.starts_with("on")
            || (URL_ATTRIBUTES.contains(&name.as_str()) && is_active_url(value));
        if active {
            out.push_str(tag[kept_until..attr_start].trim_end());
            kept_until = pos;
        }
    }
    out.push_str(&tag[kept_until..]);
    out
}

/// `data:`, `javascript:` or `vbscript:` anywhere in a (possibly comma-separated srcset) value,
/// ignoring the whitespace and control characters browsers skip before the scheme.
fn is_active_url(value: &str) -> bool {
    let decoded = decode_entities(value);
    let value = decoded.as_str();
    value.split(',').any(|candidate| {
        let scheme: String = candidate
            .chars()
            .filter(|ch| !ch.is_whitespace() && !ch.is_control())
            .take(11)
            .collect::<String>()
            .to_ascii_lowercase();
        ["data:", "javascript:", "vbscript:"]
            .iter()
            .any(|prefix| scheme.starts_with(prefix))
    })
}

fn cap(html: String, max_bytes: usize) -> (String, bool) {
    if html.len() <= max_bytes {
        return (html, false);
    }
    let mut cut = max_bytes;
    while !html.is_char_boundary(cut) {
        cut -= 1;
    }
    // Prefer ending on a complete tag, unless that would throw away most of the budget.
    if let Some(gt) = html[..cut].rfind('>')
        && gt + 1 >= max_bytes / 2
    {
        cut = gt + 1;
    }
    (html[..cut].to_string(), true)
}

/// ammonia-sanitized HTML: default allowlist, `rel="noopener noreferrer"` on links, relative
/// URLs resolved against `base`, only http/https/mailto schemes (so `javascript:` and `data:`
/// are dropped), no event handlers, images lazy and referrer-free.
pub fn sanitize(html: &str, base: Option<&Url>) -> String {
    let url_relative = match base {
        Some(base) => UrlRelative::RewriteWithBase(base.clone()),
        None => UrlRelative::PassThrough,
    };
    let mut builder = ammonia::Builder::default();
    builder
        .url_schemes(HashSet::from(["http", "https", "mailto"]))
        .url_relative(url_relative)
        .link_rel(Some("noopener noreferrer"))
        .set_tag_attribute_value("a", "target", "_blank")
        .set_tag_attribute_value("img", "loading", "lazy")
        .set_tag_attribute_value("img", "decoding", "async")
        .set_tag_attribute_value("img", "referrerpolicy", "no-referrer");
    builder.add_tag_attributes("code", ["class"]);
    builder.clean(html).to_string()
}

#[derive(Default)]
struct PictureSources {
    supported: Option<String>,
    fallback: Option<String>,
}

/// Promote responsive and lazy-loading image candidates into `img[src]` before sanitizing or
/// converting HTML. Publisher CSS and scripts are intentionally discarded, so leaving the real
/// URL only in `srcset`, `data-src*`, or a `<picture><source>` would otherwise lose the image.
pub(crate) fn normalize_image_sources(html: &str) -> String {
    let mut normalized = String::with_capacity(html.len());
    let mut pictures = Vec::<PictureSources>::new();
    let mut position = 0;
    while position < html.len() {
        let Some(start) = html[position..].find('<').map(|offset| position + offset) else {
            normalized.push_str(&html[position..]);
            break;
        };
        normalized.push_str(&html[position..start]);
        if let Some(length) = comment_end(&html[start..]) {
            normalized.push_str(&html[start..start + length]);
            position = start + length;
            continue;
        }
        let Some(tag) = parse_tag(&html[start..]) else {
            normalized.push('<');
            position = start + 1;
            continue;
        };
        let Some(length) = tag.end else {
            normalized.push_str(&html[start..]);
            break;
        };
        let raw = &html[start..start + length];
        match (tag.closing, tag.name.as_str()) {
            (false, "picture") => {
                pictures.push(PictureSources::default());
                normalized.push_str(raw);
            }
            (true, "picture") => {
                pictures.pop();
                normalized.push_str(raw);
            }
            (false, "source") => {
                if let Some(picture) = pictures.last_mut()
                    && let Some(candidate) = image_candidate(raw)
                {
                    picture.fallback.get_or_insert_with(|| candidate.clone());
                    let supported = attribute_value(raw, "type").is_none_or(|kind| {
                        matches!(
                            kind.trim().to_ascii_lowercase().as_str(),
                            "image/jpeg" | "image/png" | "image/gif" | "image/webp"
                        )
                    });
                    if supported {
                        picture.supported.get_or_insert(candidate);
                    }
                }
                normalized.push_str(raw);
            }
            (false, "img") => {
                let own = image_candidate(raw);
                let picture = pictures
                    .last()
                    .and_then(|sources| sources.supported.as_ref().or(sources.fallback.as_ref()))
                    .cloned();
                normalized.push_str(&own.or(picture).map_or_else(
                    || raw.to_string(),
                    |source| set_attribute(raw, "src", &source),
                ));
            }
            _ => normalized.push_str(raw),
        }
        position = start + length;
    }
    normalized
}

fn image_candidate(tag: &str) -> Option<String> {
    ["data-srcset", "data-lazy-srcset", "srcset"]
        .into_iter()
        .filter_map(|attribute| attribute_value(tag, attribute))
        .find_map(best_srcset_candidate)
        .or_else(|| {
            [
                "data-src",
                "data-lazy-src",
                "data-original",
                "data-original-src",
                "data-url",
                "src",
            ]
            .into_iter()
            .filter_map(|attribute| attribute_value(tag, attribute))
            .find_map(safe_image_candidate)
        })
}

fn best_srcset_candidate(srcset: &str) -> Option<String> {
    if is_active_url(srcset) {
        return None;
    }
    crate::media::srcset::candidates(srcset)
        .into_iter()
        .enumerate()
        .filter_map(|(order, candidate)| {
            let url = safe_image_candidate(candidate.url)?;
            Some((candidate.score, order, url))
        })
        .max_by(|left, right| {
            left.0
                .total_cmp(&right.0)
                .then_with(|| left.1.cmp(&right.1))
        })
        .map(|(_, _, url)| url)
}

fn safe_image_candidate(value: &str) -> Option<String> {
    let value = decode_entities(value);
    let value = value.trim();
    (!value.is_empty() && !is_active_url(value)).then(|| value.to_string())
}

fn set_attribute(tag: &str, name: &str, value: &str) -> String {
    let value = escape_html(value);
    if let Some(range) = attribute_value_range(tag, name) {
        return format!("{}{}{}", &tag[..range.start], value, &tag[range.end..]);
    }
    let Some(end) = tag.rfind('>') else {
        return tag.to_string();
    };
    let mut insertion = end;
    while tag.as_bytes()[..insertion]
        .last()
        .is_some_and(u8::is_ascii_whitespace)
    {
        insertion -= 1;
    }
    if tag.as_bytes().get(insertion.wrapping_sub(1)) == Some(&b'/') {
        insertion -= 1;
    }
    format!(
        "{} {}=\"{}\"{}",
        &tag[..insertion],
        name,
        value,
        &tag[insertion..]
    )
}

/// [`sanitize`] then htmd. Trailing whitespace trimmed, exactly one trailing newline, runs of
/// blank lines collapsed to one.
pub fn to_markdown(html: &str, base: Option<&Url>) -> String {
    let description = normalize_youtube_description(html, base);
    let normalized_images = normalize_image_sources(&description);
    let passive = strip_active_content(&normalized_images);
    let normalized = normalize_extracted_controls(&normalize_code_blocks(&passive));
    let clean = sanitize(&restore_inline_layout_boundaries(&normalized.html), base);
    let converter = htmd::HtmlToMarkdown::builder()
        .options(htmd::options::Options {
            bullet_list_marker: htmd::options::BulletListMarker::Dash,
            br_style: htmd::options::BrStyle::Backslash,
            ul_bullet_spacing: 1,
            ol_number_spacing: 1,
            ..Default::default()
        })
        .add_handler(vec!["h1", "h2", "h3", "h4", "h5", "h6"], markdown_heading)
        .build();
    let markdown = converter
        .convert(&clean)
        .unwrap_or_else(|_| html_to_text(&clean));
    let markdown = protect_markdown_code(&markdown, |prose| {
        tidy_markdown(&repair_generated_markdown(prose))
    });
    let markdown = restore_footnote_references(markdown, normalized.footnotes.len());
    append_footnotes(markdown, &normalized.footnotes, base, &converter)
}

/// YouTube descriptions are plain text carried as paragraphs with line breaks, not Markdown.
/// Recognize only explicit dash lists, leaving rich HTML and code alone.
fn normalize_youtube_description(html: &str, base: Option<&Url>) -> String {
    let Some(id) = base.and_then(crate::sources::youtube::video_id) else {
        return html.to_string();
    };
    let mut out = String::with_capacity(html.len());
    let mut position = 0;
    while let Some(start) = html[position..].find('<').map(|offset| position + offset) {
        out.push_str(&html[position..start]);
        if let Some(tag) = parse_tag(&html[start..])
            && !tag.closing
            && tag.name == "p"
            && let Some((_, _, end)) = element_bounds(html, start, "p")
        {
            let fragment = Html::parse_fragment(&html[start..end]);
            let plain = fragment
                .root_element()
                .descendants()
                .all(|node| match node.value() {
                    scraper::Node::Element(element) => {
                        matches!(element.name(), "html" | "p" | "br")
                    }
                    _ => true,
                });
            let mut text = String::new();
            for node in fragment.root_element().descendants() {
                match node.value() {
                    scraper::Node::Text(value) => text.push_str(value),
                    scraper::Node::Element(element) if element.name() == "br" => text.push('\n'),
                    _ => {}
                }
            }
            if plain && text.lines().any(|line| description_bullet(line).is_some()) {
                out.push_str(&description_blocks(&text, &id));
            } else {
                out.push_str(&html[start..end]);
            }
            position = end;
        } else {
            out.push('<');
            position = start + 1;
        }
    }
    out.push_str(&html[position..]);
    out
}

fn description_bullet(line: &str) -> Option<&str> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    trimmed
        .strip_prefix("- ")
        .map(str::trim)
        .filter(|line| !line.is_empty())
}

fn description_blocks(text: &str, video_id: &str) -> String {
    let lines = text.lines().collect::<Vec<_>>();
    let mut out = String::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index].trim();
        if line.is_empty() {
            index += 1;
            continue;
        }
        if description_bullet(lines[index]).is_some() {
            out.push_str("<ul>");
            while let Some(bullet) = lines.get(index).and_then(|line| description_bullet(line)) {
                out.push_str("<li>");
                out.push_str(&description_chapter(bullet, video_id));
                out.push_str("</li>");
                index += 1;
            }
            out.push_str("</ul>");
        } else if line.ends_with(':')
            && line.len() <= 80
            && lines
                .get(index + 1)
                .is_some_and(|line| description_bullet(line).is_some())
        {
            out.push_str(&format!(
                "<h3>{}</h3>",
                escape_html(line.trim_end_matches(':'))
            ));
            index += 1;
        } else {
            out.push_str("<p>");
            out.push_str(&escape_html(lines[index]));
            index += 1;
            while let Some(line) = lines.get(index)
                && !line.trim().is_empty()
                && description_bullet(line).is_none()
            {
                out.push_str("<br>");
                out.push_str(&escape_html(line));
                index += 1;
            }
            out.push_str("</p>");
        }
    }
    out
}

fn description_chapter(text: &str, video_id: &str) -> String {
    let Some((timestamp, rest)) = text.split_once(char::is_whitespace) else {
        return escape_html(text);
    };
    let parts = timestamp.split(':').collect::<Vec<_>>();
    if !(2..=3).contains(&parts.len())
        || parts.iter().any(|part| {
            part.is_empty() || part.len() > 3 || !part.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return escape_html(text);
    }
    let values = parts
        .iter()
        .filter_map(|part| part.parse::<u64>().ok())
        .collect::<Vec<_>>();
    if values.len() != parts.len() || values[1..].iter().any(|value| *value >= 60) {
        return escape_html(text);
    }
    let seconds = values.into_iter().fold(0, |total, part| total * 60 + part);
    format!(
        "<a href=\"https://www.youtube.com/watch?v={video_id}&amp;t={seconds}\">{timestamp}</a> {}",
        escape_html(rest)
    )
}

fn markdown_heading(
    handlers: &dyn htmd::element_handler::Handlers,
    element: htmd::Element<'_>,
) -> Option<htmd::element_handler::HandlerResult> {
    let level = element.tag.strip_prefix('h')?.parse::<usize>().ok()?;
    let content = handlers.walk_children(element.node).content;
    let content = content.trim();
    let heading = if level <= 2 && content.contains("\\\n") {
        let underline = if level == 1 { "===" } else { "---" };
        format!("{content}\n{underline}")
    } else {
        // Setext supports multiline h1/h2; higher levels must stay on one ATX line.
        format!("{} {}", "#".repeat(level), content.replace("\\\n", " "))
    };
    Some(format!("\n\n{heading}\n\n").into())
}

fn code_language(element: scraper::ElementRef<'_>) -> Option<String> {
    let value = element.value();
    let language = value
        .attr("data-lang")
        .or_else(|| value.attr("data-language"))
        .or_else(|| {
            value.attr("class").and_then(|classes| {
                classes.split_whitespace().find_map(|class| {
                    class
                        .strip_prefix("language-")
                        .or_else(|| class.strip_prefix("lang-"))
                        .or_else(|| class.strip_prefix("highlight-source-"))
                })
            })
        })?;
    (language.len() <= 40
        && !language.is_empty()
        && language
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || b"_+-#".contains(&ch)))
    .then(|| language.to_ascii_lowercase())
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Flatten highlighting wrappers before htmd can trim the line endings inside their spans.
fn normalize_code_blocks(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut position = 0;
    while let Some(start) = html[position..].find('<').map(|offset| position + offset) {
        out.push_str(&html[position..start]);
        if let Some(tag) = parse_tag(&html[start..])
            && !tag.closing
            && tag.name == "pre"
            && let Some((_, _, end)) = element_bounds(html, start, "pre")
        {
            let fragment = Html::parse_fragment(&html[start..end]);
            let language = Selector::parse("code,pre")
                .ok()
                .and_then(|selector| fragment.select(&selector).find_map(code_language));
            let mut code = String::new();
            for node in fragment.tree.nodes() {
                match node.value() {
                    scraper::Node::Text(text) => code.push_str(text),
                    scraper::Node::Element(element) if element.name() == "br" => code.push('\n'),
                    _ => {}
                }
            }
            out.push_str("<pre><code");
            if let Some(language) = language {
                out.push_str(&format!(" class=\"language-{language}\""));
            }
            out.push('>');
            out.push_str(&escape_html(&code));
            out.push_str("</code></pre>");
            position = end;
        } else {
            out.push('<');
            position = start + 1;
        }
    }
    out.push_str(&html[position..]);
    out
}

fn markdown_code_ranges(markdown: &str) -> Vec<std::ops::Range<usize>> {
    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, markdown, &comrak::Options::default());
    let mut lines = vec![0];
    lines.extend(markdown.match_indices('\n').map(|(index, _)| index + 1));
    root.descendants()
        .filter_map(|node| {
            let data = node.data.borrow();
            if !matches!(
                data.value,
                comrak::nodes::NodeValue::Code(_) | comrak::nodes::NodeValue::CodeBlock(_)
            ) {
                return None;
            }
            let start = *lines.get(data.sourcepos.start.line.checked_sub(1)?)?
                + data.sourcepos.start.column.saturating_sub(1);
            let end =
                *lines.get(data.sourcepos.end.line.checked_sub(1)?)? + data.sourcepos.end.column;
            (start <= end
                && end <= markdown.len()
                && markdown.is_char_boundary(start)
                && markdown.is_char_boundary(end))
            .then_some(start..end)
        })
        .collect()
}

fn protect_markdown_code(markdown: &str, transform: impl FnOnce(&str) -> String) -> String {
    let ranges = markdown_code_ranges(markdown);
    let mut protected = markdown.to_string();
    for (index, range) in ranges.iter().enumerate().rev() {
        protected.replace_range(range.clone(), &format!("\u{e010}{index}\u{e011}"));
    }
    let mut transformed = transform(&protected);
    for (index, range) in ranges.iter().enumerate() {
        transformed = transformed.replace(
            &format!("\u{e010}{index}\u{e011}"),
            &markdown[range.clone()],
        );
    }
    transformed
}

struct NormalizedHtml {
    html: String,
    footnotes: Vec<String>,
}

/// Turn presentation-only controls retained by Readability into durable document semantics.
/// Sidenotes become ordinary Markdown footnotes later in the pipeline; expand/collapse controls
/// are discarded because Readability has already retained their complete content.
fn normalize_extracted_controls(html: &str) -> NormalizedHtml {
    let mut normalized = String::with_capacity(html.len());
    let mut footnotes = Vec::new();
    let mut suppressed_spans = Vec::new();
    let mut position = 0;

    while position < html.len() {
        let Some(tag_start) = html[position..].find('<').map(|offset| position + offset) else {
            normalized.push_str(&html[position..]);
            break;
        };
        normalized.push_str(&html[position..tag_start]);
        if let Some(index) = suppressed_spans
            .iter()
            .position(|(start, _)| *start == tag_start)
        {
            let (_, end) = suppressed_spans.swap_remove(index);
            position = end;
            continue;
        }

        let Some(tag) = parse_tag(&html[tag_start..]) else {
            normalized.push('<');
            position = tag_start + 1;
            continue;
        };
        let Some(tag_len) = tag.end else {
            normalized.push_str(&html[tag_start..]);
            break;
        };
        let tag_end = tag_start + tag_len;
        let tag_html = &html[tag_start..tag_end];

        if !tag.closing
            && matches!(tag.name.as_str(), "pre" | "code")
            && let Some((_, _, end)) = element_bounds(html, tag_start, &tag.name)
        {
            normalized.push_str(&html[tag_start..end]);
            position = end;
            continue;
        }

        if !tag.closing
            && tag.name == "span"
            && let Some(length) = html[tag_end..].find('<').filter(|length| *length <= 128)
            && let Some(closing) = parse_tag(&html[tag_end + length..])
            && closing.closing
            && closing.name == "span"
            && let Some(closing_length) = closing.end
            && is_accessibility_label(&decode_entities(&html[tag_end..tag_end + length]))
        {
            position = tag_end + length + closing_length;
            continue;
        }

        if !tag.closing && tag.name == "label" {
            if let Some(sidenote) = sidenote_at(html, tag_start)
                && !suppressed_spans
                    .iter()
                    .any(|(start, _)| *start == sidenote.span_start)
            {
                footnotes.push(sidenote.content.to_string());
                suppressed_spans.push((sidenote.span_start, sidenote.span_end));
                normalized.push(FOOTNOTE_REF_START);
                normalized.push_str(&footnotes.len().to_string());
                normalized.push(FOOTNOTE_REF_END);
                position = sidenote.reference_end;
                continue;
            }
            if let Some((_, inner_end, end)) = element_bounds(html, tag_start, "label")
                && is_expand_control(tag_html, &html[tag_end..inner_end])
            {
                position = end;
                continue;
            }
        }

        if !tag.closing && tag.name == "input" && is_hidden_expand_input(tag_html) {
            position = tag_end;
            continue;
        }

        normalized.push_str(tag_html);
        position = tag_end;
    }

    NormalizedHtml {
        html: normalized,
        footnotes,
    }
}

fn restore_footnote_references(mut markdown: String, count: usize) -> String {
    for number in 1..=count {
        markdown = markdown.replace(
            &format!("{FOOTNOTE_REF_START}{number}{FOOTNOTE_REF_END}"),
            &format!("[^{number}]"),
        );
    }
    markdown
}

struct SidenoteMatch<'a> {
    reference_end: usize,
    span_start: usize,
    span_end: usize,
    content: &'a str,
}

/// Pair a numbered sidenote label with its matching span in the same containing block. The span
/// may follow intervening main prose because CSS can move it into the margin independently of its
/// source position.
fn sidenote_at(html: &str, label_start: usize) -> Option<SidenoteMatch<'_>> {
    let label = parse_tag(&html[label_start..])?;
    let label_end = label_start + label.end?;
    let label_html = &html[label_start..label_end];
    let target = attribute_value(label_html, "for")?;
    let number = attribute_value(label_html, "data-n");
    if !target.starts_with("fn-") && !has_class(label_html, "sidenote-number") {
        return None;
    }

    let (_, label_inner_end, after_label) = element_bounds(html, label_start, "label")?;
    if !html_to_text(&html[label_end..label_inner_end])
        .trim()
        .is_empty()
    {
        return None;
    }

    let mut reference_end = after_label;
    let mut next = skip_html_whitespace(html, reference_end);
    if html[next..].starts_with('<')
        && let Some(input) = parse_tag(&html[next..])
        && !input.closing
        && input.name == "input"
    {
        let input_end = next + input.end?;
        let input_html = &html[next..input_end];
        let belongs_to_note = attribute_value(input_html, "id") == Some(target)
            || has_class(input_html, "margin-toggle");
        if !belongs_to_note {
            return None;
        }
        reference_end = input_end;
        next = skip_html_whitespace(html, input_end);
    }

    let (span_start, content_start, content_end, span_end) =
        find_sidenote_span(html, next, number)?;
    Some(SidenoteMatch {
        reference_end,
        span_start,
        span_end,
        content: &html[content_start..content_end],
    })
}

fn find_sidenote_span(
    html: &str,
    mut position: usize,
    number: Option<&str>,
) -> Option<(usize, usize, usize, usize)> {
    while let Some(tag_start) = html[position..].find('<').map(|offset| position + offset) {
        let tag = parse_tag(&html[tag_start..])?;
        let tag_end = tag_start + tag.end?;
        let tag_html = &html[tag_start..tag_end];
        if !tag.closing && tag.name == "span" {
            let matches = match number {
                Some(number) => attribute_value(tag_html, "data-n") == Some(number),
                None => has_class(tag_html, "sidenote"),
            };
            if matches {
                let (content_start, content_end, span_end) =
                    element_bounds(html, tag_start, "span")?;
                return Some((tag_start, content_start, content_end, span_end));
            }
        }
        if BLOCK_ELEMENTS.contains(&tag.name.as_str()) && !matches!(tag.name.as_str(), "br" | "hr")
        {
            return None;
        }
        position = tag_end;
    }
    None
}

fn is_expand_control(tag: &str, inner: &str) -> bool {
    if has_class(tag, "ex-more") {
        return true;
    }
    attribute_value(tag, "for").is_some_and(|target| target.ends_with("-more"))
        && opening_element_count(inner, "span") >= 2
}

fn is_hidden_expand_input(tag: &str) -> bool {
    has_class(tag, "ex-toggle")
        || (attribute_value(tag, "type")
            .is_some_and(|value| value.eq_ignore_ascii_case("checkbox"))
            && attribute_value(tag, "id").is_some_and(|id| id.ends_with("-more")))
}

fn opening_element_count(html: &str, name: &str) -> usize {
    let mut count = 0;
    let mut position = 0;
    while let Some(tag_start) = html[position..].find('<').map(|offset| position + offset) {
        let Some(tag) = parse_tag(&html[tag_start..]) else {
            position = tag_start + 1;
            continue;
        };
        let Some(tag_len) = tag.end else {
            break;
        };
        if !tag.closing && tag.name == name {
            count += 1;
        }
        position = tag_start + tag_len;
    }
    count
}

/// `(opening tag end, closing tag start, closing tag end)` for an element, accounting for nested
/// elements with the same name.
fn element_bounds(html: &str, start: usize, name: &str) -> Option<(usize, usize, usize)> {
    let opening = parse_tag(&html[start..])?;
    if opening.closing || opening.self_closing || opening.name != name {
        return None;
    }
    let opening_end = start + opening.end?;
    let mut depth = 1;
    let mut position = opening_end;

    while let Some(tag_start) = html[position..].find('<').map(|offset| position + offset) {
        let Some(tag) = parse_tag(&html[tag_start..]) else {
            position = tag_start + 1;
            continue;
        };
        let tag_end = tag_start + tag.end?;
        if tag.name == name {
            if tag.closing {
                depth -= 1;
                if depth == 0 {
                    return Some((opening_end, tag_start, tag_end));
                }
            } else if !tag.self_closing {
                depth += 1;
            }
        }
        position = tag_end;
    }
    None
}

fn skip_html_whitespace(html: &str, mut position: usize) -> usize {
    while html
        .as_bytes()
        .get(position)
        .is_some_and(u8::is_ascii_whitespace)
    {
        position += 1;
    }
    position
}

fn attribute_value<'a>(tag: &'a str, wanted: &str) -> Option<&'a str> {
    attribute_value_range(tag, wanted).map(|range| &tag[range])
}

fn attribute_value_range(tag: &str, wanted: &str) -> Option<std::ops::Range<usize>> {
    let bytes = tag.as_bytes();
    let mut position = 1;
    if bytes.get(position) == Some(&b'/') {
        position += 1;
    }
    while position < bytes.len() && is_name_byte(bytes[position]) {
        position += 1;
    }

    while position < bytes.len() {
        while position < bytes.len()
            && (bytes[position].is_ascii_whitespace() || bytes[position] == b'/')
        {
            position += 1;
        }
        if bytes.get(position) == Some(&b'>') {
            break;
        }
        let name_start = position;
        while position < bytes.len()
            && !bytes[position].is_ascii_whitespace()
            && !b"=>/".contains(&bytes[position])
        {
            position += 1;
        }
        if name_start == position {
            position += 1;
            continue;
        }
        let name = &tag[name_start..position];
        while position < bytes.len() && bytes[position].is_ascii_whitespace() {
            position += 1;
        }
        if bytes.get(position) != Some(&b'=') {
            continue;
        }
        position += 1;
        while position < bytes.len() && bytes[position].is_ascii_whitespace() {
            position += 1;
        }
        let (value_start, value_end) = match bytes.get(position) {
            Some(quote @ (b'"' | b'\'')) => {
                position += 1;
                let start = position;
                while position < bytes.len() && bytes[position] != *quote {
                    position += 1;
                }
                let end = position;
                position = (position + 1).min(bytes.len());
                (start, end)
            }
            _ => {
                let start = position;
                while position < bytes.len()
                    && !bytes[position].is_ascii_whitespace()
                    && bytes[position] != b'>'
                {
                    position += 1;
                }
                (start, position)
            }
        };
        if name.eq_ignore_ascii_case(wanted) {
            return Some(value_start..value_end);
        }
    }
    None
}

fn has_class(tag: &str, class: &str) -> bool {
    attribute_value(tag, "class").is_some_and(|classes| {
        classes
            .split_ascii_whitespace()
            .any(|candidate| candidate == class)
    })
}

fn append_footnotes(
    mut markdown: String,
    footnotes: &[String],
    base: Option<&Url>,
    converter: &htmd::HtmlToMarkdown,
) -> String {
    if footnotes.is_empty() {
        return markdown;
    }
    markdown = markdown.trim_end().to_string();

    for (index, footnote) in footnotes.iter().enumerate() {
        markdown.push_str("\n\n");
        let clean = sanitize(
            &restore_inline_layout_boundaries(&normalize_code_blocks(footnote)),
            base,
        );
        let converted = converter
            .convert(&clean)
            .unwrap_or_else(|_| html_to_text(&clean));
        let converted = protect_markdown_code(&converted, |prose| {
            tidy_markdown(&repair_generated_markdown(prose))
        });
        let mut lines = converted.trim_end().lines();
        markdown.push_str(&format!("[^{}]:", index + 1));
        if let Some(first) = lines.next() {
            markdown.push(' ');
            markdown.push_str(first);
        }
        for line in lines {
            markdown.push('\n');
            if !line.is_empty() {
                markdown.push_str("    ");
                markdown.push_str(line);
            }
        }
    }
    markdown.push('\n');
    markdown
}

/// Restore separators that were supplied by the origin page's CSS rather than its text nodes.
/// Directly adjacent links are usually action/button rows, so retain their visual separation with
/// a line break. Other adjacent layout wrappers need a space to avoid merging dates, labels, and
/// footnote text into neighboring words after the wrappers are discarded by the converter.
fn restore_inline_layout_boundaries(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut position = 0;

    while position < html.len() {
        let Some(tag_start) = html[position..].find('<').map(|offset| position + offset) else {
            out.push_str(&html[position..]);
            break;
        };
        out.push_str(&html[position..tag_start]);

        let Some(tag) = parse_tag(&html[tag_start..]) else {
            out.push('<');
            position = tag_start + 1;
            continue;
        };
        let Some(tag_len) = tag.end else {
            out.push_str(&html[tag_start..]);
            break;
        };
        let after_tag = tag_start + tag_len;
        out.push_str(&html[tag_start..after_tag]);
        position = after_tag;

        if !tag.closing || !LAYOUT_INLINE_ELEMENTS.contains(&tag.name.as_str()) {
            continue;
        }
        let Some(next) = parse_tag(&html[position..]) else {
            continue;
        };
        if next.closing || !LAYOUT_INLINE_ELEMENTS.contains(&next.name.as_str()) {
            continue;
        }
        let Some(next_len) = next.end else {
            continue;
        };
        let next_content = position + next_len;
        let boundary_already_spaced = html[..tag_start]
            .chars()
            .next_back()
            .is_some_and(char::is_whitespace)
            || html[next_content..]
                .chars()
                .next()
                .is_some_and(char::is_whitespace);
        if boundary_already_spaced {
            continue;
        }
        if tag.name == "a" && next.name == "a" {
            out.push_str("<br>");
        } else {
            out.push(' ');
        }
    }

    out
}

/// Shared cleanup for every feed and extracted article, both on storage and when building older
/// archives. Restrict standalone controls to document boundaries, never code or body paragraphs.
pub fn strip_article_metadata(
    markdown: &str,
    published: Option<DateTime<Utc>>,
    source_slug: &str,
) -> String {
    let markdown = strip_boundary_controls(markdown);
    let markdown = strip_leading_metadata(&markdown, published, source_slug);
    strip_boundary_controls(&markdown)
}

fn is_accessibility_label(text: &str) -> bool {
    let text = text.replace('\u{2060}', "").to_ascii_lowercase();
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = text
        .strip_prefix('(')
        .and_then(|text| text.strip_suffix(')'))
        .unwrap_or(&text)
        .trim();
    matches!(
        text,
        "opens in new window" | "opens in a new window" | "opens in new tab" | "opens in a new tab"
    )
}

fn strip_boundary_controls(markdown: &str) -> String {
    let trimmed = markdown.trim();
    let first = trimmed
        .split_once("\n\n")
        .map_or(trimmed, |(first, _)| first);
    let last = trimmed
        .rsplit_once("\n\n")
        .map_or(trimmed, |(_, last)| last);
    if ![first, last].iter().any(|text| {
        let text = text.to_ascii_lowercase();
        text.contains("comment") || text.contains("opens in") || text.contains("advertisement")
    }) {
        return markdown.to_string();
    }
    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, markdown, &comrak::Options::default());
    let blocks = root.children().collect::<Vec<_>>();
    if blocks.is_empty() {
        return markdown.to_string();
    }
    let mut leading = 0;
    while leading < blocks.len() {
        let node = blocks[leading];
        let advertisement = boundary_paragraph_text(node)
            .is_some_and(|(text, _)| matches!(text.as_str(), "advertisement" | "advertisement •"));
        let ad_separator = leading > 0
            && boundary_paragraph_text(blocks[leading - 1])
                .is_some_and(|(text, _)| text == "advertisement")
            && boundary_paragraph_text(node).is_some_and(|(text, _)| text == "•");
        if !is_boundary_control(node) && !advertisement && !ad_separator {
            break;
        }
        leading += 1;
    }
    if leading == blocks.len() {
        return String::new();
    }
    let trailing = blocks
        .iter()
        .rev()
        .take_while(|node| is_boundary_control(node))
        .count();
    if leading == 0 && trailing == 0 {
        return markdown.to_string();
    }
    let mut lines = vec![0];
    lines.extend(markdown.match_indices('\n').map(|(index, _)| index + 1));
    let start = if leading > 0 {
        let end = blocks[leading - 1].data.borrow().sourcepos.end.line;
        lines.get(end).copied().unwrap_or(markdown.len())
    } else {
        0
    };
    let end = if trailing > 0 {
        let start = blocks[blocks.len() - trailing]
            .data
            .borrow()
            .sourcepos
            .start
            .line;
        lines
            .get(start.saturating_sub(1))
            .copied()
            .unwrap_or(markdown.len())
    } else {
        markdown.len()
    };
    let kept = &markdown[start..end];
    let kept = if leading > 0 {
        kept.trim_start_matches('\n')
    } else {
        kept
    };
    if trailing > 0 {
        format!("{}\n", kept.trim_end_matches('\n'))
    } else {
        kept.to_string()
    }
}

fn boundary_paragraph_text<'a>(node: &'a comrak::nodes::AstNode<'a>) -> Option<(String, bool)> {
    use comrak::nodes::NodeValue;
    if !matches!(node.data.borrow().value, NodeValue::Paragraph) {
        return None;
    }
    let mut text = String::new();
    let mut linked = false;
    for child in node.descendants() {
        match &child.data.borrow().value {
            NodeValue::Text(value) => text.push_str(value),
            NodeValue::SoftBreak | NodeValue::LineBreak => text.push(' '),
            NodeValue::Link(_) => linked = true,
            NodeValue::Paragraph | NodeValue::Emph | NodeValue::Strong => {}
            _ => return None,
        }
    }
    let text = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    Some((text, linked))
}

fn is_boundary_control<'a>(node: &'a comrak::nodes::AstNode<'a>) -> bool {
    let Some((text, linked)) = boundary_paragraph_text(node) else {
        return false;
    };
    let text = text.trim();
    if is_accessibility_label(text) {
        return true;
    }
    let text = text
        .strip_prefix('[')
        .and_then(|text| text.strip_suffix(']'))
        .unwrap_or(text)
        .trim();
    let text = if linked {
        text.strip_suffix("(opens in a new window)")
            .unwrap_or(text)
            .trim()
    } else {
        text
    };
    let count = text
        .strip_suffix(" comments")
        .or_else(|| text.strip_suffix(" comment"))
        .or_else(|| {
            text.strip_prefix("comments (")
                .and_then(|text| text.strip_suffix(')'))
        });
    if count
        .is_some_and(|count| !count.is_empty() && count.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return true;
    }
    linked && matches!(text, "no comments" | "leave a comment")
}

/// Remove a metadata line that readability promoted to the first Markdown paragraph. This covers
/// a publication date (including a short suffix such as `- Link Blog`) and a source-name-only
/// accessibility label. Compact name/date bylines require an exact publication-date match.
/// A standalone pipe after the date is its orphaned metadata separator.
/// Normal prose containing a date remains intact.
pub fn strip_leading_metadata(
    markdown: &str,
    published: Option<DateTime<Utc>>,
    source_slug: &str,
) -> String {
    let Some((first, rest)) = markdown.split_once("\n\n") else {
        return markdown.to_string();
    };
    if first.lines().count() != 1 {
        return markdown.to_string();
    }
    let plain = html_to_text(&render_markdown(first));
    let source_only = plain.chars().count() <= 80 && slug::slugify(plain.trim()) == source_slug;
    let matching_date = published.is_some_and(|published| {
        date_prefixes(&plain).any(|value| {
            parse_date_only(value).is_some_and(|candidate| {
                candidate
                    .signed_duration_since(published.date_naive())
                    .num_days()
                    .unsigned_abs()
                    <= 1
            })
        }) || matching_leading_byline(first, &plain, published.date_naive())
    });
    if !source_only && !matching_date {
        return markdown.to_string();
    }
    let rest = rest.trim_start_matches('\n');
    if matching_date {
        return rest.strip_prefix("|\n\n").unwrap_or(rest).to_string();
    }
    rest.to_string()
}

fn matching_leading_byline(markdown: &str, plain: &str, published: NaiveDate) -> bool {
    let Some((author, date)) = plain
        .trim()
        .strip_prefix("By ")
        .and_then(|byline| byline.rsplit_once(' '))
    else {
        return false;
    };
    let names = author.split_whitespace().collect::<Vec<_>>();
    if !(2..=4).contains(&names.len())
        || author.chars().count() > 80
        || !names.iter().all(|name| {
            name.chars().next().is_some_and(char::is_uppercase)
                && name
                    .chars()
                    .all(|ch| ch.is_alphabetic() || matches!(ch, '\'' | '’' | '-' | '.'))
        })
        || !["%m.%d.%y", "%d.%m.%y"].iter().any(|format| {
            NaiveDate::parse_from_str(date, format).is_ok_and(|date| date == published)
        })
    {
        return false;
    }
    use comrak::nodes::NodeValue;
    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, markdown, &comrak::Options::default());
    let Some(paragraph) = root.first_child() else {
        return false;
    };
    paragraph.next_sibling().is_none()
        && matches!(paragraph.data.borrow().value, NodeValue::Paragraph)
        && paragraph.descendants().all(|node| {
            matches!(
                node.data.borrow().value,
                NodeValue::Paragraph
                    | NodeValue::Text(_)
                    | NodeValue::Emph
                    | NodeValue::Strong
                    | NodeValue::Link(_)
            )
        })
}

fn date_prefixes(value: &str) -> impl Iterator<Item = &str> {
    std::iter::once(value.trim()).chain(
        [" - ", " | ", " — ", " – "]
            .into_iter()
            .filter_map(|separator| value.split_once(separator).map(|(prefix, _)| prefix.trim())),
    )
}

/// Fix conversion artefacts caused by accessibility labels and leading whitespace inside links.
/// htmd intentionally trims link labels, which can otherwise turn `than <a> 54,000…</a>` into
/// `than[54,000…](…)`.
fn repair_generated_markdown(markdown: &str) -> String {
    let mut value = markdown.to_string();
    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, &value, &comrak::Options::default());
    let mut lines = vec![0];
    lines.extend(value.match_indices('\n').map(|(index, _)| index + 1));
    let insertions = root
        .descendants()
        .filter_map(|node| {
            let data = node.data.borrow();
            if !matches!(data.value, comrak::nodes::NodeValue::Link(_)) {
                return None;
            }
            let index = *lines.get(data.sourcepos.start.line.checked_sub(1)?)?
                + data.sourcepos.start.column.saturating_sub(1);
            (index > 0
                && value.as_bytes().get(index) == Some(&b'[')
                && value
                    .as_bytes()
                    .get(index - 1)
                    .is_some_and(u8::is_ascii_alphanumeric))
            .then_some(index)
        })
        .collect::<Vec<_>>();
    for index in insertions.into_iter().rev() {
        value.insert(index, ' ');
    }
    value
}

fn parse_date_only(raw: &str) -> Option<NaiveDate> {
    let value = raw
        .trim()
        .trim_matches(['*', '_'])
        .strip_prefix("Published on ")
        .or_else(|| raw.trim().strip_prefix("Posted on "))
        .unwrap_or(raw.trim());
    let value = without_ordinal_suffixes(value);
    ["%Y-%m-%d", "%d %B %Y", "%d %b %Y", "%B %d, %Y", "%b %d, %Y"]
        .iter()
        .find_map(|format| NaiveDate::parse_from_str(value.trim(), format).ok())
}

fn without_ordinal_suffixes(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut index = 0;
    while index < bytes.len() {
        if index > 0
            && bytes[index - 1].is_ascii_digit()
            && matches!(
                bytes.get(index..index + 2),
                Some(b"st" | b"nd" | b"rd" | b"th")
            )
            && bytes
                .get(index + 2)
                .is_none_or(|next| !next.is_ascii_alphabetic())
        {
            index += 2;
            continue;
        }
        let ch = value[index..]
            .chars()
            .next()
            .expect("valid character boundary");
        out.push(ch);
        index += ch.len_utf8();
    }
    out
}

fn tidy_markdown(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut blank_run = 0;
    for line in markdown.lines().map(str::trim_end) {
        if line.is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    let trimmed = out.trim().to_string();
    if trimmed.is_empty() {
        trimmed
    } else {
        trimmed + "\n"
    }
}

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

    fn reader_html(&self) -> &str {
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
        if self.resource_range.is_none() {
            self.with_images(images, dimensions)
        } else {
            enhance_rendered_images(self.reader_html(), images, dimensions)
        }
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResourceLink {
    pub label: String,
    pub url: String,
}

fn leading_resources(html: &str) -> Option<(Vec<ResourceLink>, std::ops::Range<usize>)> {
    let mut cursor = 0;
    for heroes in 0..=3 {
        let remaining = &html[cursor..];
        let start = cursor + remaining.len() - remaining.trim_start().len();
        let paragraph = html[start..].strip_prefix("<p>")?;
        let end = start + 3 + paragraph.find("</p>")? + 4;
        let fragment = &html[start..end];
        let has_image = fragment.contains("<img ");
        if end - start > 4096 || (!has_image && fragment.match_indices("<a ").take(2).count() < 2) {
            return None;
        }
        let document = Html::parse_fragment(fragment);
        let selector = Selector::parse("p").ok()?;
        let paragraph = document.select(&selector).next()?;
        if has_image && image_only_paragraph(paragraph) {
            if heroes == 3 {
                return None;
            }
            cursor = end;
            continue;
        }
        return Some((paragraph_resources(paragraph)?, start..end));
    }
    None
}

fn image_only_paragraph(paragraph: scraper::ElementRef<'_>) -> bool {
    let mut images = 0;
    let image_only = paragraph
        .descendants()
        .skip(1)
        .all(|node| match node.value() {
            scraper::Node::Text(text) => text.chars().all(char::is_whitespace),
            scraper::Node::Element(element) if element.name() == "img" => {
                images += 1;
                true
            }
            scraper::Node::Element(element) => {
                matches!(element.name(), "a" | "picture" | "source" | "br")
            }
            _ => false,
        });
    image_only && images > 0
}

fn paragraph_resources(paragraph: scraper::ElementRef<'_>) -> Option<Vec<ResourceLink>> {
    let mut resources = Vec::new();
    let mut destinations = HashSet::new();
    for node in paragraph.children() {
        match node.value() {
            scraper::Node::Text(text)
                if text
                    .chars()
                    .all(|c| c.is_whitespace() || matches!(c, '|' | '·' | '•')) => {}
            scraper::Node::Element(element) if element.name() == "br" => {}
            scraper::Node::Element(element) if element.name() == "a" => {
                let anchor = scraper::ElementRef::wrap(node)?;
                if anchor
                    .descendants()
                    .skip(1)
                    .any(|child| match child.value() {
                        scraper::Node::Text(_) => false,
                        scraper::Node::Element(element) => {
                            !matches!(element.name(), "em" | "strong")
                        }
                        _ => true,
                    })
                {
                    return None;
                }
                let label = anchor
                    .text()
                    .collect::<String>()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                if label.is_empty()
                    || label.chars().count() > 60
                    || label.split_whitespace().count() > 6
                {
                    return None;
                }
                let href = element.attr("href")?;
                let url = Url::parse(href).ok()?;
                if !resource_url(&url) {
                    return None;
                }
                if destinations.insert(crate::model::normalize_link(href)) {
                    resources.push(ResourceLink {
                        label,
                        url: href.to_string(),
                    });
                }
                if resources.len() > 8 {
                    return None;
                }
            }
            _ => return None,
        }
    }
    (resources.len() >= 2).then_some(resources)
}

fn resource_url(url: &Url) -> bool {
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    let parts = url.path().trim_matches('/').split('/').collect::<Vec<_>>();
    if parts.iter().any(|part| part.is_empty()) {
        return false;
    }
    match host.strip_prefix("www.").unwrap_or(host) {
        "huggingface.co" => {
            parts.len() >= 2 && !matches!(parts[0], "docs" | "blog" | "posts" | "organizations")
        }
        "modelscope.cn" | "modelscope.ai" => {
            parts.len() >= 2
                && matches!(parts[0], "collections" | "models" | "datasets" | "studios")
        }
        "github.com" | "gitlab.com" | "codeberg.org" => {
            parts.len() >= 2
                && !matches!(
                    parts[0],
                    "orgs" | "users" | "explore" | "topics" | "sponsors"
                )
                && !matches!(parts[1], "followers" | "following")
        }
        "arxiv.org" => parts.len() >= 2 && matches!(parts[0], "abs" | "pdf" | "html"),
        "doi.org" => parts.len() >= 2 && parts[0].starts_with("10."),
        "openreview.net" => {
            parts == ["forum"]
                && url
                    .query_pairs()
                    .any(|(key, value)| key == "id" && !value.is_empty())
        }
        "zenodo.org" => parts.len() == 2 && matches!(parts[0], "record" | "records"),
        "kaggle.com" => parts.len() >= 3 && parts[0] == "datasets",
        "pypi.org" => parts.len() >= 2 && parts[0] == "project",
        "npmjs.com" => parts.len() >= 2 && parts[0] == "package",
        _ => url.path().to_ascii_lowercase().ends_with(".pdf"),
    }
}

/// Render safe Markdown and replace only validated publisher images with immutable local assets.
/// The first image is allowed to become the LCP resource; later images use native lazy loading.
pub fn render_markdown_with_images(markdown: &str, images: &[LocalImage]) -> String {
    render_markdown_with_image_dimensions(markdown, images, &[])
}

fn render_markdown_html(markdown: &str, plugins: &comrak::options::Plugins<'_>) -> String {
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
    out
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

/// Plain text of an HTML fragment: tags stripped, entities decoded, whitespace collapsed.
pub fn html_to_text(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut i = 0;
    while i < html.len() {
        let Some(lt) = html[i..].find('<').map(|pos| i + pos) else {
            text.push_str(&html[i..]);
            break;
        };
        text.push_str(&html[i..lt]);
        let rest = &html[lt..];
        if let Some(after) = comment_end(rest) {
            i = lt + after;
            continue;
        }
        match parse_tag(rest) {
            Some(tag) => {
                if BLOCK_ELEMENTS.contains(&tag.name.as_str()) {
                    text.push(' ');
                }
                let after = tag.end.map_or(html.len(), |end| lt + end);
                i = if !tag.closing && RAW_TEXT_ELEMENTS.contains(&tag.name.as_str()) {
                    skip_element(html, after, &tag.name)
                } else {
                    after
                };
            }
            None => {
                text.push('<');
                i = lt + 1;
            }
        }
    }
    let decoded = decode_entities(&text);
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let escaped = text.replace('<', "&lt;").replace('>', "&gt;");
    Html::parse_fragment(&escaped)
        .root_element()
        .text()
        .collect()
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

    #[test]
    fn leading_resource_links_move_only_in_reader_html_and_stay_in_portable_outputs() {
        let markdown = "[HUGGING FACE](https://huggingface.co/collections/Qwen/qwen-scope) [MODELSCOPE](https://modelscope.cn/collections/Qwen/Qwen-Scope) [TECHNICAL REPORT](https://arxiv.org/abs/2605.11887)\n\nInterpretability helps us understand models.\n\nTry [Hugging Face](https://huggingface.co/collections/Qwen/qwen-scope) yourself.";
        let prepared = PreparedMarkdown::new(markdown);
        assert_eq!(prepared.resources().len(), 3);
        assert_eq!(prepared.resources()[0].label, "HUGGING FACE");
        assert_eq!(
            prepared.resources()[1].url,
            "https://modelscope.cn/collections/Qwen/Qwen-Scope"
        );
        assert_eq!(
            prepared.resources()[2].url,
            "https://arxiv.org/abs/2605.11887"
        );
        assert!(prepared.excerpt(240).starts_with("Interpretability"));
        assert!(!prepared.plain_text().contains("TECHNICAL REPORT"));
        let reader = prepared.reader_html_with_images(&[], &[]);
        assert!(!reader.contains("TECHNICAL REPORT"));
        assert!(
            reader.contains(">Hugging Face</a>"),
            "links in prose remain in place"
        );
        for html in [
            prepared.portable_html().to_string(),
            prepared.with_images(&[], &[]),
            render_markdown(markdown),
        ] {
            for resource in prepared.resources() {
                assert!(
                    html.contains(&resource.url),
                    "portable output lost {}",
                    resource.url
                );
            }
            assert!(html.contains("TECHNICAL REPORT"));
        }
    }

    #[test]
    fn resource_links_after_qwen_hero_preserve_images_and_all_portable_destinations() {
        let hero =
            "https://qianwen-res.oss-accelerate.aliyuncs.com/qwen-scope/Figures/overview.png";
        let links = "[HUGGING FACE](https://huggingface.co/collections/Qwen/qwen-scope) [MODELSCOPE](https://modelscope.cn/collections/Qwen/Qwen-Scope) [TECHNICAL REPORT](https://arxiv.org/abs/2605.11887)";
        let markdown = format!(
            "![Qwen-Scope main image]({hero})\n\n{links}\n\nInterpretability research has emerged as a critical area for understanding LLM behaviors."
        );
        let prepared = PreparedMarkdown::new(&markdown);
        assert_eq!(prepared.resources().len(), 3);
        let reader = prepared.reader_html_with_images(&[], &[]);
        assert!(reader.contains(hero));
        assert!(reader.contains("Qwen-Scope main image"));
        assert!(std::ptr::eq(prepared.reader_html(), prepared.reader_html()));
        assert!(reader.contains("fetchpriority=\"high\""));
        assert!(!reader.contains("TECHNICAL REPORT"));
        assert!(
            prepared
                .excerpt(240)
                .starts_with("Interpretability research")
        );
        assert!(!prepared.plain_text().contains("HUGGING FACE"));
        let portable = prepared.portable_html();
        assert!(portable.contains(hero));
        for resource in prepared.resources() {
            assert!(portable.contains(&resource.url));
        }
        assert!(portable.contains("TECHNICAL REPORT"));
        assert_eq!(prepared.with_images(&[], &[]), render_markdown(&markdown));
    }

    #[test]
    fn resource_hero_skipping_is_bounded_and_stops_at_prose_captions_and_headings() {
        let links =
            "[Code](https://github.com/lab/project) [Paper](https://arxiv.org/abs/1234.5678)";
        let image = "![Hero](https://example.com/hero.png)\n\n";
        for count in 1..=3 {
            let prepared =
                PreparedMarkdown::new(&format!("{}{links}\n\nArticle prose.", image.repeat(count)));
            assert_eq!(prepared.resources().len(), 2);
            assert_eq!(
                prepared
                    .reader_html_with_images(&[], &[])
                    .matches("<img ")
                    .count(),
                count
            );
        }
        for prefix in [
            image.repeat(4),
            format!("{image}Introduction.\n\n"),
            format!("{image}## Resources\n\n"),
            "![Hero](https://example.com/hero.png) A caption.\n\n".into(),
        ] {
            let prepared = PreparedMarkdown::new(&format!("{prefix}{links}\n\nArticle prose."));
            assert!(prepared.resources().is_empty());
            assert_eq!(
                prepared.reader_html_with_images(&[], &[]),
                prepared.portable_html()
            );
        }
    }

    #[test]
    fn resource_detection_preserves_prose_tocs_people_code_and_untrusted_links() {
        for markdown in [
            "[Alice](https://github.com/alice) [Bob](https://github.com/bob)",
            "[Introduction](#intro) [Methods](#methods)",
            "[Code](https://github.com/lab/project) [Follow us](https://x.com/lab)",
            "See [Code](https://github.com/lab/project) and [Paper](https://arxiv.org/abs/1234.5678).",
            "[Code](https://github.com/lab/project)",
            "[Code](https://github.com/lab/project) [Mirror](https://github.com/lab/project#readme)",
            "[Code](https://evil.test/github.com/lab/project) [Paper](https://arxiv.org/abs/1234.5678)",
            "[Code](https://user:password@github.com/lab/project) [Paper](https://arxiv.org/abs/1234.5678)",
            "[Code](javascript:alert) [Paper](https://arxiv.org/abs/1234.5678)",
            "`[Code](https://github.com/lab/project)` [Paper](https://arxiv.org/abs/1234.5678)",
            "> [Code](https://github.com/lab/project) [Paper](https://arxiv.org/abs/1234.5678)",
            "- [Code](https://github.com/lab/project)\n- [Paper](https://arxiv.org/abs/1234.5678)",
            "```md\n[Code](https://github.com/lab/project) [Paper](https://arxiv.org/abs/1234.5678)\n```",
            "Ordinary introduction.\n\n[Code](https://github.com/lab/project) [Paper](https://arxiv.org/abs/1234.5678)",
        ] {
            let prepared = PreparedMarkdown::new(markdown);
            assert!(prepared.resources().is_empty(), "{markdown}");
            assert_eq!(
                prepared.reader_html_with_images(&[], &[]),
                prepared.portable_html()
            );
        }
    }

    #[test]
    fn extraction_preserves_captioned_figures_named_like_comment_widgets() {
        let prose = "In order to let people know about new articles, I post announcements on social media. From time to time I examine traffic on these sites, comparing engagement and referrals across the platforms. ".repeat(5);
        let page = format!(
            "<html><head><title>Social media engagement</title></head><body><article><h1>Social media engagement</h1><p>{prose}</p><div class='figure ' id='retweets.png'><img src='2026-social-traffic/retweets.png'></img><p class='photoCaption'>Figure 1: number of retweets</p></div><div class='figure ' id='replies.png'><img src='2026-social-traffic/replies.png'></img><p class='photoCaption'>Figure 2: number of replies for the same period</p></div><figure id='comments-chart'><img src='2026-social-traffic/comments.png'><figcaption>Figure 3: comment counts</figcaption></figure><p>{prose}</p><div class='replies'><p>Unrelated visitor discussion</p></div></article><aside class='sidebar'><div class='figure'><img src='/advert.png'><p class='caption'>Advertising</p></div></aside></body></html>"
        );
        let url = Url::parse("https://martinfowler.com/articles/2026-social-traffic.html").unwrap();
        let article = extract_article(&page, &url).unwrap();
        let markdown = to_markdown(&article.html, Some(&url));
        for (caption, image) in [
            ("Figure 1", "retweets.png"),
            ("Figure 2", "replies.png"),
            ("Figure 3", "comments.png"),
        ] {
            assert!(markdown.contains(caption), "{markdown}");
            assert!(
                markdown.contains(&format!(
                    "https://martinfowler.com/articles/2026-social-traffic/{image}"
                )),
                "{markdown}"
            );
        }
        assert!(markdown.find("Figure 1") < markdown.find("Figure 2"));
        assert!(markdown.find("Figure 2") < markdown.find("Figure 3"));
        assert!(
            !markdown.contains("Unrelated visitor discussion"),
            "{markdown}"
        );
        assert!(!markdown.contains("Advertising"), "{markdown}");
        assert!(!markdown.contains("advert.png"), "{markdown}");
    }

    #[test]
    #[ignore = "requires saved upstream HTML in AGGR_ARTICLE_HTML"]
    fn saved_fowler_captioned_figures_survive_extraction() {
        let page = std::fs::read_to_string(std::env::var("AGGR_ARTICLE_HTML").unwrap()).unwrap();
        let url = Url::parse("https://martinfowler.com/articles/2026-social-traffic.html").unwrap();
        let article = extract_article(&page, &url).unwrap();
        let markdown = to_markdown(&article.html, Some(&url));
        for number in 1..=6 {
            assert!(
                markdown.contains(&format!("Figure {number}:")),
                "missing figure {number}"
            );
        }
        assert!(
            markdown.contains("https://martinfowler.com/articles/2026-social-traffic/replies.png")
        );
        assert!(!markdown.contains("<script"));
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

    #[tokio::test]
    async fn cancelled_extraction_keeps_its_cpu_slot_until_blocking_work_finishes() {
        let limit = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel();
        let task_limit = limit.clone();
        let task = tokio::spawn(async move {
            limited_extraction(task_limit, move || {
                started_tx.send(()).unwrap();
                finish_rx.recv().unwrap();
                Ok(())
            })
            .await
        });
        started_rx.await.unwrap();
        task.abort();
        let _ = task.await;
        let slots_while_running = limit.available_permits();
        finish_tx.send(()).unwrap();
        let _released = tokio::time::timeout(std::time::Duration::from_secs(1), limit.acquire())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(slots_while_running, 0);
    }

    const HOSTILE: &str = r#"<p>Hi <b>there</b></p>
<script>alert(1)</script>
<img src=x onerror=alert(1)>
<a href="javascript:alert(1)">x</a>
<img src="data:image/png;base64,AAAA">
<svg onload=alert(1)><circle/></svg>
<iframe src="https://evil"></iframe>
<!-- comment -->
<style>body{}</style>
<p>Bye</p>"#;

    const FORBIDDEN: &[&str] = &[
        "<script",
        "onerror",
        "javascript:",
        "data:",
        "<svg",
        "<iframe",
        "<style",
        "onload",
    ];

    fn assert_clean(label: &str, output: &str) {
        for needle in FORBIDDEN {
            assert!(
                !output.to_ascii_lowercase().contains(needle),
                "{label} leaked {needle:?}:\n{output}"
            );
        }
    }

    fn base() -> Url {
        Url::parse("https://example.com/blog/post/").unwrap()
    }

    #[test]
    fn hostile_input_is_neutralized_everywhere() {
        let (stored, truncated) = storage_html(HOSTILE, usize::MAX);
        assert!(!truncated);
        assert_clean("storage_html", &stored);
        assert!(stored.contains("<p>Hi <b>there</b></p>"), "{stored}");
        assert!(stored.contains("<p>Bye</p>"), "{stored}");
        assert!(stored.contains("<img src=x>"), "{stored}");
        assert!(stored.contains("<a>x</a>"), "{stored}");

        let clean = sanitize(HOSTILE, Some(&base()));
        assert_clean("sanitize", &clean);
        assert!(clean.contains("Hi <b>there</b>"), "{clean}");

        let md = to_markdown(HOSTILE, Some(&base()));
        assert_clean("to_markdown", &md);
        assert!(md.contains("Hi **there**"), "{md}");

        assert_clean("render_markdown", &render_markdown(&md));
    }

    #[test]
    fn storage_drops_nested_svg_and_uppercase_tags() {
        let html = "<DIV><SVG><g><svg><circle/></svg></g></SVG>kept</DIV><Script>x</Script>!";
        let (out, _) = storage_html(html, usize::MAX);
        assert_eq!(out, "<DIV>kept</DIV>!");
    }

    #[test]
    fn storage_handles_quoted_gt_and_unterminated_tags() {
        let (out, _) = storage_html(r#"<a title="a > b" href="/x">t</a>"#, usize::MAX);
        assert_eq!(out, r#"<a title="a > b" href="/x">t</a>"#);
        let (out, _) = storage_html("<p>ok</p><div class=\"unterminated", usize::MAX);
        assert_eq!(out, "<p>ok</p>");
        let (out, _) = storage_html("<p>ok</p><!-- never closed", usize::MAX);
        assert_eq!(out, "<p>ok</p>");
        let (out, _) = storage_html("1 < 2 and <3 <", usize::MAX);
        assert_eq!(out, "1 < 2 and <3 <");
        let (out, _) = storage_html("<script>never closed", usize::MAX);
        assert_eq!(out, "");
    }

    #[test]
    fn storage_strips_active_attributes_only() {
        let html = r#"<img alt="a" src='data:image/png;base64,AA' width=1><a href=data:text/html,x>l</a><img srcset="data:x 1x, /b.png 2x"><a href="https://ok/?q=data:">k</a>"#;
        let (out, _) = storage_html(html, usize::MAX);
        assert_eq!(
            out,
            r#"<img alt="a" width=1><a>l</a><img><a href="https://ok/?q=data:">k</a>"#
        );
        let html = r#"<a href=" JavaScript:x" ONCLICK="y" class="one">l</a><p onmouseover=z title="t">p</p>"#;
        let (out, _) = storage_html(html, usize::MAX);
        assert_eq!(out, r#"<a class="one">l</a><p title="t">p</p>"#);
    }

    #[test]
    fn storage_caps_on_char_boundary() {
        let body = "é".repeat(5_000);
        let html = format!("<p>{body}</p>");
        let (out, truncated) = storage_html(&html, 1000);
        assert!(truncated);
        assert!(out.len() <= 1000, "{}", out.len());
        assert!(out.starts_with("<p>é"));

        let html = format!("<p>{}</p><p>{}</p>", "a".repeat(700), "b".repeat(9_000));
        let (out, truncated) = storage_html(&html, 1000);
        assert!(truncated);
        assert_eq!(out, format!("<p>{}</p><p>", "a".repeat(700)));

        assert_eq!(
            storage_html("<p>tiny</p>", 1000),
            ("<p>tiny</p>".to_string(), false)
        );
    }

    #[test]
    fn sanitize_resolves_relative_urls_and_sets_rel() {
        let out = sanitize(
            r#"<a href="../other">o</a><img src="pic.png">"#,
            Some(&base()),
        );
        assert!(
            out.contains(r#"href="https://example.com/blog/other""#),
            "{out}"
        );
        assert!(out.contains(r#"rel="noopener noreferrer""#), "{out}");
        assert!(out.contains(r#"target="_blank""#), "{out}");
        assert!(
            out.contains(r#"src="https://example.com/blog/post/pic.png""#),
            "{out}"
        );
        assert!(out.contains(r#"loading="lazy""#), "{out}");

        let out = sanitize(r#"<a href="/rel">o</a>"#, None);
        assert!(out.contains(r#"href="/rel""#), "{out}");
    }

    #[test]
    fn markdown_keeps_complete_cdn_urls_containing_srcset_commas() {
        let url = "https://substackcdn.com/image/fetch/w_1456,c_limit,f_webp,q_auto:good,fl_progressive:steep/https%3A%2F%2Fimages.example%2Ffigure.png";
        let small = url.replace("w_1456", "w_424");
        let html = format!("<img src=\"fallback.png\" srcset=\"{small} 424w, {url} 1456w\">");
        let base = Url::parse("https://publisher.example/p/article").unwrap();
        let markdown = to_markdown(&html, Some(&base));
        assert!(markdown.contains(url), "{markdown}");
        assert!(!markdown.contains("publisher.example/p/fl_progressive"));
        let candidates = crate::media::body_candidates(&html, &base);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].url.as_str(), url);
        for unsafe_url in [
            "javascript:alert(1)",
            "data:image/png;base64,AAAA",
            "https://user:secret@images.example/figure.png",
        ] {
            let html = format!("<img srcset=\"{unsafe_url} 2x\">");
            assert!(
                crate::media::body_candidates(&html, &base).is_empty(),
                "{unsafe_url}"
            );
        }
    }

    #[test]
    fn markdown_promotes_lazy_srcsets_and_picture_sources() {
        let html = r#"
          <img alt="Responsive" src="tiny.jpg" srcset="medium.jpg 640w, large.jpg 1280w">
          <img alt="Lazy" src="data:image/gif;base64,AAAA" data-srcset="lazy-small.webp 1x, lazy-large.webp 2x">
          <picture>
            <source type="image/avif" srcset="hero.avif 1x">
            <source type="image/webp" data-srcset="hero-small.webp 320w, hero-large.webp 1200w">
            <img alt="Hero" src="data:image/gif;base64,AAAA">
          </picture>
        "#;
        let normalized = normalize_image_sources(html);
        assert!(normalized.contains(r#"src="large.jpg""#), "{normalized}");
        assert!(
            normalized.contains(r#"src="lazy-large.webp""#),
            "{normalized}"
        );
        assert!(
            normalized.contains(r#"alt="Hero" src="hero-large.webp""#),
            "{normalized}"
        );
        let markdown = to_markdown(html, Some(&base()));
        assert!(
            markdown.contains("![Responsive](https://example.com/blog/post/large.jpg)"),
            "{markdown}"
        );
        assert!(
            markdown.contains("![Lazy](https://example.com/blog/post/lazy-large.webp)"),
            "{markdown}"
        );
        assert!(
            markdown.contains("![Hero](https://example.com/blog/post/hero-large.webp)"),
            "{markdown}"
        );
    }

    #[test]
    fn markdown_conversion_covers_common_structures() {
        let html = r#"<h2>Title</h2>
<p>Para with <a href="/x">link</a><br>next line</p>
<ul><li>one</li><li>two</li></ul>
<pre><code>let x = 1;
</code></pre>



<p>end</p>"#;
        let md = to_markdown(html, Some(&base()));
        assert!(md.starts_with("## Title\n"), "{md}");
        assert!(md.contains("[link](https://example.com/x)"), "{md}");
        assert!(md.contains("/x)\\\nnext line"), "{md:?}");
        assert!(md.contains("- one\n- two"), "{md:?}");
        assert!(md.contains("```\nlet x = 1;\n```"), "{md}");
        assert!(md.ends_with("end\n"), "{md}");
        assert!(!md.ends_with("\n\n"), "{md}");
        assert!(!md.contains("\n\n\n"), "{md}");
        assert_eq!(to_markdown("", None), "");
    }

    #[test]
    fn highlighted_code_preserves_lines_language_and_literal_markdown() {
        let html = concat!(
            "<pre><code data-lang=\"bash\"><span>$ z dotfiles\n</span>",
            "<span>$ <span>pwd</span>\n</span><span>/Users/example/dotfiles\n</span>",
            "<span>\n\n  foo[bar](baz)  \n</span></code></pre>"
        );
        assert_eq!(
            to_markdown(html, None),
            concat!(
                "```bash\n$ z dotfiles\n$ pwd\n/Users/example/dotfiles\n",
                "\n\n  foo[bar](baz)  \n```\n"
            )
        );
        assert!(
            to_markdown(
                "<p><code>foo[bar](baz) (opens in a new window)</code></p>",
                None
            )
            .contains("`foo[bar](baz) (opens in a new window)`")
        );
    }

    #[test]
    fn accessibility_label_cleanup_only_removes_short_unformatted_markers() {
        let marker = "<span>\u{2060}(opens in a new window)</span>";
        assert_eq!(normalize_extracted_controls(marker).html, "");
        let literal = "<span><em>(opens in a new window)</em></span>";
        assert_eq!(normalize_extracted_controls(literal).html, literal);
        let deeply_nested = format!(
            "{}literal{}",
            "<span>".repeat(2_000),
            "</span>".repeat(2_000)
        );
        assert_eq!(
            normalize_extracted_controls(&deeply_nested).html,
            deeply_nested
        );
    }

    #[test]
    fn accessibility_labels_are_removed_from_html_and_retained_article_boundaries() {
        for marker in [
            "opens in new window",
            "Opens in a new window",
            "(opens in a new window)",
            "opens in new tab",
        ] {
            assert_eq!(
                to_markdown(
                    &format!("<p><span>{marker}</span></p><p>Article body.</p>"),
                    None
                ),
                "Article body.\n"
            );
            assert_eq!(
                strip_article_metadata(
                    &format!("{marker}\n\nArticle body.\n\n{marker}\n"),
                    None,
                    "apple"
                ),
                "Article body.\n"
            );
        }
        for prose in [
            "This link opens in new window mode.\n\nArticle body.\n",
            "`opens in new window`\n\nArticle body.\n",
            "```text\nopens in new window\n```\n\nArticle body.\n",
            "Article body.\n\nopens in new window\n\nMore body.\n",
        ] {
            assert_eq!(strip_article_metadata(prose, None, "apple"), prose);
        }
    }

    #[test]
    fn storage_strips_entity_encoded_active_urls() {
        let (html, _) = storage_html(
            "<a href=\"jav&#x61;script:alert(1)\">bad</a><img src=\"d&#97;ta:x\">",
            usize::MAX,
        );
        assert_eq!(html, "<a>bad</a><img>");
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
    fn html_heading_breaks_remain_inside_one_heading() {
        for level in 1..=6 {
            let markdown = to_markdown(
                &format!(
                    "<h{level}>40 hours, $2M+ AI credits,<br><em>solve an open problem.</em></h{level}><p>Details.</p>"
                ),
                None,
            );
            let html = render_markdown(&markdown);
            let separator = if level <= 2 { "<br />\n" } else { " " };
            assert!(html.contains(&format!("<h{level}>40 hours, $2M+ AI credits,{separator}<em>solve an open problem.</em></h{level}>")), "{markdown:?}\n{html}");
            assert!(html.contains("<p>Details.</p>"), "{html}");
        }
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
    fn boundary_comment_controls_are_removed_for_every_source() {
        let controls = [
            r"\[[0 comments](https://blog.netbsd.org/post#comment-form)\]",
            "[1 comment](https://example.com/post#comments)",
            "[42 COMMENTS](https://example.com/comments)",
            "[Comments (12)](https://example.com/post#comments)",
            "[No comments](https://example.com/post#comments)",
            "[Leave a comment](https://example.com/post#respond)",
            "[0 comments]",
            "0 comments",
            "[**12 comments**](https://example.com/#comments)",
            "[0 comments (opens in a new window)](https://example.com/#comments)",
        ];
        for source in [
            "hnrss-org-frontpage",
            "a-normal-feed",
            "an-html-source",
            "imported",
        ] {
            for control in controls {
                for body in [
                    format!("{control}\n\nActual article.\n"),
                    format!("Actual article.\n\n{control}\n"),
                    format!("{control}\n\nActual article.\n\n{control}\n"),
                ] {
                    assert_eq!(
                        strip_article_metadata(&body, None, source),
                        "Actual article.\n",
                        "{body}"
                    );
                }
            }
        }
    }

    #[test]
    fn comment_cleanup_preserves_article_content_and_code() {
        for body in [
            "The article has [0 comments](https://example.com/#comments).\n",
            "This code has 0 comments.\n",
            "`0 comments`\n",
            "```text\n0 comments\n```\n",
            "> [0 comments](https://example.com/#comments)\n",
            "- [0 comments](https://example.com/#comments)\n",
            "## 0 comments\n",
            "Article.\n\n[0 comments](https://example.com/#comments)\n\nMore article.\n",
            "[Read the comments about the algorithm](https://example.com/#comments)\n",
            "![0 comments](https://example.com/image.png)\n",
            "Leave a comment\n",
            "Comment 42\n",
            "[0 comments]: https://example.com/#comments\n",
        ] {
            assert_eq!(strip_article_metadata(body, None, "feed"), body, "{body}");
        }
    }

    #[test]
    fn boundary_controls_compose_with_metadata_and_html_conversion() {
        let published = DateTime::parse_from_rfc3339("2026-09-06T15:44:24Z")
            .unwrap()
            .with_timezone(&Utc);
        let html = "<div><p>[<a href=\"https://example.com/#comments\">0 comments</a>]</p><p>September 06, 2026</p><p>Actual article.</p><p>[<a href=\"https://example.com/#comment-form\">0 comments</a>]</p></div>";
        let body = to_markdown(html, None);
        assert_eq!(
            strip_article_metadata(&body, Some(published), "feed"),
            "Actual article.\n"
        );
        assert_eq!(strip_article_metadata("0 comments\n", None, "feed"), "");
    }

    #[test]
    fn youtube_plain_description_has_sections_lists_and_chapter_links() {
        let url = Url::parse("https://www.youtube.com/watch?v=abc123").unwrap();
        let html = "<p>Streamed live: https://twitch.tv/example<br>Enable subtitles</p><p>References:<br>- https://example.com/a<br>- https://example.com/b</p><p>Chapters:<br>- 00:00:00 - Intro<br>- 01:13:41 - More</p>";
        let markdown = to_markdown(html, Some(&url));
        assert!(
            markdown.contains("### References\n\n- https://example.com/a\n- https://example.com/b"),
            "{markdown}"
        );
        assert!(markdown.contains("### Chapters"), "{markdown}");
        assert!(
            markdown.contains("[01:13:41](https://www.youtube.com/watch?v=abc123&t=4421)"),
            "{markdown}"
        );
        assert!(!markdown.contains("\\-"), "{markdown}");
        let ordinary = to_markdown(html, Some(&Url::parse("https://example.com/").unwrap()));
        assert!(ordinary.contains("\\-"), "{ordinary}");
    }

    #[test]
    fn youtube_description_does_not_guess_rich_html_or_invalid_timestamps() {
        let url = Url::parse("https://youtu.be/abc123").unwrap();
        let rich = "<p><strong>References:</strong><br>- preserve my formatting</p>";
        assert_eq!(to_markdown(rich, Some(&url)), to_markdown(rich, None));
        for literal in [
            "12:90 - invalid",
            "1:2:3:4 - too many parts",
            "01:23x - text",
            "- literal dash",
            "12:30",
        ] {
            assert_eq!(description_chapter(literal, "abc123"), escape_html(literal));
        }
        assert_eq!(description_bullet("    - indented literal"), None);
        assert_eq!(description_bullet("- "), None);
        assert!(description_chapter("12:30 - chapter", "abc123").contains("&amp;t=750"));
    }

    #[test]
    fn youtube_description_preserves_code_and_literal_markup() {
        let url = Url::parse("https://www.youtube.com/watch?v=abc123").unwrap();
        let code = "<pre><code>Chapters:\n- 01:02 - example</code></pre>";
        assert_eq!(to_markdown(code, Some(&url)), to_markdown(code, None));
        let literal =
            "<p>Quotes:<br>- &lt;script&gt;alert(1)&lt;/script&gt; &amp; literal *stars*</p>";
        let output = render_markdown(&to_markdown(literal, Some(&url)));
        assert!(output.contains("&lt;script&gt;"), "{output}");
        assert!(!output.contains("<script>"), "{output}");
        assert!(!output.contains("<em>stars</em>"), "{output}");
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
    fn publisher_code_hints_survive_conversion_into_language_labels() {
        for attribute in [
            "data-lang=\"python\"",
            "data-language=\"python\"",
            "class=\"language-python\"",
            "class=\"lang-python\"",
            "class=\"highlight-source-python\"",
        ] {
            let markdown = to_markdown(
                &format!("<pre><code {attribute}>value = 1\n</code></pre>"),
                None,
            );
            assert!(
                markdown.starts_with("```python\n"),
                "{attribute}: {markdown}"
            );
            let html = render_markdown(&markdown);
            assert!(html.contains("data-language=\"Python\""), "{html}");
        }
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
        for language in ["text", "plaintext", "unknown-language", "evil\"<img/src=x>"] {
            let html = render_markdown(&format!("```{language}\nfn main() {{}}\n```\n"));
            assert!(html.contains("data-language=\"Text\""), "{html}");
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
        ] {
            let html = render_markdown(&format!("```\n{code}\n```\n"));
            assert!(html.contains("data-language=\"Text\""), "{html}");
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
    fn strips_orphaned_divider_only_after_matching_publication_date() {
        use chrono::{TimeZone as _, Utc};

        let published = Utc.with_ymd_and_hms(2026, 9, 2, 15, 40, 0).unwrap();
        let markdown = to_markdown(
            "<p>Sep 02, 2026</p><span>|</span><p>The actual article.</p>",
            None,
        );
        assert_eq!(
            strip_leading_metadata(&markdown, Some(published), "blog-google"),
            "The actual article.\n"
        );

        for body in [
            "|\n\nAn article about the pipe symbol.\n",
            "`|`\n\nAn article about the pipe symbol.\n",
            "```sh\ncat input | sort\n```\n\nBody.\n",
            "| Input | Output |\n| --- | --- |\n| a | b |\n",
            "The expression a | b combines the values.\n",
        ] {
            assert_eq!(
                strip_leading_metadata(body, Some(published), "blog-google"),
                body
            );
            assert_eq!(
                strip_leading_metadata(
                    &format!("Sep 02, 2026\n\n{body}"),
                    Some(published),
                    "blog-google"
                ),
                if body.starts_with("|\n\n") {
                    "An article about the pipe symbol.\n"
                } else {
                    body
                }
            );
        }
        let wrong_date = "Sep 02, 2025\n\n|\n\nBody.\n";
        assert_eq!(
            strip_leading_metadata(wrong_date, Some(published), "blog-google"),
            wrong_date
        );
        assert_eq!(
            strip_leading_metadata(
                "Blog Google\n\n|\n\nBody.\n",
                Some(published),
                "blog-google"
            ),
            "|\n\nBody.\n"
        );
    }

    #[test]
    fn strips_leading_advertisement_and_its_orphaned_bullet() {
        let body = "Mullenweg, co-founder of WordPress, wrote in a company-wide Slack message.\n\n![Photo](https://example.com/photo.jpg)\n";
        for prefix in [
            "Advertisement\n\n•\n\n",
            "**ADVERTISEMENT**\n\n•\n\n",
            "Advertisement\n•\n\n",
            "Advertisement\n\n",
            "[Advertisement](/ads)\n\n•\n\n",
        ] {
            assert_eq!(
                strip_article_metadata(&format!("{prefix}{body}"), None, "hnrss-org-frontpage"),
                body,
                "{prefix}"
            );
        }
        for prefix in [
            "•\n\n",
            "# Advertisement\n\n•\n\n",
            "> Advertisement\n\n•\n\n",
            "`Advertisement`\n\n•\n\n",
            "Advertisement is how this publication is funded.\n\n•\n\n",
            "An introduction.\n\nAdvertisement\n\n•\n\n",
            "```text\nAdvertisement\n•\n```\n\n",
        ] {
            let markdown = format!("{prefix}{body}");
            assert_eq!(
                strip_article_metadata(&markdown, None, "hnrss-org-frontpage"),
                markdown
            );
        }
    }

    #[test]
    fn strips_cognition_leading_byline_with_matching_publication_date() {
        use chrono::TimeZone as _;

        let published = Utc.with_ymd_and_hms(2026, 9, 9, 17, 0, 0).unwrap();
        let markdown = to_markdown(
            "<article><header><p><span>By Eric Lu</span><span>09.09.26</span></p></header><section><p>Over the past few weeks, the Cognition research team and I have been optimizing our job scheduler.</p></section></article>",
            None,
        );
        assert!(markdown.starts_with("By Eric Lu 09.09.26\n\n"));
        let expected = "Over the past few weeks, the Cognition research team and I have been optimizing our job scheduler.\n";
        assert_eq!(
            strip_article_metadata(&markdown, Some(published), "cognition-com-blog"),
            expected
        );
        assert_eq!(
            strip_article_metadata(expected, Some(published), "cognition-com-blog"),
            expected
        );

        for body in [
            "By Eric Lu 09.09.25\n\nBody.\n",
            "By Eric Lu\n\nBody.\n",
            "By using algebra we solved it on 09.09.26\n\nBody.\n",
            "By September 09.09.26\n\nBody.\n",
            "By Eric Lu 09.09.26 we had solved it.\n\nBody.\n",
            "`By Eric Lu 09.09.26`\n\nBody.\n",
            "# By Eric Lu 09.09.26\n\nBody.\n",
            "> By Eric Lu 09.09.26\n\nBody.\n",
            "An opening.\n\nBy Eric Lu 09.09.26\n\nBody.\n",
        ] {
            assert_eq!(
                strip_article_metadata(body, Some(published), "cognition-com-blog"),
                body
            );
        }
        assert_eq!(
            strip_article_metadata(&markdown, None, "cognition-com-blog"),
            markdown
        );
    }

    #[test]
    fn strips_only_leading_metadata_that_matches_the_item() {
        use chrono::{TimeZone as _, Utc};

        let published = Utc.with_ymd_and_hms(2026, 9, 2, 14, 16, 42).unwrap();
        let body = "2nd September 2026\n\nAnthropic published the prompts.\n";
        assert_eq!(
            strip_leading_metadata(body, Some(published), "anthropic"),
            "Anthropic published the prompts.\n"
        );
        assert_eq!(
            strip_leading_metadata(
                "[September 2, 2026](/archive)\n\nBody.\n",
                Some(published),
                "blog"
            ),
            "Body.\n"
        );
        assert_eq!(
            strip_leading_metadata("2nd September 2025\n\nBody.\n", Some(published), "blog"),
            "2nd September 2025\n\nBody.\n"
        );
        assert_eq!(
            strip_leading_metadata(
                "We met on 2nd September 2026.\n\nBody.\n",
                Some(published),
                "blog"
            ),
            "We met on 2nd September 2026.\n\nBody.\n"
        );
        assert_eq!(
            strip_leading_metadata(
                "3rd September 2026 - Link Blog\n\nThe actual opening.\n",
                Some(Utc.with_ymd_and_hms(2026, 9, 3, 8, 0, 0).unwrap()),
                "simon-willison"
            ),
            "The actual opening.\n"
        );
        assert_eq!(
            strip_leading_metadata("OpenAI\n\nSafety starts here.\n", Some(published), "openai"),
            "Safety starts here.\n"
        );
        assert_eq!(
            strip_leading_metadata(
                "OpenAI builds systems.\n\nBody.\n",
                Some(published),
                "openai"
            ),
            "OpenAI builds systems.\n\nBody.\n"
        );
    }

    #[test]
    fn markdown_repairs_accessibility_link_labels_and_word_boundaries() {
        let html = concat!(
            r#"<p>using more than<span></span><a href="/tasks"><span> 54,000 internal Codex tasks</span><span>"#,
            "\u{2060}(opens in a new window)",
            r#"</span></a> every day</p>"#
        );
        assert_eq!(
            to_markdown(html, Some(&base())),
            "using more than [54,000 internal Codex tasks](https://example.com/tasks) every day\n"
        );
    }

    #[test]
    fn markdown_preserves_boundaries_between_semantic_inline_siblings() {
        let html = r#"<p><time>5/11</time><span>First observed event.</span></p>
<p><a href="/revision"><time>2026-06-21 10:35:01</time> <span>OECDDec29Agent</span><span>open in the wiki</span></a></p>
<p>Sentence.<label for="note"></label><span data-n="3">Note in the margin.</span></p>
<p><span>Show the whole post</span><span>Show less</span></p>"#;

        assert_eq!(
            to_markdown(html, Some(&base())),
            concat!(
                "5/11 First observed event.\n\n",
                "[2026-06-21 10:35:01 OECDDec29Agent open in the wiki]",
                "(https://example.com/revision)\n\n",
                "Sentence. Note in the margin.\n\n",
                "Show the whole post Show less\n",
            )
        );
    }

    #[test]
    fn markdown_puts_adjacent_action_links_on_separate_lines() {
        let html = r#"<p><a href="/explorer">Open the data explorer</a><a href="/download">Download all the data</a></p>"#;

        assert_eq!(
            to_markdown(html, Some(&base())),
            concat!(
                "[Open the data explorer](https://example.com/explorer)\\\n",
                "[Download all the data](https://example.com/download)\n",
            )
        );
    }

    #[test]
    fn markdown_converts_readability_sidenotes_to_footnotes() {
        let html = r#"<p>Before.<label for="fn-later" data-n="8"></label><input type="checkbox" id="fn-later"><span class="sidenote" data-n="8"><b>Note:</b> read the <a href="/source">source</a>.</span> After.</p>
<p>Second<label for="fn-earlier" data-n="2"></label><span data-n="2">Another note.</span></p>"#;

        assert_eq!(
            to_markdown(html, Some(&base())),
            concat!(
                "Before.[^1] After.\n\n",
                "Second[^2]\n\n",
                "[^1]: **Note:** read the [source](https://example.com/source).\n\n",
                "[^2]: Another note.\n",
            )
        );
    }

    #[test]
    fn markdown_indents_multiblock_sidenotes_as_one_footnote() {
        let html = r#"<p>Body<label for="fn-detail" data-n="1"></label><span data-n="1"><p>First paragraph.</p><p>Second paragraph with <em>emphasis</em>.</p></span></p>"#;

        assert_eq!(
            to_markdown(html, Some(&base())),
            concat!(
                "Body[^1]\n\n",
                "[^1]: First paragraph.\n\n",
                "    Second paragraph with *emphasis*.\n",
            )
        );
    }

    #[test]
    fn markdown_pairs_a_sidenote_after_intervening_prose_in_the_same_block() {
        let html = r#"<p>Lead<label for="fn-delayed" data-n="27"></label> Main prose ends here.<span data-n="27">Deferred note.</span><label for="fn-next" data-n="28"></label><span data-n="28">Next note.</span> Tail.</p>"#;

        assert_eq!(
            to_markdown(html, Some(&base())),
            concat!(
                "Lead[^1] Main prose ends here.[^2] Tail.\n\n",
                "[^1]: Deferred note.\n\n",
                "[^2]: Next note.\n",
            )
        );
    }

    #[test]
    fn markdown_does_not_recover_sidenotes_from_active_content() {
        let html = r#"<p>Safe.</p><script><label for="fn-hidden" data-n="1"></label><span data-n="1">Hidden script text.</span></script>"#;
        assert_eq!(to_markdown(html, Some(&base())), "Safe.\n");
    }

    #[test]
    fn markdown_discards_expand_controls_but_keeps_complete_content() {
        let html = r#"<figure>
<a class="ex-head" href="/revision"><time>2026-06-21 10:35:01</time> <span class="who">Agent</span><span class="ex-open">open in the wiki</span></a>
<input type="checkbox" id="example-more">
<pre class="ex-body"><span>Complete post body.</span></pre>
<label for="example-more"><span>Show the whole post</span><span>Show less</span></label>
</figure>"#;
        let markdown = to_markdown(html, Some(&base()));

        assert!(
            markdown.contains("2026-06-21 10:35:01 Agent open in the wiki"),
            "{markdown}"
        );
        assert!(markdown.contains("Complete post body."), "{markdown}");
        assert!(!markdown.contains("Show the whole post"), "{markdown}");
        assert!(!markdown.contains("Show less"), "{markdown}");
    }

    #[test]
    fn inline_boundary_repair_respects_existing_content_whitespace() {
        let html = "<pre><span>left </span><span> right</span></pre>";
        assert_eq!(restore_inline_layout_boundaries(html), html);
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
    fn html_to_text_strips_and_decodes() {
        assert_eq!(html_to_text("<p>a &amp; b</p>\n<p>c</p>"), "a & b c");
        assert_eq!(html_to_text("<p>a<b>b</b>.</p><p>c</p>"), "ab. c");
        assert_eq!(
            html_to_text("x&#39;y&#x41;&nbsp;z &unknown; 1 < 2"),
            "x'yA z &unknown; 1 < 2"
        );
        assert_eq!(html_to_text("<script>var a = '<b>';</script>text"), "text");
        assert_eq!(html_to_text(""), "");
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

    #[test]
    fn readability_keeps_the_article_and_drops_page_chrome() {
        let page = r#"<!doctype html><title>Post</title><nav>Home Products About Contact</nav>
<main><article><h1>A useful post</h1><p>This is the complete article body with enough useful prose for extraction.</p><p>It has a second paragraph, unlike the short feed summary.</p></article></main>
<footer>Copyright and navigation</footer>"#;
        let extracted = extract_article(page, &base()).unwrap();
        let text = html_to_text(&extracted.html);
        assert!(text.contains("complete article body"), "{text}");
        assert!(text.contains("second paragraph"), "{text}");
        assert!(!text.contains("Products About Contact"), "{text}");
    }

    #[test]
    fn readability_accepts_an_image_only_article() {
        let page = r#"<!doctype html><title>Comic</title>
<main><article><figure><img src="/comic.png" alt="Today's comic"></figure></article></main>"#;

        let extracted = extract_article(page, &base()).unwrap();
        let clean = sanitize(&normalize_image_sources(&extracted.html), Some(&base()));

        assert!(clean.contains("https://example.com/comic.png"), "{clean}");
        assert!(clean.contains("Today's comic"), "{clean}");
        assert!(!has_meaningful_extracted_content(
            r#"<img src="data:image/png;base64,AAAA" alt="tracker">"#,
            &base()
        ));
    }
}
