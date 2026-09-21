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
    title: &str,
    published: Option<DateTime<Utc>>,
    source_slug: &str,
) -> String {
    let markdown = strip_boundary_controls(markdown);
    let markdown = strip_leading_metadata(&markdown, title, published, source_slug);
    let markdown = strip_boundary_controls(&markdown);
    let markdown = strip_separator_rows(&markdown);
    let markdown = decorative_rules_to_breaks(&markdown);
    // Separators are presentation, not content: tidying them here rather than only at capture
    // means an archive written by an older version reads correctly too. A body with no rule in it
    // is left byte-for-byte alone, so nothing is rewritten for the sake of whitespace.
    if markdown.lines().any(|line| {
        is_thematic_break(line, true)
            || line.trim_end().len() != line.len()
            || line.trim_end().ends_with('\\')
    }) {
        return protect_markdown_code(&markdown, tidy_markdown);
    }
    markdown
}

/// Clean publisher metadata and move explicit boundary hashtags into article labels.
pub fn normalize_article_body(
    markdown: &str,
    title: &str,
    published: Option<DateTime<Utc>>,
    source_slug: &str,
) -> (String, Vec<String>) {
    let mut body = crate::threads::format_archived_x_embeds(markdown);
    let mut labels = Vec::new();
    loop {
        let (cleaned, found) = boundary_hashtags(&body);
        labels.extend(found);
        let cleaned = strip_article_metadata(&cleaned, title, published, source_slug);
        if cleaned == body {
            return (body, crate::model::normalize_labels(labels));
        }
        body = cleaned;
    }
}

fn boundary_hashtags(markdown: &str) -> (String, Vec<String>) {
    if !markdown.contains('#') {
        return (markdown.to_string(), Vec::new());
    }
    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, markdown, &comrak::Options::default());
    let blocks = root.children().collect::<Vec<_>>();
    let mut first = 0;
    let mut last = blocks.len();
    let mut labels = Vec::new();
    while first < last {
        let Some(tags) = hashtag_paragraph(blocks[first]) else {
            break;
        };
        labels.extend(tags);
        first += 1;
    }
    while last > first {
        let Some(tags) = hashtag_paragraph(blocks[last - 1]) else {
            break;
        };
        labels.extend(tags);
        last -= 1;
    }
    if labels.is_empty() {
        return (markdown.to_string(), labels);
    }
    if first == last {
        return (String::new(), labels);
    }
    let mut lines = vec![0];
    lines.extend(markdown.match_indices('\n').map(|(index, _)| index + 1));
    let start = if first > 0 {
        lines[blocks[first].data.borrow().sourcepos.start.line - 1]
    } else {
        0
    };
    let end = if last < blocks.len() {
        lines[blocks[last].data.borrow().sourcepos.start.line - 1]
    } else {
        markdown.len()
    };
    let kept = &markdown[start..end];
    let body = if last < blocks.len() {
        format!("{}\n", kept.trim_end_matches('\n'))
    } else {
        kept.to_string()
    };
    (body, labels)
}

fn hashtag_paragraph<'a>(node: &'a comrak::nodes::AstNode<'a>) -> Option<Vec<String>> {
    use comrak::nodes::NodeValue;
    if !matches!(node.data.borrow().value, NodeValue::Paragraph) {
        return None;
    }
    let mut text = String::new();
    for child in node.descendants() {
        match &child.data.borrow().value {
            NodeValue::Text(value) => text.push_str(value),
            NodeValue::SoftBreak | NodeValue::LineBreak => text.push(' '),
            NodeValue::Paragraph | NodeValue::Link(_) | NodeValue::Emph | NodeValue::Strong => {}
            _ => return None,
        }
    }
    let tags = text
        .split_whitespace()
        .map(|token| {
            let tag = token.strip_prefix('#')?;
            (!tag.is_empty()
                && tag.chars().count() <= 64
                && tag.chars().any(char::is_alphanumeric)
                && tag
                    .chars()
                    .all(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '-')))
            .then(|| tag.to_string())
        })
        .collect::<Option<Vec<_>>>()?;
    (!tags.is_empty()).then_some(tags)
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
    let mut options = comrak::Options::default();
    options.extension.table = true;
    let root = comrak::parse_document(&arena, markdown, &options);
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
        .take_while(|node| is_boundary_control(node) || is_lwn_index_table(node))
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

/// LWN ends articles with a category/topic index. Match its complete navigation shape and host,
/// not merely the heading: articles can legitimately discuss indexes or quote the same table.
fn is_lwn_index_table<'a>(node: &'a comrak::nodes::AstNode<'a>) -> bool {
    use comrak::nodes::NodeValue;
    if !matches!(node.data.borrow().value, NodeValue::Table(_)) {
        return false;
    }
    let rows = node.children().collect::<Vec<_>>();
    let Some(header) = rows.first() else {
        return false;
    };
    let cells = header.children().collect::<Vec<_>>();
    if rows.len() < 2 || cells.len() != 2 || cells[1].first_child().is_some() {
        return false;
    }
    let label = cells[0].children().collect::<Vec<_>>();
    if label.len() != 1
        || !matches!(&label[0].data.borrow().value, NodeValue::Text(text) if text.trim() == "Index entries for this article")
    {
        return false;
    }
    rows[1..].iter().all(|row| {
        let cells = row.children().collect::<Vec<_>>();
        if cells.len() != 2 {
            return false;
        }
        let Some(category) = lwn_index_cell_url(cells[0]) else {
            return false;
        };
        let Some(topic) = lwn_index_cell_url(cells[1]) else {
            return false;
        };
        category.fragment().is_none()
            && topic
                .fragment()
                .is_some_and(|fragment| !fragment.is_empty())
            && category.path() == topic.path()
    })
}

