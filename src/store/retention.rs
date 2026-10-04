//! Per-feed retention over complete article families. Historical Git objects remain unchanged.

use crate::config::Limits;
use crate::model::Item;
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;

/// Remove expired articles, then oldest excess articles until count and byte limits fit.
/// Sizes include Markdown, HTML, and owned media companions. Each feed has its own allowance.
pub fn plan(
    items: &[Item],
    sizes: &BTreeMap<String, u64>,
    limits: impl Fn(&str) -> Limits,
    now: DateTime<Utc>,
) -> Vec<String> {
    let mut feeds: BTreeMap<&str, Vec<&Item>> = BTreeMap::new();
    for item in items {
        feeds.entry(&item.front.source).or_default().push(item);
    }
    let mut removed = Vec::new();
    for (source, mut items) in feeds {
        let limits = limits(source);
        items.sort_by(|a, b| {
            b.created_at()
                .cmp(&a.created_at())
                .then_with(|| a.path.cmp(&b.path))
        });
        let cutoff = limits.cutoff(now);
        let mut retained = Vec::new();
        for item in items {
            if cutoff.is_some_and(|cutoff| item.created_at() < cutoff)
                || limits.max_items != 0 && retained.len() >= limits.max_items
            {
                removed.push(item.path.clone());
            } else {
                retained.push(item);
            }
        }
        if limits.max_bytes != 0 {
            let mut bytes = retained
                .iter()
                .map(|item| u128::from(sizes.get(&item.path).copied().unwrap_or(0)))
                .sum::<u128>();
            while bytes > u128::from(limits.max_bytes) {
                let Some(item) = retained.pop() else {
                    break;
                };
                bytes -= u128::from(sizes.get(&item.path).copied().unwrap_or(0));
                removed.push(item.path.clone());
            }
        }
    }
    removed.sort();
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FrontMatter;
    use chrono::{Duration, TimeZone};
    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap()
    }
    fn item(source: &str, name: &str, days: i64) -> Item {
        Item {
            path: format!("items/{source}/{name}"),
            front: FrontMatter {
                source: source.into(),
                published: Some(now() - Duration::days(days)),
                first_seen: now(),
                ..Default::default()
            },
            body: String::new(),
        }
    }
    fn unlimited() -> Limits {
        Limits {
            max_items: 0,
            max_age_days: 0,
            max_bytes: 0,
            since: None,
        }
    }

    #[test]
    fn each_feed_gets_its_own_count_and_age_allowance() {
        let items = vec![
            item("a", "new", 1),
            item("a", "old", 4),
            item("b", "new", 1),
            item("b", "older", 731),
        ];
        let removed = plan(
            &items,
            &BTreeMap::new(),
            |source| Limits {
                max_items: if source == "a" { 1 } else { 250 },
                ..Limits::default()
            },
            now(),
        );
        assert_eq!(removed, ["items/a/old", "items/b/older"]);
    }
    #[test]
    fn bytes_evict_oldest_complete_families_only_from_the_overfull_feed() {
        let items = vec![
            item("a", "new", 1),
            item("a", "middle", 2),
            item("a", "old", 3),
            item("b", "only", 4),
        ];
        let sizes = items.iter().map(|item| (item.path.clone(), 40)).collect();
        let removed = plan(
            &items,
            &sizes,
            |source| Limits {
                max_bytes: if source == "a" { 80 } else { 40 },
                ..unlimited()
            },
            now(),
        );
        assert_eq!(removed, ["items/a/old"]);
    }
    #[test]
    fn absolute_date_is_inclusive_and_missing_publication_uses_capture_time() {
        let mut boundary = item("a", "boundary", 0);
        boundary.front.published = Some(Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap());
        let mut undated = item("a", "undated", 1000);
        undated.front.published = None;
        let items = vec![boundary, undated, item("a", "before", 4)];
        let removed = plan(
            &items,
            &BTreeMap::new(),
            |_| Limits {
                since: Some(chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap()),
                ..unlimited()
            },
            now(),
        );
        assert_eq!(removed, ["items/a/before"]);
    }
    #[test]
    fn equal_dates_and_oversized_newest_items_have_deterministic_results() {
        let items = vec![item("a", "z", 1), item("a", "a", 1)];
        assert_eq!(
            plan(
                &items,
                &BTreeMap::new(),
                |_| Limits {
                    max_items: 1,
                    ..unlimited()
                },
                now()
            ),
            ["items/a/z"]
        );
        let sizes = items.iter().map(|item| (item.path.clone(), 200)).collect();
        assert_eq!(
            plan(
                &items,
                &sizes,
                |_| Limits {
                    max_bytes: 100,
                    ..unlimited()
                },
                now()
            ),
            ["items/a/a", "items/a/z"]
        );
        assert!(plan(&items, &sizes, |_| unlimited(), now()).is_empty());
    }
}
