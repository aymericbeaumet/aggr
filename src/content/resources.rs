//! Leading resource links (code, paper, model, dataset) that the reader lifts out of the body,
//! and the shape rule that decides which destinations qualify.

use std::collections::HashSet;

use scraper::{Html, Selector};
use serde::Serialize;
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResourceLink {
    pub label: String,
    pub url: String,
}

pub(super) fn leading_resources(html: &str) -> Option<(Vec<ResourceLink>, std::ops::Range<usize>)> {
    let mut cursor = 0;
    for heroes in 0..=3 {
        let remaining = &html[cursor..];
        let start = cursor + remaining.len() - remaining.trim_start().len();
        let paragraph = html[start..].strip_prefix("<p>")?;
        let end = start + 3 + paragraph.find("</p>")? + 4;
        let fragment = &html[start..end];
        let has_image = fragment.contains("<img ");
        if end - start > 4096 || (!has_image && fragment.match_indices("<a ").take(2).count() < 2) {
            return None;
        }
        let document = Html::parse_fragment(fragment);
        let selector = Selector::parse("p").ok()?;
        let paragraph = document.select(&selector).next()?;
        if has_image && image_only_paragraph(paragraph) {
            if heroes == 3 {
                return None;
            }
            cursor = end;
            continue;
        }
        return Some((paragraph_resources(paragraph)?, start..end));
    }
    None
}

fn image_only_paragraph(paragraph: scraper::ElementRef<'_>) -> bool {
    let mut images = 0;
    let image_only = paragraph
        .descendants()
        .skip(1)
        .all(|node| match node.value() {
            scraper::Node::Text(text) => text.chars().all(char::is_whitespace),
            scraper::Node::Element(element) if element.name() == "img" => {
                images += 1;
                true
            }
            scraper::Node::Element(element) => {
                matches!(element.name(), "a" | "picture" | "source" | "br")
            }
            _ => false,
        });
    image_only && images > 0
}

fn paragraph_resources(paragraph: scraper::ElementRef<'_>) -> Option<Vec<ResourceLink>> {
    let mut resources = Vec::new();
    let mut destinations = HashSet::new();
    for node in paragraph.children() {
        match node.value() {
            scraper::Node::Text(text)
                if text
                    .chars()
                    .all(|c| c.is_whitespace() || matches!(c, '|' | '·' | '•')) => {}
            scraper::Node::Element(element) if element.name() == "br" => {}
            scraper::Node::Element(element) if element.name() == "a" => {
                let anchor = scraper::ElementRef::wrap(node)?;
                if anchor
                    .descendants()
                    .skip(1)
                    .any(|child| match child.value() {
                        scraper::Node::Text(_) => false,
                        scraper::Node::Element(element) => {
                            !matches!(element.name(), "em" | "strong")
                        }
                        _ => true,
                    })
                {
                    return None;
                }
                let label = anchor
                    .text()
                    .collect::<String>()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                if label.is_empty()
                    || label.chars().count() > 60
                    || label.split_whitespace().count() > 6
                {
                    return None;
                }
                let href = element.attr("href")?;
                let url = Url::parse(href).ok()?;
                if !resource_url(&url) {
                    return None;
                }
                if destinations.insert(crate::model::normalize_link(href)) {
                    resources.push(ResourceLink {
                        label,
                        url: href.to_string(),
                    });
                }
                if resources.len() > 8 {
                    return None;
                }
            }
            _ => return None,
        }
    }
    (resources.len() >= 2).then_some(resources)
}

/// An artifact has a page of its own: a nested path (`owner/repository`, `abs/2605.11887`,
/// `records/123`), a query that names a record (`forum?id=…`), or a document file. Profile and
/// section pages one segment deep (`github.com/alice`, `x.com/lab`) are people and places, not
/// resources.
fn resource_url(url: &Url) -> bool {
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_none()
    {
        return false;
    }
    let parts = url.path().trim_matches('/').split('/').collect::<Vec<_>>();
    if parts.iter().any(|part| part.is_empty()) {
        return false;
    }
    parts.len() >= 2
        || url.query().is_some_and(|query| !query.is_empty())
        || url.path().to_ascii_lowercase().ends_with(".pdf")
}

#[cfg(test)]
mod tests {
    use crate::content::{PreparedMarkdown, render_markdown};

    #[test]
    fn leading_resource_links_move_only_in_reader_html_and_stay_in_portable_outputs() {
        let markdown = "[HUGGING FACE](https://huggingface.co/collections/Qwen/qwen-scope) [MODELSCOPE](https://modelscope.cn/collections/Qwen/Qwen-Scope) [TECHNICAL REPORT](https://arxiv.org/abs/2605.11887)\n\nInterpretability helps us understand models.\n\nTry [Hugging Face](https://huggingface.co/collections/Qwen/qwen-scope) yourself.";
        let prepared = PreparedMarkdown::new(markdown);
        assert_eq!(prepared.resources().len(), 3);
        assert_eq!(prepared.resources()[0].label, "HUGGING FACE");
        assert_eq!(
            prepared.resources()[1].url,
            "https://modelscope.cn/collections/Qwen/Qwen-Scope"
        );
        assert_eq!(
            prepared.resources()[2].url,
            "https://arxiv.org/abs/2605.11887"
        );
        assert!(prepared.excerpt(240).starts_with("Interpretability"));
        assert!(!prepared.plain_text().contains("TECHNICAL REPORT"));
        let reader = prepared.reader_html_with_images(&[], &[]);
        assert!(!reader.contains("TECHNICAL REPORT"));
        assert!(
            reader.contains(">Hugging Face</a>"),
            "links in prose remain in place"
        );
        for html in [
            prepared.portable_html().to_string(),
            prepared.with_images(&[], &[]),
            render_markdown(markdown),
        ] {
            for resource in prepared.resources() {
                assert!(
                    html.contains(&resource.url),
                    "portable output lost {}",
                    resource.url
                );
            }
            assert!(html.contains("TECHNICAL REPORT"));
        }
    }