fn lwn_index_cell_url<'a>(cell: &'a comrak::nodes::AstNode<'a>) -> Option<url::Url> {
    use comrak::nodes::NodeValue;
    let children = cell.children().collect::<Vec<_>>();
    if children.len() != 1 {
        return None;
    }
    let NodeValue::Link(link) = &children[0].data.borrow().value else {
        return None;
    };
    // A navigation cell contains only its linked label; embedded code or images may be article data.
    if !children[0]
        .children()
        .all(|node| matches!(&node.data.borrow().value, NodeValue::Text(_)))
    {
        return None;
    }
    let url = url::Url::parse(&link.url).ok()?;
    let segments = url.path_segments()?.collect::<Vec<_>>();
    (matches!(url.scheme(), "http" | "https")
        && url.host_str() == Some("lwn.net")
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url.query().is_none()
        && segments.len() == 2
        && !segments[0].is_empty()
        && segments[1] == "Index")
        .then_some(url)
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
    /// A relative timestamp (`2 hours ago`): the page is newsroom chrome around the article.
    Recent,
    /// The source's own name, promoted from an accessibility label.
    SourceName,
    /// A name or role line following one of the above.
    Byline,
    /// The page's own heading repeating the item title, above the byline it introduces.
    Title,
    /// A category label or eyebrow above the opening: a lone short link, or a few words that
    /// introduce the heading right after them.
    Kicker,
}

