//! The hand-written tag scanner every other pass builds on. It finds tags, their attributes
//! and their matching close tags without parsing the document, so a pass can copy the HTML it
//! does not touch through byte for byte.

use super::escape_html;

/// Raw-text elements: their content ends at the first matching close tag, no nesting.
pub(super) const RAW_TEXT_ELEMENTS: &[&str] = &["script", "style"];

/// Elements that separate words when flattened to text; inline tags do not.
pub(super) const BLOCK_ELEMENTS: &[&str] = &[
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

/// Length of the comment starting `s`, if any. An unterminated comment swallows the rest of the
/// document, as in HTML itself.
pub(super) fn comment_end(s: &str) -> Option<usize> {
    let body = s.strip_prefix("<!--")?;
    Some(body.find("-->").map_or(s.len(), |end| 4 + end + 3))
}

pub(super) struct Tag {
    pub(super) name: String,
    pub(super) closing: bool,
    pub(super) self_closing: bool,
    /// Byte offset just past `>`, `None` when the input ends inside the tag.
    pub(super) end: Option<usize>,
}

/// Parse `<name …>` / `</name …>` at the start of `s`. `None` when `<` does not start a tag.
pub(super) fn parse_tag(s: &str) -> Option<Tag> {
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'<') {
        return None;
    }
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

pub(super) fn is_name_byte(b: u8) -> bool {
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
pub(super) fn skip_element(raw: &str, from: usize, name: &str) -> usize {
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

pub(super) fn set_attribute(tag: &str, name: &str, value: &str) -> String {
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

pub(super) fn opening_element_count(html: &str, name: &str) -> usize {
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
pub(super) fn element_bounds(
    html: &str,
    start: usize,
    name: &str,
) -> Option<(usize, usize, usize)> {
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

pub(super) fn skip_html_whitespace(html: &str, mut position: usize) -> usize {
    while html
        .as_bytes()
        .get(position)
        .is_some_and(u8::is_ascii_whitespace)
    {
        position += 1;
    }
    position
}

pub(super) fn attribute_value<'a>(tag: &'a str, wanted: &str) -> Option<&'a str> {
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

/// The first character of text in `html`, looking past any tags in front of it.
fn first_text_char(html: &str) -> Option<char> {
    let mut rest = html;
    while let Some(after) = rest.strip_prefix('<') {
        rest = &after[after.find('>')? + 1..];
    }
    rest.chars().next()
}

/// The last character of text in `html`, looking past any tags behind it.
fn last_text_char(html: &str) -> Option<char> {
    let mut rest = html;
    while let Some(before) = rest.strip_suffix('>') {
        rest = &before[..before.rfind('<')?];
    }
    rest.chars().next_back()
}

/// `true` when removing whatever sat between `before` and `after` would run two words together
/// (`Bitwarden<svg/></a>or`): the page relied on the removed element for visual separation.
pub(super) fn fuses_words(before: &str, after: &str) -> bool {
    last_text_char(before).is_some_and(char::is_alphanumeric)
        && first_text_char(after).is_some_and(char::is_alphanumeric)
}

/// Copy the closing tags that immediately follow `position` to `out`, then separate the words a
/// removed element used to keep apart. Returns the position after the copied tags.
pub(super) fn close_removed_element(html: &str, mut position: usize, out: &mut String) -> usize {
    while html[position..].starts_with("</")
        && let Some(tag) = parse_tag(&html[position..])
        && tag.closing
        && let Some(end) = tag.end
    {
        out.push_str(&html[position..position + end]);
        position += end;
    }
    if fuses_words(out, &html[position..]) {
        out.push(' ');
    }
    position
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_parser_requires_an_opening_angle_bracket() {
        for prose in [
            " a large portion <i>later</i>",
            "another <span>phrase</span>",
            "",
            "a",
        ] {
            assert!(parse_tag(prose).is_none(), "{prose:?}");
        }
        let opening = parse_tag("<a href=\"/source\">label</a>").unwrap();
        assert_eq!(opening.name, "a");
        assert!(!opening.closing);
        assert_eq!(opening.end, Some(18));
        let closing = parse_tag("</A>").unwrap();
        assert_eq!(closing.name, "a");
        assert!(closing.closing);
    }
}
