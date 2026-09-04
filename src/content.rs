//! HTML in, safe Markdown/HTML/text out. Three views of the same content:
//! the raw `.html` copy (stripped of active content, capped), the sanitized HTML view, and the
//! Markdown body derived from it. Rendering Markdown back to HTML never emits raw HTML.

use std::collections::HashSet;

use ammonia::UrlRelative;
use anyhow::{Context, Result, bail};
use chrono::{DateTime, NaiveDate, Utc};
use dom_smoothie::Readability;
use url::Url;

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
pub fn extract_article(page: &str, url: &Url) -> Result<String> {
    let mut readability = Readability::new(page, Some(url.as_str()), None)
        .context("parsing the original article page")?;
    let article = readability
        .parse()
        .context("extracting readable article content")?;
    let html = article.content.to_string();
    if html_to_text(&html).trim().is_empty() {
        bail!("extracted article is empty");
    }
    Ok(html)
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
    ammonia::Builder::default()
        .url_schemes(HashSet::from(["http", "https", "mailto"]))
        .url_relative(url_relative)
        .link_rel(Some("noopener noreferrer"))
        .set_tag_attribute_value("a", "target", "_blank")
        .set_tag_attribute_value("img", "loading", "lazy")
        .set_tag_attribute_value("img", "decoding", "async")
        .set_tag_attribute_value("img", "referrerpolicy", "no-referrer")
        .clean(html)
        .to_string()
}

/// [`sanitize`] then htmd. Trailing whitespace trimmed, exactly one trailing newline, runs of
/// blank lines collapsed to one.
pub fn to_markdown(html: &str, base: Option<&Url>) -> String {
    let clean = sanitize(html, base);
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
    tidy_markdown(&repair_generated_markdown(&markdown))
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
    let mut value = markdown
        .replace("\u{2060}(opens in a new window)", "")
        .replace(" (opens in a new window)", "");
    let bytes = value.as_bytes();
    let mut insertions = Vec::new();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'[' || index == 0 || !bytes[index - 1].is_ascii_alphanumeric() {
            continue;
        }
        let Some(close) = value[index + 1..]
            .find("](")
            .map(|offset| index + 1 + offset)
        else {
            continue;
        };
        if close > index + 1 {
            insertions.push(index);
        }
    }
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
    let mut options = comrak::Options::default();
    options.extension.table = true;
    options.extension.strikethrough = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    options.extension.footnotes = true;
    options.render.r#unsafe = false;
    options.render.escape = true;
    comrak::markdown_to_html(markdown, &options).replace(
        "<a href=\"",
        "<a target=\"_blank\" rel=\"noopener noreferrer\" href=\"",
    )
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
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let candidate = &rest[amp..];
        match candidate
            .find(';')
            .filter(|&semi| semi <= 10)
            .and_then(|semi| decode_entity(&candidate[1..semi]).map(|ch| (ch, semi + 1)))
        {
            Some((decoded, len)) => {
                out.push(decoded);
                rest = &candidate[len..];
            }
            None => {
                out.push('&');
                rest = &candidate[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn decode_entity(name: &str) -> Option<char> {
    let code = match name {
        "amp" => return Some('&'),
        "lt" => return Some('<'),
        "gt" => return Some('>'),
        "quot" => return Some('"'),
        "apos" => return Some('\''),
        "nbsp" => return Some(' '),
        "hellip" => return Some('…'),
        "mdash" => return Some('—'),
        "ndash" => return Some('–'),
        "lsquo" => return Some('‘'),
        "rsquo" => return Some('’'),
        "ldquo" => return Some('“'),
        "rdquo" => return Some('”'),
        "copy" => return Some('©'),
        _ => name.strip_prefix('#')?,
    };
    let value = match code.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => code.parse().ok()?,
    };
    char::from_u32(value).filter(|ch| *ch != '\0')
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
        let text = html_to_text(&extracted);
        assert!(text.contains("complete article body"), "{text}");
        assert!(text.contains("second paragraph"), "{text}");
        assert!(!text.contains("Products About Contact"), "{text}");
    }
}
