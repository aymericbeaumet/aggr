//! `[store] max_age_days` / `max_items`: which items to drop from the tree. History keeps
//! them, so this only bounds the checkout; starred items are never dropped.

use chrono::{DateTime, Duration, Utc};

use crate::model::Item;

#[derive(Debug, Clone, Copy, Default)]
pub struct Limits {
    pub max_age_days: Option<u32>,
    pub max_items: Option<usize>,
}

impl Limits {
    pub fn is_unbounded(&self) -> bool {
        self.max_age_days.is_none() && self.max_items.is_none()
    }
}

/// Paths (without extension) to delete: everything older than `max_age_days`, then whatever
/// exceeds `max_items` starting from the oldest. Sorted, so the plan is stable.
pub fn plan(items: &[Item], limits: Limits, now: DateTime<Utc>) -> Vec<String> {
    if limits.is_unbounded() {
        return Vec::new();
    }
    let mut candidates: Vec<&Item> = items.iter().filter(|item| !item.front.starred).collect();
    candidates.sort_by_key(|item| std::cmp::Reverse(item.sort_date()));

    let cutoff = limits
        .max_age_days
        .map(|days| now - Duration::days(i64::from(days)));
    let mut drop: Vec<String> = candidates
        .iter()
        .enumerate()
        .filter(|(index, item)| {
            let too_old = cutoff.is_some_and(|cutoff| item.sort_date() < cutoff);
            let too_many = limits.max_items.is_some_and(|max| *index >= max);
            too_old || too_many
        })
        .map(|(_, item)| item.path.clone())
        .collect();
    drop.sort();
    drop
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FrontMatter;
    use chrono::TimeZone;

    fn item(path: &str, days_ago: i64, starred: bool) -> Item {
        let now = Utc.with_ymd_and_hms(2026, 9, 2, 12, 0, 0).unwrap();
        Item {
            path: path.to_string(),
            front: FrontMatter {
                published: Some(now - Duration::days(days_ago)),
                first_seen: now,
                starred,
                ..FrontMatter::default()
            },
            body: String::new(),
        }
    }

    #[test]
    fn unbounded_keeps_everything() {
        let items = vec![item("a", 1000, false)];
        let now = Utc.with_ymd_and_hms(2026, 9, 2, 12, 0, 0).unwrap();
        assert!(plan(&items, Limits::default(), now).is_empty());
    }

    #[test]
    fn drops_old_and_excess_but_never_starred() {
        let now = Utc.with_ymd_and_hms(2026, 9, 2, 12, 0, 0).unwrap();
        let items = vec![
            item("new", 1, false),
            item("mid", 10, false),
            item("old", 400, false),
            item("old-star", 500, true),
            item("older", 401, false),
        ];
        let by_age = plan(
            &items,
            Limits {
                max_age_days: Some(365),
                max_items: None,
            },
            now,
        );
        assert_eq!(by_age, vec!["old", "older"]);

        let by_count = plan(
            &items,
            Limits {
                max_age_days: None,
                max_items: Some(2),
            },
            now,
        );
        assert_eq!(by_count, vec!["old", "older"]);

        let both = plan(
            &items,
            Limits {
                max_age_days: Some(5),
                max_items: Some(1),
            },
            now,
        );
        assert_eq!(both, vec!["mid", "old", "older"]);
    }
}
