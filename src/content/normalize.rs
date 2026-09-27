//! Recover passive text semantics that publisher scripts or obsolete HTML would otherwise hide.

use super::escape_html;
use super::scan::{attribute_value, comment_end, element_bounds, parse_tag, skip_element};

pub(super) fn publisher_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut position = 0;
    while let Some(start) = html[position..].find('<').map(|offset| position + offset) {
        out.push_str(&html[position..start]);
        if let Some(length) = comment_end(&html[start..]) {
            out.push_str(&html[start..start + length]);
            position = start + length;
            continue;
        }
        let Some(tag) = parse_tag(&html[start..]) else {
            out.push('<');
            position = start + 1;
            continue;
        };
        let Some(length) = tag.end else {
            position = start;
            break;
        };
        let after = start + length;
        let raw = &html[start..after];
        if !tag.closing && tag.name == "plaintext" {
            out.push_str(&html[start..]);
            position = html.len();
            break;
        }
        if !tag.closing
            && matches!(
                tag.name.as_str(),
                "script"
                    | "style"
                    | "textarea"
                    | "title"
                    | "xmp"
                    | "iframe"
                    | "noembed"
                    | "noframes"
            )
        {
            position = skip_element(html, after, &tag.name);
            out.push_str(&html[start..position]);
            continue;
        }
        if !tag.closing
            && matches!(tag.name.as_str(), "a" | "span")
            && let Some(decoded) = protected_text(raw)
            && let Some(end) = protected_element_end(html, start, &tag.name)
        {
            out.push_str(&escape_html(&decoded));
            position = end;
            continue;
        }
        if tag.name == "tt" {
            // Keep attributes for the normal sanitizer; only restore the lost text-level meaning.
            let name_start = if tag.closing { 2 } else { 1 };
            out.push_str(&raw[..name_start]);
            out.push_str("code");
            out.push_str(&raw[name_start + 2..]);
        } else {
            out.push_str(raw);
        }
        position = after;
    }
    out.push_str(&html[position..]);
    out
}

fn protected_element_end(html: &str, start: usize, name: &str) -> Option<usize> {
    // Real placeholders are tiny. Bound malformed/unclosed wrappers before Readability runs.
    let mut end = start.saturating_add(16 * 1024).min(html.len());
    while !html.is_char_boundary(end) {
        end -= 1;
    }
    element_bounds(&html[start..end], 0, name).map(|(_, _, end)| start + end)
}

