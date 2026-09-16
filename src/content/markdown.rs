//! HTML to Markdown: the normalization passes that hand htmd durable document structure
//! (image sources, code listings, figures, footnotes, sidenotes, YouTube descriptions) and the
//! conversion itself.

use scraper::{Html, Selector};
use url::Url;

use super::cleanup::{is_accessibility_label, protect_markdown_code, tidy_markdown};
use super::scan::{
    BLOCK_ELEMENTS, attribute_value, comment_end, element_bounds, has_class, opening_element_count,
    parse_tag, set_attribute, skip_html_whitespace,
};
use super::strip::{decode_entities, html_to_text, is_active_url, sanitize, strip_active_content};
use super::{balanced_json_object, escape_html};

/// Inline wrappers commonly used as CSS layout children. Readability keeps these elements but not
/// the CSS `gap` or grid columns that visually separated directly adjacent siblings.
const LAYOUT_INLINE_ELEMENTS: &[&str] = &["a", "label", "span", "time"];

const FOOTNOTE_REF_START: char = '\u{e000}';
const FOOTNOTE_REF_END: char = '\u{e001}';
const MARKDOWN_LINK_START: char = '\u{e002}';

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
    if value.is_empty() || is_active_url(value) {
        return None;
    }
    Some(responsive_json_candidate(value).unwrap_or_else(|| value.to_string()))
}

/// Some templates leak their unrendered responsive source map into the attribute, leaving a URL
/// with a JSON object glued to it. The widest declared rendition is the one a reader wants.
fn responsive_json_candidate(value: &str) -> Option<String> {
    let object = balanced_json_object(&value[value.find('{')?..])?;
    let sources: std::collections::BTreeMap<String, String> = serde_json::from_str(object).ok()?;
    ["desktop", "tablet", "mobile"]
        .into_iter()
        .find_map(|key| sources.get(key))
        .or_else(|| sources.values().next())
        .map(String::from)
        .filter(|url| url.starts_with("http://") || url.starts_with("https://"))
}

