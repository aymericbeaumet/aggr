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
    sanitize_with_code_classes(html, base, true)
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
    srcset
        .split(',')
        .enumerate()
        .filter_map(|(order, candidate)| {
            let mut fields = candidate.split_ascii_whitespace();
            let url = safe_image_candidate(fields.next()?)?;
            let score = fields
                .next()
                .and_then(|descriptor| {
                    descriptor
                        .strip_suffix(['w', 'x'])
                        .and_then(|value| value.parse::<f64>().ok())
                })
                .unwrap_or(order as f64);
            Some((score, order, url))
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

fn sanitize_with_code_classes(html: &str, base: Option<&Url>, code_classes: bool) -> String {
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
    if code_classes {
        builder.add_tag_attributes("code", ["class"]);
    }
    builder.clean(html).to_string()
}

/// [`sanitize`] then htmd. Trailing whitespace trimmed, exactly one trailing newline, runs of
/// blank lines collapsed to one.
pub fn to_markdown(html: &str, base: Option<&Url>) -> String {
    let normalized_images = normalize_image_sources(html);
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

fn code_language(element: scraper::ElementRef<'_>) -> Option<String> {
    let value = element.value();
    let language = value
        .attr("data-lang")
        .or_else(|| value.attr("data-language"))
        .or_else(|| {
            value.attr("class").and_then(|classes| {
                classes
                    .split_whitespace()
                    .find_map(|class| class.strip_prefix("language-"))
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

/// Recover only code blocks demonstrably damaged by the previous converter. Markdown remains
/// authoritative for edits and prose; the caller excludes truncated HTML companions.
pub fn effective_markdown(stored: &str, retained_html: Option<&str>, base: Option<&Url>) -> String {
    let Some(html) = retained_html else {
        return stored.to_string();
    };
    let stored_blocks = fenced_blocks(stored);
    if stored_blocks.is_empty() {
        return stored.to_string();
    }
    let source = sanitize_with_code_classes(
        &restore_inline_layout_boundaries(&strip_active_content(html)),
        base,
        false,
    );
    let converter = htmd::HtmlToMarkdown::builder()
        .options(htmd::options::Options {
            bullet_list_marker: htmd::options::BulletListMarker::Dash,
            br_style: htmd::options::BrStyle::Backslash,
            ul_bullet_spacing: 1,
            ol_number_spacing: 1,
            ..Default::default()
        })
        .build();
    let Ok(legacy) = converter.convert(&source) else {
        return stored.to_string();
    };
    let legacy = tidy_markdown(&repair_generated_markdown(&legacy));
    // Captures made before inline-layout repair have no separators between highlighting spans.
    let earlier = converter
        .convert(&sanitize_with_code_classes(
            &strip_active_content(html),
            base,
            false,
        ))
        .map(|markdown| tidy_markdown(&repair_generated_markdown(&markdown)))
        .unwrap_or_default();
    let corrected = to_markdown(html, base);
    let old_blocks = fenced_blocks(&legacy);
    let earlier_blocks = fenced_blocks(&earlier);
    let new_blocks = fenced_blocks(&corrected);
    if old_blocks.len() != new_blocks.len() || old_blocks.len() != stored_blocks.len() {
        return stored.to_string();
    }
    let mut result = stored.to_string();
    for (index, ((old, new), kept)) in old_blocks
        .iter()
        .zip(&new_blocks)
        .zip(&stored_blocks)
        .enumerate()
        .rev()
    {
        let matches =
            |original: &FencedBlock| kept.literal == original.literal && kept.info == original.info;
        if matches(old)
            || (earlier_blocks.len() == old_blocks.len()
                && earlier_blocks.get(index).is_some_and(matches))
        {
            result.replace_range(kept.range.clone(), &corrected[new.range.clone()]);
        }
    }
    result
}

struct FencedBlock {
    literal: String,
    info: String,
    range: std::ops::Range<usize>,
}

fn fenced_blocks(markdown: &str) -> Vec<FencedBlock> {
    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, markdown, &comrak::Options::default());
    let mut lines = vec![0];
    lines.extend(markdown.match_indices('\n').map(|(index, _)| index + 1));
    root.descendants()
        .filter_map(|node| {
            let data = node.data.borrow();
            let comrak::nodes::NodeValue::CodeBlock(code) = &data.value else {
                return None;
            };
            if !code.fenced {
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
            .then(|| FencedBlock {
                literal: code.literal.clone(),
                info: code.info.clone(),
                range: start..end,
            })
        })
        .collect()
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
            && decode_entities(&html[tag_end..tag_end + length])
                .replace('\u{2060}', "")
                .trim()
                == "(opens in a new window)"
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

/// Remove a metadata line that readability promoted to the first Markdown paragraph. This covers
/// a publication date (including a short suffix such as `- Link Blog`) and a source-name-only
/// accessibility label. Normal prose containing a date remains intact.
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
        })
    });
    if !source_only && !matching_date {
        return markdown.to_string();
    }
    rest.trim_start_matches('\n').to_string()
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
pub fn reading_metrics(markdown: &str) -> (usize, usize) {
    use unicode_segmentation::UnicodeSegmentation as _;

    let html = comrak::markdown_to_html(markdown, &markdown_options());
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
}

/// Render safe Markdown and replace only validated publisher images with immutable local assets.
/// The first image is allowed to become the LCP resource; later images use native lazy loading.
pub fn render_markdown_with_images(markdown: &str, images: &[LocalImage]) -> String {
    let options = markdown_options();
    let mut plugins = comrak::options::Plugins::default();
    plugins.render.codefence_syntax_highlighter = Some(&CodeHighlighter);
    let html = comrak::markdown_to_html_with_plugins(markdown, &options, &plugins);
    enhance_rendered_images(&add_link_navigation_attributes(&html), images)
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

fn enhance_rendered_images(html: &str, images: &[LocalImage]) -> String {
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
        out.push_str(&render_image_tag(original, image_index, images));
        image_index += 1;
        position = start + end;
    }
    out.push_str(&html[position..]);
    out
}

fn render_image_tag(tag: &str, index: usize, images: &[LocalImage]) -> String {
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
    let alt = image.value().attr("alt").unwrap_or_default();
    let title = image.value().attr("title");
    let key = normalized_image_url(source);
    let local = images
        .iter()
        .find(|candidate| normalized_image_url(&candidate.source) == key);
    let loading = if index == 0 { "eager" } else { "lazy" };
    let priority = if index == 0 { "high" } else { "low" };
    let source_dimensions = image
        .value()
        .attr("width")
        .and_then(|width| width.parse::<u32>().ok())
        .filter(|width| *width > 0)
        .zip(
            image
                .value()
                .attr("height")
                .and_then(|height| height.parse::<u32>().ok())
                .filter(|height| *height > 0),
        )
        .map(|(width, height)| format!(" width=\"{width}\" height=\"{height}\""))
        .unwrap_or_default();
    let (src, dimensions, class) = local.map_or_else(
        || (source, source_dimensions, String::new()),
        |local| {
            (
                local.original.as_str(),
                format!(" width=\"{}\" height=\"{}\"", local.width, local.height),
                " class=\"progressive-image\"".to_string(),
            )
        },
    );
    let title = title
        .map(|title| format!(" title=\"{}\"", escape_html(title)))
        .unwrap_or_default();
    let tag = format!(
        "<img src=\"{}\"{} alt=\"{}\"{} loading=\"{}\" decoding=\"async\" fetchpriority=\"{}\" referrerpolicy=\"no-referrer\"{}>",
        escape_html(src),
        dimensions,
        escape_html(alt),
        title,
        loading,
        priority,
        class
    );
    let Some(local) = local else {
        return tag;
    };
    let placeholder = local
        .variants
        .first()
        .map(|variant| format!(" data-placeholder=\"{}\"", escape_html(&variant.url)))
        .unwrap_or_default();
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
        "<picture class=\"article-picture\"{placeholder} style=\"--image-width:{}px;--image-ratio:{} / {};--image-placeholder:{}\">{source}{tag}</picture>",
        local.width,
        local.width,
        local.height,
        escape_html(&local.color)
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
        use syntect::html::{ClassStyle, ClassedHTMLGenerator};
        use syntect::parsing::SyntaxSet;
        static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
        let language = language.unwrap_or_default();
        if language.is_empty()
            || code.len() > 100 * 1024
            || code.lines().any(|line| line.len() > 2_000)
        {
            return output.write_str(&escape_html(code));
        }
        let syntaxes = SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines);
        let Some(syntax) = syntaxes.find_syntax_by_token(language) else {
            return output.write_str(&escape_html(code));
        };
        let mut generator = ClassedHTMLGenerator::new_with_class_style(
            syntax,
            syntaxes,
            ClassStyle::SpacedPrefixed { prefix: "syntax-" },
        );
        for line in syntect::util::LinesWithEndings::from(code) {
            if generator
                .parse_html_for_line_which_includes_newline(line)
                .is_err()
            {
                return output.write_str(&escape_html(code));
            }
        }
        output.write_str(&generator.finalize())
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
    let text = html_to_text(&render_markdown(markdown));
    if text.chars().count() <= max_chars {
        return text;
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
            html.contains("data-placeholder=\"assets/images/small.webp\""),
            "{html}"
        );
        assert!(
            html.contains(
                "style=\"--image-width:1200px;--image-ratio:1200 / 800;--image-placeholder:#285a8c\""
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
    fn retained_html_recovers_only_matching_legacy_code() {
        let html = "<p>Old prose.</p><pre><code data-lang=bash><span><span>$ z dotfiles\n</span></span><span><span>$ <span>pwd</span>\n</span></span><span><span>/Users/example/dotfiles\n</span></span></code></pre>";
        let stored = "Edited prose stays.\n\n```\n$ z dotfiles$ pwd/Users/example/dotfiles\n```\n";
        assert_eq!(
            effective_markdown(stored, Some(html), None),
            "Edited prose stays.\n\n```bash\n$ z dotfiles\n$ pwd\n/Users/example/dotfiles\n```\n"
        );
        let edited = stored.replace("$ z dotfiles", "$ z elsewhere");
        assert_eq!(effective_markdown(&edited, Some(html), None), edited);
        assert_eq!(effective_markdown(stored, None, None), stored);
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
