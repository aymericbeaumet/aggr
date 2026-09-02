//! The daily digest: one GitHub issue per day listing what arrived since the previous digest.
//! Each posted digest is pinned by a ref `refs/aggr/digest/<yyyy-mm-dd>` on the data commit it
//! covered, which is both the once-a-day gate and the "since" boundary of the next one. The
//! issue number in the title is simply how many such refs exist.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use chrono::{DateTime, Duration, NaiveDate, NaiveTime, Utc};
use chrono_tz::Tz;
use serde::Serialize;

use crate::config::Source;
use crate::model::Item;
use crate::site::context::{GitHubLinks, domain_of, item_url};

pub const REF_PREFIX: &str = "refs/aggr/digest/";
pub const LABEL_COLOR: &str = "0f766e";
pub const LABEL_DESCRIPTION: &str = "Daily digest posted by aggr";

/// Whether a digest is due now, and for which local date. Due once per local day, from `at` on.
pub fn due(
    now: DateTime<Utc>,
    tz: Tz,
    at: NaiveTime,
    posted: &BTreeSet<NaiveDate>,
) -> Option<NaiveDate> {
    let local = now.with_timezone(&tz);
    let today = local.date_naive();
    (local.time() >= at && !posted.contains(&today)).then_some(today)
}

pub fn ref_name(date: NaiveDate) -> String {
    format!("{REF_PREFIX}{}", date.format("%Y-%m-%d"))
}

pub fn ref_date(name: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(name.strip_prefix(REF_PREFIX)?, "%Y-%m-%d").ok()
}

/// Items that belong in the digest, newest first: those whose file was added since the last
/// digest when git can tell us, else everything first seen after `since`.
pub fn select(
    mut items: Vec<Item>,
    added: Option<&HashSet<String>>,
    since: DateTime<Utc>,
) -> Vec<Item> {
    items.retain(|item| {
        !item.front.hidden
            && match added {
                Some(paths) => paths.contains(&item.md_path()),
                None => item.front.first_seen > since,
            }
    });
    items.sort_by(|a, b| {
        b.created_at()
            .cmp(&a.created_at())
            .then_with(|| b.path.cmp(&a.path))
    });
    items
}

/// `{number}`, `{date}`, `{count}`, `{title}` placeholders.
pub fn title(
    template: &str,
    number: usize,
    date: NaiveDate,
    count: usize,
    site_title: &str,
) -> String {
    template
        .replace("{number}", &number.to_string())
        .replace("{date}", &date.format("%Y-%m-%d").to_string())
        .replace("{count}", &count.to_string())
        .replace("{title}", site_title)
}

