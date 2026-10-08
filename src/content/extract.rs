//! Readability extraction of the primary article from a complete origin page, keeping the
//! script-drawn charts and share-named figure wrappers that Readability would otherwise lose.

use std::sync::OnceLock;

use anyhow::{Context, Result, bail};
use dom_smoothie::Readability;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use url::Url;

use super::escape_html;
use super::markdown::normalize_image_sources;
use super::scan::{attribute_value, parse_tag};
use super::strip::{html_to_text, sanitize, strip_active_content};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedArticle {
    pub html: String,
    pub image: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
}

/// Extract the primary article from a complete origin page. Readability intentionally does not
/// sanitize its output, so callers must still pass this HTML through the normal storage and
/// Markdown safety pipeline.
pub fn extract_article(page: &str, url: &Url) -> Result<ExtractedArticle> {
    let config = dom_smoothie::Config {
        max_elements_to_parse: 100_000,
        ..Default::default()
    };
    let page = super::normalize::publisher_html(page);
    let page = super::markdown::normalize_code_blocks(&page);
    let page = expand_embedded_charts(&page);
    let page = unwrap_noscript_prose(&page);
    if let Some(html) = code_file_listing(&page) {
        return Ok(ExtractedArticle {
            html,
            image: None,
            labels: Vec::new(),
        });
    }
    let mut readability = Readability::new(page.as_ref(), Some(url.as_str()), Some(config))
        .context("parsing the original article page")?;
    prepare_article_structure(&readability, url);
    let mut labels = Vec::new();
    for meta in readability.doc.select("meta[property]").iter() {
        if meta
            .attr("property")
            .is_some_and(|value| value.eq_ignore_ascii_case("article:tag"))
            && let Some(label) = meta
                .attr("content")
                .and_then(|value| publisher_label(&value))
        {
            labels.push(label);
        }
    }
    // rel=tag belongs to this article only; navigation and related-article panels are not topics.
    if let Some(article) = readability
        .doc
        .select("article")
        .iter()
        .find(|article| !article.ancestors(None).is("nav, aside"))
    {
        for link in article.select("a[rel~=tag]").iter() {
            if !link.ancestors(None).is("nav, aside")
                && link
                    .ancestors(None)
                    .nodes()
                    .iter()
                    .filter(|node| {
                        node.node_name()
                            .is_some_and(|name| name.as_ref() == "article")
                    })
                    .count()
                    == 1
                && let Some(label) = publisher_label(&link.text())
            {
                labels.push(label);
            }
        }
        for leading in [true, false] {
            let mut boundary = article.clone();
            loop {
                let children = boundary.children();
                let child = if leading {
                    children.first()
                } else {
                    children.last()
                };
                if child.is_empty() {
                    break;
                }
                if child.is("p, div, footer, ul") && explicit_tag_group(&child.html()) {
                    child.remove();
                } else if child.is("div, section, header, footer") {
                    boundary = child;
                } else {
                    break;
                }
            }
        }
    }
    let labels = crate::model::normalize_labels(labels);
    remove_translation_widgets(&readability);
    preserve_share_named_media_wrappers(&readability);
    preserve_cited_notes(&readability);
    // Figure filenames such as `replies.png` can resemble comment widgets to Readability.
    // Explicit image-and-caption structure supplies stronger evidence than those incidental IDs.
    readability.doc.select(
        "figure:has(img):has(figcaption), div.figure:has(img):has(figcaption, .photoCaption, .caption)",
    ).add_class("readability-content");
    // A caption that repeats its image's alt text is marked `aria-hidden` by some generators, and
    // Readability drops hidden nodes. On the page it is visible text, so keep it.
    readability
        .doc
        .select("figcaption[aria-hidden]")
        .remove_attr("aria-hidden");
    // Keep the semantic article above equally scored figure siblings. Otherwise a score tie
    // can select a figure and discard unscored neighboring figures.
    readability
        .doc
        .select("article:has(figure img), article:has(div.figure img)")
        .add_class("readability-content");
    // The dek lives beside the title, outside the region Readability keeps. Capture it while
    // the whole page is still in the document.
    let subtitle = article_subtitle(&readability);
    // Readability drops an inline chart without scoring it, and storage then strips every `<svg>`.
    // A sponsor logo inside the article is the post's, even when its list is classed `sponsor`.
    replace_inline_chart_svgs(&readability);
    keep_article_sponsor_images(&readability);
    let article = readability
        .parse()
        .context("extracting readable article content")?;
    let mut html = article.content.to_string();
    // A dek must not turn an empty extraction into an article: the page still failed.
    if !has_meaningful_extracted_content(&html, url) {
        bail!("extracted article has no readable content");
    }
    if let Some(subtitle) = subtitle.filter(|subtitle| !extracted_contains(&html, subtitle)) {
        html = format!("<p>{}</p>{html}", escape_html(&subtitle));
    }
    Ok(ExtractedArticle {
        html,
        image: article.image,
        labels,
    })
}

fn prepare_article_structure(readability: &Readability, url: &Url) {
    // Publisher breadcrumbs and jump menus remain navigation even inside an article wrapper.
    readability
        .doc
        .select(
            "[itemscope*='BreadcrumbList'], [itemtype*='BreadcrumbList'], uni-article-jumplinks",
        )
        .remove();

    if url
        .host_str()
        .is_some_and(|host| host == "arstechnica.com" || host.ends_with(".arstechnica.com"))
    {
        let blocks = readability
            .doc
            .select(".post-content.post-content-double")
            .iter()
            .collect::<Vec<_>>();
        if let Some((first, rest)) = blocks.split_first()
            && !rest.is_empty()
        {
            let combined = blocks
                .iter()
                .map(|block| block.inner_html().to_string())
                .collect::<Vec<_>>()
                .join("\n");
            first.set_html(combined);
            for block in rest {
                block.remove();
            }
        }
    }

    if url.host_str() == Some("slack-status.com") {
        readability
            .doc
            .select(".incident:has(.note)")
            .add_class("readability-content");
        for note in readability.doc.select(".incident .note").iter() {
            let badge = note.select("span").first();
            let label = badge.text().trim().to_string();
            let date = note.select(".date");
            let time = date.text().trim().to_string();
            if label.is_empty() || time.is_empty() {
                continue;
            }
            badge.remove();
            date.remove();
            note.select(".circle, .outer_circle, [aria-hidden='true']")
                .remove();
            note.prepend_html(format!(
                "<h4>{} · {}</h4>",
                escape_html(&label),
                escape_html(&time)
            ));
        }
        readability
            .doc
            .select(".incident .right, .incident_type_icon")
            .remove();
    }
}

/// GitHub gists (and blob views built the same way) show a file as a table of highlighted lines
/// under a header that names it. Readability scores that table below the discussion beneath it,
/// so the page would be archived as its comments; the files are the article. Several files become
/// named sections, a single file is just its code.
fn code_file_listing(page: &str) -> Option<String> {
    if !page.contains("blob-code-content") {
        return None;
    }
    let document = Html::parse_document(page);
    let (Ok(listings), Ok(lines), Ok(names)) = (
        Selector::parse(".blob-code-content"),
        Selector::parse("td.blob-code"),
        Selector::parse(
            ".file-header .gist-blob-name, .file-header [data-path], .file-header strong",
        ),
    ) else {
        return None;
    };
    let mut files = Vec::new();
    for listing in document.select(&listings) {
        let code = listing
            .select(&lines)
            .map(|line| line.text().collect::<String>())
            .map(|line| line.trim_end_matches(['\r', '\n']).to_string())
            .collect::<Vec<_>>();
        if code.iter().all(|line| line.trim().is_empty()) {
            continue;
        }
        let name = listing
            .ancestors()
            .filter_map(scraper::ElementRef::wrap)
            .find_map(|ancestor| ancestor.select(&names).next())
            .map(|name| name.text().collect::<String>().trim().to_string())
            .filter(|name| !name.is_empty());
        files.push((name, code.join("\n")));
    }
    if files.is_empty() {
        return None;
    }
    let mut html = String::new();
    let several = files.len() > 1;
    for (name, code) in files {
        if several && let Some(name) = &name {
            html.push_str(&format!("<h2>{}</h2>\n", escape_html(name)));
        }
        let language = name
            .as_deref()
            .and_then(|name| name.rsplit_once('.'))
            .and_then(|(_, extension)| super::highlight::hint_token(extension))
            .map(|language| format!(" class=\"language-{language}\" data-language=\"{language}\""))
            .unwrap_or_default();
        html.push_str(&format!(
            "<pre><code{language}>{}</code></pre>\n",
            escape_html(&code)
        ));
    }
    Some(html)
}

fn publisher_label(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty() && text.chars().count() <= 128 && !text.chars().any(char::is_control))
        .then(|| text.to_string())
}

/// Only a group made of explicitly marked tag links and a short optional label is metadata.
/// A short ordinary link list or prose surrounding a tag link stays in the article.
fn explicit_tag_group(html: &str) -> bool {
    let fragment = Html::parse_fragment(html);
    let mut outside = String::new();
    let mut tags = 0;
    for node in fragment.tree.nodes() {
        let inside_tag = node
            .ancestors()
            .filter_map(scraper::ElementRef::wrap)
            .any(|ancestor| {
                ancestor.value().name() == "a"
                    && ancestor
                        .value()
                        .attr("rel")
                        .is_some_and(|rel| rel.split_whitespace().any(|value| value == "tag"))
            });
        match node.value() {
            scraper::Node::Text(text) if !inside_tag => outside.push_str(text),
            scraper::Node::Element(element) if element.name() == "a" => {
                if !element
                    .attr("rel")
                    .is_some_and(|rel| rel.split_whitespace().any(|value| value == "tag"))
                    || publisher_label(
                        &scraper::ElementRef::wrap(node)
                            .map(|node| node.text().collect::<String>())
                            .unwrap_or_default(),
                    )
                    .is_none()
                {
                    return false;
                }
                tags += 1;
            }
            scraper::Node::Element(element)
                if !matches!(
                    element.name(),
                    "html"
                        | "body"
                        | "p"
                        | "div"
                        | "span"
                        | "footer"
                        | "ul"
                        | "li"
                        | "em"
                        | "strong"
                        | "br"
                ) =>
            {
                return false;
            }
            _ => {}
        }
    }
    let outside = outside
        .trim()
        .trim_end_matches(':')
        .trim()
        .to_ascii_lowercase();
    tags > 0 && matches!(outside.as_str(), "" | "tags" | "topics")
}