/// Remove the metadata lines readability promoted to the front of the article: a heading that
/// repeats the item title, a publication date (including a short suffix such as `- Link Blog`), a
/// relative timestamp, the source's own name, and the byline that follows them. A standalone pipe
/// after a date is its orphaned separator. Normal prose containing a date remains intact, and a
/// byline only goes with a title or stamp it follows.
pub fn strip_leading_metadata(
    markdown: &str,
    title: &str,
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
        let Some(kind) =
            leading_metadata(first, &plain, title, published, source_slug, tail, previous)
        else {
            if let Some(without_stamp) = stamp_behind_lede(first, &plain, tail) {
                return without_stamp;
            }
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
        if matches!(kind, LeadingMetadata::Stamp | LeadingMetadata::Recent) {
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
    title: &str,
    published: Option<DateTime<Utc>>,
    source_slug: &str,
    tail: &str,
    previous: Option<LeadingMetadata>,
) -> Option<LeadingMetadata> {
    if matches!(previous, None | Some(LeadingMetadata::Kicker))
        && (is_title_heading(block, plain, title)
            || (is_title_line(block, plain, title) && stamp_follows(tail)))
    {
        return Some(LeadingMetadata::Title);
    }
    if matches!(previous, None | Some(LeadingMetadata::Kicker)) && is_kicker(block, plain, tail) {
        return Some(LeadingMetadata::Kicker);
    }
    // `Carlo Piovesan, Geertjan Wielenga` over `2026-09-18 | 9 min`: the authors line of a
    // metadata row is the byline, whatever follows it.
    if matches!(
        previous,
        None | Some(LeadingMetadata::Kicker | LeadingMetadata::Title)
    ) && !block.contains("](")
        && is_name_list(plain)
        && stamp_follows(tail)
    {
        return Some(LeadingMetadata::Byline);
    }
    // `September 13, 2026 11 min read`, `11 min read`: a reading-time estimate only ever sits in
    // the page's own metadata row, so whatever date it follows is metadata too.
    if let Some(rest) = without_read_time(plain)
        && (rest.is_empty() || date_prefixes(rest).any(|prefix| parse_date_only(prefix).is_some()))
    {
        return Some(LeadingMetadata::Stamp);
    }
    if plain.chars().count() <= 80 && slug::slugify(plain.trim()) == source_slug {
        return Some(LeadingMetadata::SourceName);
    }
    if is_relative_timestamp(plain) {
        return Some(LeadingMetadata::Recent);
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
    let after_recent = previous == Some(LeadingMetadata::Recent);
    (previous.is_some() && is_byline_line(plain, after_byline, after_recent))
        .then_some(LeadingMetadata::Byline)
}

/// A kicker above the opening: the category link a publisher sets over its title (`Newsletter`,
/// `Current Linguistics`), or the eyebrow a landing page puts right before its heading (`A live,
/// local experiment`). Both are a few capitalised words with no sentence punctuation; prose that
/// opens an article is longer, or ends a sentence, or does not introduce a heading.
fn is_kicker(block: &str, plain: &str, tail: &str) -> bool {
    let text = plain.trim();
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() || text.ends_with(['.', '!', '?', ':', ';', ',']) {
        return false;
    }
    let block = block.trim();
    let lone_link = block.starts_with('[')
        && block.ends_with(')')
        && block.matches("](").count() == 1
        && !block[1..].contains('[');
    if lone_link {
        return words.len() <= 3
            && words.iter().all(|word| {
                word.chars().next().is_some_and(char::is_uppercase)
                    && word
                        .chars()
                        .all(|ch| ch.is_alphabetic() || matches!(ch, '-' | '&' | '/' | '’' | '\''))
            });
    }
    let heading_follows = tail
        .trim_start()
        .strip_prefix('#')
        .is_some_and(|rest| rest.trim_start_matches('#').starts_with(' '));
    heading_follows
        && words.len() <= 6
        && !block.contains("](")
        && !block.starts_with('#')
        && text.chars().next().is_some_and(char::is_uppercase)
}

/// The page's own `<h1>` (or a demoted heading) repeating the item title: the reader already
/// shows the title above the body, so the heading is a duplicate. Case, curly quotes, trailing
/// punctuation and a `Title:` label (arxiv.org) do not make it a different title.
fn is_title_heading(block: &str, plain: &str, title: &str) -> bool {
    let marks = block
        .trim_start()
        .bytes()
        .take_while(|byte| *byte == b'#')
        .count();
    if !(1..=6).contains(&marks) || !block.trim_start()[marks..].starts_with(' ') {
        return false;
    }
    let normalize = |text: &str| {
        let text = text
            .replace(['\u{2019}', '\u{2018}'], "'")
            .replace(['\u{201c}', '\u{201d}'], "\"")
            .to_lowercase();
        let text = text
            .trim()
            .trim_end_matches(['.', ':', '!', '?', '\u{2026}']);
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    };
    let expected = normalize(title);
    if expected.is_empty() {
        return false;
    }
    let heading = normalize(plain);
    same_title(&heading, &expected)
        || heading
            .strip_prefix("title:")
            .is_some_and(|rest| same_title(rest.trim(), &expected))
}

/// The page's own title as a plain paragraph (a `<header>` that styles a `<p>` as the heading):
/// the same duplicate as a heading, minus the marks.
fn is_title_line(block: &str, plain: &str, title: &str) -> bool {
    let block = block.trim_start();
    if block.starts_with(['#', '-', '*', '>', '|', '`', '!', '['])
        || block.starts_with(|ch: char| ch.is_ascii_digit())
    {
        return false;
    }
    let normalize = |text: &str| {
        let text = text
            .replace(['\u{2019}', '\u{2018}'], "'")
            .replace(['\u{201c}', '\u{201d}'], "\"")
            .to_lowercase();
        let text = text
            .trim()
            .trim_end_matches(['.', ':', '!', '?', '\u{2026}']);
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    };
    let expected = normalize(title);
    !expected.is_empty() && same_title(&normalize(plain), &expected)
}

/// Whether the block after a title paragraph is the page's date, reading time or timestamp: a
/// title styled as a paragraph inside a `<header>` sits above those, while an article whose
/// opening sentence repeats its title goes straight on with prose.
fn stamp_follows(tail: &str) -> bool {
    let next = tail.split("\n\n").next().unwrap_or_default();
    if next.lines().count() != 1 {
        return false;
    }
    let plain = html_to_text(&render_markdown(next));
    is_relative_timestamp(&plain)
        || without_read_time(&plain).is_some()
        || date_prefixes(&plain).any(|prefix| parse_date_only(prefix).is_some())
}

/// Two normalised titles that name the same article. An aggregator (Hacker News) shortens words
/// when it edits a title (`repositories` → `repos`), so a word that is a prefix of its partner,
/// three characters or longer, still matches.
fn same_title(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (a, b): (Vec<&str>, Vec<&str>) = (
        a.split_whitespace().collect(),
        b.split_whitespace().collect(),
    );
    a.len() == b.len()
        && !a.is_empty()
        && a.iter().zip(&b).all(|(x, y)| {
            x == y || (x.len().min(y.len()) >= 3 && (x.starts_with(y) || y.starts_with(x)))
        })
}

/// `plain` without a trailing reading-time estimate (`11 min read`, `5-minute read`,
/// `3 mins`), or `None` when it has none. What remains is trimmed of separators.
fn without_read_time(plain: &str) -> Option<&str> {
    let text = plain.trim().trim_end_matches('.');
    let lower = text.to_ascii_lowercase();
    let suffix = [
        " min read",
        " mins read",
        " minute read",
        " minutes read",
        "-minute read",
        " min",
        " mins",
    ]
    .into_iter()
    .find(|suffix| lower.ends_with(suffix))?;
    let before = text[..text.len() - suffix.len()].trim_end();
    let digits = before.rsplit([' ', '\u{a0}']).next().unwrap_or(before);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(
        before[..before.len() - digits.len()]
            .trim_end()
            .trim_end_matches(['·', '•', '|', '-', '—', '–', ','])
            .trim_end(),
    )
}

/// A paragraph that is nothing but a row of separators (`/ / / /`) once the controls between them
/// are gone: the tabs of an install box, buttons that Readability drops. Rules made of `*`, `-` or
/// `_` are thematic breaks and stay, and a single stray glyph is the boundary rules' concern.
/// `❄ ❄ ❄ ❄`, `✦ ✦ ✦`, `~ ~ ~`: one ornament repeated across a line is a section break the
/// publisher drew by hand. Markdown syntax characters and single ornaments are not.
fn is_decorative_rule(text: &str) -> bool {
    let mut ornament = None;
    let mut count = 0;
    for ch in text.chars() {
        if ch.is_whitespace() {
            continue;
        }
        if ch.is_alphanumeric() || "*-_#>`|\\[]()<+!.:,;\"'".contains(ch) {
            return false;
        }
        match ornament {
            None => ornament = Some(ch),
            Some(first) if first != ch => return false,
            Some(_) => {}
        }
        count += 1;
    }
    count >= 3
}

/// Replace hand-drawn ornament lines by a thematic break, which the reader styles as a rule.
fn decorative_rules_to_breaks(markdown: &str) -> String {
    if !markdown.lines().any(is_decorative_rule) {
        return markdown.to_string();
    }
    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, markdown, &comrak::Options::default());
    let rules = root
        .children()
        .filter(|node| {
            let position = node.data.borrow().sourcepos;
            position.start.line == position.end.line
                && boundary_paragraph_text(node).is_some_and(|(text, _)| is_decorative_rule(&text))
        })
        .map(|node| node.data.borrow().sourcepos.start.line)
        .collect::<std::collections::BTreeSet<_>>();
    if rules.is_empty() {
        return markdown.to_string();
    }
    markdown
        .split_inclusive('\n')
        .enumerate()
        .map(|(index, line)| {
            if rules.contains(&(index + 1)) {
                if line.ends_with('\n') {
                    "* * *\n"
                } else {
                    "* * *"
                }
            } else {
                line
            }
        })
        .collect()
}

fn is_separator_row(text: &str) -> bool {
    let mut glyphs = 0;
    for ch in text.chars() {
        match ch {
            '/' | '|' | '\u{b7}' | '\u{2022}' | '\u{2014}' | '\u{2013}' => glyphs += 1,
            ch if ch.is_whitespace() => {}
            _ => return false,
        }
    }
    glyphs >= 2
}

fn strip_separator_rows(markdown: &str) -> String {
    if !markdown.lines().any(is_separator_row) {
        return markdown.to_string();
    }
    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, markdown, &comrak::Options::default());
    let doomed = root
        .children()
        .filter(|node| {
            let position = node.data.borrow().sourcepos;
            position.start.line == position.end.line
                && boundary_paragraph_text(node).is_some_and(|(text, _)| is_separator_row(&text))
        })
        .map(|node| node.data.borrow().sourcepos.start.line)
        .collect::<std::collections::BTreeSet<_>>();
    if doomed.is_empty() {
        return markdown.to_string();
    }
    let mut out = String::with_capacity(markdown.len());
    let mut skip_blank = false;
    for (index, line) in markdown.split_inclusive('\n').enumerate() {
        if doomed.contains(&(index + 1)) {
            // The blank line that followed the row would otherwise double the one before it.
            skip_blank = out.is_empty() || out.ends_with("\n\n");
            continue;
        }
        if skip_blank && line.trim().is_empty() {
            skip_blank = false;
            continue;
        }
        skip_blank = false;
        out.push_str(line);
    }
    out
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

/// One to four capitalised words: the shape of a person's name.
fn name_like(value: &str) -> bool {
    let words: Vec<&str> = value.split_whitespace().collect();
    (1..=4).contains(&words.len())
        && words.iter().all(|word| {
            word.chars().next().is_some_and(char::is_uppercase)
                && word
                    .chars()
                    .all(|ch| ch.is_alphabetic() || matches!(ch, '\'' | '’' | '-' | '.'))
        })
}

/// `Carlo Piovesan, Geertjan Wielenga`, `Ann Lee and Bob Ray`: one to four names, each of at least
/// two words, so a Title Case phrase (`Current Linguistics`) is not read as people.
fn is_name_list(text: &str) -> bool {
    let names: Vec<&str> = text
        .split([',', '&'])
        .flat_map(|part| part.split(" and "))
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    (1..=4).contains(&names.len())
        && names
            .iter()
            .all(|name| name.split_whitespace().count() >= 2 && name_like(name))
}

/// `By Amy Walker`, `Amy Walker and`, `Nick Beake, Europe correspondent`: a name that announces
/// itself as a byline. A bare Title Case line is indistinguishable from a kicker, so a name with a
/// role only counts while a byline is already being read.
fn is_byline_line(text: &str, after_byline: bool, after_recent: bool) -> bool {
    let text = text.trim();
    if text.is_empty()
        || text.chars().count() > 60
        || text.ends_with(['.', '!', '?', ':', ';', ','])
        || text.split_whitespace().count() > 8
    {
        return false;
    }
    if let Some(name) = text.strip_prefix("By ") {
        return name_like(name.split(',').next().unwrap_or(name));
    }
    if let Some(name) = text
        .strip_suffix(" and")
        .or_else(|| text.strip_suffix(" &"))
    {
        return name_like(name);
    }
    if after_byline
        && text.split_once(',').is_some_and(|(name, role)| {
            name_like(name) && (1..=5).contains(&role.split_whitespace().count())
        })
    {
        return true;
    }
    // Right behind a live timestamp (`2 hours ago` / `Jessica Rawnsley` on bbc.com) a bare
    // two-to-four-word name is the contributor: that chrome belongs to a newsroom page. Under a
    // plain date a Title Case line is as likely a kicker, so it stays.
    after_recent && !text.contains(',') && text.split_whitespace().count() >= 2 && name_like(text)
}

/// `By Eric Lu 09.09.26`, `Aleksandar Filipovski, 2026-09-16`: an author's name with the item's
/// own publication date beside it. Without the `By`, only a two-to-four-word capitalised name in
/// front of the date counts, so a dateline (`London, 16 September 2026`) is left alone.
fn matching_leading_byline(markdown: &str, plain: &str, published: NaiveDate) -> bool {
    let text = plain.trim();
    let text = text.strip_prefix("By ").unwrap_or(text);
    let matches_published = |date: &str| {
        let date = date.trim();
        ["%m.%d.%y", "%d.%m.%y"]
            .iter()
            .find_map(|format| NaiveDate::parse_from_str(date, format).ok())
            .or_else(|| parse_date_only(date).map(|parsed| parsed.date))
            .is_some_and(|date| {
                date.signed_duration_since(published)
                    .num_days()
                    .unsigned_abs()
                    <= 1
            })
    };
    let Some((author, _)) = [", ", " - ", " | ", " — ", " – ", " · ", " "]
        .iter()
        .filter_map(|separator| text.rsplit_once(separator))
        .find(|(_, date)| matches_published(date))
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

/// A publisher's date line that sits behind a one-line lede (`Looking back on the first year.`
/// / `Posted 2026-09-15` / the article): the lede is prose to keep, the labelled date is not.
/// Only a labelled date qualifies; a bare date behind prose stays.
fn stamp_behind_lede(lede: &str, lede_text: &str, tail: &str) -> Option<String> {
    if lede_text.chars().count() > 200 {
        return None;
    }
    let (stamp, after) = tail
        .split_once("\n\n")
        .map(|(stamp, after)| (stamp, after.trim_start_matches('\n')))
        .unwrap_or((tail.trim_end_matches('\n'), ""));
    if stamp.lines().count() != 1 {
        return None;
    }
    let plain = html_to_text(&render_markdown(stamp));
    let labelled = date_prefixes(&plain)
        .filter_map(parse_date_only)
        .any(|candidate| candidate.labelled);
    if !labelled {
        return None;
    }
    Some(if after.is_empty() {
        format!("{lede}\n")
    } else {
        format!("{lede}\n\n{after}")
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
/// Longer labels come first: `posted on ` must win over `posted ` or the date never parses.
const DATE_LABELS: [&str; 10] = [
    "last updated on ",
    "last updated ",
    "written on ",
    "written ",
    "published on ",
    "published ",
    "posted on ",
    "posted ",
    "updated on ",
    "updated ",
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
    for line in drop_dangling_breaks(markdown) {
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

/// A hard line break (`\\`) with nothing after it on the page (`<br>` closing a paragraph, or
/// stacked `<br>`s between blocks) has no Markdown meaning and renders as a literal backslash.
/// Drop the backslash that ends a block and the lines made only of backslashes; a break between
/// two lines of one paragraph stays.
fn drop_dangling_breaks(markdown: &str) -> Vec<&str> {
    let lines: Vec<&str> = markdown.lines().map(str::trim_end).collect();
    let mut out = Vec::with_capacity(lines.len());
    for (index, line) in lines.iter().enumerate() {
        let only_breaks =
            !line.is_empty() && line.chars().all(|ch| ch == '\\' || ch.is_whitespace());
        if only_breaks {
            continue;
        }
        let block_ends = lines
            .get(index + 1)
            .is_none_or(|next| next.trim().is_empty());
        if block_ends && line.ends_with('\\') && !line.ends_with("\\\\") {
            out.push(line[..line.len() - 1].trim_end());
        } else {
            out.push(line);
        }
    }
    out
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
    fn article_tags_move_only_explicit_boundary_groups_into_labels() {
        let (body, labels) = normalize_article_body(
            "#RUST #AI\n\nCarlo Piovesan, Geertjan Wielenga\n\n2026-09-18 | 9 min\n\nActual prose about #rust.\n\n#interior\n\nMore prose.\n\n[#Jev](https://youtube.com/hashtag/jev) **#AI**\n\n#coding #中文\n",
            "",
            None,
            "video",
        );
        assert_eq!(
            body,
            "Actual prose about #rust.\n\n#interior\n\nMore prose.\n"
        );
        assert_eq!(labels, ["ai", "coding", "jev", "rust", "中文"]);
        assert_eq!(
            normalize_article_body(&body, "", None, "video"),
            (body, Vec::new())
        );
    }

    #[test]
    fn article_tags_preserve_prose_code_lists_quotes_and_headings() {
        for body in [
            "Discuss #rust and #ai.\n",
            "# A heading\n",
            "`#rust #ai`\n",
            "```sh\n#rust #ai\n```\n",
            "    #rust #ai\n",
            "> #rust #ai\n",
            "- #rust\n- #ai\n",
            "#rust is great\n",
            "#rust.\n",
            "#rust/path\n",
            "![#rust](https://example.com/image.png)\n",
            "Opening.\n\n#rust #ai\n\nClosing.\n",
        ] {
            assert_eq!(
                normalize_article_body(body, "", None, "feed"),
                (body.to_string(), Vec::new()),
                "{body}"
            );
        }
    }

    #[test]
    fn boundary_update_notices_are_removed_but_prose_mentions_stay() {
        let body = "Opening paragraph.\n\nMore reporting follows here.\n\n*This article was updated on 08 September 2026.*\n";
        assert_eq!(
            strip_article_metadata(body, "", None, "spectrum"),
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
                strip_article_metadata(&body, "", None, "spectrum"),
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
                strip_article_metadata(body, "", None, "spectrum"),
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
                    "",
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
            assert_eq!(strip_article_metadata(prose, "", None, "apple"), prose);
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
                        strip_article_metadata(&body, "", None, source),
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
                "",
                None,
                "an-archive"
            ),
            "Actual article.\n"
        );
        // Prose that merely carries several links keeps its sentence.
        let prose = "See [one](https://example.com/1), [two](https://example.com/2) and [three](https://example.com/3) for the details.";
        assert_eq!(
            strip_article_metadata(&format!("Actual article.\n\n{prose}\n"), "", None, "a-blog"),
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
            assert_eq!(
                strip_article_metadata(body, "", None, "feed"),
                body,
                "{body}"
            );
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
            strip_article_metadata(&body, "", Some(published), "feed"),
            "Actual article.\n"
        );
        assert_eq!(strip_article_metadata("0 comments\n", "", None, "feed"), "");
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
            strip_leading_metadata(&markdown, "", Some(published), "blog-google"),
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
                strip_leading_metadata(body, "", Some(published), "blog-google"),
                body
            );
            assert_eq!(
                strip_leading_metadata(
                    &format!("Sep 02, 2026\n\n{body}"),
                    "",
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
                "",
                Some(published),
                "blog-google"
            ),
            "Body.\n"
        );
        let unrelated_date = "Sep 02, 2025\n\nBody.\n";
        assert_eq!(
            strip_leading_metadata(unrelated_date, "", Some(published), "blog-google"),
            unrelated_date
        );
        assert_eq!(
            strip_leading_metadata(
                "Blog Google\n\n|\n\nBody.\n",
                "",
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
                strip_article_metadata(&format!("{prefix}{body}"), "", None, "hnrss-org-frontpage"),
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
                strip_article_metadata(&markdown, "", None, "hnrss-org-frontpage"),
                markdown
            );
        }
    }

    #[test]
    fn strips_a_heading_repeating_the_title_and_the_name_date_byline_below_it() {
        use chrono::TimeZone as _;

        // filipovski.net: `<h3>Backups aren't simple</h3><p><em><a href="/">Aleksandar
        // Filipovski</a>, 2026-09-16</em></p>` above the article, whose feed title is Title Case.
        let published = Utc.with_ymd_and_hms(2026, 9, 16, 20, 27, 16).unwrap();
        let title = "Backups Aren't Simple";
        let source = "hnrss-org-frontpage";
        let body = "### Backups aren't simple\n\n*[Aleksandar Filipovski](https://filipovski.net/), 2026-09-16*\n\n**See also: John Salvatier\u{2019}s excellent blog**\n\n* * *\n\nI read a comment somewhere that stuck with me.\n";
        let expected = "**See also: John Salvatier\u{2019}s excellent blog**\n\n* * *\n\nI read a comment somewhere that stuck with me.\n";
        assert_eq!(
            strip_article_metadata(body, title, Some(published), source),
            expected
        );
        assert_eq!(
            strip_article_metadata(expected, title, Some(published), source),
            expected
        );
        // arxiv.org labels its heading: `<h1 class="title">Title:Breaking the 1.58-bit …</h1>`.
        assert_eq!(
            strip_article_metadata(
                "## Title:Breaking the 1.58-bit Barrier for Ternary LLMs\n\n[View PDF](https://arxiv.org/pdf/2609.16338)\n",
                "Breaking the 1.58-bit Barrier for Ternary LLMs",
                None,
                source
            ),
            "[View PDF](https://arxiv.org/pdf/2609.16338)\n"
        );
        // A different heading, a heading further down, a plain paragraph, a byline dated another
        // day, a byline without a title or stamp before it, and a dateline all stay.
        for (body, title) in [
            ("### Backups aren't simple\n\nBody.\n", "Backups Are Hard"),
            ("Lead.\n\n### Backups aren't simple\n\nBody.\n", title),
            ("Backups aren't simple\n\nBody.\n", title),
            (
                "### Backups aren't simple\n\n*[Aleksandar Filipovski](https://filipovski.net/), 2026-09-01*\n\nBody.\n",
                "Backups Are Hard",
            ),
            (
                "### Backups aren't simple\n\nAleksandar Filipovski\n\nBody.\n",
                "Backups Are Hard",
            ),
            ("London, 16 September 2026\n\nBody.\n", title),
            ("### Backups aren't simple\n\nBody.\n", ""),
        ] {
            assert_eq!(
                strip_article_metadata(body, title, Some(published), source),
                body,
                "{body}"
            );
        }
    }

    #[test]
    fn separator_rows_left_by_dropped_controls_are_removed() {
        // openspec.dev: `<button>npm</button><span>/</span><button>pnpm</button>…` — Readability
        // drops the buttons and leaves their separators as a paragraph.
        let body = "## Installation\n\n/ / / /\n\n## Compatibility\n\nClaude Code Codex\n";
        assert_eq!(
            strip_article_metadata(body, "", None, "blog"),
            "## Installation\n\n## Compatibility\n\nClaude Code Codex\n"
        );
        assert_eq!(
            strip_article_metadata("Lead.\n\n\u{b7} \u{b7}\n\nBody.\n", "", None, "blog"),
            "Lead.\n\nBody.\n"
        );
        for body in [
            "\u{b7}\n\nBody.\n",
            "Lead.\n\n* * *\n\nBody.\n",
            "Lead.\n\n---\n\nBody.\n",
            "a / b\n\nBody.\n",
            "```\n/ / / /\n```\n",
            "|   |   |\n| - | - |\n| 1 | 2 |\n",
        ] {
            assert_eq!(
                strip_article_metadata(body, "", None, "blog"),
                body,
                "{body}"
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
            strip_article_metadata(&markdown, "", Some(published), "cognition-com-blog"),
            expected
        );
        assert_eq!(
            strip_article_metadata(expected, "", Some(published), "cognition-com-blog"),
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
                strip_article_metadata(body, "", Some(published), "cognition-com-blog"),
                body
            );
        }
        assert_eq!(
            strip_article_metadata(&markdown, "", None, "cognition-com-blog"),
            markdown
        );
    }

    #[test]
    fn strips_only_leading_metadata_that_matches_the_item() {
        use chrono::{TimeZone as _, Utc};

        let published = Utc.with_ymd_and_hms(2026, 9, 2, 14, 16, 42).unwrap();
        let body = "2nd September 2026\n\nAnthropic published the prompts.\n";
        assert_eq!(
            strip_leading_metadata(body, "", Some(published), "anthropic"),
            "Anthropic published the prompts.\n"
        );
        assert_eq!(
            strip_leading_metadata(
                "[September 2, 2026](/archive)\n\nBody.\n",
                "",
                Some(published),
                "blog"
            ),
            "Body.\n"
        );
        assert_eq!(
            strip_leading_metadata("2nd September 2025\n\nBody.\n", "", Some(published), "blog"),
            "2nd September 2025\n\nBody.\n"
        );
        assert_eq!(
            strip_leading_metadata(
                "We met on 2nd September 2026.\n\nBody.\n",
                "",
                Some(published),
                "blog"
            ),
            "We met on 2nd September 2026.\n\nBody.\n"
        );
        assert_eq!(
            strip_leading_metadata(
                "3rd September 2026 - Link Blog\n\nThe actual opening.\n",
                "",
                Some(Utc.with_ymd_and_hms(2026, 9, 3, 8, 0, 0).unwrap()),
                "simon-willison"
            ),
            "The actual opening.\n"
        );
        assert_eq!(
            strip_leading_metadata(
                "OpenAI\n\nSafety starts here.\n",
                "",
                Some(published),
                "openai"
            ),
            "Safety starts here.\n"
        );
        assert_eq!(
            strip_leading_metadata(
                "OpenAI builds systems.\n\nBody.\n",
                "",
                Some(published),
                "openai"
            ),
            "OpenAI builds systems.\n\nBody.\n"
        );
    }

    #[test]
    fn a_title_paragraph_and_a_read_time_stamp_are_metadata() {
        // hacktron.ai: a `<header>` with the title as a paragraph, then date and reading time.
        // Hacker News shortened the title it submitted.
        let title = "A heap overflow and SSO misconfiguration to compromise OpenAI internal repos";
        let body = "A heap overflow and SSO misconfiguration to compromise OpenAI internal repositories\n\nSeptember 13, 2026 11 min read\n\n## Intro\n\nOn July 25 we chained two bugs.\n";
        assert_eq!(
            strip_leading_metadata(body, title, None, "hnrss"),
            "## Intro\n\nOn July 25 we chained two bugs.\n"
        );
        assert_eq!(
            strip_leading_metadata("11 min read\n\nBody.\n", "", None, "hnrss"),
            "Body.\n"
        );
        assert_eq!(
            strip_leading_metadata("Sep 13, 2026 · 5-minute read\n\nBody.\n", "", None, "hnrss"),
            "Body.\n"
        );
        for body in [
            // A different title, a sentence that merely starts with the title, prose with minutes,
            // and the title as an opening line with prose right after it.
            "A heap overflow in libheif\n\nBody.\n",
            "A heap overflow and SSO misconfiguration to compromise OpenAI internal repositories\n\nBody.\n",
            "A heap overflow and SSO misconfiguration to compromise OpenAI internal repos was found.\n\nBody.\n",
            "It took 11 min to read\n\nBody.\n",
            "September 13, 2026\n\nBody.\n",
        ] {
            assert_eq!(
                strip_leading_metadata(body, title, None, "hnrss"),
                body,
                "{body}"
            );
        }
        assert!(same_title(
            "compromise openai internal repos",
            "compromise openai internal repositories"
        ));
        assert!(!same_title(
            "openai internal repos",
            "openai internal report"
        ));
        assert!(!same_title("a b", "a b c"));
        assert_eq!(
            without_read_time("September 13, 2026 11 min read"),
            Some("September 13, 2026")
        );
        assert_eq!(without_read_time("11 min read"), Some(""));
        assert_eq!(
            without_read_time("Read this in 11 min"),
            Some("Read this in")
        );
        assert_eq!(without_read_time("a long read"), None);
    }

    #[test]
    fn an_authors_line_above_the_date_row_is_the_byline() {
        // duckdb.org: `<span class="author">` over `<span class="date"> | <span class="readingtime">`.
        assert_eq!(
            strip_leading_metadata(
                "Carlo Piovesan, Geertjan Wielenga\n\n2026-09-18 | 9 min\n\n*TL;DR: it works.*\n",
                "",
                None,
                "lobste-rs"
            ),
            "*TL;DR: it works.*\n"
        );
        for body in [
            // A single word or a phrase is a kicker or prose, and a name without a stamp stays.
            "Interpretability\n\n2026-09-18 | 9 min\n\nBody.\n",
            "Carlo Piovesan\n\nThe post begins here.\n",
        ] {
            assert_eq!(
                strip_leading_metadata(body, "", None, "lobste-rs"),
                body,
                "{body}"
            );
        }
    }

    #[test]
    fn duckdb_byline_is_removed_when_feed_date_is_the_next_day() {
        let published = DateTime::parse_from_rfc3339("2026-09-19T18:46:39Z")
            .unwrap()
            .with_timezone(&Utc);
        let markdown = to_markdown(
            "<p>Carlo Piovesan, Geertjan Wielenga</p><p>2026-09-18 | 9 min</p><p><em>TL;DR: DuckDB-Wasm can open a persistent database file.</em></p>",
            None,
        );
        assert_eq!(
            strip_article_metadata(
                &markdown,
                "Persistent Databases in the Browser",
                Some(published),
                "lobste-rs"
            ),
            "*TL;DR: DuckDB-Wasm can open a persistent database file.*\n"
        );
    }

    #[test]
    fn trailing_lwn_index_navigation_and_surrounding_breaks_are_removed() {
        let footer = "| Index entries for this article | |\n| --- | --- |\n| [Kernel](https://lwn.net/Kernel/Index) | [io\\_uring](https://lwn.net/Kernel/Index#io_uring) |";
        for body in [
            format!("Better performance.\n\n{footer}\n"),
            format!("Better performance.\\\n\n{footer}\n\n\\\n"),
        ] {
            assert_eq!(
                strip_article_metadata(&body, "", None, "lobste-rs"),
                "Better performance.\n"
            );
        }
        let html = "<p>Better performance.<br clear=all></p><table class=IndexEntries><tr><th colspan=2>Index entries for this article</th></tr><tr><td><a href='https://lwn.net/Kernel/Index'>Kernel</a></td><td><a href='https://lwn.net/Kernel/Index#io_uring'>io_uring</a></td></tr></table><br clear=all>";
        assert_eq!(
            strip_article_metadata(&to_markdown(html, None), "", None, "lobste-rs"),
            "Better performance.\n"
        );
    }

    #[test]
    fn index_navigation_cleanup_preserves_body_tables_and_code() {
        let footer = "| Index entries for this article | |\n| --- | --- |\n| [Kernel](https://lwn.net/Kernel/Index) | [io\\_uring](https://lwn.net/Kernel/Index#io_uring) |";
        for body in [
            format!("Opening.\n\n{footer}\n\nMore article.\n"),
            format!("Opening.\n\n```markdown\n{footer}\n```\n"),
            format!("Opening.\n\n{}\n", footer.replace("lwn.net", "example.com")),
            format!(
                "Opening.\n\n{}\n",
                footer.replace("lwn.net", "lwn.net.example.com")
            ),
            format!(
                "Opening.\n\n{}\n",
                footer.replace("Index entries for this article", "Article data")
            ),
            format!(
                "Opening.\n\n{}\n",
                footer.replace("[Kernel](https://lwn.net/Kernel/Index)", "Kernel data")
            ),
            format!(
                "Opening.\n\n{}\n",
                footer.replace("| [io", "| Explanation [io")
            ),
            format!("{footer}\n\nArticle body.\n"),
            format!("Opening.\n\n> {}\n", footer.replace('\n', "\n> ")),
        ] {
            assert_eq!(
                strip_article_metadata(&body, "", None, "lobste-rs"),
                body,
                "{body}"
            );
        }
    }

    #[test]
    fn dangling_hard_breaks_are_dropped_and_real_ones_stay() {
        // lwn.net: a <br> closes the last paragraph and two more follow the index table.
        let body = "Better performance.\\\n\n| Index | Entries |\n| ----- | ------- |\n| a | b |\n\n\\\n \\\n";
        assert_eq!(
            strip_article_metadata(body, "", None, "lobste-rs"),
            "Better performance.\n\n| Index | Entries |\n| ----- | ------- |\n| a | b |\n"
        );
        let poem = "Roses are red\\\nviolets are blue\n";
        assert_eq!(strip_article_metadata(poem, "", None, "lobste-rs"), poem);
        let literal = "Ends with a backslash \\\\\n";
        assert_eq!(
            strip_article_metadata(literal, "", None, "lobste-rs"),
            literal
        );
    }

    #[test]
    fn category_labels_and_eyebrows_above_the_opening_are_metadata() {
        // linguisticdiscovery.com: the tag link Ghost places over the title.
        assert_eq!(
            strip_leading_metadata(
                "[Newsletter](https://example.com/tags/articles/)\n\nAround 1,000 Greek words remain.\n",
                "",
                None,
                "hnrss"
            ),
            "Around 1,000 Greek words remain.\n"
        );
        // openjev.com: the eyebrow of a landing page's hero, then its heading.
        assert_eq!(
            strip_leading_metadata(
                "A live, local experiment\n\n## Decision model in your browser.\n\nA local model reads probabilities.\n",
                "",
                None,
                "hnrss"
            ),
            "## Decision model in your browser.\n\nA local model reads probabilities.\n"
        );
        for body in [
            // A link that reads as a sentence, or with lowercase words, opens the article.
            "[Read the PDF](https://example.com/paper.pdf)\n\nBody.\n",
            "[Newsletter](https://example.com/tags/) and more\n\nBody.\n",
            // Short prose before a heading still ends a sentence, or is not followed by one.
            "Hello there.\n\n## Intro\n\nBody.\n",
            "A live, local experiment\n\nBody follows without a heading.\n",
        ] {
            assert_eq!(
                strip_leading_metadata(body, "", None, "hnrss"),
                body,
                "{body}"
            );
        }
    }

    #[test]
    fn a_bare_name_behind_the_timestamp_is_the_byline() {
        // bbc.com: `<time>2 hours ago</time>` then the contributor's name in its own paragraph.
        let body = "2 hours ago\n\nJessica Rawnsley\n\nCanada has welcomed the proposal.\n";
        assert_eq!(
            strip_leading_metadata(body, "", None, "hnrss"),
            "Canada has welcomed the proposal.\n"
        );
        for body in [
            // Without the live timestamp a capitalised line could be a kicker.
            "Jessica Rawnsley\n\nCanada has welcomed the proposal.\n",
            "17 September 2026\n\nJessica Rawnsley\n\nCanada has welcomed the proposal.\n",
            // Prose, a dateline and a single word stay.
            "2 hours ago\n\nA short opening sentence\n\nBody.\n",
            "2 hours ago\n\nLondon, England\n\nBody.\n",
            "2 hours ago\n\nAnalysis\n\nBody.\n",
        ] {
            let expected = body.trim_start_matches("2 hours ago\n\n");
            assert_eq!(
                strip_leading_metadata(body, "", None, "hnrss"),
                expected,
                "{body}"
            );
        }
    }

    #[test]
    fn ornament_lines_become_thematic_breaks() {
        // martinfowler.com draws section breaks as a row of snowflakes.
        let body = "First section.\n\n ❄                ❄                ❄                ❄                ❄\n\nSecond section.\n\n✦ ✦ ✦\n\nThird.\n";
        let cleaned = strip_article_metadata(body, "", None, "martinfowler-com");
        assert!(
            cleaned.contains("First section.\n\n* * *\n\nSecond section.\n\n* * *\n\nThird."),
            "{cleaned}"
        );
        // Two ornaments, mixed ornaments, emphasis and list markers are not rules.
        for body in [
            "A.\n\n❄ ❄\n\nB.\n",
            "A.\n\n❄ ✦ ❄\n\nB.\n",
            "A.\n\n* * *\n\nB.\n",
            "A.\n\n- one\n- two\n",
        ] {
            assert!(
                !is_decorative_rule(body.lines().nth(2).unwrap_or("")),
                "{body}"
            );
        }
        assert!(is_decorative_rule("~ ~ ~ ~"));
        assert!(!is_decorative_rule("!!!"));
    }

    #[test]
    fn strips_a_labelled_date_behind_a_one_line_lede() {
        use chrono::{TimeZone as _, Utc};

        // servo.org: the description Readability keeps, then the page's own date line, then the
        // article. Hacker News published the submission two days later.
        let submitted = Utc.with_ymd_and_hms(2026, 9, 17, 8, 13, 54).unwrap();
        let body = "Looking back on Servo's first donation-funded role.\n\nPosted 2026-09-15\n\nLast September, the project announced a role.\n";
        assert_eq!(
            strip_leading_metadata(body, "", Some(submitted), "hnrss"),
            "Looking back on Servo's first donation-funded role.\n\nLast September, the project announced a role.\n"
        );
        // A stamp that closes the item and a heading lede work the same way.
        assert_eq!(
            strip_leading_metadata("## A lede\n\nPosted 2026-09-15\n", "", None, "hnrss"),
            "## A lede\n"
        );
        // Prose that happens to sit after a lede stays, and so does a bare date behind prose:
        // without a label it needs the item's own date, which the loop never reaches here.
        for body in [
            "A lede.\n\nPosted 2026-09-15 the draft finally made sense.\n\nBody.\n",
            "A lede.\n\n2026-09-15\n\nBody.\n",
            "A lede.\n\nPosted\n2026-09-15\n\nBody.\n",
        ] {
            assert_eq!(
                strip_leading_metadata(body, "", Some(submitted), "hnrss"),
                body
            );
        }
        let long_lede = format!(
            "{} lede.\n\nPosted 2026-09-15\n\nBody.\n",
            "very ".repeat(60)
        );
        assert_eq!(
            strip_leading_metadata(&long_lede, "", None, "hnrss"),
            long_lede
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
            "Posted 2026-09-07",
            "Published 7 September 2026",
            "Last updated on 7th September 2026",
        ] {
            assert_eq!(
                strip_leading_metadata(
                    &format!("{byline}\n\nThe actual opening.\n"),
                    "",
                    None,
                    "hnrss"
                ),
                "The actual opening.\n",
                "{byline}"
            );
            assert_eq!(
                strip_leading_metadata(
                    &format!("{byline}\n\nThe actual opening.\n"),
                    "",
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
            assert_eq!(
                strip_leading_metadata(body, "", Some(submitted), "hnrss"),
                body
            );
        }
        // A bare date still needs the item's own published date to back it up.
        let bare = "September 07, 2026\n\nBody.\n";
        assert_eq!(
            strip_leading_metadata(bare, "", Some(submitted), "hnrss"),
            bare
        );
        assert_eq!(strip_leading_metadata(bare, "", None, "hnrss"), bare);
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
                strip_article_metadata(body, "", Some(published), "blog-google"),
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
            let kept = strip_article_metadata(body, "", Some(published), "blog-google");
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
                "",
                Some(published),
                "hnrss-org-frontpage"
            ),
            "## Chop up your books\n\nThis is my appeal.\n"
        );
        // An eight-digit number that is not this item's date stays where it is.
        let unrelated = "20190104\n\nThe build number above matters.\n";
        assert_eq!(
            strip_article_metadata(unrelated, "", Some(published), "hnrss-org-frontpage"),
            unrelated
        );
    }

    #[test]
    fn stacked_separators_are_tidied_in_archives_written_before_the_rule() {
        // Capture-time tidying cannot reach a body already on the branch, so the build tidies too.
        assert_eq!(
            strip_article_metadata(
                "Lead.\n\n* * *\n\n* * *\n\nBody.\n\n* * *\n",
                "",
                None,
                "blog"
            ),
            "Lead.\n\n* * *\n\nBody.\n"
        );
        // A rule inside a code block is code.
        let fenced = "```\n* * *\n\n* * *\n```\n";
        assert_eq!(strip_article_metadata(fenced, "", None, "blog"), fenced);
    }
}
