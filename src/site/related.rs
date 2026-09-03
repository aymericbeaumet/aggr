//! Build-time chronological navigation and article suggestions.

use std::collections::{BTreeMap, BTreeSet};

use super::context::ItemCtx;

const RECOMMENDATION_COUNT: usize = 3;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Recommendation {
    pub previous: Option<usize>,
    pub next: Option<usize>,
    pub articles: Vec<usize>,
}

/// Resolve navigation for items sorted newest first. Suggestions follow the same weighted-index
/// model used by established static-site generators: shared labels dominate, then category,
/// uncommon title terms, and source. An inverted index keeps this proportional to matching
/// features rather than comparing every pair of articles.
pub fn resolve(items: &[ItemCtx]) -> Vec<Recommendation> {
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
        .map(|(index, _)| {
            let previous = index.checked_sub(1);
            let next = (index + 1 < items.len()).then_some(index + 1);
            let mut scores = BTreeMap::<usize, u64>::new();
            for feature in &features[index] {
                let candidates = &postings[feature.as_str()];
                let weight = feature_weight(feature, items.len(), candidates.len());
                for &candidate in candidates {
                    if candidate != index {
                        *scores.entry(candidate).or_default() += weight;
                    }
                }
            }
            let mut ranked: Vec<_> = scores.into_iter().collect();
            ranked.sort_by(|(a_index, a_score), (b_index, b_score)| {
                b_score
                    .cmp(a_score)
                    .then_with(|| items[*b_index].date.cmp(&items[*a_index].date))
                    .then_with(|| items[*a_index].path.cmp(&items[*b_index].path))
            });
            let mut articles: Vec<_> = ranked
                .into_iter()
                .map(|(candidate, _)| candidate)
                .take(RECOMMENDATION_COUNT)
                .collect();
            fill_fallback(&mut articles, items.len(), index);
            debug_assert_eq!(
                articles.len(),
                RECOMMENDATION_COUNT.min(items.len().saturating_sub(1))
            );
            Recommendation {
                previous,
                next,
                articles,
            }
        })
        .collect()
}

fn fill_fallback(articles: &mut Vec<usize>, len: usize, current: usize) {
    for candidate in 0..len {
        if articles.len() == RECOMMENDATION_COUNT {
            break;
        }
        if candidate != current && !articles.contains(&candidate) {
            articles.push(candidate);
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
    features.insert(format!("source:{}", item.source));
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
    } else if feature.starts_with("source:") {
        12
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
            category: Some(category.into()),
            date,
            age_band: "fresh",
            published: Some(date),
            updated: None,
            first_seen: date,
            authors: Vec::new(),
            labels: labels.iter().map(|label| (*label).into()).collect(),
            discussions: Vec::new(),
            summary: None,
            excerpt: String::new(),
            content: crate::model::ContentKind::Extracted,
            extra: BTreeMap::new(),
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
        assert_eq!(recommendations[0].articles, vec![2, 3, 1]);
        assert_eq!(resolve(&items), recommendations);
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
}
