//! Build-time chronological navigation and article suggestions.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use sha1::{Digest as _, Sha1};

use super::context::ItemCtx;

const RECOMMENDATION_COUNT: usize = 3;
const CROSS_SOURCE_POOL: usize = 32;
const CROSS_SOURCE_BONUS: u64 = 48;
const RECENCY_BONUS: u64 = 36;
const RECENCY_HALF_LIFE_HOURS: u64 = 7 * 24;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Recommendation {
    pub previous: Option<usize>,
    pub next: Option<usize>,
    pub articles: Vec<usize>,
}

/// Resolve navigation for items sorted newest first. Suggestions follow the same weighted-index
/// model used by established static-site generators: shared labels dominate, then category and
/// uncommon title terms. An inverted index keeps this proportional to matching
/// features rather than comparing every pair of articles.
pub fn resolve(items: &[ItemCtx], bodies: &[&str]) -> Vec<Recommendation> {
    assert_eq!(items.len(), bodies.len());
    let identities = identities(items, bodies);
    let mut preceding = vec![None; items.len()];
    let mut following = vec![None; items.len()];
    for index in 1..items.len() {
        preceding[index] = if identities[index] == identities[index - 1] {
            preceding[index - 1]
        } else {
            Some(index - 1)
        };
    }
    for index in (0..items.len().saturating_sub(1)).rev() {
        following[index] = if identities[index] == identities[index + 1] {
            following[index + 1]
        } else {
            Some(index + 1)
        };
    }
    let newest = items.iter().map(|item| item.date).max();
    let features: Vec<Vec<String>> = items.iter().map(item_features).collect();
    let mut postings: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (index, item_features) in features.iter().enumerate() {
        for feature in item_features {
            postings.entry(feature).or_default().push(index);
        }
    }

    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let previous = preceding[index];
            let next = following[index];
            let eligible = |candidate: usize, articles: &[usize]| {
                identities[candidate] != identities[index]
                    && !articles
                        .iter()
                        .any(|&other| identities[other] == identities[candidate])
                    && [previous, next].into_iter().flatten().all(|adjacent| {
                        adjacent == candidate || identities[adjacent] != identities[candidate]
                    })
            };
            let mut scores = BTreeMap::<usize, u64>::new();
            for feature in &features[index] {
                let candidates = &postings[feature.as_str()];
                let weight = feature_weight(feature, items.len(), candidates.len());
                for &candidate in candidates {
                    if identities[candidate] != identities[index] {
                        *scores.entry(candidate).or_default() += weight;
                    }
                }
            }
            // Seed only a bounded recent pool rather than comparing every article with every
            // other article. Recency decays smoothly with a seven-day half-life; a separate
            // cross-source bonus keeps suggestions diverse while build cost stays linear.
            for (candidate, candidate_item) in items.iter().enumerate().take(CROSS_SOURCE_POOL) {
                if identities[candidate] == identities[index] {
                    continue;
                }
                let score = scores.entry(candidate).or_default();
                *score += newest.map_or(0, |newest| recency_bonus(newest, candidate_item.date));
                if candidate_item.source != item.source {
                    *score += CROSS_SOURCE_BONUS;
                }
            }
            let mut ranked: Vec<_> = scores.into_iter().collect();
            ranked.sort_by(|(a_index, a_score), (b_index, b_score)| {
                b_score
                    .cmp(a_score)
                    .then_with(|| items[*b_index].date.cmp(&items[*a_index].date))
                    .then_with(|| items[*a_index].path.cmp(&items[*b_index].path))
            });
            let mut articles = Vec::with_capacity(RECOMMENDATION_COUNT);
            for (candidate, _) in ranked {
                if articles.len() == RECOMMENDATION_COUNT {
                    break;
                }
                if eligible(candidate, &articles) {
                    articles.push(candidate);
                }
            }
            fill_fallback(&mut articles, items, index, eligible);
            Recommendation {
                previous,
                next,
                articles,
            }
        })
        .collect()
}

fn root(parents: &mut [usize], mut index: usize) -> usize {
    while parents[index] != index {
        parents[index] = parents[parents[index]];
        index = parents[index];
    }
    index
}

/// Recommendations may cross retained URLs, but never repeat the same readable article. Identity
/// joins are transitive: a canonical alias of a mirrored body is still the same article. Substantial
/// exact text matches count; short feed teasers, similar titles and shared topics do not.
fn identities(items: &[ItemCtx], bodies: &[&str]) -> Vec<usize> {
    let mut parents: Vec<_> = (0..items.len()).collect();
    let mut seen = BTreeMap::new();
    for (index, (item, body)) in items.iter().zip(bodies).enumerate() {
        let mut fingerprint = Sha1::new();
        let mut words = 0;
        for word in body.split_whitespace() {
            fingerprint.update(word.as_bytes());
            fingerprint.update(b" ");
            words += 1;
        }
        let keys = [
            (0, item.path.clone()),
            (1, item.url.clone()),
            (2, crate::model::normalize_link(&item.link)),
            (
                3,
                if words >= 64 {
                    hex::encode(fingerprint.finalize())
                } else {
                    String::new()
                },
            ),
        ];
        for key in keys.into_iter().filter(|(_, value)| !value.is_empty()) {
            if let Some(&other) = seen.get(&key) {
                let left = root(&mut parents, index);
                let right = root(&mut parents, other);
                parents[left.max(right)] = left.min(right);
            } else {
                seen.insert(key, index);
            }
        }
    }
    (0..items.len())
        .map(|index| root(&mut parents, index))
        .collect()
}