const MAX_CHART_ROWS: usize = 400;
const MAX_CHART_COLUMNS: usize = 12;

/// Charts that a page draws with JavaScript (Vega-Lite specs shipped in a Next.js payload, as on
/// openai.com) leave an empty placeholder in the server HTML. aggr never runs scripts, so the
/// chart's own data is written into the placeholder as a table: the numbers stay readable and
/// searchable even though the drawn chart cannot be reproduced.
fn expand_embedded_charts(page: &str) -> std::borrow::Cow<'_, str> {
    if !page.contains("vegaLiteSpec") || !page.contains("<div id=\"chart-") {
        return std::borrow::Cow::Borrowed(page);
    }
    let payload = next_flight_payload(page);
    let mut out = String::with_capacity(page.len() + 4096);
    let mut position = 0;
    while let Some(offset) = page[position..].find("<div id=\"chart-") {
        let start = position + offset;
        let Some(tag) = parse_tag(&page[start..]).and_then(|tag| tag.end) else {
            out.push_str(&page[position..start + 1]);
            position = start + 1;
            continue;
        };
        let open = &page[start..start + tag];
        let table = attribute_value(open, "id")
            .and_then(|id| embedded_chart_spec(&payload, id.strip_prefix("chart-")?))
            .and_then(|spec| chart_table(&spec));
        out.push_str(&page[position..start + tag]);
        if let Some(table) = table {
            out.push_str(&table);
        }
        position = start + tag;
    }
    out.push_str(&page[position..]);
    std::borrow::Cow::Owned(out)
}

/// Concatenate the React Flight chunks that Next.js streams through `self.__next_f.push`.
fn next_flight_payload(page: &str) -> String {
    let mut payload = String::new();
    let mut position = 0;
    while let Some(offset) = page[position..].find("self.__next_f.push([1,\"") {
        let start = position + offset + "self.__next_f.push([1,\"".len();
        let mut end = start;
        let bytes = page.as_bytes();
        while end < bytes.len() {
            match bytes[end] {
                b'\\' => end += 2,
                b'"' => break,
                _ => end += 1,
            }
        }
        if end > bytes.len() {
            break;
        }
        if let Ok(chunk) = serde_json::from_str::<String>(&format!("\"{}\"", &page[start..end])) {
            payload.push_str(&chunk);
        }
        position = end + 1;
    }
    payload
}

fn embedded_chart_spec(payload: &str, id: &str) -> Option<serde_json::Value> {
    let marker = format!("{{\"id\":{},\"data\":", serde_json::to_string(id).ok()?);
    let start = payload.find(&marker)?;
    let object = balanced_json_object(&payload[start..])?;
    let value: serde_json::Value = serde_json::from_str(object).ok()?;
    value.get("data")?.get("vegaLiteSpec").cloned()
}

pub(crate) fn balanced_json_object(text: &str) -> Option<&str> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (index, byte) in text.bytes().enumerate() {
        if in_string {
            match byte {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(&text[..=index]);
                }
            }
            _ => {}
        }
    }
    None
}

fn chart_table(spec: &serde_json::Value) -> Option<String> {
    let rows = spec.get("data")?.get("values")?.as_array()?;
    let rows = rows
        .iter()
        .filter_map(serde_json::Value::as_object)
        .take(MAX_CHART_ROWS)
        .collect::<Vec<_>>();
    if rows.is_empty() {
        return None;
    }
    // Text columns first, then numbers, so each row reads as a label followed by its values.
    let mut columns = rows
        .iter()
        .flat_map(|row| row.keys())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    columns.sort_by_key(|column| {
        rows.iter()
            .all(|row| row.get(*column).is_none_or(serde_json::Value::is_number))
    });
    columns.truncate(MAX_CHART_COLUMNS);
    let title = match spec.get("title") {
        Some(serde_json::Value::String(text)) => Some(text.clone()),
        Some(serde_json::Value::Object(title)) => {
            let text = title.get("text").and_then(serde_json::Value::as_str);
            let subtitle = title.get("subtitle").and_then(serde_json::Value::as_str);
            match (text, subtitle) {
                (Some(text), Some(subtitle)) => Some(format!("{text} — {subtitle}")),
                (Some(text), None) => Some(text.to_string()),
                _ => None,
            }
        }
        _ => None,
    };
    let mut html = String::from("<table class=\"chart-data\">");
    html.push_str("<caption>");
    html.push_str(&escape_html(&title.unwrap_or_else(|| "Chart".to_string())));
    html.push_str(" (chart data)</caption><thead><tr>");
    for column in &columns {
        let label = column.replace('_', " ");
        let mut label = label.chars();
        let label = match label.next() {
            Some(first) => first.to_uppercase().collect::<String>() + label.as_str(),
            None => String::new(),
        };
        html.push_str("<th>");
        html.push_str(&escape_html(&label));
        html.push_str("</th>");
    }
    html.push_str("</tr></thead><tbody>");
    for row in rows {
        html.push_str("<tr>");
        for column in &columns {
            html.push_str("<td>");
            html.push_str(&escape_html(&chart_cell(row.get(*column))));
            html.push_str("</td>");
        }
        html.push_str("</tr>");
    }
    html.push_str("</tbody></table>");
    Some(html)
}

/// gleam.run draws its benchmark figures with Lustre (`<svg class="gerbil">`) inside the article.
/// The bars are the figure: their lengths against the labeled axis are the numbers, and the
/// drawing itself cannot be stored. An icon `<svg>` has no `bar` rectangles and stays out.
fn replace_inline_chart_svgs(readability: &Readability) {
    let charts = readability
        .doc
        .select("article svg")
        .iter()
        .collect::<Vec<_>>();
    let tables = charts
        .iter()
        .map(|svg| svg_chart_table(&svg.html()))
        .collect::<Vec<_>>();
    for (svg, table) in charts.iter().zip(tables).rev() {
        if let Some(table) = table {
            svg.replace_with_html(table);
        }
    }
}

struct SvgText {
    x: f64,
    y: f64,
    label: String,
}

