//! Recognize aggregator feed bookkeeping without treating it as publisher prose.

use std::collections::BTreeMap;

use comrak::nodes::NodeValue;

use crate::model::{ContentKind, FrontMatter};

pub fn normalize_aggregator_metadata(body: &str, front: &mut FrontMatter) -> String {
    // An aggregator's summary describes the submission rather than the article — `Article URL: …
    // Points: …` is machine metadata whichever way the body was captured — so it is dropped before
    // the extracted-body shortcut below, or it would survive on exactly the articles that read
    // best and stand in for their excerpt wherever one is shown.
    if front.summary.as_deref().is_some_and(|summary| {
        parse_metadata(
            &summary.split_whitespace().collect::<Vec<_>>().join(" "),
            &front.link,
        )
        .is_some()
    }) {
        front.summary = None;
    }
    if front.content == ContentKind::Extracted {
        return body.to_string();
    }
    let Some(cleaned) = clean_feed_body(body, &front.link) else {
        return body.to_string();
    };
    for (key, value) in cleaned.metadata {
        front.extra.entry(key).or_insert(value);
    }
    cleaned.body
}

struct CleanedFeedBody {
    body: String,
    metadata: BTreeMap<String, serde_yaml_ng::Value>,
}

fn clean_feed_body(markdown: &str, article_link: &str) -> Option<CleanedFeedBody> {
    if !markdown.contains("Article URL:") || !markdown.contains("Comments URL:") {
        return None;
    }
    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, markdown, &comrak::Options::default());
    let blocks = root.children().collect::<Vec<_>>();
    let paragraphs = blocks
        .iter()
        .map(|node| {
            if !matches!(node.data.borrow().value, NodeValue::Paragraph) {
                return None;
            }
            let mut text = String::new();
            for child in node.descendants() {
                match &child.data.borrow().value {
                    NodeValue::Text(value) => text.push_str(value),
                    NodeValue::SoftBreak | NodeValue::LineBreak => text.push(' '),
                    NodeValue::Paragraph | NodeValue::Link(_) => {}
                    _ => return None,
                }
            }
            Some(text)
        })
        .collect::<Vec<_>>();
    // Only remove a complete metadata group at an article boundary. Interior examples,
    // fenced code, and quotes remain byte-for-byte intact.
    for start in 0..blocks.len() {
        if !paragraphs[start]
            .as_deref()
            .is_some_and(|text| text.starts_with("Article URL:"))
        {
            continue;
        }
        let mut combined = String::new();
        let mut best = None;
        for (end, paragraph) in paragraphs.iter().enumerate().skip(start).take(4) {
            let Some(paragraph) = paragraph else { break };
            if !combined.is_empty() {
                combined.push(' ');
            }
            combined.push_str(paragraph);
            if let Some(metadata) = parse_metadata(&combined, article_link) {
                if start == 0 || end + 1 == blocks.len() {
                    best = Some((end + 1, metadata));
                }
            } else if paragraph.starts_with("Points:") || paragraph.starts_with("# Comments:") {
                best = None;
                break;
            }
        }
        if let Some((end, metadata)) = best {
            let lines = markdown.split_inclusive('\n').collect::<Vec<_>>();
            let from = blocks[start].data.borrow().sourcepos.start.line - 1;
            let to = blocks[end - 1].data.borrow().sourcepos.end.line;
            let before = lines[..from].concat();
            let after = lines[to..].concat();
            let body = if start == 0 {
                after.trim_start_matches('\n').to_string()
            } else {
                before.trim_end_matches('\n').to_string() + "\n"
            };
            return Some(CleanedFeedBody { body, metadata });
        }
    }
    None
}