fn recency_bonus(newest: DateTime<Utc>, candidate: DateTime<Utc>) -> u64 {
    let age = newest.signed_duration_since(candidate).num_hours().max(0) as u64;
    RECENCY_BONUS * RECENCY_HALF_LIFE_HOURS / (RECENCY_HALF_LIFE_HOURS + age)
}

fn fill_fallback(
    articles: &mut Vec<usize>,
    items: &[ItemCtx],
    current: usize,
    eligible: impl Fn(usize, &[usize]) -> bool,
) {
    for prefer_other_source in [true, false] {
        for candidate in 0..items.len() {
            if articles.len() == RECOMMENDATION_COUNT {
                return;
            }
            let other_source = items[candidate].source != items[current].source;
            if other_source == prefer_other_source && eligible(candidate, articles) {
                articles.push(candidate);
            }
        }
    }
}

fn item_features(item: &ItemCtx) -> Vec<String> {
    let mut features = BTreeSet::new();
    for label in &item.labels {
        let label = slug::slugify(label);
        if !label.is_empty() {
            features.insert(format!("label:{label}"));
        }
    }
    if let Some(category) = &item.category {
        let category = slug::slugify(category);
        if !category.is_empty() {
            features.insert(format!("category:{category}"));
        }
    }
    for token in title_terms(&item.title) {
        features.insert(format!("title:{token}"));
    }
    features.into_iter().collect()
}

fn feature_weight(feature: &str, documents: usize, frequency: usize) -> u64 {
    if feature.starts_with("label:") {
        100
    } else if feature.starts_with("category:") {
        40
    } else {
        // Integer inverse-document-frequency: rare title terms matter more, without floating-point
        // ordering or platform-dependent output.
        8 + ((documents.max(1) as u64 * 20) / frequency.max(1) as u64).min(48)
    }
}

