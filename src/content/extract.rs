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
}

/// Extract the primary article from a complete origin page. Readability intentionally does not
/// sanitize its output, so callers must still pass this HTML through the normal storage and
/// Markdown safety pipeline.
pub fn extract_article(page: &str, url: &Url) -> Result<ExtractedArticle> {
    let config = dom_smoothie::Config {
        max_elements_to_parse: 100_000,
        ..Default::default()
    };
    let page = expand_embedded_charts(page);
    let mut readability = Readability::new(page.as_ref(), Some(url.as_str()), Some(config))
        .context("parsing the original article page")?;
    preserve_share_named_media_wrappers(&readability);
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

/// Publishers such as Apple Newsroom wrap every article figure in a `…-sharesheet` container.
/// Readability weighs any class or id mentioning sharing negatively and drops such low-text
/// wrappers, taking the picture with them. A wrapper that holds real media and no sharing links
/// is presentation, not a share widget; it is renamed to a neutral, content-positive class.
fn preserve_share_named_media_wrappers(readability: &Readability) {
    let share_link = |href: &str| {
        let href = href.to_ascii_lowercase();
        [
            "share",
            "/intent/",
            "sharer",
            "mailto:",
            "twitter.com/",
            "x.com/",
            "facebook.com/",
            "linkedin.com/",
            "reddit.com/",
            "pinterest.",
            "whatsapp",
            "t.me/",
            "threads.net/",
            "bsky.app/",
        ]
        .iter()
        .any(|marker| href.contains(marker))
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

#[cfg(test)]
mod tests {
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