fn protected_text(tag: &str) -> Option<String> {
    let href = attribute_value(tag, "href");
    let fragment = href.and_then(|href| {
        if let Some(value) = href.strip_prefix("/cdn-cgi/l/email-protection#") {
            return Some(value.to_owned());
        }
        let url = url::Url::parse(href).ok()?;
        (matches!(url.scheme(), "https" | "http") && url.path() == "/cdn-cgi/l/email-protection")
            .then(|| url.fragment().map(str::to_owned))
            .flatten()
    });
    let encoded = attribute_value(tag, "data-cfemail").or(fragment.as_deref())?;
    // The prefix byte is an XOR key, not encrypted/private data: the publisher's browser script
    // performs this same bounded text recovery. npm package versions can be misidentified as email.
    if !(4..=4096).contains(&encoded.len()) || encoded.len() % 2 != 0 {
        return None;
    }
    let bytes = hex::decode(encoded).ok()?;
    let (&key, payload) = bytes.split_first()?;
    let decoded = String::from_utf8(payload.iter().map(|byte| byte ^ key).collect()).ok()?;
    (decoded.contains('@') && !decoded.chars().any(char::is_control)).then_some(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn protected(text: &str) -> String {
        let key = 0xa2;
        hex::encode(
            std::iter::once(key)
                .chain(text.bytes().map(|byte| byte ^ key))
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn cloudflare_code_keeps_package_version_without_links_or_private_markers() {
        let html = r#"<p>Install <code>npm install @duckdb/<a href="/cdn-cgi/l/email-protection" class="__cf_email__" data-cfemail="a2c6d7c1c9c6c08fd5c3d1cfe2938c91908c92">[email&#160;protected]</a></code>.</p>"#;
        let normalized = publisher_html(html);
        assert!(normalized.contains("<code>npm install @duckdb/duckdb-wasm@1.32.0</code>"));
        let markdown = crate::content::to_markdown(&normalized, None);
        assert_eq!(
            markdown.trim(),
            "Install `npm install @duckdb/duckdb-wasm@1.32.0`."
        );
        assert!(!markdown.contains('\u{e002}'));
    }

    #[test]
    fn protected_text_handles_spans_fragments_unicode_and_escaping() {
        for text in [
            "reader@example.com",
            "élève@example.com",
            "<tag>&\"@example.com",
        ] {
            let encoded = protected(text);
            for html in [
                format!(
                    "<span class=\"__cf_email__\" data-cfemail=\"{encoded}\">[email protected]</span>"
                ),
                format!(
                    "<a href=\"https://example.com/cdn-cgi/l/email-protection#{encoded}\">[email protected]</a>"
                ),
            ] {
                assert_eq!(publisher_html(&html), crate::content::escape_html(text));
            }
        }
    }

    #[test]
    fn malformed_or_unrelated_email_markup_is_preserved() {
        for encoded in ["", "aa", "aab", "not-hex", "00ff", "000a", "00616263"] {
            let html = format!("<a data-cfemail=\"{encoded}\">[email protected]</a>");
            assert_eq!(publisher_html(&html), html);
        }
        let oversized = format!("<span data-cfemail=\"{}\">keep</span>", "a2".repeat(8192));
        assert_eq!(publisher_html(&oversized), oversized);
        let oversized_wrapper = format!(
            "<span data-cfemail=\"{}\">{}</span>",
            protected("reader@example.com"),
            "x".repeat(17 * 1024)
        );
        assert_eq!(publisher_html(&oversized_wrapper), oversized_wrapper);
        for value in ["reader@example.com\n", "reader\u{0085}@example.com"] {
            let html = format!("<span data-cfemail=\"{}\">keep</span>", protected(value));
            assert_eq!(publisher_html(&html), html);
        }
        let html = "<a href=\"/documentation/email-protection#abcdef\">keep</a>";
        assert_eq!(publisher_html(html), html);
    }

    #[test]
    fn legacy_inline_code_survives_conversion_and_extraction() {
        let html = "<p>The call runs immediately within <TT class='function'>io_uring_enter()</TT>. Another <a href='https://example.com/manual'><tt>function()</tt></a> stays linked.</p>";
        let normalized = publisher_html(html);
        assert_eq!(publisher_html(&normalized), normalized);
        let markdown = crate::content::to_markdown(&normalized, None);
        assert!(markdown.contains("`io_uring_enter()`"), "{markdown}");
        assert!(
            markdown.contains("[`function()`](https://example.com/manual)"),
            "{markdown}"
        );
        assert!(publisher_html("<p>&lt;tt&gt; is literal</p>").contains("&lt;tt&gt;"));
        let page = format!(
            "<html><title>System calls</title><article>{html}<p>This explanatory paragraph supplies enough ordinary prose for article extraction to keep the example and its code semantics together.</p></article></html>"
        );
        let extracted = crate::content::extract::extract_article(
            &page,
            &url::Url::parse("https://example.com/article").unwrap(),
        )
        .unwrap();
        let markdown = crate::content::to_markdown(&extracted.html, None);
        assert!(markdown.contains("`io_uring_enter()`"), "{markdown}");
    }

    #[test]
    fn normalization_does_not_rewrite_comments_or_script_text() {
        let html = "<!-- <tt>comment</tt> --><script>const markup = '<tt>script</tt>';</script><style>/* <tt>style</tt> */</style><textarea><tt>literal</tt></textarea>";
        assert_eq!(publisher_html(html), html);
        for element in ["xmp", "iframe", "noembed", "noframes", "plaintext"] {
            let html = format!("<{element}><tt>literal</tt></{element}>");
            assert_eq!(publisher_html(&html), html);
        }
    }
}
