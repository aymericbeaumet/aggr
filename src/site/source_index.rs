//! Denormalized publisher/feed membership for one canonical rendered article.

use std::collections::{BTreeMap, BTreeSet};

use super::context::{self, ItemCtx, SourceCtx, SourceMembershipCtx};
use crate::model::Item;

pub(super) fn capture_sources(items: &[Item]) -> Vec<(String, String)> {
    items
        .iter()
        .filter(|item| !item.front.hidden)
        .map(|item| (context::item_url(&item.path), item.front.source.clone()))
        .collect()
}

/// Publisher identity comes from the actual host, never an account path, port, or provider alias.
fn publisher_host(url: &url::Url) -> Option<String> {
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?.trim_end_matches('.');
    Some(host.strip_prefix("www.").unwrap_or(host).to_string())
}

fn publisher_root(article: &str) -> Option<url::Url> {
    let mut url = url::Url::parse(article).ok()?;
    let host = publisher_host(&url)?;
    url.set_host(Some(&host)).ok()?;
    url.set_path("/");
    url.set_query(None);
    url.set_fragment(None);
    let _ = url.set_username("");
    let _ = url.set_password(None);
    Some(url)
}

fn source_host(source: &SourceCtx) -> Option<String> {
    source
        .url
        .iter()
        .chain(source.feed_url.iter())
        .chain(source.site_url.iter())
        .filter_map(|value| url::Url::parse(value).ok())
        .find_map(|url| publisher_host(&url))
}

/// The label an item's publisher reads by. Grouping is by host, but the canonical name a source's
/// own metadata resolved to keeps the account path where one host carries many publishers
/// (`youtube.com/@channel`) and the port where one host serves many ports. A label naming any
/// other host describes a different publisher than the article's, so the host stands alone.
fn publisher_label(resolved: &str, host: &str) -> String {
    match resolved.strip_prefix(host) {
        Some("") => resolved.to_string(),
        Some(rest) if rest.starts_with('/') || rest.starts_with(':') => resolved.to_string(),
        _ => host.to_string(),
    }
}

fn membership(source: &SourceCtx) -> SourceMembershipCtx {
    SourceMembershipCtx {
        query_value: source.slug.clone(),
        slug: source.slug.clone(),
        name: source.name.clone(),
        display: source.slug.clone(),
    }
}