fn title_terms(title: &str) -> BTreeSet<String> {
    const STOPWORDS: &[&str] = &[
        "about", "after", "again", "against", "also", "and", "are", "but", "for", "from", "has",
        "have", "how", "into", "its", "not", "of", "on", "our", "that", "the", "their", "this",
        "through", "to", "using", "was", "what", "when", "where", "which", "with", "you", "your",
    ];
    title
        .split(|character: char| !character.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|term| term.chars().count() >= 3 && !STOPWORDS.contains(&term.as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::{TimeZone as _, Utc};

    use super::*;

    fn resolve(items: &[ItemCtx]) -> Vec<Recommendation> {
        super::resolve(items, &vec![""; items.len()])
    }

    #[test]
    fn chronology_and_discovery_skip_the_current_identity_and_repeated_content() {
        let mut items = vec![
            item("Original article", "publisher", "news", &[], 12),
            item("Syndicated title", "aggregator", "news", &[], 11),
            item("Retitled copy", "mirror", "news", &[], 10),
            item("Different article", "publisher", "news", &[], 9),
            item("More reading", "other", "news", &[], 8),
            item("More reading mirror", "syndicated", "news", &[], 7),
        ];
        items[0].link = "http://www.publisher.example/story?utm_source=feed".into();
        items[1].link = "https://publisher.example/story#comments".into();
        items[5].link = items[4].link.clone();
        let body = (0..80)
            .map(|n| format!("word{n}"))
            .collect::<Vec<_>>()
            .join(" ");
        let mirrored_body = body.replace(' ', "\n  ");
        let bodies = [
            body.as_str(),
            "",
            mirrored_body.as_str(),
            "Different prose",
            "More prose",
            "More prose",
        ];
        let recommendations = super::resolve(&items, &bodies);
        assert_eq!(recommendations[0].next, Some(3));
        assert_eq!(recommendations[1].previous, None);
        assert_eq!(recommendations[2].previous, None);
        assert!(
            super::resolve(&items[..3], &bodies[..3])
                .iter()
                .all(|recommendation| {
                    recommendation.previous.is_none()
                        && recommendation.next.is_none()
                        && recommendation.articles.is_empty()
                })
        );
        assert!(!recommendations[0].articles.contains(&1));
        assert!(!recommendations[0].articles.contains(&2));
        assert_eq!(
            recommendations[0]
                .articles
                .iter()
                .filter(|&&index| index == 4 || index == 5)
                .count(),
            1
        );
        for index in [0, 1] {
            assert!(!recommendations[index].articles.contains(&index));
        }
    }

    #[test]
    fn different_product_and_release_pages_remain_adjacent_and_short_teasers_are_not_identity() {
        let mut items = vec![
            item("Apple Unveils iPhone Duo", "hn", "news", &[], 12),
            item("iPhone Duo", "hn", "news", &[], 11),
            item("Another story", "hn", "news", &[], 10),
        ];
        items[0].link = "https://www.apple.com/newsroom/2026/09/apple-unveils-iphone-duo/".into();
        items[1].link = "https://www.apple.com/iphone-duo/".into();
        let bodies = [
            "Read the full story",
            "Read the full story",
            "A different story",
        ];
        assert_eq!(super::resolve(&items, &bodies)[0].next, Some(1));
        items[1].path = items[0].path.clone();
        assert_eq!(super::resolve(&items, &bodies)[0].next, Some(2));
    }

    fn item(title: &str, source: &str, category: &str, labels: &[&str], hour: u32) -> ItemCtx {
        let date = Utc.with_ymd_and_hms(2026, 9, 3, hour, 0, 0).unwrap();
        ItemCtx {
            path: format!("items/{source}/{}", slug::slugify(title)),
            url: format!("items/{source}/{}/", slug::slugify(title)),
            title: title.into(),
            link: format!("https://{source}.example/{}", slug::slugify(title)),
            domain: format!("{source}.example"),
            source: source.into(),
            source_name: source.into(),
            source_display: "example.com".into(),
            source_title: "Example".into(),
            source_url: "https://example.com/".into(),
            feed_display: "example.com".into(),
            is_aggregated: false,
            is_youtube: false,
            category: Some(category.into()),
            date,
            age_band: "fresh",
            published: Some(date),
            updated: None,
            first_seen: date,
            replicated_at: None,
            authors: Vec::new(),
            labels: labels.iter().map(|label| (*label).into()).collect(),
            resources: Vec::new(),
            discussions: Vec::new(),
            summary: None,
            excerpt: String::new(),
            content: crate::model::ContentKind::Extracted,
            word_count: 0,
            reading_minutes: 0,
            preview: None,
            article_preview: None,
            video: None,
            document: None,
            interactive: None,
            native_media: None,
            item_type: crate::site::item_type::ItemType::Article,
            extra: BTreeMap::new(),
            metadata: super::super::display::Metadata::default(),
            permalink: None,
            raw_url: None,
            history_url: None,
            edit_url: None,
            previous_article: None,
            next_article: None,
            recommended_articles: Vec::new(),
            body_html: None,
        }
    }

    #[test]
    fn chronology_and_recommendations_are_deterministic_and_distinct() {
        let items = vec![
            item("Rust async runtimes", "a", "engineering", &["rust"], 12),
            item("Product update", "b", "news", &["release"], 11),
            item("Rust ownership patterns", "c", "engineering", &["rust"], 10),
            item("Database internals", "a", "engineering", &["storage"], 9),
        ];
        let recommendations = resolve(&items);
        assert_eq!(recommendations.len(), items.len());
        assert_eq!(recommendations[0].previous, None);
        assert_eq!(recommendations[0].next, Some(1));
        assert_eq!(recommendations[1].previous, Some(0));
        assert_eq!(recommendations[1].next, Some(2));
        assert_eq!(recommendations[0].articles, vec![2, 1, 3]);
        assert_eq!(resolve(&items), recommendations);
    }

    #[test]
    fn fallback_prefers_other_sources() {
        let items = vec![
            item("Current", "a", "one", &[], 12),
            item("Same source", "a", "two", &[], 11),
            item("Other one", "b", "three", &[], 10),
            item("Other two", "c", "four", &[], 9),
            item("Other three", "d", "five", &[], 8),
        ];
        assert_eq!(resolve(&items)[0].articles, vec![2, 3, 4]);
    }

    #[test]
    fn unrelated_articles_still_receive_three_distinct_suggestions() {
        let items = vec![
            item("Alpha", "a", "one", &["red"], 12),
            item("Bravo", "b", "two", &["blue"], 11),
            item("Charlie", "c", "three", &["green"], 10),
            item("Delta", "d", "four", &["yellow"], 9),
            item("Echo", "e", "five", &["purple"], 8),
        ];

        for (current, recommendation) in resolve(&items).into_iter().enumerate() {
            assert_eq!(recommendation.articles.len(), 3);
            assert!(!recommendation.articles.contains(&current));
            assert_eq!(
                recommendation
                    .articles
                    .iter()
                    .collect::<BTreeSet<_>>()
                    .len(),
                3
            );
        }
    }

    #[test]
    fn recency_bonus_has_a_seven_day_half_life() {
        let newest = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap();
        assert_eq!(recency_bonus(newest, newest), 36);
        assert_eq!(
            recency_bonus(newest, newest - chrono::Duration::days(7)),
            18
        );
        assert_eq!(
            recency_bonus(newest, newest - chrono::Duration::days(21)),
            9
        );
    }
}