/// [`sanitize`] then htmd. Trailing whitespace trimmed, exactly one trailing newline, runs of
/// blank lines collapsed to one.
pub fn to_markdown(html: &str, base: Option<&Url>) -> String {
    let description = normalize_youtube_description(html, base);
    let normalized_images = normalize_image_sources(&description);
    let passive = strip_active_content(&normalized_images);
    let document = normalize_document_footnotes(&normalize_code_blocks(&normalize_code_tables(
        &normalize_figure_captions(&strip_audio_players(&passive)),
    )));
    let normalized = normalize_extracted_controls(&document.html, document.footnotes);
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
        .add_handler(vec!["a"], markdown_link)
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

fn markdown_link(
    handlers: &dyn htmd::element_handler::Handlers,
    element: htmd::Element<'_>,
) -> Option<htmd::element_handler::HandlerResult> {
    let mut result = handlers.fallback(element)?;
    if result.content.starts_with('[') {
        // An exclamation in an adjacent text node must not turn this link into an image.
        result.content.insert(0, MARKDOWN_LINK_START);
    }
    Some(result)
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

/// Highlighters routinely lay a listing out as a table with a line-number gutter. Read as a table
/// each line becomes its own row, so the snippet arrives as alternating numbers and fragments;
/// rebuilt as a code block it keeps its indentation, its language and its copyability.
fn normalize_code_tables(html: &str) -> String {
    if !html.contains("<table") {
        return html.to_string();
    }
    let mut out = String::with_capacity(html.len());
    let mut position = 0;
    while let Some(start) = html[position..].find('<').map(|offset| position + offset) {
        out.push_str(&html[position..start]);
        let listing = parse_tag(&html[start..])
            .filter(|tag| !tag.closing && tag.name == "table")
            .and_then(|_| element_bounds(html, start, "table"))
            .and_then(|(.., end)| {
                let table = &html[start..end];
                Some((code_table(table).or_else(|| headed_table(table))?, end))
            });
        match listing {
            Some((code, end)) => {
                out.push_str(&code);
                position = end;
            }
            None => {
                out.push('<');
                position = start + 1;
            }
        }
    }
    out.push_str(&html[position..]);
    out
}

/// The `<pre><code>` replacement for a table that is really a numbered listing, or `None` when the
/// table carries data a reader needs to keep as a table.
fn code_table(table: &str) -> Option<String> {
    let fragment = Html::parse_fragment(table);
    if fragment
        .select(&Selector::parse("th").ok()?)
        .next()
        .is_some()
    {
        return None;
    }
    let cells = Selector::parse("td").ok()?;
    let rows: Vec<Vec<_>> = fragment
        .select(&Selector::parse("tr").ok()?)
        .map(|row| row.select(&cells).collect())
        .collect();
    if rows.is_empty() || rows.iter().any(|row| row.len() != 2) {
        return None;
    }
    // A single row keeps the gutter and the listing whole in two cells; otherwise every line is
    // its own row. Either way the numbers must run consecutively, which data never does by chance.
    if rows.len() == 1 {
        let numbers = element_text(&rows[0][0]);
        let mut numbers = numbers.split_whitespace().map(str::parse::<i64>);
        let first = numbers.next()?.ok()?;
        let mut expected = first;
        for number in numbers {
            expected += 1;
            if number.ok()? != expected {
                return None;
            }
        }
        if expected == first
            || rows[0][1]
                .select(&Selector::parse("pre").ok()?)
                .next()
                .is_none()
        {
            return None;
        }
        return Some(rows[0][1].inner_html());
    }
    let mut expected = None;
    let mut lines = Vec::with_capacity(rows.len());
    for row in &rows {
        let number: i64 = element_text(&row[0]).trim().parse().ok()?;
        if expected
            .replace(number + 1)
            .is_some_and(|next| next != number)
        {
            return None;
        }
        lines.push(element_text(&row[1]));
    }
    Some(format!(
        "<pre><code>{}</code></pre>",
        escape_html(&lines.join("\n"))
    ))
}

/// Markdown tables need a header row, so a table that has none degrades into a run of loose
/// paragraphs. Give it an empty one and the rows stay a table.
fn headed_table(table: &str) -> Option<String> {
    let fragment = Html::parse_fragment(table);
    if fragment
        .select(&Selector::parse("th").ok()?)
        .next()
        .is_some()
    {
        return None;
    }
    let columns = fragment
        .select(&Selector::parse("tr").ok()?)
        .map(|row| row.select(&Selector::parse("td").unwrap()).count())
        .max()
        .filter(|columns| *columns > 0)?;
    let header = "<th></th>".repeat(columns);
    let (inner, closing, _) = element_bounds(table, 0, "table")?;
    Some(format!(
        "{}<thead><tr>{header}</tr></thead>{}{}",
        &table[..inner],
        &table[inner..closing],
        &table[closing..]
    ))
}

/// Text content with explicit line breaks preserved, as a code listing needs.
fn element_text(element: &scraper::ElementRef<'_>) -> String {
    let mut text = String::new();
    for node in element.descendants() {
        match node.value() {
            scraper::Node::Text(value) => text.push_str(value),
            scraper::Node::Element(element) if element.name() == "br" => text.push('\n'),
            _ => {}
        }
    }
    text
}

/// Keep a figure's caption attached to its media. Markdown has no figure, so the caption becomes a
/// hard line break after the image — portable Markdown reads correctly, and the reader's renderer
/// puts the pair back together as a real `<figure>`.
fn normalize_figure_captions(html: &str) -> String {
    if !html.contains("<figcaption") {
        return html.to_string();
    }
    let mut out = String::with_capacity(html.len());
    let mut position = 0;
    while let Some(start) = html[position..].find('<').map(|at| position + at) {
        out.push_str(&html[position..start]);
        let caption = parse_tag(&html[start..])
            .filter(|tag| !tag.closing && tag.name == "figcaption")
            .and_then(|_| element_bounds(html, start, "figcaption"));
        match caption {
            Some((inner, closing, end))
                if !html_to_text(&html[inner..closing]).trim().is_empty() =>
            {
                out.push_str("<br>");
                out.push_str(&html[inner..closing]);
                position = end;
            }
            _ => {
                out.push('<');
                position = start + 1;
            }
        }
    }
    out.push_str(&html[position..]);
    out
}

/// Mailing-list archives wrap a whole message in one `<pre>`, navigation bar included. A first or
/// last line made only of bracketed labels, at least one of them a link, is that bar and not code.
fn strip_listing_navigation(inner: &str) -> &str {
    let mut body = inner;
    loop {
        let trimmed = body.trim_matches(['\n', '\r']);
        let head = trimmed.split_once('\n').map_or(trimmed, |(head, _)| head);
        let tail = trimmed.rsplit_once('\n').map_or(trimmed, |(_, tail)| tail);
        let next = if is_bracketed_navigation(head) {
            trimmed.split_once('\n').map_or("", |(_, rest)| rest)
        } else if trimmed.contains('\n') && is_bracketed_navigation(tail) {
            trimmed.rsplit_once('\n').map_or("", |(rest, _)| rest)
        } else {
            return trimmed;
        };
        if next.len() == body.len() {
            return trimmed;
        }
        body = next;
    }
}

fn is_bracketed_navigation(line: &str) -> bool {
    if !line.contains("<a ") {
        return false;
    }
    let text = html_to_text(line);
    let mut rest = text.trim();
    if rest.is_empty() {
        return false;
    }
    while let Some(after) = rest.strip_prefix('[') {
        let Some((label, tail)) = after.split_once(']') else {
            return false;
        };
        if label.len() > 40 {
            return false;
        }
        rest = tail.trim_start();
    }
    rest.is_empty()
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
            && let Some((inner, closing, end)) = element_bounds(html, start, "pre")
        {
            let body = strip_listing_navigation(&html[inner..closing]);
            let fragment = Html::parse_fragment(&format!("<pre>{body}</pre>"));
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

struct NormalizedHtml {
    html: String,
    footnotes: Vec<String>,
}

/// Standard document footnotes: numbered references pointing at an endnotes list in the same page.
/// Without this the reference degrades to a bare number linking off-site and the notes pile up as a
/// loose trailing list, so both halves are rebuilt as real Markdown footnotes.
fn normalize_document_footnotes(html: &str) -> NormalizedHtml {
    let Some(notes_range) = endnotes_bounds(html) else {
        return NormalizedHtml {
            html: html.to_string(),
            footnotes: Vec::new(),
        };
    };
    let (notes_start, notes_end) = (notes_range.start, notes_range.end);
    let notes = endnote_items(&html[notes_range.inner]);
    if notes.is_empty() {
        return NormalizedHtml {
            html: html.to_string(),
            footnotes: Vec::new(),
        };
    }

    let document = format!("{}{}", &html[..notes_start], &html[notes_end..]);
    let mut out = String::with_capacity(document.len());
    let mut used = vec![false; notes.len()];
    let mut position = 0;
    while let Some(tag_start) = document[position..].find('<').map(|at| position + at) {
        out.push_str(&document[position..tag_start]);
        let Some(tag) = parse_tag(&document[tag_start..]) else {
            out.push('<');
            position = tag_start + 1;
            continue;
        };
        let Some(tag_len) = tag.end else {
            out.push_str(&document[tag_start..]);
            position = document.len();
            break;
        };
        // A reference is an anchor into the endnotes list, usually wrapped in its own superscript.
        let reference = (!tag.closing && matches!(tag.name.as_str(), "a" | "sup"))
            .then(|| footnote_reference(&document, tag_start, &tag.name, &notes))
            .flatten();
        match reference {
            Some((index, end)) => {
                used[index] = true;
                out.push(FOOTNOTE_REF_START);
                out.push_str(&(index + 1).to_string());
                out.push(FOOTNOTE_REF_END);
                position = end;
            }
            None => {
                out.push_str(&document[tag_start..tag_start + tag_len]);
                position = tag_start + tag_len;
            }
        }
    }
    out.push_str(&document[position..]);

    // An endnote nothing refers to would silently disappear; keep the original document instead.
    if used.iter().any(|used| !used) {
        return NormalizedHtml {
            html: html.to_string(),
            footnotes: Vec::new(),
        };
    }
    NormalizedHtml {
        html: out,
        footnotes: notes.into_iter().map(|(_, note)| note).collect(),
    }
}

struct ElementRange {
    start: usize,
    inner: std::ops::Range<usize>,
    end: usize,
}

/// Longest wrapper text still considered player chrome rather than article prose.
const PLAYER_CHROME_CHARS: usize = 400;

/// Reader bodies are Markdown, so an `<audio>` element has no player: all that survives is its
/// "your browser does not support" fallback and the labels around it. Drop the element, and its
/// wrapper too when the wrapper holds nothing but that chrome.
fn strip_audio_players(html: &str) -> String {
    if !html.contains("<audio") {
        return html.to_string();
    }
    let mut out = String::with_capacity(html.len());
    let mut position = 0;
    while let Some(tag_start) = html[position..].find('<').map(|at| position + at) {
        out.push_str(&html[position..tag_start]);
        let Some(tag) = parse_tag(&html[tag_start..]) else {
            out.push('<');
            position = tag_start + 1;
            continue;
        };
        let Some(tag_len) = tag.end else {
            out.push_str(&html[tag_start..]);
            position = html.len();
            break;
        };
        let dropped = (!tag.closing && !tag.self_closing)
            .then(|| match tag.name.as_str() {
                "audio" => element_bounds(html, tag_start, "audio").map(|(.., end)| end),
                "div" | "section" | "figure" | "aside" | "p" => {
                    let (inner, closing, end) = element_bounds(html, tag_start, &tag.name)?;
                    let content = &html[inner..closing];
                    (content.contains("<audio")
                        && html_to_text(content).chars().count() <= PLAYER_CHROME_CHARS)
                        .then_some(end)
                }
                _ => None,
            })
            .flatten();
        match dropped {
            Some(end) => position = end,
            None => {
                out.push_str(&html[tag_start..tag_start + tag_len]);
                position = tag_start + tag_len;
            }
        }
    }
    out.push_str(&html[position..]);
    out
}

/// The endnotes container Readability keeps at the end of the article.
fn endnotes_bounds(html: &str) -> Option<ElementRange> {
    let mut position = 0;
    while let Some(tag_start) = html[position..].find('<').map(|at| position + at) {
        let Some(tag) = parse_tag(&html[tag_start..]) else {
            position = tag_start + 1;
            continue;
        };
        let Some(tag_len) = tag.end else {
            break;
        };
        let tag_html = &html[tag_start..tag_start + tag_len];
        if !tag.closing
            && matches!(tag.name.as_str(), "div" | "section" | "aside" | "ol")
            && (has_class(tag_html, "footnotes")
                || attribute_value(tag_html, "role") == Some("doc-endnotes"))
            && let Some((inner, closing, end)) = element_bounds(html, tag_start, &tag.name)
        {
            return Some(ElementRange {
                start: tag_start,
                inner: inner..closing,
                end,
            });
        }
        position = tag_start + tag_len;
    }
    None
}

/// `(anchor id, note HTML)` for each list item in the endnotes container, in document order.
fn endnote_items(inner: &str) -> Vec<(String, String)> {
    let mut notes = Vec::new();
    let mut position = 0;
    while let Some(tag_start) = inner[position..].find('<').map(|at| position + at) {
        let Some(tag) = parse_tag(&inner[tag_start..]) else {
            position = tag_start + 1;
            continue;
        };
        let Some(tag_len) = tag.end else {
            break;
        };
        if tag.closing || tag.name != "li" {
            position = tag_start + tag_len;
            continue;
        }
        let Some((content, closing, end)) = element_bounds(inner, tag_start, "li") else {
            position = tag_start + tag_len;
            continue;
        };
        let id = attribute_value(&inner[tag_start..tag_start + tag_len], "id").unwrap_or_default();
        if id.is_empty() {
            return Vec::new();
        }
        notes.push((
            id.to_string(),
            strip_backreferences(&inner[content..closing]),
        ));
        position = end;
    }
    notes
}

/// The `↩` link back to the reference is navigation, not note content.
fn strip_backreferences(note: &str) -> String {
    let mut out = String::with_capacity(note.len());
    let mut position = 0;
    while let Some(tag_start) = note[position..].find('<').map(|at| position + at) {
        out.push_str(&note[position..tag_start]);
        let Some(tag) = parse_tag(&note[tag_start..]) else {
            out.push('<');
            position = tag_start + 1;
            continue;
        };
        let Some(tag_len) = tag.end else {
            out.push_str(&note[tag_start..]);
            position = note.len();
            break;
        };
        let tag_html = &note[tag_start..tag_start + tag_len];
        let backreference = !tag.closing
            && tag.name == "a"
            && (has_class(tag_html, "footnote-backref")
                || attribute_value(tag_html, "role") == Some("doc-backlink"))
            && attribute_value(tag_html, "href").is_some_and(|href| href.starts_with('#'));
        if backreference && let Some((.., end)) = element_bounds(note, tag_start, "a") {
            position = end;
            continue;
        }
        out.push_str(tag_html);
        position = tag_start + tag_len;
    }
    out.push_str(&note[position..]);
    out
}

/// `(note index, end offset)` when the element at `start` is a reference into `notes`. A wrapping
/// superscript is consumed with the anchor so the Markdown reference does not nest inside it.
fn footnote_reference(
    html: &str,
    start: usize,
    name: &str,
    notes: &[(String, String)],
) -> Option<(usize, usize)> {
    let (inner, closing, end) = element_bounds(html, start, name)?;
    if name == "sup" {
        let anchor = skip_html_whitespace(html, inner);
        let (index, anchor_end) = footnote_reference(html, anchor, "a", notes)?;
        return html[skip_html_whitespace(html, anchor_end)..closing]
            .is_empty()
            .then_some((index, end));
    }
    let target = attribute_value(&html[start..inner], "href")?.strip_prefix('#')?;
    let index = notes.iter().position(|(id, _)| id == target)?;
    // The label is the note's number; anything else is prose that must survive.
    html_to_text(&html[inner..closing])
        .trim()
        .trim_matches(['[', ']', '(', ')'])
        .parse::<u32>()
        .ok()
        .map(|_| (index, end))
}

/// Turn presentation-only controls retained by Readability into durable document semantics.
/// Sidenotes become ordinary Markdown footnotes later in the pipeline; expand/collapse controls
/// are discarded because Readability has already retained their complete content.
fn normalize_extracted_controls(html: &str, mut footnotes: Vec<String>) -> NormalizedHtml {
    let mut normalized = String::with_capacity(html.len());
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

/// Keep punctuation before generated links literal and restore trimmed link-label boundaries.
/// htmd intentionally trims link labels, which can otherwise turn `than <a> 54,000…</a>` into
/// `than[54,000…](…)`.
fn repair_generated_markdown(markdown: &str) -> String {
    let mut value = markdown
        .replace(&format!("!{MARKDOWN_LINK_START}["), "\\![")
        .replace(MARKDOWN_LINK_START, "");
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::render_markdown;

    fn base() -> Url {
        Url::parse("https://example.com/blog/post/").unwrap()
    }

    #[test]
    fn markdown_preserves_exclamations_before_links_and_footnotes() {
        let base = Url::parse("https://example.com/article").unwrap();
        for html in [
            r##"<p>Terminal emulators!<sup><a href="#fn-1">1</a></sup></p>"##,
            r##"<p>Terminal emulators!<a href="#fn-1">1</a></p>"##,
            r##"<p>Terminal emulators&#33;<span><a href="#fn-1">1</a></span></p>"##,
        ] {
            let markdown = to_markdown(html, Some(&base));
            let rendered = render_markdown(&markdown);
            assert!(!rendered.contains("<img"), "{markdown}: {rendered}");
            assert!(rendered.contains("emulators!"), "{rendered}");
            assert!(
                rendered.contains(r#"href="https://example.com/article#fn-1""#),
                "{rendered}"
            );
        }
        let html = r#"<p>Image!<a href="/full.png"><img src="/small.png" alt="Diagram"></a></p><pre><code>![literal](image.png)</code></pre>"#;
        let rendered = render_markdown(&to_markdown(html, Some(&base)));
        assert_eq!(rendered.matches("<img").count(), 1, "{rendered}");
        assert!(rendered.contains("![literal](image.png)"), "{rendered}");
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
        assert_eq!(normalize_extracted_controls(marker, Vec::new()).html, "");
        let literal = "<span><em>(opens in a new window)</em></span>";
        assert_eq!(
            normalize_extracted_controls(literal, Vec::new()).html,
            literal
        );
        let deeply_nested = format!(
            "{}literal{}",
            "<span>".repeat(2_000),
            "</span>".repeat(2_000)
        );
        assert_eq!(
            normalize_extracted_controls(&deeply_nested, Vec::new()).html,
            deeply_nested
        );
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
    fn code_listings_drop_only_their_bracketed_navigation_lines() {
        // marc.info wraps a whole mailing-list message, navigation included, in one `<pre>`.
        let html = r#"<pre><b>[<a href="?m=1">prev in list</a>] [<a href="?m=2">next in list</a>] </b>
List:       openbsd-tech

  [not navigation] because this is code
<b>[<a href="?m=1">prev in list</a>] [<a href="?m=2">next in list</a>] </b>
</pre>"#;
        assert_eq!(
            to_markdown(html, Some(&base())),
            "```\nList:       openbsd-tech\n\n  [not navigation] because this is code\n```\n"
        );
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
    fn figure_captions_survive_as_captions() {
        // Pandoc marks a caption that repeats its alt text `aria-hidden`, as on bkovac.github.io.
        let html = r#"<figure><img src="https://example.com/thing.jpg" alt="The thing" width="1225" height="1496"><figcaption aria-hidden="true">The thing</figcaption></figure>"#;
        let markdown = to_markdown(html, Some(&base()));
        assert_eq!(
            markdown,
            "![The thing](https://example.com/thing.jpg)\\\nThe thing\n"
        );
        let rendered = render_markdown(&markdown);
        assert!(
            rendered.contains("<figure class=\"article-figure\">"),
            "{rendered}"
        );
        assert!(
            rendered.contains("<figcaption>The thing</figcaption>"),
            "{rendered}"
        );
    }

    #[test]
    fn markdown_rebuilds_numbered_code_listings_laid_out_as_tables() {
        // strix.ai renders snippets as a highlight.js table with a line-number gutter column.
        let html = r#"<div class="hljs"><table><tbody>
<tr><td class="linenos">1</td><td class="whitespace-pre"><span class="hljs-keyword">ARG</span> GITHUB_TOKEN</td></tr>
<tr><td class="linenos">2</td><td class="whitespace-pre">  if [[ "$X" != "" ]]; then \</td></tr>
<tr><td class="linenos">3</td><td class="whitespace-pre">  fi</td></tr>
</tbody></table></div>"#;
        assert_eq!(
            to_markdown(html, Some(&base())),
            "```\nARG GITHUB_TOKEN\n  if [[ \"$X\" != \"\" ]]; then \\\n  fi\n```\n"
        );
    }

    #[test]
    fn markdown_keeps_data_tables_that_merely_start_with_numbers() {
        let html = r#"<table><tbody>
<tr><td>1</td><td>First</td></tr>
<tr><td>7</td><td>Second</td></tr>
</tbody></table>"#;
        let markdown = to_markdown(html, Some(&base()));
        assert!(markdown.contains('|'), "{markdown}");
        assert!(!markdown.contains("```"), "{markdown}");
    }

    #[test]
    fn leaked_responsive_source_maps_resolve_to_a_real_image() {
        // blog.google shipped an unrendered template in `src`, so the URL carried a JSON object.
        let html = r#"<p><img alt="a chart showing the Speech to Speech Index" src="https://blog.google/models/gemini-3-8-live/{
            &quot;mobile&quot;: &quot;https://storage.googleapis.com/images/evals__S2S-inde.width-500.format-webp.webp&quot;,
            &quot;desktop&quot;: &quot;https://storage.googleapis.com/images/evals__S2S-ind.width-1000.format-webp.webp&quot;
          }"></p>"#;
        assert_eq!(
            to_markdown(html, Some(&base())),
            "![a chart showing the Speech to Speech Index](https://storage.googleapis.com/images/evals__S2S-ind.width-1000.format-webp.webp)\n"
        );
        // A URL that merely contains a brace is not a source map.
        let plain = r#"<p><img alt="Chart" src="https://example.com/a%7Bb%7D.png"></p>"#;
        assert!(
            to_markdown(plain, Some(&base())).contains("a%7Bb%7D.png"),
            "{plain}"
        );
    }

    #[test]
    fn markdown_drops_audio_players_and_the_chrome_around_them() {
        // blog.google's `uni-audio-player-tts` block, as Readability retains it.
        let html = r#"<p>Deck paragraph.</p>
<div data-component="uni-audio-player-tts" uni-l10n="{ &quot;timeText&quot;: &quot;[[duration]] minutes&quot; }" data-tts-audios="[{&quot;voice_name&quot;: &quot;Umbriel&quot;}]">
  <p><audio title="The Gemini app is now available for Windows">
      <source src="https://storage.googleapis.com/gweb-uniblog-publish-prod/media/tts_audio.mp3" type="audio/mpeg">
      <p>Your browser does not support the audio element.</p>
  </audio></p><div aria-label="">
        <p><span>
          Listen to article
        </span></p><p>[[duration]] minutes</p>
        <p><span tabindex="0" role="tooltip" aria-label="This content is generated by Google AI. Generative AI is experimental">
          <p>This content is generated by Google AI. Generative AI is experimental</p>
        </span></p></div>
</div>
<p>Article body.</p>"#;
        assert_eq!(
            to_markdown(html, Some(&base())),
            "Deck paragraph.\n\nArticle body.\n"
        );
    }

    #[test]
    fn markdown_keeps_prose_that_merely_surrounds_an_audio_clip() {
        let long = "Real prose that carries the argument of the article. ".repeat(12);
        let html = format!(
            r#"<div><p>{long}</p><audio><p>Your browser does not support the audio element.</p></audio></div>"#
        );
        let markdown = to_markdown(&html, Some(&base()));
        assert!(markdown.contains("Real prose"), "{markdown}");
        assert!(!markdown.contains("does not support"), "{markdown}");
    }

    #[test]
    fn markdown_collapses_stacked_and_trailing_separators() {
        // blog.google's hero block emits two rules around a section that carries no content.
        let html = "<p>Lead.</p><hr><hr><p>Body.</p><hr>";
        assert_eq!(
            to_markdown(html, Some(&base())),
            "Lead.\n\n* * *\n\nBody.\n"
        );
    }

    #[test]
    fn markdown_rebuilds_standard_document_footnotes() {
        // Hugo/Goldmark endnotes, as published by codyho.dev.
        let html = r##"<p>Documented blobs<sup id="fnref:1"><a href="#fn:1" class="footnote-ref" role="doc-noteref">1</a></sup> so we could ship<sup id="fnref:2"><a href="#fn:2" class="footnote-ref" role="doc-noteref">2</a></sup>.</p>
<h2 id="footnotes">Footnotes</h2>
<div class="footnotes" role="doc-endnotes"><hr><ol>
<li id="fn:1"><p>See <a href="/helpers">the helper programs</a>.&#160;<a href="#fnref:1" class="footnote-backref" role="doc-backlink">&#8617;</a></p></li>
<li id="fn:2"><p>A brief list:</p><p>Second paragraph.&#160;<a href="#fnref:2" class="footnote-backref" role="doc-backlink">&#8617;</a></p></li>
</ol></div>"##;

        assert_eq!(
            to_markdown(html, Some(&base())),
            concat!(
                "Documented blobs[^1] so we could ship[^2].\n\n",
                "## Footnotes\n\n",
                "[^1]: See [the helper programs](https://example.com/helpers).\n\n",
                "[^2]: A brief list:\n\n",
                "    Second paragraph.\n",
            )
        );
    }

    #[test]
    fn markdown_keeps_endnote_lists_nothing_points_at() {
        // Without a matching reference the note would vanish, so the original list is preserved.
        let html = r##"<p>Body with no reference.</p>
<div class="footnotes" role="doc-endnotes"><ol><li id="fn:1"><p>Orphan note.</p></li></ol></div>"##;
        let markdown = to_markdown(html, Some(&base()));
        assert!(markdown.contains("Orphan note."), "{markdown}");
        assert!(!markdown.contains("[^1]"), "{markdown}");
    }

    #[test]
    fn markdown_keeps_prose_links_that_point_into_the_endnotes() {
        let html = r##"<p>Read <a href="#fn:1">the appendix</a> first.<sup><a href="#fn:1">1</a></sup></p>
<div class="footnotes"><ol><li id="fn:1"><p>Appendix.</p></li></ol></div>"##;
        let markdown = to_markdown(html, Some(&base()));
        assert!(markdown.contains("[the appendix]("), "{markdown}");
        assert!(markdown.contains("first.[^1]"), "{markdown}");
        assert!(markdown.contains("[^1]: Appendix."), "{markdown}");
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
}