#[derive(Debug, Serialize)]
pub struct DigestCtx {
    pub number: usize,
    pub date: String,
    pub count: usize,
    pub omitted: usize,
    pub since: DateTime<Utc>,
    pub site_url: Option<String>,
    pub repository: Option<String>,
    pub data_sha: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Group {
    pub slug: String,
    pub name: String,
    pub items: Vec<DigestItem>,
}

#[derive(Debug, Serialize)]
pub struct DigestItem {
    pub title: String,
    pub link: String,
    pub domain: String,
    pub date: DateTime<Utc>,
    pub page: Option<String>,
    pub permalink: Option<String>,
}

/// Group the newest `max_items` by source, in configuration order, unknown sources last.
pub fn group(
    items: &[Item],
    sources: &[Source],
    names: &BTreeMap<String, String>,
    links: Option<&GitHubLinks<'_>>,
    site_url: Option<&str>,
    max_items: usize,
) -> (Vec<Group>, usize) {
    let shown = &items[..items.len().min(max_items)];
    let order: BTreeMap<&str, usize> = sources
        .iter()
        .enumerate()
        .map(|(i, s)| (s.slug.as_str(), i))
        .collect();
    let mut groups: BTreeMap<(usize, &str), Vec<DigestItem>> = BTreeMap::new();
    for item in shown {
        let slug = item.front.source.as_str();
        let rank = order.get(slug).copied().unwrap_or(usize::MAX);
        groups.entry((rank, slug)).or_default().push(DigestItem {
            title: item.front.title.clone(),
            link: item.front.link.clone(),
            domain: domain_of(&item.front.link),
            date: item.created_at(),
            page: site_url.map(|root| format!("{root}{}", item_url(&item.path))),
            permalink: links.map(|l| l.permalink(&item.md_path())),
        });
    }
    let groups = groups
        .into_iter()
        .map(|((_, slug), items)| Group {
            slug: slug.to_string(),
            name: names.get(slug).cloned().unwrap_or_else(|| slug.to_string()),
            items,
        })
        .collect();
    (groups, items.len() - shown.len())
}

/// Boundary for the first digest ever: the last 24 hours.
pub fn first_digest_since(now: DateTime<Utc>) -> DateTime<Utc> {
    now - Duration::hours(24)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FrontMatter;
    use chrono::TimeZone;

    fn at(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    #[test]
    fn due_once_a_day_after_the_local_time() {
        let paris = chrono_tz::Europe::Paris;
        // 06:30 UTC = 08:30 Paris (CEST).
        let now = Utc.with_ymd_and_hms(2026, 9, 2, 6, 30, 0).unwrap();
        let none = BTreeSet::new();
        assert_eq!(
            due(now, paris, at(8, 0), &none),
            Some(NaiveDate::from_ymd_opt(2026, 9, 2).unwrap())
        );
        assert_eq!(due(now, paris, at(9, 0), &none), None);
        assert_eq!(due(now, chrono_tz::UTC, at(8, 0), &none), None);
        let posted: BTreeSet<_> = [NaiveDate::from_ymd_opt(2026, 9, 2).unwrap()].into();
        assert_eq!(due(now, paris, at(8, 0), &posted), None);
        // Just before local midnight it is still "today"; the next day is a fresh gate.
        let late = Utc.with_ymd_and_hms(2026, 9, 2, 21, 59, 0).unwrap();
        assert_eq!(due(late, paris, at(8, 0), &posted), None);
        let next = Utc.with_ymd_and_hms(2026, 9, 3, 6, 0, 0).unwrap();
        assert_eq!(
            due(next, paris, at(8, 0), &posted),
            Some(NaiveDate::from_ymd_opt(2026, 9, 3).unwrap())
        );
    }

    #[test]
    fn ref_names_round_trip() {
        let date = NaiveDate::from_ymd_opt(2026, 9, 2).unwrap();
        assert_eq!(ref_name(date), "refs/aggr/digest/2026-09-02");
        assert_eq!(ref_date("refs/aggr/digest/2026-09-02"), Some(date));
        assert_eq!(ref_date("refs/aggr/last-good"), None);
        assert_eq!(ref_date("refs/aggr/digest/nope"), None);
    }

    fn item(path: &str, source: &str, seen_hour: u32) -> Item {
        Item {
            path: path.into(),
            front: FrontMatter {
                title: format!("T {path}"),
                link: format!("https://ex.com/{path}"),
                source: source.into(),
                first_seen: Utc.with_ymd_and_hms(2026, 9, 2, seen_hour, 0, 0).unwrap(),
                ..Default::default()
            },
            body: String::new(),
        }
    }

    #[test]
    fn selects_by_added_paths_or_first_seen() {
        let items = vec![
            item("items/a/2026/09/x", "a", 1),
            item("items/b/2026/09/y", "b", 5),
            item("items/a/2026/09/z", "a", 9),
        ];
        let added: HashSet<String> = ["items/a/2026/09/z.md".to_string()].into();
        let picked = select(
            items.clone(),
            Some(&added),
            Utc.with_ymd_and_hms(2000, 1, 1, 0, 0, 0).unwrap(),
        );
        assert_eq!(
            picked.iter().map(|i| i.path.as_str()).collect::<Vec<_>>(),
            ["items/a/2026/09/z"]
        );
        let since = Utc.with_ymd_and_hms(2026, 9, 2, 4, 0, 0).unwrap();
        let picked = select(items, None, since);
        assert_eq!(
            picked.iter().map(|i| i.path.as_str()).collect::<Vec<_>>(),
            ["items/a/2026/09/z", "items/b/2026/09/y"]
        );
    }

    #[test]
    fn groups_in_config_order_and_caps() {
        let items = vec![
            item("items/b/2026/09/y", "b", 5),
            item("items/a/2026/09/z", "a", 9),
            item("items/zz/2026/09/q", "zz", 3),
        ];
        let sources = vec![
            Source {
                slug: "a".into(),
                name: None,
                category: None,
                labels: vec![],
                headers: vec![],
                html: true,
                engine: crate::config::Engine::Feed {
                    url: url::Url::parse("https://a/").unwrap(),
                },
            },
            Source {
                slug: "b".into(),
                name: None,
                category: None,
                labels: vec![],
                headers: vec![],
                html: true,
                engine: crate::config::Engine::Feed {
                    url: url::Url::parse("https://b/").unwrap(),
                },
            },
        ];
        let names: BTreeMap<String, String> = [("a".to_string(), "Alpha".to_string())].into();
        let links = GitHubLinks {
            repository: "o/r",
            branch: "aggr",
            data_sha: Some("abc"),
        };
        let (groups, omitted) = group(
            &items,
            &sources,
            &names,
            Some(&links),
            Some("https://s.io/"),
            2,
        );
        assert_eq!(omitted, 1);
        assert_eq!(
            groups.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(),
            ["Alpha", "b"]
        );
        assert_eq!(
            groups[0].items[0].page.as_deref(),
            Some("https://s.io/items/a/2026/09/z/")
        );
        assert_eq!(
            groups[0].items[0].permalink.as_deref(),
            Some("https://github.com/o/r/blob/abc/items/a/2026/09/z.md")
        );
        let (groups, _) = group(&items, &sources, &names, None, None, 10);
        assert_eq!(groups.last().unwrap().slug, "zz");
    }

    #[test]
    fn titles_expand_placeholders() {
        let date = NaiveDate::from_ymd_opt(2026, 9, 2).unwrap();
        assert_eq!(
            title(
                "Digest #{number} · {date} · {count} new",
                3,
                date,
                12,
                "Reads"
            ),
            "Digest #3 · 2026-09-02 · 12 new"
        );
        assert_eq!(title("{title}", 1, date, 0, "Reads"), "Reads");
    }
}