fn parse_metadata(
    text: &str,
    article_link: &str,
) -> Option<BTreeMap<String, serde_yaml_ng::Value>> {
    let (article, rest) = text
        .strip_prefix("Article URL: ")?
        .split_once(" Comments URL: ")?;
    let article = url::Url::parse(article).ok()?;
    let original = url::Url::parse(article_link).ok()?;
    if article != original || !matches!(article.scheme(), "http" | "https") {
        return None;
    }
    let (comments, counts) = rest.split_once(' ').unwrap_or((rest, ""));
    let comments = url::Url::parse(comments).ok()?;
    if !matches!(comments.scheme(), "http" | "https")
        || !comments.username().is_empty()
        || comments.password().is_some()
    {
        return None;
    }
    let known_discussion = match comments.host_str()? {
        "news.ycombinator.com" => {
            comments.path() == "/item"
                && comments.query_pairs().any(|(key, value)| {
                    key == "id" && !value.is_empty() && value.chars().all(|ch| ch.is_ascii_digit())
                })
        }
        "lobste.rs" => comments
            .path()
            .strip_prefix("/s/")
            .is_some_and(|path| !path.split('/').next().unwrap_or_default().is_empty()),
        _ => false,
    };
    if !known_discussion {
        return None;
    }
    let mut metadata = BTreeMap::from([("comments_url".to_string(), comments.to_string().into())]);
    let mut remaining = counts;
    if let Some(points) = remaining.strip_prefix("Points: ") {
        let (value, tail) = points.split_once(' ').unwrap_or((points, ""));
        metadata.insert("points".into(), value.parse::<u64>().ok()?.into());
        remaining = tail;
    }
    if let Some(comments) = remaining.strip_prefix("# Comments: ") {
        metadata.insert("num_comments".into(), comments.parse::<u64>().ok()?.into());
        remaining = "";
    }
    remaining.is_empty().then_some(metadata)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARTICLE: &str = "http://gameshelf.jmac.org/2008/11/13/GrimPuzzleDoc_small.pdf";
    const COMMENTS: &str = "https://news.ycombinator.com/item?id=49783495";

    fn bookkeeping() -> String {
        format!(
            "Article URL: [{ARTICLE}]({ARTICLE})\n\nComments URL: [{COMMENTS}]({COMMENTS})\n\nPoints: 120\n\n\\# Comments: 21\n"
        )
    }

    #[test]
    fn aggregator_metadata_only_body_becomes_empty() {
        let cleaned = clean_feed_body(&bookkeeping(), ARTICLE).unwrap();
        assert!(cleaned.body.is_empty());
        assert_eq!(cleaned.metadata["comments_url"].as_str(), Some(COMMENTS));
        assert_eq!(cleaned.metadata["points"].as_u64(), Some(120));
        assert_eq!(cleaned.metadata["num_comments"].as_u64(), Some(21));
        let summary =
            format!("Article URL: {ARTICLE} Comments URL: {COMMENTS} Points: 120 # Comments: 21");
        assert!(parse_metadata(&summary, ARTICLE).is_some());
    }

    #[test]
    fn aggregator_metadata_is_feed_only_and_preserves_existing_fields() {
        let summary =
            format!("Article URL: {ARTICLE} Comments URL: {COMMENTS} Points: 120 # Comments: 21");
        let mut front: FrontMatter = serde_yaml_ng::from_str(&format!("title: PDF\nlink: {ARTICLE}\nsource: arbitrary-feed-name\nfirst_seen: 2026-09-21T00:00:00Z\ncontent: feed\n")).unwrap();
        front.summary = Some(summary.clone());
        front.extra.insert("points".into(), 125.into());
        assert!(normalize_aggregator_metadata(&bookkeeping(), &mut front).is_empty());
        assert!(front.summary.is_none());
        assert_eq!(front.extra["points"].as_u64(), Some(125));
        assert_eq!(front.extra["comments_url"].as_str(), Some(COMMENTS));
        front.summary = Some("A meaningful description of the document.".into());
        normalize_aggregator_metadata(&bookkeeping(), &mut front);
        assert_eq!(
            front.summary.as_deref(),
            Some("A meaningful description of the document.")
        );
        // An extracted body is the article's own and is left alone, but the aggregator's summary
        // describes the submission whichever way the body arrived, so it goes either way.
        front.content = ContentKind::Extracted;
        front.summary = Some(summary.clone());
        assert_eq!(
            normalize_aggregator_metadata(&bookkeeping(), &mut front),
            bookkeeping()
        );
        assert!(front.summary.is_none());
        front.content = ContentKind::None;
        front.summary = Some(summary.clone());
        assert!(normalize_aggregator_metadata(&summary, &mut front).is_empty());
        assert!(front.summary.is_none());
    }

    #[test]
    fn aggregator_metadata_keeps_meaningful_feed_prose() {
        let prose = "A newly recovered puzzle design document.\n";
        for body in [
            format!("{}\n{prose}", bookkeeping()),
            format!("{prose}\n{}", bookkeeping()),
        ] {
            assert_eq!(clean_feed_body(&body, ARTICLE).unwrap().body, prose);
        }
    }

    #[test]
    fn aggregator_metadata_does_not_remove_prose_code_quotes_or_unrelated_urls() {
        let group = bookkeeping();
        for body in [
            format!("An explanation.\n\n{group}\nMore analysis.\n"),
            format!("```markdown\n{group}\n```\n"),
            group.lines().map(|line| format!("> {line}\n")).collect(),
            group.replace(COMMENTS, "https://publisher.example/item?id=12"),
            group.replace(ARTICLE, "https://publisher.example/other.pdf"),
            group.replace("120", "120 points discussed in this essay"),
        ] {
            assert!(clean_feed_body(&body, ARTICLE).is_none(), "{body}");
        }
    }
}