struct SvgBar {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

fn svg_chart_table(svg_html: &str) -> Option<String> {
    let fragment = Html::parse_fragment(svg_html);
    let text_selector = Selector::parse("text").ok()?;
    let bar_selector = Selector::parse("rect").ok()?;
    let texts = fragment
        .select(&text_selector)
        .filter_map(|node| {
            let label = normalize_space(&node.text().collect::<String>());
            if label.is_empty() {
                return None;
            }
            Some(SvgText {
                x: element_f64(node, "x")?,
                y: element_f64(node, "y")?,
                label,
            })
        })
        .collect::<Vec<_>>();
    let bars = fragment
        .select(&bar_selector)
        .filter(|node| {
            node.value().attr("class").is_some_and(|class| {
                class
                    .split_whitespace()
                    .any(|token| token.eq_ignore_ascii_case("bar"))
            })
        })
        .filter_map(|node| {
            Some(SvgBar {
                x: element_f64(node, "x").unwrap_or(0.0),
                y: element_f64(node, "y").unwrap_or(0.0),
                width: element_f64(node, "width")?,
                height: element_f64(node, "height")?,
            })
        })
        .filter(|bar| bar.width > 1.0 && bar.height > 1.0)
        .take(MAX_CHART_ROWS)
        .collect::<Vec<_>>();
    if bars.len() < 2 {
        return None;
    }
    let measurements = texts
        .iter()
        .filter_map(|text| {
            let (value, unit) = measurement(&text.label)?;
            Some((text, value, unit))
        })
        .collect::<Vec<_>>();
    let unit = measurements.first()?.2;
    if measurements.len() < 2 || measurements.iter().any(|(_, _, label)| *label != unit) {
        return None;
    }
    let horizontal = axis_span(measurements.iter().map(|(text, _, _)| text.y)) < 4.0
        && axis_span(measurements.iter().map(|(text, _, _)| text.x)) > 10.0;
    let vertical = axis_span(measurements.iter().map(|(text, _, _)| text.x)) < 4.0
        && axis_span(measurements.iter().map(|(text, _, _)| text.y)) > 10.0;
    let position = |text: &SvgText| if horizontal { text.x } else { text.y };
    if !horizontal && !vertical {
        return None;
    }
    let mut axis = measurements
        .iter()
        .map(|(text, value, _)| (position(text), *value))
        .collect::<Vec<_>>();
    axis.sort_by(|left, right| left.0.total_cmp(&right.0));
    let (origin, origin_value) = *axis.first()?;
    let (end, end_value) = *axis.last()?;
    let span = end - origin;
    if span.abs() < 1.0 {
        return None;
    }
    let at = |pos: f64| origin_value + (pos - origin) * (end_value - origin_value) / span;
    let categories = texts
        .iter()
        .filter(|text| {
            measurement(&text.label).is_none() && text.label.chars().any(char::is_alphabetic)
        })
        .collect::<Vec<_>>();
    if categories.len() < 2 {
        return None;
    }
    let mut pairs = Vec::new();
    for (bar_index, bar) in bars.iter().enumerate() {
        let center = if horizontal {
            bar.y + bar.height / 2.0
        } else {
            bar.x + bar.width / 2.0
        };
        for (label_index, label) in categories.iter().enumerate() {
            let label_pos = if horizontal { label.y } else { label.x };
            pairs.push(((center - label_pos).abs(), bar_index, label_index));
        }
    }
    pairs.sort_by(|left, right| left.0.total_cmp(&right.0));
    let mut used_bars = vec![false; bars.len()];
    let mut used_labels = vec![false; categories.len()];
    let mut rows = Vec::new();
    for (distance, bar_index, label_index) in pairs {
        if used_bars[bar_index] || used_labels[label_index] {
            continue;
        }
        let bar = &bars[bar_index];
        let thickness = if horizontal { bar.height } else { bar.width };
        if distance > thickness.max(8.0) {
            continue;
        }
        let value = if horizontal {
            (at(bar.x + bar.width) - at(bar.x)).abs()
        } else {
            (at(bar.y) - at(bar.y + bar.height)).abs()
        };
        used_bars[bar_index] = true;
        used_labels[label_index] = true;
        let order = if horizontal { bar.y } else { bar.x };
        rows.push((order, categories[label_index].label.clone(), value));
    }
    if rows.len() < 2 {
        return None;
    }
    rows.sort_by(|left, right| left.0.total_cmp(&right.0));
    let mut html = String::from(
        "<table class=\"chart-data\"><caption>Chart (chart data)</caption><thead><tr><th>Label</th><th>",
    );
    html.push_str(&escape_html(&chart_value_heading(unit)));
    html.push_str("</th></tr></thead><tbody>");
    for (_, label, value) in rows {
        html.push_str("<tr><td>");
        html.push_str(&escape_html(&label));
        html.push_str("</td><td>");
        html.push_str(&escape_html(&format_chart_value(value)));
        html.push_str("</td></tr>");
    }
    html.push_str("</tbody></table>");
    Some(html)
}

fn element_f64(node: scraper::ElementRef<'_>, name: &str) -> Option<f64> {
    node.value().attr(name)?.trim().parse().ok()
}

fn axis_span(values: impl Iterator<Item = f64>) -> f64 {
    let mut min = f64::MAX;
    let mut max = f64::MIN;
    for value in values {
        min = min.min(value);
        max = max.max(value);
    }
    if min > max { 0.0 } else { max - min }
}

/// `100ms`, `1.5s`, `50%`: an axis tick, not a category such as `Gleam v1.19`.
fn measurement(text: &str) -> Option<(f64, &str)> {
    let text = text.trim();
    let split = text
        .find(|character: char| {
            !(character.is_ascii_digit() || character == '.' || character == '-')
        })
        .unwrap_or(text.len());
    if split == 0 {
        return None;
    }
    let value = text[..split].parse().ok()?;
    let unit = text[split..].trim();
    (unit.len() <= 8
        && unit
            .chars()
            .all(|character| character.is_ascii_alphabetic() || character == '%'))
    .then_some((value, unit))
}

fn chart_value_heading(unit: &str) -> String {
    match unit {
        "ms" => "Time (ms)".to_string(),
        "s" => "Time (s)".to_string(),
        "%" => "Percent".to_string(),
        "" => "Value".to_string(),
        other => other.to_string(),
    }
}

fn format_chart_value(value: f64) -> String {
    let rounded = value.round();
    if (value - rounded).abs() < 0.05 {
        format!("{}", rounded as i64)
    } else {
        let text = format!("{value:.2}");
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// `ul.top-sponsors` on gleam.run is the logo under "special thanks to our top sponsor".
/// Readability treats any class containing `sponsor` as an ad, except on an anchor, which is
/// why the GitHub mark survives and the Lambda Class mark does not. A pitch that has something
/// to say is still an ad.
fn keep_article_sponsor_images(readability: &Readability) {
    let credits = readability
        .doc
        .select("article [class*='sponsor' i], article [id*='sponsor' i]")
        .iter()
        .filter(|node| !node.is("a, img, svg"))
        .filter(|node| !node.select("img[src]").is_empty())
        .filter(|node| node.text().split_whitespace().count() <= 12)
        .collect::<Vec<_>>();
    for node in credits {
        for name in ["class", "id"] {
            let Some(value) = node.attr(name).map(|value| value.to_string()) else {
                continue;
            };
            let tokens = value.split_whitespace().collect::<Vec<_>>();
            let kept = tokens
                .iter()
                .copied()
                .filter(|token| !token.to_ascii_lowercase().contains("sponsor"))
                .collect::<Vec<_>>();
            if kept.len() == tokens.len() {
                continue;
            }
            if kept.is_empty() {
                node.remove_attr(name);
            } else {
                node.set_attr(name, &kept.join(" "));
            }
        }
    }
}

fn chart_cell(value: Option<&serde_json::Value>) -> String {
    match value {
        None | Some(serde_json::Value::Null) => String::new(),
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(serde_json::Value::Bool(flag)) => flag.to_string(),
        Some(serde_json::Value::Number(number)) => match number.as_f64() {
            Some(float) if float.fract() != 0.0 => {
                let rounded = format!("{float:.4}");
                rounded
                    .trim_end_matches('0')
                    .trim_end_matches('.')
                    .to_string()
            }
            _ => number.to_string(),
        },
        Some(other) => other.to_string(),
    }
}

/// aggr reads pages the way a browser without scripting does, so the `<noscript>` fallback a
/// page carries is what it is meant to see. Readability discards the element, so a fallback that
/// holds prose (paragraphs or headings, not a tracking pixel) is unwrapped into the document.
fn unwrap_noscript_prose(page: &str) -> std::borrow::Cow<'_, str> {
    if !page.contains("<noscript") {
        return std::borrow::Cow::Borrowed(page);
    }
    let mut out = String::with_capacity(page.len());
    let mut position = 0;
    while let Some(start) = page[position..].find("<noscript").map(|at| position + at) {
        let Some(tag) = parse_tag(&page[start..]) else {
            out.push_str(&page[position..start + 1]);
            position = start + 1;
            continue;
        };
        let Some(tag_len) = tag.end else {
            break;
        };
        let inner_start = start + tag_len;
        let Some(close) = page[inner_start..].find("</noscript") else {
            break;
        };
        let inner = &page[inner_start..inner_start + close];
        let after = inner_start
            + close
            + page[inner_start + close..]
                .find('>')
                .map_or(0, |offset| offset + 1);
        out.push_str(&page[position..start]);
        let prose = inner.contains("<p") || ["<h1", "<h2", "<h3"].iter().any(|h| inner.contains(h));
        if prose {
            out.push_str(inner);
        }
        position = after;
    }
    out.push_str(&page[position..]);
    std::borrow::Cow::Owned(out)
}

/// A note the prose cites by number is content however link-heavy it is: a citation is often
/// no more than a title and its link, which Readability's link-density cleaning would discard,
/// leaving the reference pointing at nothing.
fn preserve_cited_notes(readability: &Readability) {
    for anchor in readability.doc.select("a[href^='#']").iter() {
        let Some(href) = anchor.attr("href") else {
            continue;
        };
        let id = href.trim_start_matches('#');
        if id.is_empty()
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':' | '.'))
        {
            continue;
        }
        let label = anchor.text();
        let label = label.trim().trim_matches(['[', ']', '(', ')']);
        if label.is_empty() || !label.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        // The note and the block that groups the notes: a short list of citations is
        // link-dense as a whole too.
        let note = readability.doc.select(&format!("[id=\"{id}\"]"));
        note.add_class("readability-content");
        note.parent().add_class("readability-content");
    }
}

/// Some publishers wrap every article figure in a `…-sharesheet` container. Readability weighs
/// any class or id mentioning sharing negatively and drops such low-text wrappers, taking the
/// picture with them. A wrapper that holds real media and no sharing links is presentation, not a
/// share widget; it is renamed to a neutral, content-positive class.
fn preserve_share_named_media_wrappers(readability: &Readability) {
    // A sharing endpoint says so in its path, or is handed the page's own address to pass on.
    let share_link = |href: &str| {
        let lower = href.to_ascii_lowercase();
        ["share", "/intent/", "mailto:"]
            .iter()
            .any(|marker| lower.contains(marker))
            || Url::parse(href).is_ok_and(|url| {
                url.query_pairs()
                    .any(|(_, value)| value.starts_with("http://") || value.starts_with("https://"))
            })
    };
    for wrapper in readability
        .doc
        .select("[class*=\"share\" i]:has(img, picture), [id*=\"share\" i]:has(img, picture)")
        .iter()
    {
        let links = wrapper
            .select("a[href]")
            .iter()
            .filter_map(|link| link.attr("href").map(|href| href.to_string()))
            .collect::<Vec<_>>();
        if links.iter().any(|href| share_link(href)) {
            continue;
        }
        let class = wrapper.attr("class").map(|value| value.to_string());
        let kept = class
            .as_deref()
            .unwrap_or_default()
            .split_ascii_whitespace()
            .filter(|token| !token.to_ascii_lowercase().contains("share"))
            .chain(std::iter::once("aggr-figure-content"))
            .collect::<Vec<_>>()
            .join(" ");
        wrapper.set_attr("class", &kept);
        if wrapper
            .attr("id")
            .is_some_and(|id| id.to_ascii_lowercase().contains("share"))
        {
            wrapper.remove_attr("id");
        }
    }
}

/// A publisher's "translate this article" block (diff.wikimedia.org's `#translate-post`).
/// `notranslate` does not count: that token opts text out of translation, and it does not start
/// with `translate`. A long section that happens to be named like a widget stays; the solicitation
/// is a short block whose own text is about translating.
const TRANSLATION_WIDGET_WORDS: std::ops::RangeInclusive<usize> = 1..=90;

fn names_translation_widget(value: &str) -> bool {
    value
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|token| token.to_ascii_lowercase().starts_with("translate"))
}

fn remove_translation_widgets(readability: &Readability) {
    let widgets = readability
        .doc
        .select("[class*='translate' i], [id*='translate' i]")
        .iter()
        .filter(|node| {
            node.attr("class")
                .is_some_and(|value| names_translation_widget(&value))
                || node
                    .attr("id")
                    .is_some_and(|value| names_translation_widget(&value))
        })
        .filter(|node| {
            !node.ancestors(None).iter().any(|ancestor| {
                ancestor
                    .attr("class")
                    .is_some_and(|value| names_translation_widget(&value))
                    || ancestor
                        .attr("id")
                        .is_some_and(|value| names_translation_widget(&value))
            })
        })
        .filter(|node| {
            let text = node.text();
            TRANSLATION_WIDGET_WORDS.contains(&text.split_whitespace().count())
                && text.to_ascii_lowercase().contains("translat")
        })
        .collect::<Vec<_>>();
    for widget in widgets {
        widget.remove();
    }
}

/// The sentence under the title. Readability never sees it when the publisher leaves it in the
/// headline rather than the article, and a feed summary is not a substitute: most of those are
/// teasers. The visible paragraph wins, including a short release line classed as the subtitle
/// (`hero-subtitle` on gleam.run). A byline or a date in that spot is not one. schema.org
/// `alternativeHeadline` covers a page that states the same sentence only in its structured data.
fn article_subtitle(readability: &Readability) -> Option<String> {
    let subtitle = headline_dek(readability).or_else(|| alternative_headline(readability))?;
    let title = readability
        .doc
        .select("h1")
        .iter()
        .next()
        .map(|heading| normalize_space(&heading.text()))
        .unwrap_or_default();
    (!same_folded(&subtitle, &title)).then_some(subtitle)
}