pub(super) fn resolve(
    items: &mut [ItemCtx],
    sources: &mut Vec<SourceCtx>,
    stored: &[(String, String)],
    redirects: &[(String, String)],
) {
    let targets: BTreeMap<_, _> = redirects
        .iter()
        .map(|(from, to)| (from.as_str(), to.as_str()))
        .collect();
    let mut origins = BTreeMap::<String, BTreeSet<String>>::new();
    for (url, source) in stored {
        let canonical = targets.get(url.as_str()).copied().unwrap_or(url);
        origins
            .entry(canonical.to_string())
            .or_default()
            .insert(source.clone());
    }
    let mut publishers = BTreeMap::<String, String>::new();
    for item in items.iter() {
        if let Some(root) = publisher_root(&item.link)
            && let Some(host) = publisher_host(&root)
        {
            let root = root.to_string();
            publishers
                .entry(host)
                .and_modify(|current| {
                    if root < *current {
                        *current = root.clone();
                    }
                })
                .or_insert(root);
        }
    }

    // Archive IDs remain stable; only the public collections are grouped by trusted host metadata.
    let mut feed_ids = BTreeMap::new();
    let mut grouped = BTreeMap::<String, Vec<SourceCtx>>::new();
    for source in std::mem::take(sources) {
        if let Some(host) = source_host(&source) {
            feed_ids.insert(source.slug.clone(), host.clone());
            grouped.entry(host).or_default().push(source);
        }
    }
    for (host, mut feeds) in grouped {
        feeds.sort_by(|a, b| a.slug.cmp(&b.slug));
        let mut source = feeds[0].clone();
        source.slug = host.clone();
        source.query_value = host.clone();
        source.page = format!("sources/{host}/");
        if feeds.iter().any(|feed| feed.name != source.name) {
            source.name = host.clone();
        }
        if feeds.iter().any(|feed| feed.language != source.language) {
            source.language = None;
        }
        if feeds.iter().any(|feed| feed.category != source.category) {
            source.category = None;
        }
        source.error = feeds
            .iter()
            .filter_map(|feed| feed.error.clone())
            .min_by_key(|error| error.since);
        if feeds.len() > 1 {
            source.url = feeds
                .iter()
                .filter_map(|feed| feed.url.as_deref().and_then(publisher_root))
                .map(|url| url.to_string())
                .min();
            source.site_url = source.url.clone();
            source.feed_url = None;
        }
        sources.push(source);
    }
    let configured_indices: BTreeMap<_, _> = sources
        .iter()
        .enumerate()
        .map(|(index, source)| (source.slug.clone(), index))
        .collect();
    for (host, root) in &publishers {
        if let Some(index) = configured_indices.get(host) {
            let source = &mut sources[*index];
            if source.feed_url.is_none()
                && source.url.as_ref().is_some_and(|url| {
                    url::Url::parse(url)
                        .ok()
                        .is_some_and(|url| url.path() != "/" || url.query().is_some())
                })
            {
                source.feed_url = source.url.clone();
            }
            source.url = Some(root.clone());
            source.site_url = Some(root.clone());
        } else {
            sources.push(SourceCtx {
                page: format!("sources/{host}/"),
                query_value: host.clone(),
                slug: host.clone(),
                name: host.clone(),
                url: Some(root.clone()),
                site_url: Some(root.clone()),
                feed_url: None,
                language: None,
                category: None,
                engine: "publisher".into(),
                count: 0,
                latest: None,
                error: None,
            });
        }
    }
    let directory: BTreeMap<_, _> = sources
        .iter()
        .map(|source| (source.slug.as_str(), source))
        .collect();
    for item in items.iter_mut() {
        let mut captures = origins.get(&item.url).cloned().unwrap_or_default();
        captures.insert(item.source.clone());
        let mut memberships: BTreeSet<String> = captures
            .iter()
            .filter_map(|source| feed_ids.get(source).cloned())
            .collect();
        if let Some(root) = publisher_root(&item.link)
            && let Some(host) = publisher_host(&root)
        {
            item.publisher_source = host.clone();
            item.source_display = publisher_label(&item.source_display, &host);
            item.source_title = directory
                .get(host.as_str())
                .map(|source| source.name.clone())
                .unwrap_or_else(|| host.clone());
            item.source_url = root.to_string();
            memberships.insert(host);
        } else {
            item.publisher_source = feed_ids.get(&item.source).cloned().unwrap_or_default();
            item.source_display = item.publisher_source.clone();
        }
        item.source_memberships = memberships
            .iter()
            .filter_map(|slug| {
                directory
                    .get(slug.as_str())
                    .map(|source| membership(source))
            })
            .collect();
        item.is_aggregated = item
            .source_memberships
            .iter()
            .any(|source| source.slug != item.publisher_source);
        item.feed_display = item
            .source_memberships
            .iter()
            .filter(|source| source.slug != item.publisher_source)
            .map(|source| source.display.as_str())
            .collect::<Vec<_>>()
            .join(", ");
    }
    let members = super::directory::source_members(items);
    for source in sources.iter_mut() {
        let indices = members
            .get(&source.slug)
            .map(Vec::as_slice)
            .unwrap_or_default();
        source.count = indices.len();
        source.latest = indices.iter().map(|index| items[*index].date).max();
    }
    for item in items.iter_mut() {
        item.metadata = super::display::Metadata::from(&*item);
    }
    sources.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.slug.cmp(&b.slug))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(url: &str) -> SourceCtx {
        SourceCtx {
            query_value: "configured".into(),
            slug: "configured".into(),
            name: "Configured publisher".into(),
            url: Some(url.into()),
            site_url: None,
            feed_url: None,
            language: None,
            category: None,
            engine: "web".into(),
            count: 0,
            latest: None,
            error: None,
            page: "sources/configured/".into(),
        }
    }

    fn article(source: &str, stem: &str, link: &str) -> ItemCtx {
        let item = Item {
            path: format!("items/{source}/2026/09/{stem}"),
            front: crate::model::FrontMatter {
                source: source.into(),
                link: link.into(),
                title: stem.into(),
                first_seen: chrono::Utc::now(),
                ..Default::default()
            },
            body: String::new(),
        };
        ItemCtx::from_item(
            &item,
            context::ItemOptions {
                reading_metrics: (0, 0),
                source_name: source,
                category: None,
                links: None,
                excerpt: String::new(),
                discussions: &[],
                resolutions: &crate::discussions::ResolutionSet::default(),
                now: chrono::Utc::now(),
            },
        )
    }

    #[test]
    fn publisher_identity_uses_only_the_actual_host_across_paths_and_ports() {
        let mut items = vec![
            article("configured", "alice", "https://social.example/@alice/1"),
            article("configured", "bob", "https://social.example/@bob/2"),
            article("configured", "port", "https://social.example:8443/@alice/1"),
        ];
        let links: Vec<_> = items.iter().map(|item| item.link.clone()).collect();
        let mut sources = vec![source("https://feeds.example/rss")];
        resolve(&mut items, &mut sources, &[], &[]);
        for item in &items {
            assert_eq!(item.publisher_source, "social.example");
            assert_eq!(item.metadata.source_slug, "social.example");
            assert_eq!(item.source_display, "social.example");
            assert!(!item.source_url.contains('@'));
            assert!(
                item.source_memberships
                    .iter()
                    .any(|source| source.slug == "feeds.example")
            );
        }
        assert_eq!(
            items
                .iter()
                .map(|item| item.link.clone())
                .collect::<Vec<_>>(),
            links
        );
        assert_eq!(
            sources
                .iter()
                .find(|source| source.slug == "social.example")
                .unwrap()
                .count,
            3
        );
    }

    #[test]
    fn platform_publishers_keep_their_account_label_while_grouping_by_host() {
        let channel = SourceCtx {
            query_value: "youtube-com-veritasium".into(),
            slug: "youtube-com-veritasium".into(),
            name: "Veritasium".into(),
            site_url: Some("https://www.youtube.com/channel/UC123".into()),
            feed_url: Some("https://www.youtube.com/feeds/videos.xml?channel_id=UC123".into()),
            ..source("https://www.youtube.com/@veritasium")
        };
        let mut items = vec![article(
            "youtube-com-veritasium",
            "enigma",
            "https://www.youtube.com/watch?v=abc",
        )];
        items[0].set_source(&channel);
        let mut sources = vec![channel];
        resolve(&mut items, &mut sources, &[], &[]);

        let item = &items[0];
        assert_eq!(item.source_display, "youtube.com/@veritasium");
        assert_eq!(item.metadata.source_display, "youtube.com/@veritasium");
        assert_eq!(item.source_title, "Veritasium");
        assert!(!item.is_aggregated);
        // The account names the publisher; the collection it joins is still the whole host.
        assert_eq!(item.publisher_source, "youtube.com");
        assert_eq!(item.metadata.source_slug, "youtube.com");
        assert_eq!(item.source_url, "https://youtube.com/");
        assert!(sources.iter().any(|source| source.slug == "youtube.com"));
    }

    #[test]
    fn publisher_labels_keep_only_an_account_of_the_article_host() {
        for (resolved, host, expected) in [
            (
                "youtube.com/@veritasium",
                "youtube.com",
                "youtube.com/@veritasium",
            ),
            (
                "example.test:8443/@alice",
                "example.test",
                "example.test:8443/@alice",
            ),
            ("example.com", "example.com", "example.com"),
            // The feed names a platform the article does not come from.
            (
                "spotify.com/underscore",
                "publisher.example",
                "publisher.example",
            ),
            // A different host that merely starts with the same letters.
            ("youtube.community", "youtube.com", "youtube.com"),
        ] {
            assert_eq!(publisher_label(resolved, host), expected, "{resolved}");
        }
    }

    #[test]
    fn hostname_normalization_keeps_subdomains_and_actual_provider_domains() {
        for (url, expected) in [
            ("https://WWW.DuckDB.org./news/article", "duckdb.org"),
            ("https://news.example.co.uk/article", "news.example.co.uk"),
            ("https://alice.github.io/article", "alice.github.io"),
            ("https://bob.github.io/article", "bob.github.io"),
            ("https://twitter.com/alice/status/1", "twitter.com"),
            ("https://x.com/bob/status/1", "x.com"),
            ("https://m.youtube.com/watch?v=123", "m.youtube.com"),
            ("https://youtu.be/123", "youtu.be"),
            ("https://mastodon.social/@alice/1", "mastodon.social"),
            ("https://www.reddit.com/r/rust/comments/1", "reddit.com"),
            ("http://127.0.0.1:8080/article", "127.0.0.1"),
            ("https://BÜCHER.example/article", "xn--bcher-kva.example"),
        ] {
            let root = publisher_root(url).unwrap();
            assert_eq!(publisher_host(&root).as_deref(), Some(expected), "{url}");
            assert_eq!(root.path(), "/");
            assert!(root.query().is_none());
            assert!(root.fragment().is_none());
        }
        assert!(publisher_root("javascript:alert(1)").is_none());
    }

    #[test]
    fn configured_account_feeds_aggregate_under_one_host_without_changing_archive_ids() {
        let mut alice = source("https://social.example/@alice/rss");
        alice.slug = "alice-feed".into();
        alice.name = "Alice".into();
        let mut bob = source("https://social.example/@bob/rss");
        bob.slug = "bob-feed".into();
        bob.name = "Bob".into();
        let mut sources = vec![alice, bob];
        let mut items = vec![
            article("alice-feed", "a", "https://social.example/@alice/1"),
            article("bob-feed", "b", "https://social.example/@bob/2"),
        ];
        resolve(&mut items, &mut sources, &[], &[]);
        for (item, feed) in items.iter().zip(["alice-feed", "bob-feed"]) {
            assert_eq!(item.publisher_source, "social.example");
            assert_eq!(item.source, feed);
            assert!(item.metadata.feed_sources.is_empty());
            assert_eq!(item.source_memberships.len(), 1);
            assert_eq!(item.metadata.source_query, "social.example");
        }
        assert_eq!(
            sources
                .iter()
                .find(|source| source.slug == "social.example")
                .unwrap()
                .count,
            2
        );
    }

    #[test]
    fn unrelated_feed_slug_collision_never_merges_publisher_memberships() {
        let mut feed = source("https://unrelated.example/feed.xml");
        feed.slug = "maharship.com".into();
        feed.name = "Original configured feed".into();
        let mut sources = vec![feed];
        let mut items = vec![
            article("maharship.com", "a", "https://maharship.com/post"),
            article("maharship.com", "b", "https://unrelated.example/post"),
        ];
        resolve(&mut items, &mut sources, &[], &[]);
        assert!(items.iter().all(|item| item.source == "maharship.com"));
        assert_eq!(items[0].publisher_source, "maharship.com");
        assert_eq!(items[1].publisher_source, "unrelated.example");
        assert!(
            items[1]
                .source_memberships
                .iter()
                .all(|source| source.slug != "maharship.com")
        );
        assert_eq!(items[0].metadata.feed_sources[0].slug, "unrelated.example");
        assert!(items[1].metadata.feed_sources.is_empty());
        assert_eq!(
            sources
                .iter()
                .find(|source| source.slug == "maharship.com")
                .unwrap()
                .count,
            1
        );
        let feed = sources
            .iter()
            .find(|source| source.slug == "unrelated.example")
            .unwrap();
        assert_eq!(feed.count, 2);
        assert_eq!(feed.page, "sources/unrelated.example/");
    }

    #[test]
    fn a_compatible_configured_hostname_is_reused_without_duplicate_memberships() {
        let mut feed = source("https://maharship.com/feed.xml");
        feed.slug = "maharship.com".into();
        let mut sources = vec![feed];
        let mut items = vec![article(
            "maharship.com",
            "a",
            "https://maharship.com/posts/1",
        )];
        resolve(&mut items, &mut sources, &[], &[]);
        assert_eq!(sources.len(), 1);
        assert_eq!(items[0].source_memberships.len(), 1);
        assert_eq!(items[0].publisher_source, "maharship.com");
        assert_eq!(items[0].source_url, "https://maharship.com/");
        assert_eq!(
            sources[0].feed_url.as_deref(),
            Some("https://maharship.com/feed.xml")
        );
    }

    #[test]
    fn source_labels_use_canonical_hosts_even_for_a_single_path_based_feed() {
        for url in [
            "https://hnrss.org/frontpage",
            "https://hnrss.org:8443/newest?points=100",
            "https://feeds.example/@author/rss",
        ] {
            let mut sources = vec![source(url)];
            let mut items = vec![article(
                "configured",
                "post",
                "https://publisher.example/post",
            )];
            resolve(&mut items, &mut sources, &[], &[]);
            let expected = publisher_host(&url::Url::parse(url).unwrap()).unwrap();
            let item = &items[0];
            assert_eq!(item.source_display, "publisher.example");
            assert_eq!(item.feed_display, expected);
            assert_eq!(
                item.metadata.feed_display.as_deref(),
                Some(expected.as_str())
            );
            assert_eq!(item.metadata.feed_sources[0].display, expected);
            assert_eq!(item.metadata.feed_sources[0].query_value, expected);
        }
    }

    #[test]
    fn source_filters_use_feed_hosts_instead_of_names_paths_or_ports() {
        let mut front = source("https://hnrss.org/frontpage");
        front.slug = "hnrss-org-frontpage".into();
        front.name = "Hacker News: Front Page".into();
        let mut newest = front.clone();
        newest.slug = "hnrss-org-newest".into();
        newest.url = Some("https://hnrss.org:8443/newest".into());
        newest.name = "Hacker News: Newest".into();
        let mut sources = vec![front, newest];
        let mut items = vec![
            article("hnrss-org-frontpage", "first", "https://news.example/a"),
            article("hnrss-org-newest", "second", "https://news.example/b"),
        ];
        let originals: Vec<_> = items
            .iter()
            .map(|item| (item.source.clone(), item.url.clone()))
            .collect();
        resolve(&mut items, &mut sources, &[], &[]);
        assert_eq!(sources.len(), 2);
        let feed = sources
            .iter()
            .find(|source| source.slug == "hnrss.org")
            .unwrap();
        assert_eq!(feed.query_value, "hnrss.org");
        assert_eq!(feed.count, 2);
        assert_eq!(feed.page, "sources/hnrss.org/");
        assert_eq!(
            feed.feed_url, None,
            "a grouped host must not claim one subscription endpoint"
        );
        for (item, original) in items.iter().zip(originals) {
            assert_eq!((item.source.clone(), item.url.clone()), original);
            assert_eq!(item.metadata.feed_sources[0].slug, "hnrss.org");
            assert_eq!(item.metadata.feed_sources[0].query_value, "hnrss.org");
        }
    }

    #[test]
    fn retained_source_metadata_supplies_hosts_and_unknown_origins_do_not_guess() {
        let mut retired = source("https://placeholder.example");
        retired.url = None;
        retired.feed_url = Some("https://feeds.example/retired.xml".into());
        retired.slug = "retired".into();
        let mut unknown = retired.clone();
        unknown.slug = "news.example".into();
        unknown.feed_url = None;
        let mut sources = vec![retired, unknown];
        let mut items = vec![
            article("retired", "first", "https://news.example/a"),
            article("news.example", "second", "https://other.example/b"),
            article("news.example", "anonymous", ""),
        ];
        resolve(&mut items, &mut sources, &[], &[]);
        assert_eq!(items[0].metadata.feed_sources[0].slug, "feeds.example");
        assert_eq!(items[1].source_memberships.len(), 1);
        assert_eq!(items[1].source_memberships[0].slug, "other.example");
        assert!(items[2].source_memberships.is_empty());
        assert!(items[2].metadata.source_slug.is_empty());
        assert!(items[2].metadata.source_display.is_empty());
        assert_eq!(
            sources
                .iter()
                .find(|source| source.slug == "news.example")
                .unwrap()
                .count,
            1
        );
    }
}
