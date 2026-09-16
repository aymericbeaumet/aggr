//! Markdown cleanup shared by capture and build: boundary controls, the publisher metadata
//! Readability promotes to the front of an article (dates, bylines, the source's own name),
//! separator tidying, and the code-aware rewriting the other passes rely on.

use chrono::{DateTime, NaiveDate, Utc};

use super::render_markdown;
use super::strip::html_to_text;

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

pub(super) fn protect_markdown_code(
    markdown: &str,
    transform: impl FnOnce(&str) -> String,
) -> String {
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

/// Shared cleanup for every feed and extracted article, both on storage and when building older
/// archives. Restrict standalone controls to document boundaries, never code or body paragraphs.
pub fn strip_article_metadata(
    markdown: &str,
    published: Option<DateTime<Utc>>,
    source_slug: &str,
) -> String {
    let markdown = strip_boundary_controls(markdown);
    let markdown = strip_leading_metadata(&markdown, published, source_slug);
    let markdown = strip_boundary_controls(&markdown);
    // Separators are presentation, not content: tidying them here rather than only at capture
    // means an archive written by an older version reads correctly too. A body with no rule in it
    // is left byte-for-byte alone, so nothing is rewritten for the sake of whitespace.
    if markdown
        .lines()
        .any(|line| is_thematic_break(line, true) || line.trim_end().len() != line.len())
    {
        return protect_markdown_code(&markdown, tidy_markdown);
    }
    markdown
}

pub(super) fn is_accessibility_label(text: &str) -> bool {
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
        let lower = text.to_ascii_lowercase();
        lower.contains("comment")
            || lower.contains("opens in")
            || lower.contains("advertisement")
            || lower.contains("updated")
            || lower.contains("corrected")
            || lower.contains('|')
            || text.trim().chars().all(|ch| ch == '\\')
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
    if is_navigation_bar(node) {
        return true;
    }
    let Some((text, linked)) = boundary_paragraph_text(node) else {
        return false;
    };
    let text = text.trim();
    // A line break whose surrounding content is gone renders as a stray escape.
    if !text.is_empty() && text.chars().all(|ch| ch == '\\') {
        return true;
    }
    if is_accessibility_label(text) || is_update_notice(text) {
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

/// A row of links joined by pipes or bullets at a document boundary is the site's own footer or
/// navigation. Prose that happens to carry several links separates them with ordinary punctuation,
/// so the separators are what distinguishes the two.
fn is_navigation_bar<'a>(node: &'a comrak::nodes::AstNode<'a>) -> bool {
    use comrak::nodes::NodeValue;
    if !matches!(node.data.borrow().value, NodeValue::Paragraph) {
        return false;
    }
    let (mut links, mut between) = (0, String::new());
    if !navigation_shape(node, &mut links, &mut between) || links < 3 {
        return false;
    }
    let separators = between
        .chars()
        .filter(|ch| matches!(ch, '|' | '·' | '•' | '›' | '»'))
        .count();
    let residual = between.chars().filter(|ch| ch.is_alphanumeric()).count();
    separators + 1 >= links && residual <= 24
}

/// `false` when the paragraph holds anything but links, inline emphasis and the text between them.
fn navigation_shape<'a>(
    node: &'a comrak::nodes::AstNode<'a>,
    links: &mut usize,
    between: &mut String,
) -> bool {
    use comrak::nodes::NodeValue;
    node.children()
        .all(|child| match &child.data.borrow().value {
            NodeValue::Link(_) => {
                *links += 1;
                true
            }
            NodeValue::Text(value) => {
                between.push_str(value);
                true
            }
            NodeValue::SoftBreak | NodeValue::LineBreak => {
                between.push(' ');
                true
            }
            NodeValue::Emph | NodeValue::Strong => navigation_shape(child, links, between),
            _ => false,
        })
}

/// A standalone editorial note such as `This article was updated on 08 September 2026.` at a
/// document boundary. Publication and update times already appear in the item metadata.
fn is_update_notice(text: &str) -> bool {
    let text = text.trim().trim_end_matches(['.', '!']).trim();
    if text.len() > 160 || !text.bytes().any(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let rest = [
        "this article",
        "this post",
        "this story",
        "this piece",
        "article",
        "post",
    ]
    .into_iter()
    .find_map(|subject| text.strip_prefix(subject))
    .map(str::trim_start)
    .and_then(|rest| {
        ["was", "has been"]
            .into_iter()
            .find_map(|verb| rest.strip_prefix(verb))
    })
    .map(str::trim_start)
    .unwrap_or(text);
    let rest = rest.strip_prefix("last ").unwrap_or(rest);
    let rest = ["updated", "corrected", "revised", "amended"]
        .into_iter()
        .find_map(|verb| rest.strip_prefix(verb));
    let Some(rest) = rest else {
        return false;
    };
    let rest = rest.trim_start_matches([':', ' ']);
    let rest = ["on ", "at "]
        .into_iter()
        .find_map(|preposition| rest.strip_prefix(preposition))
        .unwrap_or(rest);
    // What remains must be a date/time, optionally with a short reason, never a full sentence.
    rest.split_whitespace().count() <= 12
        && rest
            .split_whitespace()
            .next()
            .is_some_and(|word| word.bytes().any(|byte| byte.is_ascii_digit()) || word.len() >= 3)
}

/// What a leading block turned out to be, when it is metadata rather than the article's opening.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LeadingMetadata {
    /// A publication date, a relative timestamp, or a compact name/date byline.
    Stamp,
    /// The source's own name, promoted from an accessibility label.
    SourceName,
    /// A name or role line following one of the above.
    Byline,
}

/// Remove the metadata lines readability promoted to the front of the article: a publication date
/// (including a short suffix such as `- Link Blog`), a relative timestamp, the source's own name,
/// and the byline that follows them. A standalone pipe after a date is its orphaned separator.
/// Normal prose containing a date remains intact, and a byline only goes with a stamp it follows.
pub fn strip_leading_metadata(
    markdown: &str,
    published: Option<DateTime<Utc>>,
    source_slug: &str,
) -> String {
    // Stored bodies can begin with a blank line; the first block is still the first block.
    let mut rest = markdown.trim_start_matches('\n');
    let mut bylines = 0;
    let mut previous = None;
    while let Some((first, tail)) = rest.split_once("\n\n") {
        if first.lines().count() != 1 {
            break;
        }
        let tail = tail.trim_start_matches('\n');
        let plain = html_to_text(&render_markdown(first));
        let Some(kind) = leading_metadata(first, &plain, published, source_slug, tail, previous)
        else {
            break;
        };
        if kind == LeadingMetadata::Byline {
            bylines += 1;
            if bylines > 3 {
                break;
            }
        }
        previous = Some(kind);
        rest = tail;
        if kind == LeadingMetadata::Stamp {
            // The separator that sat between the date and whatever followed it is now orphaned.
            if let Some((separator, after)) = rest.split_once("\n\n")
                && is_lone_separator(separator)
            {
                rest = after.trim_start_matches('\n');
            }
        }
    }
    rest.to_string()
}

fn leading_metadata(
    block: &str,
    plain: &str,
    published: Option<DateTime<Utc>>,
    source_slug: &str,
    tail: &str,
    previous: Option<LeadingMetadata>,
) -> Option<LeadingMetadata> {
    if plain.chars().count() <= 80 && slug::slugify(plain.trim()) == source_slug {
        return Some(LeadingMetadata::SourceName);
    }
    if is_relative_timestamp(plain) {
        return Some(LeadingMetadata::Stamp);
    }
    let dated = date_prefixes(plain)
        .filter_map(parse_date_only)
        .any(|candidate| {
            candidate.labelled
                || published.is_some_and(|published| {
                    candidate
                        .date
                        .signed_duration_since(published.date_naive())
                        .num_days()
                        .unsigned_abs()
                        <= 1
                })
                // A date the item does not share is still metadata when the publisher left its
                // own separator behind it; prose never opens that way.
                || tail
                    .split_once("\n\n")
                    .is_some_and(|(separator, _)| is_lone_separator(separator))
        })
        || published
            .is_some_and(|published| matching_leading_byline(block, plain, published.date_naive()));
    if dated {
        return Some(LeadingMetadata::Stamp);
    }
    let after_byline = previous == Some(LeadingMetadata::Byline);
    (previous.is_some() && is_byline_line(plain, after_byline)).then_some(LeadingMetadata::Byline)
}

/// A one- or two-character separator left over from a metadata row. Longer runs are rules.
fn is_lone_separator(block: &str) -> bool {
    let block = block.trim();
    !block.is_empty()
        && block.chars().count() <= 2
        && block
            .chars()
            .all(|ch| matches!(ch, '|' | '·' | '•' | '—' | '–' | '-'))
}

/// `15 minutes ago`, `just now`: true when the page was read, meaningless in an archive.
fn is_relative_timestamp(text: &str) -> bool {
    let text = text.trim().to_ascii_lowercase();
    if matches!(text.as_str(), "just now" | "yesterday" | "today") {
        return true;
    }
    let Some(rest) = text.strip_suffix(" ago") else {
        return false;
    };
    let mut parts = rest.split_whitespace();
    let (Some(count), Some(unit), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    (count.bytes().all(|byte| byte.is_ascii_digit())
        || matches!(count, "a" | "an" | "one" | "two" | "three" | "few"))
        && matches!(
            unit.trim_end_matches('s'),
            "second" | "minute" | "min" | "hour" | "hr" | "day" | "week" | "month" | "year"
        )
}

/// `By Amy Walker`, `Amy Walker and`, `Nick Beake, Europe correspondent`: a name that announces
/// itself as a byline. A bare Title Case line is indistinguishable from a kicker, so a name with a
/// role only counts while a byline is already being read.
fn is_byline_line(text: &str, after_byline: bool) -> bool {
    let text = text.trim();
    if text.is_empty()
        || text.chars().count() > 60
        || text.ends_with(['.', '!', '?', ':', ';', ','])
        || text.split_whitespace().count() > 8
    {
        return false;
    }
    let name_like = |value: &str| {
        let words: Vec<&str> = value.split_whitespace().collect();
        (1..=4).contains(&words.len())
            && words.iter().all(|word| {
                word.chars().next().is_some_and(char::is_uppercase)
                    && word
                        .chars()
                        .all(|ch| ch.is_alphabetic() || matches!(ch, '\'' | '’' | '-' | '.'))
            })
    };
    if let Some(name) = text.strip_prefix("By ") {
        return name_like(name.split(',').next().unwrap_or(name));
    }
    if let Some(name) = text
        .strip_suffix(" and")
        .or_else(|| text.strip_suffix(" &"))
    {
        return name_like(name);
    }
    after_byline
        && text.split_once(',').is_some_and(|(name, role)| {
            name_like(name) && (1..=5).contains(&role.split_whitespace().count())
        })
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

/// Bylines that introduce the date on a metadata line. They name the article's own date, which a
/// feed aggregating submissions (Hacker News, Lobsters) does not share, so a labelled line is
/// dropped on its own evidence instead of being matched against the item's published date.
const DATE_LABELS: [&str; 5] = [
    "written on ",
    "published on ",
    "posted on ",
    "last updated on ",
    "updated on ",
];

struct LeadingDate {
    date: NaiveDate,
    labelled: bool,
}

fn parse_date_only(raw: &str) -> Option<LeadingDate> {
    let value = raw.trim().trim_matches(['*', '_']).trim();
    let lowercase = value.to_ascii_lowercase();
    let (value, labelled) = DATE_LABELS
        .iter()
        .find_map(|label| {
            lowercase
                .starts_with(label)
                .then(|| (&value[label.len()..], true))
        })
        .unwrap_or((value, false));
    let value = without_ordinal_suffixes(value);
    // `%Y%m%d` is a compact permalink date; it only ever strips a line that matches the item's
    // own publication date, so an unrelated eight-digit number stays put.
    [
        "%Y-%m-%d",
        "%Y%m%d",
        "%Y/%m/%d",
        "%d %B %Y",
        "%d %b %Y",
        "%B %d, %Y",
        "%b %d, %Y",
    ]
    .iter()
    .find_map(|format| NaiveDate::parse_from_str(value.trim(), format).ok())
    .map(|date| LeadingDate { date, labelled })
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

pub(super) fn tidy_markdown(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut blank_run = 0;
    let mut pending_break = false;
    for line in markdown.lines().map(str::trim_end) {
        if line.is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
        } else if is_thematic_break(line, blank_run > 0 || out.is_empty()) {
            // Empty layout sections leave their separators stacked; one rule says the same thing.
            if pending_break {
                continue;
            }
            pending_break = true;
            blank_run = 0;
        } else {
            pending_break = false;
            blank_run = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    // A separator with nothing after it once divided the article from a section that is now gone.
    let mut trimmed = out.trim();
    while let Some((rest, last)) = trimmed.rsplit_once('\n') {
        if !is_thematic_break(last, rest.is_empty() || rest.ends_with('\n')) {
            break;
        }
        trimmed = rest.trim_end();
    }
    let trimmed = trimmed.trim().to_string();
    if trimmed.is_empty() {
        trimmed
    } else {
        trimmed + "\n"
    }
}

/// A `---` run is only a thematic break when nothing above it could make it a setext heading.
fn is_thematic_break(line: &str, starts_block: bool) -> bool {
    let line = line.trim();
    let Some(marker) = line.chars().next().filter(|c| matches!(c, '*' | '-' | '_')) else {
        return false;
    };
    if marker == '-' && !starts_block {
        return false;
    }
    line.chars().filter(|c| *c == marker).count() >= 3
        && line.chars().all(|c| c == marker || c == ' ' || c == '\t')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::to_markdown;

    #[test]
    fn boundary_update_notices_are_removed_but_prose_mentions_stay() {
        let body = "Opening paragraph.\n\nMore reporting follows here.\n\n*This article was updated on 08 September 2026.*\n";
        assert_eq!(
            strip_article_metadata(body, None, "spectrum"),
            "Opening paragraph.\n\nMore reporting follows here.\n"
        );
        for notice in [
            "Updated: 3 March 2026",
            "Last updated on March 3, 2026 with new figures.",
            "This post has been corrected on 2026-03-03.",
            "**Updated 03/03/2026 10:15 UTC**",
        ] {
            let body = format!("{notice}\n\nOpening paragraph.\n");
            assert_eq!(
                strip_article_metadata(&body, None, "spectrum"),
                "Opening paragraph.\n",
                "{notice}"
            );
        }
        for body in [
            "Opening.\n\nThe database was updated on 08 September 2026 and nothing broke, which surprised the whole team on call.\n",
            "Opening.\n\nWe updated 40 servers.\n",
            "This article was updated on 08 September 2026 because the earlier version misstated the number of vehicles and the agency has since published corrected totals.\n\nOpening.\n",
            "Opening.\n\nUpdated thinking\n",
        ] {
            assert_eq!(
                strip_article_metadata(body, None, "spectrum"),
                body,
                "{body}"
            );
        }
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
    fn boundary_navigation_bars_and_orphaned_line_breaks_are_removed() {
        // marc.info closes every archived message with a `<br>` and its own footer bar.
        let footer = "[Configure](https://marc.info/?q=configure) | [About](https://marc.info/?q=about) | [News](https://marc.info/?q=news) | [Add a list](mailto:webguy@marc.info) | Sponsored by [KoreLogic](http://www.korelogic.com/)";
        assert_eq!(
            strip_article_metadata(
                &format!("Actual article.\n\n\\\n\n{footer}\n"),
                None,
                "an-archive"
            ),
            "Actual article.\n"
        );
        // Prose that merely carries several links keeps its sentence.
        let prose = "See [one](https://example.com/1), [two](https://example.com/2) and [three](https://example.com/3) for the details.";
        assert_eq!(
            strip_article_metadata(&format!("Actual article.\n\n{prose}\n"), None, "a-blog"),
            format!("Actual article.\n\n{prose}\n")
        );
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
        // A date the item does not share is still metadata when its separator sits behind it,
        // but on its own it is prose.
        assert_eq!(
            strip_leading_metadata(
                "Sep 02, 2025\n\n|\n\nBody.\n",
                Some(published),
                "blog-google"
            ),
            "Body.\n"
        );
        let unrelated_date = "Sep 02, 2025\n\nBody.\n";
        assert_eq!(
            strip_leading_metadata(unrelated_date, Some(published), "blog-google"),
            unrelated_date
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
    fn strips_a_labelled_byline_even_when_the_feed_dates_the_submission() {
        use chrono::{TimeZone as _, Utc};

        // Aggregator feeds publish the submission, days after the article itself.
        let submitted = Utc.with_ymd_and_hms(2026, 9, 11, 6, 30, 0).unwrap();
        for byline in [
            "written on September 07, 2026",
            "Written on 7 September 2026",
            "*Published on 2026-09-07*",
            "Posted on Sep 7, 2026",
            "Last updated on 7th September 2026",
        ] {
            assert_eq!(
                strip_leading_metadata(
                    &format!("{byline}\n\nThe actual opening.\n"),
                    None,
                    "hnrss"
                ),
                "The actual opening.\n",
                "{byline}"
            );
            assert_eq!(
                strip_leading_metadata(
                    &format!("{byline}\n\nThe actual opening.\n"),
                    Some(submitted),
                    "hnrss"
                ),
                "The actual opening.\n",
                "{byline}"
            );
        }
        for body in [
            // A label without a parseable date, and prose that merely starts with one.
            "Written on a rainy afternoon\n\nBody.\n",
            "Written on September 07, 2026 the draft finally made sense.\n\nBody.\n",
        ] {
            assert_eq!(strip_leading_metadata(body, Some(submitted), "hnrss"), body);
        }
        // A bare date still needs the item's own published date to back it up.
        let bare = "September 07, 2026\n\nBody.\n";
        assert_eq!(strip_leading_metadata(bare, Some(submitted), "hnrss"), bare);
        assert_eq!(strip_leading_metadata(bare, None, "hnrss"), bare);
    }

    #[test]
    /// Each case is a real opening kept by Readability: blog.google's date/separator hero row
    /// and bbc.com's relative timestamp with its byline underneath.
    fn leading_publisher_metadata_is_removed_block_by_block() {
        let published = DateTime::parse_from_rfc3339("2026-09-16T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        for (body, expected) in [
            // A stored body may begin with a blank line; the first block is still the first block.
            ("\nSep 15, 2026\n\n|\n\nThe deck.\n", "The deck.\n"),
            // A date the item does not share, with the publisher's separator still behind it.
            ("May 19, 2026\n\n|\n\nThe deck.\n", "The deck.\n"),
            // A relative timestamp and the byline under it.
            (
                "15 minutes ago\n\nAmy Walker and\n\nNick Beake, Europe correspondent\n\nReal body.\n",
                "Real body.\n",
            ),
            ("Just now\n\nBy Amy Walker\n\nReal body.\n", "Real body.\n"),
        ] {
            assert_eq!(
                strip_article_metadata(body, Some(published), "blog-google"),
                expected,
                "{body:?}"
            );
        }
    }

    #[test]
    fn leading_prose_is_never_mistaken_for_publisher_metadata() {
        let published = DateTime::parse_from_rfc3339("2026-09-16T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        for body in [
            // An unrelated date with no separator behind it stays.
            "May 19, 2026\n\nThe deck.\n",
            // A name-shaped line with no stamp above it is the article's opening.
            "Amy Walker and\n\nReal body.\n",
            // A thematic break is not an orphaned separator.
            "Sep 15, 2026\n\n---\n\nThe deck.\n",
            // A Title Case kicker under a date is not a byline.
            "Sep 15, 2026\n\nGoogle Cloud Next\n\nReal body.\n",
            // A sentence fragment under a date is prose.
            "Sep 15, 2026\n\nwe shipped something today\n\nReal body.\n",
        ] {
            let kept = strip_article_metadata(body, Some(published), "blog-google");
            let second = body.trim_start_matches('\n').split("\n\n").nth(1).unwrap();
            assert!(kept.contains(second), "{body:?} -> {kept:?}");
        }
    }

    #[test]
    fn a_compact_permalink_date_is_metadata_only_when_the_item_shares_it() {
        // attainablefelicity.mattkirkland.com opens with `<time>20260915</time>`.
        let published = DateTime::parse_from_rfc3339("2026-09-15T18:45:31Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            strip_article_metadata(
                "20260915\n\n## Chop up your books\n\nThis is my appeal.\n",
                Some(published),
                "hnrss-org-frontpage"
            ),
            "## Chop up your books\n\nThis is my appeal.\n"
        );
        // An eight-digit number that is not this item's date stays where it is.
        let unrelated = "20190104\n\nThe build number above matters.\n";
        assert_eq!(
            strip_article_metadata(unrelated, Some(published), "hnrss-org-frontpage"),
            unrelated
        );
    }

    #[test]
    fn stacked_separators_are_tidied_in_archives_written_before_the_rule() {
        // Capture-time tidying cannot reach a body already on the branch, so the build tidies too.
        assert_eq!(
            strip_article_metadata("Lead.\n\n* * *\n\n* * *\n\nBody.\n\n* * *\n", None, "blog"),
            "Lead.\n\n* * *\n\nBody.\n"
        );
        // A rule inside a code block is code.
        let fenced = "```\n* * *\n\n* * *\n```\n";
        assert_eq!(strip_article_metadata(fenced, None, "blog"), fenced);
    }
}