fn headline_dek(readability: &Readability) -> Option<String> {
    for node in readability
        .doc
        .select(
            "[class*='headline' i] > p, [class*='standfirst' i], [class*='dek' i], [class*='subtitle' i], [id*='subtitle' i]",
        )
        .iter()
    {
        if node.ancestors(None).is("nav, aside, footer") {
            continue;
        }
        let standfirst = node
            .attr("class")
            .is_some_and(|value| names_standfirst_or_dek(&value))
            || node
                .attr("id")
                .is_some_and(|value| names_standfirst_or_dek(&value));
        let release = node.attr("class").is_some_and(|value| names_subtitle(&value))
            || node.attr("id").is_some_and(|value| names_subtitle(&value));
        let parent = node.parent();
        let follows_title = node.prev_sibling().is("h1");
        // `hero-subtitle` (gleam.run) is the release line under the title. The same letters in
        // `subtitles` name a captions track, which is not that line.
        if release && !standfirst {
            let text = normalize_space(&node.text());
            if follows_title
                && !is_attribution(&text)
                && (is_subtitle(&text) || is_short_subtitle(&text))
            {
                return Some(text);
            }
            continue;
        }
        if !standfirst && !parent.is("h1") && parent.select("h1").is_empty() {
            continue;
        }
        let text = normalize_space(&node.text());
        if is_subtitle(&text) || (standfirst && follows_title && is_short_subtitle(&text)) {
            return Some(text);
        }
    }
    None
}

fn names_standfirst_or_dek(value: &str) -> bool {
    name_tokens(value).any(|token| token == "dek" || token.contains("standfirst"))
}

/// A class or id token that is the subtitle, not a captions track (`subtitles`) or a longer word.
fn names_subtitle(value: &str) -> bool {
    name_tokens(value).any(|token| token == "subtitle")
}

fn name_tokens(value: &str) -> impl Iterator<Item = String> + '_ {
    value
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(|token| token.to_ascii_lowercase())
}

fn alternative_headline(readability: &Readability) -> Option<String> {
    for script in readability
        .doc
        .select("script[type='application/ld+json']")
        .iter()
    {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&script.text()) else {
            continue;
        };
        if let Some(subtitle) = json_alternative_headline(&value, &mut 0) {
            return Some(subtitle);
        }
    }
    None
}

fn json_alternative_headline(value: &serde_json::Value, steps: &mut usize) -> Option<String> {
    *steps += 1;
    if *steps > 512 {
        return None;
    }
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                if let Some(found) = json_alternative_headline(item, steps) {
                    return Some(found);
                }
            }
            None
        }
        serde_json::Value::Object(map) => {
            if let Some(text) = map
                .get("alternativeHeadline")
                .and_then(serde_json::Value::as_str)
            {
                let text = normalize_space(text);
                if is_subtitle(&text) {
                    return Some(text);
                }
            }
            map.get("@graph")
                .and_then(|graph| json_alternative_headline(graph, steps))
        }
        _ => None,
    }
}

const SUBTITLE_CHARS: std::ops::RangeInclusive<usize> = 12..=360;
const SUBTITLE_WORDS: std::ops::RangeInclusive<usize> = 5..=50;
const SHORT_SUBTITLE_WORDS: std::ops::RangeInclusive<usize> = 2..=4;

fn is_subtitle(text: &str) -> bool {
    SUBTITLE_CHARS.contains(&text.chars().count())
        && SUBTITLE_WORDS.contains(&text.split_whitespace().count())
        && subtitle_text(text)
}

/// `Gleam v1.19.0 released!`: a release line is shorter than a dek. The five-word floor stays for
/// an unlabelled paragraph, where `By Jane Doe` would otherwise qualify.
fn is_short_subtitle(text: &str) -> bool {
    SUBTITLE_CHARS.contains(&text.chars().count())
        && SHORT_SUBTITLE_WORDS.contains(&text.split_whitespace().count())
        && subtitle_text(text)
        && !is_attribution(text)
}

fn subtitle_text(text: &str) -> bool {
    text.chars().any(char::is_alphabetic) && !text.to_ascii_lowercase().starts_with("http")
}

/// A byline or a date sitting where a subtitle would: `By Louis Pilfold`, `05 October, 2026`.
/// The `by` prefix only counts on a short line, so a sentence that opens with "By the time" stays.
fn is_attribution(text: &str) -> bool {
    let lower = fold_text(text);
    if SHORT_SUBTITLE_WORDS.contains(&lower.split_whitespace().count())
        && (lower.starts_with("by ")
            || lower.starts_with("written by ")
            || lower.starts_with("posted by "))
    {
        return true;
    }
    if super::dates::parse(text).is_some() {
        return true;
    }
    lower
        .split_once(" by ")
        .is_some_and(|(date, _)| super::dates::parse(date).is_some())
}

