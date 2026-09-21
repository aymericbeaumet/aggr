//! Conservative recognition of subscription offers captured instead of an article.

use url::Url;

pub fn is_subscription_wall(body: &str, original: &Url) -> bool {
    let text = super::html_to_text(body)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let text = text.trim_start_matches(['#', '*', ' ']);
    if matches!(original.host_str(), Some("ft.com" | "www.ft.com"))
        && original.path().starts_with("/content/")
    {
        return text.starts_with("save ")
            && text
                .chars()
                .take(100)
                .collect::<String>()
                .contains("standard digital")
            && [
                "explore more offers",
                "premium digital",
                "complete digital access",
                "full range of subscriptions",
            ]
            .iter()
            .filter(|phrase| text.contains(**phrase))
            .count()
                >= 3;
    }
    let document = scraper::Html::parse_fragment(body);
    let Ok(selector) =
        scraper::Selector::parse("[data-testid=paywall], .paywall, #paywall, .subscription-wall")
    else {
        return false;
    };
    let gate_only = document.select(&selector).any(|gate| {
        gate.text()
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
            == text
    });
    gate_only
        && text.split_whitespace().count() < 80
        && [
            "subscribe to continue reading",
            "subscribe to read the full article",
            "this article is for subscribers",
        ]
        .iter()
        .any(|prefix| text.starts_with(prefix))
}

pub fn archive_lookup_url(original: &Url) -> String {
    let mut original = original.clone();
    original.set_fragment(None);
    format!("https://archive.ph/{original}")
}

pub fn normalize_subscription_metadata(
    body: &str,
    front: &mut crate::model::FrontMatter,
) -> String {
    front.extra.remove("archive_lookup_url");
    if front
        .extra
        .get("subscription_required")
        .and_then(serde_yaml_ng::Value::as_bool)
        != Some(true)
    {
        front.extra.remove("subscription_required");
    }
    let Ok(original) = Url::parse(&front.link) else {
        return body.to_string();
    };
    if !matches!(original.scheme(), "http" | "https")
        || !original.username().is_empty()
        || original.password().is_some()
    {
        return body.to_string();
    }
    if front
        .extra
        .get("subscription_required")
        .and_then(serde_yaml_ng::Value::as_bool)
        == Some(true)
    {
        front.extra.insert(
            "archive_lookup_url".into(),
            archive_lookup_url(&original).into(),
        );
    }
    if !is_subscription_wall(body, &original) {
        return body.to_string();
    }
    front
        .extra
        .insert("subscription_required".into(), true.into());
    front.extra.insert(
        "archive_lookup_url".into(),
        archive_lookup_url(&original).into(),
    );
    front.content = crate::model::ContentKind::Feed;
    let summary = front.summary.clone().unwrap_or_default();
    super::normalize_aggregator_metadata(&summary, front)
}

#[cfg(test)]
mod tests {
    use super::*;
    const OFFERS: &str = "## Save 50% on Standard Digital\n\nExplore more offers.\n\n### Premium Digital\n\nComplete digital access with exclusive insights.\n\nExplore our full range of subscriptions.";
    #[test]
    fn subscription_offers_are_not_article_content() {
        let original = Url::parse("https://www.ft.com/content/123").unwrap();
        assert!(is_subscription_wall(OFFERS, &original));
        let mut front: crate::model::FrontMatter = serde_yaml_ng::from_str("title: Article\nlink: https://www.ft.com/content/123\nsource: hnrss.org\nfirst_seen: 2026-09-21T00:00:00Z\ncontent: extracted\n").unwrap();
        front.summary = Some("Article URL: https://www.ft.com/content/123 Comments URL: https://news.ycombinator.com/item?id=123 Points: 42 # Comments: 2".into());
        let body = normalize_subscription_metadata(OFFERS, &mut front);
        assert!(body.is_empty());
        assert!(front.summary.is_none());
        assert_eq!(front.extra["subscription_required"].as_bool(), Some(true));
        assert_eq!(
            front.extra["archive_lookup_url"].as_str(),
            Some("https://archive.ph/https://www.ft.com/content/123")
        );
        assert!(!is_subscription_wall(
            &format!("The publisher changed its plans.\n\n{OFFERS}"),
            &original
        ));
        assert!(!is_subscription_wall(
            OFFERS,
            &Url::parse("https://review.example/article").unwrap()
        ));
    }
    #[test]
    fn subscription_detector_preserves_readable_articles_and_incidental_ctas() {
        let original = Url::parse("https://publisher.example/story").unwrap();
        let gate = "<div class='paywall'>Subscribe to continue reading. Already a subscriber? Sign in.</div>";
        assert!(is_subscription_wall(gate, &original));
        assert!(!is_subscription_wall(
            &format!("<p>This is the article's actual readable content.</p>{gate}"),
            &original
        ));
        assert!(!is_subscription_wall(
            "<p>Subscribe to continue reading is a common sales prompt.</p>",
            &original
        ));
    }
}
