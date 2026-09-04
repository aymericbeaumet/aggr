//! Build-time chronological navigation and article suggestions.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};

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
pub fn resolve(items: &[ItemCtx]) -> Vec<Recommendation> {
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
            // Seed only a bounded recent pool rather than comparing every article with every
            // other article. Recency decays smoothly with a seven-day half-life; a separate
            // cross-source bonus keeps suggestions diverse while build cost stays linear.
            for (candidate, candidate_item) in items.iter().enumerate().take(CROSS_SOURCE_POOL) {
                if candidate == index {
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
            let mut articles: Vec<_> = ranked
                .into_iter()
                .map(|(candidate, _)| candidate)
                .take(RECOMMENDATION_COUNT)
                .collect();
            fill_fallback(&mut articles, items, index);
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

fn recency_bonus(newest: DateTime<Utc>, candidate: DateTime<Utc>) -> u64 {
    let age = newest.signed_duration_since(candidate).num_hours().max(0) as u64;
    RECENCY_BONUS * RECENCY_HALF_LIFE_HOURS / (RECENCY_HALF_LIFE_HOURS + age)
}

fn fill_fallback(articles: &mut Vec<usize>, items: &[ItemCtx], current: usize) {
    for prefer_other_source in [true, false] {
        for candidate in 0..items.len() {
            if articles.len() == RECOMMENDATION_COUNT {
                return;
            }
            let other_source = items[candidate].source != items[current].source;
            if candidate != current
                && other_source == prefer_other_source
                && !articles.contains(&candidate)
            {
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
            replicated_at: None,
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