    #[test]
    fn resource_links_after_qwen_hero_preserve_images_and_all_portable_destinations() {
        let hero =
            "https://qianwen-res.oss-accelerate.aliyuncs.com/qwen-scope/Figures/overview.png";
        let links = "[HUGGING FACE](https://huggingface.co/collections/Qwen/qwen-scope) [MODELSCOPE](https://modelscope.cn/collections/Qwen/Qwen-Scope) [TECHNICAL REPORT](https://arxiv.org/abs/2605.11887)";
        let markdown = format!(
            "![Qwen-Scope main image]({hero})\n\n{links}\n\nInterpretability research has emerged as a critical area for understanding LLM behaviors."
        );
        let prepared = PreparedMarkdown::new(&markdown);
        assert_eq!(prepared.resources().len(), 3);
        let reader = prepared.reader_html_with_images(&[], &[]);
        assert!(reader.contains(hero));
        assert!(reader.contains("Qwen-Scope main image"));
        assert!(std::ptr::eq(prepared.reader_html(), prepared.reader_html()));
        assert!(reader.contains("fetchpriority=\"high\""));
        assert!(!reader.contains("TECHNICAL REPORT"));
        assert!(
            prepared
                .excerpt(240)
                .starts_with("Interpretability research")
        );
        assert!(!prepared.plain_text().contains("HUGGING FACE"));
        let portable = prepared.portable_html();
        assert!(portable.contains(hero));
        for resource in prepared.resources() {
            assert!(portable.contains(&resource.url));
        }
        assert!(portable.contains("TECHNICAL REPORT"));
        assert_eq!(prepared.with_images(&[], &[]), render_markdown(&markdown));
    }

    #[test]
    fn resource_hero_skipping_is_bounded_and_stops_at_prose_captions_and_headings() {
        let links =
            "[Code](https://github.com/lab/project) [Paper](https://arxiv.org/abs/1234.5678)";
        let image = "![Hero](https://example.com/hero.png)\n\n";
        for count in 1..=3 {
            let prepared =
                PreparedMarkdown::new(&format!("{}{links}\n\nArticle prose.", image.repeat(count)));
            assert_eq!(prepared.resources().len(), 2);
            assert_eq!(
                prepared
                    .reader_html_with_images(&[], &[])
                    .matches("<img ")
                    .count(),
                count
            );
        }
        for prefix in [
            image.repeat(4),
            format!("{image}Introduction.\n\n"),
            format!("{image}## Resources\n\n"),
            "![Hero](https://example.com/hero.png) A caption.\n\n".into(),
        ] {
            let prepared = PreparedMarkdown::new(&format!("{prefix}{links}\n\nArticle prose."));
            assert!(prepared.resources().is_empty());
            assert_eq!(
                prepared.reader_html_with_images(&[], &[]),
                prepared.portable_html()
            );
        }
    }

    #[test]
    fn resource_detection_preserves_prose_tocs_people_code_and_unsafe_links() {
        for markdown in [
            "[Alice](https://github.com/alice) [Bob](https://github.com/bob)",
            "[Introduction](#intro) [Methods](#methods)",
            "[Code](https://github.com/lab/project) [Follow us](https://x.com/lab)",
            "See [Code](https://github.com/lab/project) and [Paper](https://arxiv.org/abs/1234.5678).",
            "[Code](https://github.com/lab/project)",
            "[Code](https://github.com/lab/project) [Mirror](https://github.com/lab/project#readme)",
            "[Code](https://user:password@github.com/lab/project) [Paper](https://arxiv.org/abs/1234.5678)",
            "[Code](javascript:alert) [Paper](https://arxiv.org/abs/1234.5678)",
            "`[Code](https://github.com/lab/project)` [Paper](https://arxiv.org/abs/1234.5678)",
            "> [Code](https://github.com/lab/project) [Paper](https://arxiv.org/abs/1234.5678)",
            "- [Code](https://github.com/lab/project)\n- [Paper](https://arxiv.org/abs/1234.5678)",
            "```md\n[Code](https://github.com/lab/project) [Paper](https://arxiv.org/abs/1234.5678)\n```",
            "Ordinary introduction.\n\n[Code](https://github.com/lab/project) [Paper](https://arxiv.org/abs/1234.5678)",
        ] {
            let prepared = PreparedMarkdown::new(markdown);
            assert!(prepared.resources().is_empty(), "{markdown}");
            assert_eq!(
                prepared.reader_html_with_images(&[], &[]),
                prepared.portable_html()
            );
        }
    }
}
