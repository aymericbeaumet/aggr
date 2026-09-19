//! The safety layer: the raw `.html` copy stripped of active content and capped for storage,
//! the ammonia sanitizer that runs before display, and the plain-text view of a fragment.

use std::collections::HashSet;

use ammonia::UrlRelative;
use scraper::Html;
use url::Url;

use super::scan::{
    BLOCK_ELEMENTS, RAW_TEXT_ELEMENTS, close_removed_element, comment_end, is_name_byte, parse_tag,
    skip_element,
};

/// Elements whose content is executable, styled, or embedded: dropped whole.
const DROP_ELEMENTS: &[&str] = &["script", "style", "svg", "iframe", "object", "embed"];

/// Elements that cannot have content; only the tag itself is dropped.
const VOID_ELEMENTS: &[&str] = &["embed"];

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

/// HTML prepared for storage as the `.html` sibling: `<script>`, `<style>`, inline `<svg>`,
/// `<iframe>`/`<object>`/`<embed>`, HTML comments removed; `on*` handlers and `data:` /
/// `javascript:` URL attributes removed; then capped at `max_bytes` on a char boundary (cut at
/// the last `>` before the limit when possible). Returns `(html, truncated)`. Everything else is
/// kept verbatim: this is the raw copy, and it is never served unsanitized.
pub fn storage_html(raw: &str, max_bytes: usize) -> (String, bool) {
    cap(strip_active_content(raw), max_bytes)
}

pub(super) fn strip_active_content(raw: &str) -> String {
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
            // An icon between two words (`Bitwarden<svg/></a>or`) was their visual separator.
            if !tag.closing && matches!(tag.name.as_str(), "svg" | "iframe" | "object" | "embed") {
                i = close_removed_element(raw, i, &mut out);
            }
            continue;
        }
        out.push_str(&without_active_attributes(&rest[..end]));
        i = after_tag;
    }
    out
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
pub(super) fn is_active_url(value: &str) -> bool {
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

pub(super) fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let escaped = text.replace('<', "&lt;").replace('>', "&gt;");
    Html::parse_fragment(&escaped)
        .root_element()
        .text()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{render_markdown, to_markdown};

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
    fn storage_strips_entity_encoded_active_urls() {
        let (html, _) = storage_html(
            "<a href=\"jav&#x61;script:alert(1)\">bad</a><img src=\"d&#97;ta:x\">",
            usize::MAX,
        );
        assert_eq!(html, "<a>bad</a><img>");
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
}