fn normalize_space(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn fold_text(value: &str) -> String {
    normalize_space(value)
        .replace(['\u{2019}', '\u{2018}'], "'")
        .to_lowercase()
}

fn same_folded(left: &str, right: &str) -> bool {
    fold_text(left) == fold_text(right)
}

/// The extracted article already says this, whatever quote mark or capitalisation it used.
fn extracted_contains(html: &str, subtitle: &str) -> bool {
    let needle = fold_text(subtitle);
    !needle.is_empty() && fold_text(&html_to_text(html)).contains(&needle)
}

/// Messages a page shows while its scripts fetch the real content (harnesstax.github.io renders
/// its post into `<p class="loading">loading…</p>`; a live dashboard leaves `reconnecting…`).
/// Stored as an article they would read that way forever, so a capture that says nothing else is
/// no capture at all and the item keeps its feed content, which the daily retry can upgrade later.
const LOADING_MESSAGES: [&str; 9] = [
    "loading",
    "reconnecting",
    "connecting",
    "please wait",
    "just a moment",
    "one moment please",
    "javascript is required",
    "please enable javascript",
    "you need to enable javascript to run this app",
];

pub(super) fn is_loading_message(text: &str) -> bool {
    let text = text
        .trim()
        .trim_end_matches(|ch: char| !ch.is_alphanumeric())
        .to_lowercase();
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    LOADING_MESSAGES.contains(&text.as_str())
}

/// An archived body that is only a loading message (with at most the page's tab labels around
/// it): the capture happened before this placeholder check existed, or the extractor let a
/// different placeholder through. Such an item is retried like a feed-only capture.
pub fn is_placeholder_body(markdown: &str) -> bool {
    if markdown.len() > 400 {
        return false;
    }
    let blocks = markdown
        .split("\n\n")
        .map(|block| html_to_text(&super::render_markdown(block)))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>();
    let words: usize = blocks
        .iter()
        .map(|text| text.split_whitespace().count())
        .sum();
    words <= 8 && blocks.iter().any(|text| is_loading_message(text))
}

/// The extracted text outside `header`, `nav` and `footer`: when Readability finds no scored
/// candidate it returns the whole body, and a script-driven page's body is only its chrome.
fn text_outside_chrome(html: &str) -> String {
    let fragment = Html::parse_fragment(html);
    let mut text = String::new();
    for node in fragment.tree.nodes() {
        let Some(value) = node.value().as_text() else {
            continue;
        };
        let chrome = node.ancestors().any(|ancestor| {
            ancestor
                .value()
                .as_element()
                .is_some_and(|element| matches!(element.name(), "header" | "nav" | "footer"))
        });
        if !chrome {
            text.push_str(value);
            text.push(' ');
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn has_meaningful_extracted_content(html: &str, base: &Url) -> bool {
    let text = text_outside_chrome(html);
    if !text.is_empty() && !is_loading_message(&text) {
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

/// The runs of the feed entry's own words that the origin page repeats, normalised for
/// comparison: overlapping windows of [`SHINGLE_WORDS`] words, kept only when the page carries at
/// least [`PAGE_COVERAGE`] of them or [`MIN_PRESENT_SHINGLES`] of them in a row's worth. Empty when the entry carries no content worth the name, so a
/// title-only or teaser feed never triggers the check.
pub fn feed_content_on_page(feed_html: &str, page: &str) -> Vec<String> {
    let shingles = shingles(&tokens(&html_to_text(feed_html)));
    if shingles.is_empty() {
        return Vec::new();
    }
    let page_words = padded(&tokens(&document_text(page)));
    let present: Vec<String> = shingles
        .iter()
        .filter(|shingle| page_words.contains(&format!(" {shingle} ")))
        .cloned()
        .collect();
    // Half of the entry, or ten consecutive words of it: a feed wraps a short note in a label
    // and a tag list that the page words differently, so the note alone must be able to count.
    if present.len() * 100 < shingles.len() * PAGE_COVERAGE && present.len() < MIN_PRESENT_SHINGLES
    {
        return Vec::new();
    }
    present
}

/// Whether Readability picked a region of the page that is not the item: the feed entry's own
/// words are on the page (`present`, from [`feed_content_on_page`]) but almost none of them made
/// it into the extraction. A short note (a release, a link post) loses to a sidebar or a
/// subscription box that way. An extraction longer than both three times the entry and
/// [`LONG_EXTRACTION_WORDS`] is trusted even so, because a feed teaser is often the lede
/// Readability legitimately drops.
pub fn extraction_misses_feed_content(
    extracted_html: &str,
    feed_html: &str,
    present: &[String],
) -> bool {
    if present.is_empty() {
        return false;
    }
    let extracted_tokens = tokens(&html_to_text(extracted_html));
    let extracted = padded(&extracted_tokens);
    let kept = present
        .iter()
        .filter(|shingle| extracted.contains(&format!(" {shingle} ")))
        .count();
    if kept * 100 >= present.len() * EXTRACTED_COVERAGE {
        return false;
    }
    let feed_words = tokens(&html_to_text(feed_html)).len();
    extracted_tokens.len() <= (3 * feed_words + 40).max(LONG_EXTRACTION_WORDS)
}

const MIN_FEED_WORDS: usize = 6;
const SHINGLE_WORDS: usize = 8;
/// Percentage of the entry's shingles the page must carry before the entry counts as being on it.
const PAGE_COVERAGE: usize = 50;
/// Or this many consecutive shingles: `SHINGLE_WORDS + MIN_PRESENT_SHINGLES - 1` words in a row.
const MIN_PRESENT_SHINGLES: usize = 3;
/// Percentage of those shingles the extraction must keep to count as the same text.
const EXTRACTED_COVERAGE: usize = 20;
/// An extraction at least this long is an article whatever the entry says: a sidebar or a
/// subscription box rarely runs this long, an article whose lede Readability dropped often does.
const LONG_EXTRACTION_WORDS: usize = 300;

/// Lowercase words with punctuation stripped, so `0.65.5`, `rows,` and `GHSA-h547.` compare the
/// way a reader sees them whatever the markup around them did to spacing.
fn tokens(text: &str) -> Vec<String> {
    text.split_whitespace()
        .filter_map(|word| {
            let word: String = word
                .chars()
                .filter(|ch| ch.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect();
            (!word.is_empty()).then_some(word)
        })
        .collect()
}

fn shingles(words: &[String]) -> Vec<String> {
    if words.len() < MIN_FEED_WORDS {
        return Vec::new();
    }
    if words.len() <= SHINGLE_WORDS {
        return vec![words.join(" ")];
    }
    words
        .windows(SHINGLE_WORDS)
        .map(|window| window.join(" "))
        .collect()
}

fn padded(words: &[String]) -> String {
    format!(" {} ", words.join(" "))
}

/// Every text node of a complete page outside scripts, styles and templates.
fn document_text(page: &str) -> String {
    let document = Html::parse_document(page);
    let mut text = String::new();
    for node in document.tree.nodes() {
        let Some(value) = node.value().as_text() else {
            continue;
        };
        let inert = node.ancestors().any(|ancestor| {
            ancestor.value().as_element().is_some_and(|element| {
                matches!(element.name(), "script" | "style" | "noscript" | "template")
            })
        });
        if !inert {
            text.push_str(value);
            text.push(' ');
        }
    }
    text
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

#[cfg(test)]
mod tests {
    #[test]
    fn cited_notes_survive_link_density_cleaning() {
        // libroot.org: a citation that is a title and its link, sitting after enough prose.
        // (Readability unwraps a container holding a single note; two notes keep theirs.)
        let prose = "<p>Snowden first reached out to Glenn Greenwald in December, requesting that he set up a secure communication channel, though Greenwald did not know how to do so at the time and let the request lapse for several weeks.<a class=\"note-source\" href=\"#n13\">[13]</a> He then turned to Laura Poitras.<a href=\"#n14\">[14]</a></p>".repeat(3);
        let page = format!(
            "<html><head><title>Archive</title></head><body><article>{prose}<div class=\"notes\"><div class=\"citation\" id=\"n13\"><div class=\"marker\">[13]:</div><div class=\"text\">Menschen Machen Medien, \"<a href=\"https://mmm.example/snowden\">Snowden und die Datenmisshandlung</a>\", June 2023.</div></div><div class=\"citation\" id=\"n14\"><div class=\"marker\">[14]:</div><div class=\"text\"><a href=\"https://x.example/status/1\">On Twitter</a>, 14 March 2019 (<a href=\"https://archive.example/x\">archived</a>).</div></div></div></article></body></html>"
        );
        let url = Url::parse("https://libroot.example/posts/archive").unwrap();
        let extracted = super::extract_article(&page, &url).unwrap();
        assert!(extracted.html.contains("id=\"n13\""), "{}", extracted.html);
        assert!(
            extracted.html.contains("Menschen Machen Medien"),
            "{}",
            extracted.html
        );
        let markdown = crate::content::to_markdown(&extracted.html, Some(&url));
        assert!(
            markdown.contains("weeks.[^1] He then turned to Laura Poitras.[^2]"),
            "{markdown}"
        );
        assert!(markdown.contains("[^1]: Menschen Machen Medien, \"[Snowden und die Datenmisshandlung](https://mmm.example/snowden)\", June 2023."), "{markdown}");
        assert!(markdown.contains("[^2]: [On Twitter](https://x.example/status/1), 14 March 2019 ([archived](https://archive.example/x))."), "{markdown}");
    }

    #[test]
    fn gist_file_listings_are_the_article_not_their_comments() {
        let page = concat!(
            "<html><head><title>Laya on Mac · GitHub</title></head><body>",
            "<div class=\"file\"><div class=\"file-header\"><a href=\"#file-laya-sh\"><strong class=\"gist-blob-name\">\n laya.sh\n</strong></a></div>",
            "<div class=\"blob-wrapper\"><div class=\"blob-code-content\"><template class=\"js-file-alert-template\"><div class=\"flash\">This file contains hidden Unicode text.</div></template>",
            "<table><tr><td class=\"blob-num\">1</td><td class=\"blob-code blob-code-inner js-file-line\">curl <span class=\"pl-s\">https://laya.example/ai/run</span> \\</td></tr>",
            "<tr><td class=\"blob-num\">2</td><td class=\"blob-code blob-code-inner js-file-line\">  -H <span class=\"pl-s\">&quot;Content-Type: application/json&quot;</span></td></tr>",
            "<tr><td class=\"blob-num\">3</td><td class=\"blob-code blob-code-inner js-file-line\"></td></tr>",
            "<tr><td class=\"blob-num\">4</td><td class=\"blob-code blob-code-inner js-file-line\">echo &lt;done&gt;</td></tr></table></div></div></div>",
            "<div class=\"js-comment-container\"><div class=\"comment-body markdown-body\"><p>Nice! I just wanted to see what Laya does and put it behind an API, which turned out to be a long enough comment for Readability to prefer over the code.</p></div></div>",
            "</body></html>"
        );
        let url =
            Url::parse("https://gist.github.com/fordnox/e592d0f68b543fd044be8e6d040863a0").unwrap();
        let extracted = super::extract_article(page, &url).unwrap();
        assert_eq!(
            extracted.html,
            "<pre><code class=\"language-sh\" data-language=\"sh\">curl https://laya.example/ai/run \\\n  -H &quot;Content-Type: application/json&quot;\n\necho &lt;done&gt;</code></pre>\n"
        );
        assert!(extracted.image.is_none());
        let markdown = crate::content::to_markdown(&extracted.html, Some(&url));
        assert!(markdown.contains("```sh\ncurl https://laya.example/ai/run \\\n  -H \"Content-Type: application/json\"\n\necho <done>\n```"), "{markdown}");
        assert!(!markdown.contains("Nice!"), "{markdown}");

        // Several files keep their names as sections; a page without line tables is untouched.
        let two = page.replace("</div></div></div>", "</div></div></div><div class=\"file\"><div class=\"file-header\"><strong class=\"gist-blob-name\">notes.md</strong></div><div class=\"blob-code-content\"><table><tr><td class=\"blob-code\"># Notes</td></tr></table></div></div>");
        let extracted = super::extract_article(&two, &url).unwrap();
        assert!(
            extracted.html.starts_with("<h2>laya.sh</h2>\n<pre>"),
            "{}",
            extracted.html
        );
        assert!(extracted.html.contains("<h2>notes.md</h2>\n<pre><code class=\"language-md\" data-language=\"md\"># Notes</code></pre>"), "{}", extracted.html);
        assert!(
            super::code_file_listing("<html><body><p>No listing here.</p></body></html>").is_none()
        );
    }

    #[test]
    fn article_tags_remove_only_explicit_boundary_link_groups() {
        let page = "<html><title>Article</title><article><p><a rel='tag'>OpeningTag</a></p><h1>Article</h1><p>This article discusses <a rel='tag'>InlineTag</a> in ordinary prose and should retain that sentence as useful content. Its explanation includes enough substance for the article extractor.</p><p><a href='/other'>Ordinary link</a></p><p>Tags: <a rel='tag'>ClosingTag</a></p></article></html>";
        let extracted =
            extract_article(page, &url::Url::parse("https://example.com/post").unwrap()).unwrap();
        assert_eq!(extracted.labels, ["closingtag", "inlinetag", "openingtag"]);
        assert!(!extracted.html.contains("OpeningTag"));
        assert!(!extracted.html.contains("ClosingTag"));
        assert!(extracted.html.contains("InlineTag"));
        assert!(extracted.html.contains("Ordinary link"));
    }

    #[test]
    fn article_tags_capture_explicit_publisher_metadata_outside_the_body() {
        let page = "<html><head><title>MCP</title><meta property='article:tag' content='MCP'><meta property='article:tag' content='AI'><meta property='article:tag' content='ai'><meta name='keywords' content='generic, navigation'></head><body><nav><a rel='tag'>Site navigation</a></nav><article><h1>MCP</h1><p>The full article discusses the protocol and explains why its original design causes problems for people using these systems.</p><footer><a rel='tag' href='/tags/security'>Security</a></footer><aside><a rel='tag'>Related post</a></aside></article></body></html>";
        let extracted =
            extract_article(page, &url::Url::parse("https://example.com/post").unwrap()).unwrap();
        assert_eq!(extracted.labels, ["ai", "mcp", "security"]);
        let unrelated = page
            .replace("property='article:tag'", "property='site:tag'")
            .replace("rel='tag' href='/tags/security'", "href='/tags/security'");
        assert!(
            extract_article(
                &unrelated,
                &url::Url::parse("https://example.com/post").unwrap()
            )
            .unwrap()
            .labels
            .is_empty()
        );
    }

    #[test]
    fn noscript_prose_is_the_article_for_a_reader_without_scripts() {
        // onionfutures.com: the whole page is a script shell whose article sits in <noscript>.
        let filler = "The company offers private contracts for the future delivery of onions. ";
        let page = format!(
            "<!doctype html><html><head><title>Onion Futures</title><noscript><img src=\"/pixel.gif\"></noscript></head><body><noscript><h1>Onion Futures</h1><p>{}</p><section><h2>FAQ</h2><h3>Is this legal?</h3><p>{}</p></section></noscript><script src=\"/app.js\"></script></body></html>",
            filler.repeat(6),
            filler.repeat(6)
        );
        let article = extract_article(&page, &base()).unwrap();
        assert!(article.html.contains("Is this legal?"), "{}", article.html);
        assert!(!article.html.contains("pixel.gif"), "{}", article.html);
    }

    #[test]
    fn an_extraction_that_skipped_the_feed_entry_is_recognized() {
        use super::{extraction_misses_feed_content, feed_content_on_page};

        // simonwillison.net: a short "beat" whose page also carries a sidebar with the byline,
        // tags and a subscription box, which Readability preferred.
        // The feed summary runs the release label into the note ("0.65.5 Security fix"), while
        // the page puts a tagline between them: word runs, not sentences, are what match.
        let feed = "Release: datasette 0.65.5 Security fix for an issue where a trailing newline in a requested table name could bypass table permissions and expose private rows, reported by someone in GHSA-h547-rmjf-5m2m. Tags: security, datasette";
        let page = "<html><body><div id=\"primary\"><div class=\"beat\"><span>Release</span> <span>datasette 0.65.5</span> <span>&mdash; An open source multi-tool for exploring and publishing data</span><div class=\"beat-note\"><p>Security fix for an issue where a trailing newline in a requested table name could bypass table permissions and expose private rows, reported by <a href=\"https://example.com\">someone</a> in <a href=\"https://example.com/advisory\">GHSA-h547-rmjf-5m2m</a>.</p></div></div></div><div id=\"secondary\"><p>This is a <strong>beat</strong> by Simon, posted on 16th September 2026.</p><section><h3>Monthly briefing</h3><p>Sponsor me and get a curated digest of the month's most important developments.</p></section></div><script>var x = \"Security fix for an issue where a trailing newline\";</script></body></html>";
        let sidebar = "<div><p>This is a <strong>beat</strong> by Simon, posted on 16th September 2026.</p><section><h3>Monthly briefing</h3><p>Sponsor me and get a curated digest of the month's most important developments.</p></section></div>";
        let present = feed_content_on_page(feed, page);
        assert!(present.len() >= 10, "{present:?}");
        assert!(extraction_misses_feed_content(sidebar, feed, &present));
        // The right region, or one that merely rephrases the entry, is kept.
        let note = "<div><p>Security fix for an issue where a trailing newline in a requested table name could bypass table permissions and expose private rows, reported by someone in GHSA-h547-rmjf-5m2m.</p></div>";
        assert!(!extraction_misses_feed_content(note, feed, &present));
        // A long article whose extraction dropped a lede the feed repeats is trusted.
        let long = format!(
            "<div>{}</div>",
            "<p>Paragraph of the actual article body with many words in it.</p>".repeat(40)
        );
        assert!(!extraction_misses_feed_content(&long, feed, &present));
        // A one-line note wrapped in a release label and a tag list: the label and tags are
        // worded differently on the page, so the note's own run of words has to carry it.
        let short = "Release: datasette 1.0a39 See Datasette 1.0a39 and 0.65.4 security releases on the Datasette blog. Tags: security, datasette";
        let short_page = "<html><body><div class=\"beat-note\"><p>See <a href=\"https://example.com\">Datasette 1.0a39 and 0.65.4 security releases</a> on the Datasette blog.</p></div><div id=\"secondary\"><p>This is a <strong>beat</strong> by Simon, posted on 11th September 2026.</p></div></body></html>";
        let present = feed_content_on_page(short, short_page);
        assert!(!present.is_empty(), "{present:?}");
        assert!(extraction_misses_feed_content(sidebar, short, &present));
        // Title-only and teaser feeds never qualify.
        assert!(feed_content_on_page("<p>datasette 0.65.5</p>", page).is_empty());
        assert!(
            feed_content_on_page(feed, "<html><body><p>Unrelated page.</p></body></html>")
                .is_empty()
        );
    }

    use super::*;
    use crate::content::{html_to_text, normalize_image_sources, sanitize, to_markdown};

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
    fn script_drawn_charts_become_data_tables() {
        let prose = "The question every finance leader asks is how to get more value from spending on models, and the answer starts with measuring work instead of seats. ".repeat(10);
        let spec = r#"{"$schema":"https://vega.github.io/schema/vega-lite/v6.json","title":{"text":"DeepSWE v1.1","subtitle":"Coding"},"data":{"values":[{"model":"GPT-5.6 Sol","score":0.727,"x_value":3.4123456,"x_label":"$3.41","juice_index":2},{"model":"Claude Fable 5","score":0.699,"x_value":5,"x_label":"$5.00","juice_index":1}]},"layer":[{"mark":"line"}]}"#;
        let flight = format!(
            r#"["$","$Le7",null,{{"id":"abc123","data":{{"dotcomConfig":{{"theme":"blue"}},"vegaLiteSpec":{spec}}}}}]"#
        );
        let chunk = serde_json::to_string(&flight).unwrap();
        let html = format!(
            "<html><head><title>Scorecard</title></head><body><main><article><p>{prose}</p><figure><div><div id=\"chart-abc123\" style=\"height:400px\"></div></div><figcaption><p>DeepSWE v1.1: long-horizon tasks.</p></figcaption></figure><p>{prose}</p></article></main><script>self.__next_f.push([1,{chunk}])</script></body></html>"
        );
        let url = Url::parse("https://openai.com/index/a-scorecard-for-the-ai-age/").unwrap();
        let article = extract_article(&html, &url).unwrap();
        let markdown = to_markdown(&article.html, Some(&url));
        let compact = markdown.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            compact.contains("| Model | X label | Juice index | Score | X value |"),
            "{markdown}"
        );
        assert!(
            compact.contains("| GPT-5.6 Sol | $3.41 | 2 | 0.727 | 3.4123 |"),
            "{markdown}"
        );
        assert!(
            compact.contains("| Claude Fable 5 | $5.00 | 1 | 0.699 | 5 |"),
            "{markdown}"
        );
        assert!(
            markdown.contains("DeepSWE v1.1 — Coding (chart data)"),
            "{markdown}"
        );
        assert!(markdown.contains("long-horizon tasks"), "{markdown}");
        let untouched = html.replace("vegaLiteSpec", "otherSpec");
        assert!(
            !to_markdown(&extract_article(&untouched, &url).unwrap().html, Some(&url))
                .contains("chart data")
        );
    }

    #[test]
    fn inline_chart_svgs_become_tables_and_sponsor_logos_stay() {
        // gleam.run: the benchmark figures are `<svg class="gerbil">` bar charts inside the
        // article, and the top-sponsor logo is a list Readability treats as an advertisement.
        let prose = "The benchmark measures how long it takes to compile one hundred modules, and the bars are the result. ".repeat(8);
        let first = r#"<svg class="gerbil" viewBox="0 0 500 250"><text x="85.5" y="237.2">0ms</text><text x="165.15" y="237.2">100ms</text><text x="483.75" y="237.2">500ms</text><text x="71.5" y="61.8">Gleam v1.17</text><text x="71.5" y="169.4">Gleam v1.19</text><rect class="bar gleam" x="85.5" y="18.76" width="345.681" height="86.08"></rect><rect class="bar gleam" x="85.5" y="126.36" width="272.403" height="86.08"></rect></svg>"#;
        let second = r#"<svg class="gerbil" viewBox="0 0 500 250"><text x="131" y="237.2">0ms</text><text x="480.5" y="237.2">1000ms</text><text x="117" y="212.44">Gleam (JavaScript)</text><text x="117" y="190.92">Go</text><rect class="bar gleam" x="131" y="203.832" width="65.706" height="17.216"></rect><rect class="bar" x="131" y="182.312" width="68.1525" height="17.216"></rect></svg>"#;
        let html = format!(
            "<html><head><title>Gleam release</title></head><body><main><article class='prose'><p>{prose}</p><p>This chart shows the time taken, lower is better.</p>{first}<p>As you can see, a considerable improvement.</p><p>Here are the results:</p>{second}<a class='sponsor-level0' href='https://github.com/sponsors/gleam-lang'><img src='https://gleam.run/images/community/github.svg' alt='GitHub Sponsors'></a><ul class='top-sponsors'><li><a href='https://lambdaclass.com/'><img src='https://gleam.run/images/sponsors/lambda-class-white.png' alt='Lambda Class'></a></li></ul><p>An inline icon <svg class='icon' viewBox='0 0 16 16'><path d='M1 1h2v2z'/></svg> stays prose.</p><div class='sponsor-pitch'><img src='https://ads.example/spot.png' alt='Advertisement'><p>Buy this unrelated product from our sponsor network today and save a fortune on things you do not need.</p></div><p>{prose}</p></article></main></body></html>"
        );
        let url =
            Url::parse("https://gleam.run/news/gleam-doesnt-compile-to-erlang-source-anymore/")
                .unwrap();
        let markdown = to_markdown(&extract_article(&html, &url).unwrap().html, Some(&url));
        let compact = markdown.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            compact.contains("| Gleam v1.17 | 434 |") && compact.contains("| Gleam v1.19 | 342 |"),
            "{markdown}"
        );
        assert!(
            compact.find("Gleam v1.17").unwrap() < compact.find("Gleam v1.19").unwrap(),
            "{markdown}"
        );
        assert!(
            compact.contains("| Gleam (JavaScript) | 188 |") && compact.contains("| Go | 195 |"),
            "{markdown}"
        );
        assert_eq!(
            markdown.matches("Chart (chart data)").count(),
            2,
            "{markdown}"
        );
        assert!(markdown.contains("considerable improvement"), "{markdown}");
        assert!(
            markdown.contains("https://gleam.run/images/sponsors/lambda-class-white.png"),
            "{markdown}"
        );
        assert!(
            markdown.contains("https://gleam.run/images/community/github.svg"),
            "{markdown}"
        );
        assert!(!markdown.contains("ads.example/spot.png"), "{markdown}");
        assert!(!markdown.contains("M1 1h2v2z"), "{markdown}");
    }

    #[test]
    #[ignore = "requires saved upstream HTML in AGGR_ARTICLE_HTML"]
    fn saved_openai_scorecard_chart_becomes_a_table() {
        let page = std::fs::read_to_string(std::env::var("AGGR_ARTICLE_HTML").unwrap()).unwrap();
        let url = Url::parse("https://openai.com/index/a-scorecard-for-the-ai-age/").unwrap();
        let article = extract_article(&page, &url).unwrap();
        let markdown = to_markdown(&article.html, Some(&url));
        assert!(
            markdown.contains("DeepSWE v1.1 — Coding (chart data)"),
            "{markdown}"
        );
        let compact = markdown.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            compact.matches("| Claude Fable 5 |").count() >= 3,
            "{markdown}"
        );
    }

    #[test]
    fn share_named_media_wrappers_survive_extraction_while_share_widgets_are_dropped() {
        let prose = "Apple today introduced a new watch with an all-new health sensing system that measures more signals than before. ".repeat(12);
        let html = format!(
            "<html><head><title>Introducing Apple Watch</title></head><body><main><article><div class='pagebody'><figure class='image component' aria-label='Media'><div class='component-content'><div class='image-sharesheet'><div class='image-asset'><picture class='picture'><source media='(max-width: 734px)' srcset='/images/watch_small.jpg,\n\t/images/watch_small_2x.jpg 2x'><img class='picture-image' src='https://www.apple.com/newsroom/images/watch_big.jpg' alt='Two watches side by side'></picture></div><div class='image-description'><div class='image-caption'>Apple Watch delivers accurate sensing.</div><a href='/newsroom/images/watch.zip' download>Download media</a></div></div></div></figure><p>{prose}</p><p>{prose}</p></div><div class='share-tools'><a href='https://x.com/intent/post?url=https://www.apple.com/newsroom/'><img src='/icons/x.svg' alt='Share on X'></a><a href='https://www.facebook.com/sharer/sharer.php?u=x'><img src='/icons/facebook.svg' alt='Share on Facebook'></a></div></article></main></body></html>"
        );
        let url =
            Url::parse("https://www.apple.com/newsroom/2026/09/introducing-apple-watch/").unwrap();
        let article = extract_article(&html, &url).unwrap();
        let markdown = to_markdown(&article.html, Some(&url));
        assert!(
            markdown.contains("https://www.apple.com/newsroom/images/watch_big.jpg"),
            "{markdown}"
        );
        assert!(markdown.contains("Two watches side by side"), "{markdown}");
        assert!(!markdown.contains("icons/x.svg"), "{markdown}");
        assert!(!markdown.contains("Share on Facebook"), "{markdown}");
    }

    #[test]
    fn translation_widget_is_removed_while_translation_prose_stays() {
        // diff.wikimedia.org: `#translate-post` sits inside the article, after the prose.
        let prose = "Wikimedia volunteers have stayed resilient while the traffic from automated agents keeps climbing. ".repeat(8);
        let section = "Machine translation changes how readers reach a Wikipedia article and how editors review the result. ".repeat(8);
        let page = format!(
            "<html><head><title>Agents on Wikimedia</title></head><body><article class='entry-content'><h1>Agents on Wikimedia</h1><p>{prose}</p><h2>How translation works</h2><section class='translation-notes'><p>{section}</p></section><p class='notranslate'>A notranslate label keeps the word translate visible in the article.</p><pre><code class='notranslate'>translate(\"message\")</code></pre><div id='translate-post' class='translate-post'><div class='translate-post-image'><img src='https://diff.wikimedia.org/wp-content/themes/interconnection/assets/images/translate-post.jpg' alt=''></div><div class='translate-post-content'><h2>Can you help us translate this article?</h2><p>In order for this article to reach as many people as possible we would like your help. Can you translate this article to get the message out?</p><a class='translate-post-login' href='https://diff.wikimedia.org/wp-login.php?redirect_to=%23translate-post'>Start translation</a></div></div></article></body></html>"
        );
        let url = Url::parse("https://diff.wikimedia.org/2026/10/05/agents/").unwrap();
        let article = extract_article(&page, &url).unwrap();
        let text = html_to_text(&article.html);
        assert!(text.contains("stayed resilient"), "{text}");
        assert!(text.contains("how editors review the result"), "{text}");
        assert!(
            text.contains("A notranslate label keeps the word translate visible"),
            "{text}"
        );
        assert!(text.contains("translate(\"message\")"), "{text}");
        assert!(!text.contains("Start translation"), "{text}");
        assert!(
            !text.contains("Can you help us translate this article"),
            "{text}"
        );
        assert!(!text.contains("translate-post.jpg"), "{text}");
        assert!(!article.html.contains("translate-post"), "{}", article.html);
    }

    #[test]
    fn publisher_subtitle_is_copied_into_the_article() {
        // dev.37signals.com: the dek is the paragraph under the title, outside `.post__content`,
        // and the same sentence is schema.org alternativeHeadline. A meta description is not.
        let title = "Hot Cell v1.0: Securing Active Storage in the age of AI";
        let dek = "Hot Cell v1.0 moves Active Storage\u{2019}s image, video, and PDF processing out of your Rails app and into a locked-down sidecar container.";
        let teaser =
            "This newsletter teaser is a summary and must not become the article subtitle.";
        let prose = "Today we are releasing Hot Cell, a suite of gems, that moves attachment processing into an unprivileged sidecar container. ".repeat(8);
        // The dek's box is a sibling of the page body, not of the article, which is what leaves
        // it out of the region Readability keeps on dev.37signals.com.
        let page = |headline: &str, body: &str, json: &str| {
            format!(
                "<html><head><title>{title}</title><meta name='description' content='{teaser}'><script type='application/ld+json'>{json}</script></head><body>{headline}<main><article class='post__content content'><p>{body}</p><p>Further detail follows the opening and keeps this region the article.</p></article><p>A margin note sits outside the article.</p></main></body></html>"
            )
        };
        let json = format!(
            r#"{{"@context":"https://schema.org","@type":"BlogPosting","headline":"{title}","alternativeHeadline":"{dek}"}}"#
        );
        let headline = format!(
            "<div class='page__headline page__headline--post'><h1>{title}</h1><p>By Jane Doe</p><p>{dek}</p></div>"
        );
        let url = Url::parse("https://dev.37signals.com/hot-cell-1-0/").unwrap();
        let article = extract_article(&page(&headline, &prose, &json), &url).unwrap();
        let text = html_to_text(&article.html);
        assert_eq!(
            text.matches("locked-down sidecar container").count(),
            1,
            "{text}"
        );
        assert!(text.starts_with("Hot Cell v1.0 moves"), "{text}");
        assert!(text.contains("Today we are releasing Hot Cell"), "{text}");
        assert!(!text.contains("newsletter teaser"), "{text}");
        assert!(!text.contains("By Jane Doe"), "{text}");
        let markdown = to_markdown(&article.html, Some(&url));
        let cleaned = crate::content::cleanup::strip_article_metadata(
            &markdown,
            title,
            None,
            "dev-37signals-com",
        );
        assert!(
            cleaned.contains("locked-down sidecar container"),
            "{cleaned}"
        );

        // The same sentence only in structured data, with a WebSite script that has none.
        let website =
            r#"{"@context":"https://schema.org","@type":"WebSite","name":"37signals Dev"}"#;
        let scripts = format!(
            "<script type='application/ld+json'>{website}</script><script type='application/ld+json'>{json}</script>"
        );
        let structured = format!(
            "<html><head><title>{title}</title>{scripts}</head><body><h1>{title}</h1><article class='post__content content'><p>{prose}</p></article></body></html>"
        );
        let text = html_to_text(&extract_article(&structured, &url).unwrap().html);
        assert_eq!(
            text.matches("locked-down sidecar container").count(),
            1,
            "{text}"
        );
        assert!(text.starts_with("Hot Cell v1.0 moves"), "{text}");

        // Already the opening sentence, including a straight apostrophe against the curly one.
        let straight = dek.replace('\u{2019}', "'");
        let repeated = extract_article(
            &page(&headline, &format!("{straight} {prose}"), &json),
            &url,
        )
        .unwrap();
        assert_eq!(
            html_to_text(&repeated.html)
                .matches("locked-down sidecar container")
                .count(),
            1,
            "{}",
            repeated.html
        );

        // A standfirst is the dek even when it is not under the headline.
        let standfirst =
            "A standfirst sits under the title and tells the reader what the piece is about.";
        let introduced = format!(
            "<html><head><title>Notes</title></head><body><p class='standfirst'>{standfirst}</p><article><h1>Notes</h1><p>{prose}</p></article></body></html>"
        );
        let text = html_to_text(
            &extract_article(
                &introduced,
                &Url::parse("https://example.com/notes").unwrap(),
            )
            .unwrap()
            .html,
        );
        assert!(text.contains(standfirst), "{text}");
        assert_eq!(text.matches(standfirst).count(), 1, "{text}");

        // The title restated as alternativeHeadline is the title, not a subtitle.
        let titled = format!(
            r#"{{"@type":"BlogPosting","headline":"{title}","alternativeHeadline":"{title}"}}"#
        );
        let text = html_to_text(
            &extract_article(&page(&format!("<h1>{title}</h1>"), &prose, &titled), &url)
                .unwrap()
                .html,
        );
        assert!(!text.contains("locked-down sidecar container"), "{text}");
        assert!(text.matches(title).count() <= 1, "{text}");
        assert!(text.contains("Today we are releasing Hot Cell"), "{text}");
    }

    #[test]
    fn release_line_under_the_title_is_copied_and_a_byline_is_not() {
        // gleam.run: `<p class="hero-subtitle">` is the release line beside the h1, outside
        // `<article class="prose">`. The date and the author sit in `.post-authored` instead.
        let title = "Gleam doesn't compile to Erlang source anymore";
        let release = "Gleam v1.19.0 released!";
        let prose = "Gleam is a type-safe and scalable language, and this release changes how it compiles. ".repeat(8);
        let page = |subtitle: &str, body: &str| {
            format!(
                "<html><head><title>{title} | Gleam programming language</title><meta name='description' content='News post: {release}'></head><body><div class='page-header'><nav class='navbar'><a href='/news/'>News</a></nav><div class='hero'><div class='content'><div class='text'><h1>{title}</h1>{subtitle}</div></div></div></div><div class='player'><h2>Talk</h2><p class='subtitles'>Closed captions stay with the video and out of the article text.</p></div><main class='page content'><div class='post'><p class='post-authored'><time>05 October, 2026</time> by <a href='https://github.com/lpil'>Louis Pilfold</a></p><article class='prose'><p>{body}</p><h2>A new compilation target</h2><p>The generator now writes Erlang abstract forms, which the runtime loads directly.</p></article></div></main></body></html>"
            )
        };
        let url =
            Url::parse("https://gleam.run/news/gleam-doesnt-compile-to-erlang-source-anymore/")
                .unwrap();
        let article = extract_article(
            &page(&format!("<p class='hero-subtitle'>{release}</p>"), &prose),
            &url,
        )
        .unwrap();
        let text = html_to_text(&article.html);
        assert!(text.starts_with(release), "{text}");
        assert_eq!(text.matches(release).count(), 1, "{text}");
        assert!(text.contains("type-safe and scalable language"), "{text}");
        assert!(!text.contains("News post:"), "{text}");
        assert!(!text.contains("Louis Pilfold"), "{text}");
        assert!(!text.contains("Closed captions"), "{text}");
        let markdown = to_markdown(&article.html, Some(&url));
        let cleaned =
            crate::content::cleanup::strip_article_metadata(&markdown, title, None, "lobste-rs");
        assert!(cleaned.contains(release), "{cleaned}");

        // The same sentence already opening the article is not written twice.
        let repeated = extract_article(
            &page(
                &format!("<p class='hero-subtitle'>{release}</p>"),
                &format!("{release} {prose}"),
            ),
            &url,
        )
        .unwrap();
        assert_eq!(
            html_to_text(&repeated.html).matches(release).count(),
            1,
            "{}",
            repeated.html
        );

        // A byline, a date, a one-word kicker, and a captions track are not the release line.
        for subtitle in [
            "<p class='hero-subtitle'>By Louis Pilfold</p>",
            "<p class='hero-subtitle'>05 October, 2026</p>",
            "<p class='hero-subtitle'>05 October, 2026 by Louis Pilfold</p>",
            "<p class='hero-subtitle'>News</p>",
            "<p class='dek'>By Jane Doe</p>",
        ] {
            let text = html_to_text(&extract_article(&page(subtitle, &prose), &url).unwrap().html);
            assert!(
                text.starts_with("Gleam is a type-safe"),
                "{subtitle} -> {text}"
            );
            assert!(!text.contains("By Louis Pilfold"), "{text}");
            assert!(!text.contains("By Jane Doe"), "{text}");
        }
    }

    /// `AGGR_ARTICLE_HTML=page.html AGGR_ARTICLE_URL=https://… cargo test … saved_page_converts
    /// -- --ignored --nocapture` prints the Markdown a saved page converts to.
    #[test]
    #[ignore = "requires saved upstream HTML in AGGR_ARTICLE_HTML and its URL in AGGR_ARTICLE_URL"]
    fn saved_page_converts() {
        let page = std::fs::read_to_string(std::env::var("AGGR_ARTICLE_HTML").unwrap()).unwrap();
        let url = Url::parse(&std::env::var("AGGR_ARTICLE_URL").unwrap()).unwrap();
        let article = extract_article(&page, &url).unwrap();
        let markdown = to_markdown(&article.html, Some(&url));
        eprintln!("{markdown}");
        assert!(!markdown.trim().is_empty());
    }

    #[test]
    #[ignore = "requires saved upstream HTML in AGGR_ARTICLE_HTML"]
    fn saved_apple_newsroom_share_wrapped_figures_survive_extraction() {
        let page = std::fs::read_to_string(std::env::var("AGGR_ARTICLE_HTML").unwrap()).unwrap();
        let url = Url::parse("https://www.apple.com/newsroom/2026/09/introducing-apple-watch-series-12-with-the-all-new-health-sensing-system/").unwrap();
        let article = extract_article(&page, &url).unwrap();
        let markdown = to_markdown(&article.html, Some(&url));
        assert!(markdown.matches("![").count() >= 10, "{markdown}");
        assert!(
            markdown.contains("/article/Apple-Watch-Series-12-2up-260909_big.jpg"),
            "{markdown}"
        );
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

    fn base() -> Url {
        Url::parse("https://example.com/blog/post/").unwrap()
    }

    #[test]
    fn a_readability_pass_keeps_captions_hidden_only_from_screen_readers() {
        let page = format!(
            "<html><head><title>Post</title></head><body><article><h1>Post</h1><figure><img src=\"/one.jpg\" alt=\"The thing\" width=\"1225\" height=\"1496\"><figcaption aria-hidden=\"true\">The thing</figcaption></figure>{}</article></body></html>",
            "<p>A paragraph of real prose so the article scores as readable content.</p>".repeat(6)
        );
        let article =
            extract_article(&page, &Url::parse("https://example.com/post").unwrap()).unwrap();
        assert!(article.html.contains("figcaption"), "{}", article.html);
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
    fn publisher_navigation_is_removed_without_dropping_article_prose() {
        // OSNews puts BreadcrumbList inside its story; Google uses uni-article-jumplinks.
        let page = r##"<article><div itemscope="https://schema.org/BreadcrumbList" class="breadcrumb"><a href="/">Home</a> &gt; <a href="/google">Google</a> &gt; Story</div><p>The article begins with useful prose about the decision and its consequences.</p><uni-article-jumplinks heading="In this article"><details><summary>In this article</summary><nav><a href="#next">Next</a></nav></details></uni-article-jumplinks><h2 id="next">Next</h2><p>Another useful paragraph follows the actual article heading.</p><p>In this article I explain the decision in ordinary prose.</p></article>"##;
        let extracted = extract_article(page, &base()).unwrap();
        let markdown = to_markdown(&extracted.html, Some(&base()));
        assert!(!markdown.contains("[Home]"), "{markdown}");
        assert!(!markdown.contains("\nIn this article\n"), "{markdown}");
        assert!(markdown.contains("article begins"), "{markdown}");
        assert!(markdown.contains("Another useful paragraph"), "{markdown}");
        assert!(markdown.contains("In this article I explain"), "{markdown}");
    }

    #[test]
    fn ars_article_blocks_across_an_ad_are_joined_in_order() {
        // Ars Technica puts the opening paragraphs in one post-content block and the rest after an ad.
        let url = Url::parse("https://arstechnica.com/story").unwrap();
        let page = r#"<div><div class="post-content post-content-double"><p>The opening paragraphs describe the case and contain enough prose to be an article.</p></div><div class="ad-wrapper">Advertisement</div><div class="post-content post-content-double"><p>The later section explains the court filing in detail and adds more substantive prose.</p></div></div>"#;
        let extracted = extract_article(page, &url).unwrap();
        let markdown = to_markdown(&extracted.html, Some(&url));
        assert!(markdown.contains("opening paragraphs"), "{markdown}");
        assert!(markdown.contains("later section"), "{markdown}");
        assert!(!markdown.contains("Advertisement"), "{markdown}");
        assert!(
            markdown.find("opening paragraphs").unwrap() < markdown.find("later section").unwrap()
        );
    }

    #[test]
    fn slack_incident_updates_keep_their_labels_times_and_body() {
        // slack-status.com exposes each update as a note and the affected features in a sidebar.
        let url = Url::parse("https://slack-status.com/incident/123").unwrap();
        let page = r#"<main><div class="incident"><img class="incident_type_icon" src="/icon.png"><h3>Message failures</h3><div class="left"><div class="note"><span>Update</span><p>Access is being restored after the message loop was stopped.</p><p class="date"><span>6:58 AM PST</span></p></div><div class="note"><span>Investigating</span><p>We're investigating why message sending was blocked.</p><p class="date"><span>5:39 AM PST</span></p></div></div><div class="right"><p>Features affected</p><p>Messaging</p></div></div></main>"#;
        let extracted = extract_article(page, &url).unwrap();
        let markdown = to_markdown(&extracted.html, Some(&url));
        assert!(markdown.contains("Update · 6:58 AM PST"), "{markdown}");
        assert!(
            markdown.contains("Investigating · 5:39 AM PST"),
            "{markdown}"
        );
        assert!(markdown.contains("Access is being restored"), "{markdown}");
        assert!(!markdown.contains("Features affected"), "{markdown}");
        assert!(!markdown.contains("icon.png"), "{markdown}");
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

    #[test]
    fn a_loading_placeholder_is_not_an_article() {
        // harnesstax.github.io: the post is rendered by js/post.js into an `aria-busy` root.
        let page = r#"<!doctype html><html><head><title>HarnessTax: How Much Does the Harness Matter?</title></head><body>
<header class="masthead"><button id="theme-toggle" type="button">◐</button></header>
<div id="tab-blog"><div id="blog-root" aria-busy="true"><p class="loading loading-standalone">loading…</p></div></div>
<script type="module" src="js/post.js"></script></body></html>"#;
        let error = extract_article(page, &base()).unwrap_err();
        assert!(
            format!("{error:#}").contains("no readable content"),
            "{error:#}"
        );

        // mimo.xiaomi.com/rl/: a live dashboard whose server HTML is tab labels and a status pill.
        let page = r#"<!doctype html><html><head><title>mimo-v2.6 RL</title></head><body>
<header class="nav"><div class="wrap nav-inner"><nav class="tabs" id="tabs"><a data-view="overview">overview</a><a data-view="metrics">metrics</a><a data-view="about">about</a></nav></div></header>
<main class="wrap" id="main"><section class="view" id="view-overview"></section></main>
<div class="status-pill hidden" id="status-pill">reconnecting…</div></body></html>"#;
        let error = extract_article(page, &base()).unwrap_err();
        assert!(
            format!("{error:#}").contains("no readable content"),
            "{error:#}"
        );

        // A short real article that merely mentions loading is kept.
        let page = r#"<!doctype html><title>Post</title><main><article><h1>Loading times</h1><p>Loading… is what users saw for six seconds, so we moved the bundle to a CDN.</p></article></main>"#;
        assert!(extract_article(page, &base()).is_ok());
        for text in ["loading…", "Loading...", "Reconnecting", "Please wait."] {
            assert!(is_loading_message(text), "{text}");
        }
        for text in ["Loading the dishwasher", "Connecting rods", ""] {
            assert!(!is_loading_message(text), "{text}");
        }
    }

    #[test]
    fn archived_placeholder_bodies_are_recognised_for_retry() {
        assert!(is_placeholder_body("loading…\n"));
        assert!(is_placeholder_body(
            "overview metrics about\n\nreconnecting…\n"
        ));
        assert!(!is_placeholder_body(
            "Loading… is what users saw for six seconds, so we moved the bundle to a CDN.\n"
        ));
        assert!(!is_placeholder_body(""));
        assert!(!is_placeholder_body(
            "Article URL: [https://example.com](https://example.com)\n\nPoints: 142\n"
        ));
    }
}
