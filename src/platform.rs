//! Who is publishing, when one host is shared by many publishers.
//!
//! On an ordinary site the domain is the identity: everything under `example.com` belongs to the
//! same publisher, and `example.com/blog/feed.xml` says nothing more than `example.com` does. On a
//! platform, one host carries thousands of unrelated accounts, so the account's path is part of
//! who published the article.
//!
//! Only the hosts listed here keep their path, plus the `@handle` convention, which identifies an
//! account unambiguously on federated hosts that cannot be listed in advance.

use url::Url;

/// A platform host and the path segments that introduce an account on it. An empty prefix list
/// means the first path segment is itself the account.
const PLATFORMS: &[(&str, &[&str])] = &[
    ("bsky.app", &["profile"]),
    ("codeberg.org", &[]),
    ("github.com", &[]),
    ("gitlab.com", &[]),
    ("medium.com", &[]),
    ("reddit.com", &["r", "user"]),
    ("soundcloud.com", &[]),
    ("twitch.tv", &[]),
    ("vimeo.com", &[]),
    ("x.com", &[]),
    ("youtube.com", &["channel", "user", "c"]),
];

/// The host as aggr names it: no trailing dot, no `www.`, and the platform's own domain rather
/// than one of its short or mobile aliases.
pub fn host(url: &Url) -> Option<&str> {
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?.trim_end_matches('.');
    Some(match host.strip_prefix("www.").unwrap_or(host) {
        "youtu.be" | "m.youtube.com" | "music.youtube.com" => "youtube.com",
        "twitter.com" | "mobile.twitter.com" => "x.com",
        "old.reddit.com" | "np.reddit.com" => "reddit.com",
        "player.vimeo.com" => "vimeo.com",
        "m.twitch.tv" => "twitch.tv",
        host => host,
    })
}

/// The host with its port, when the port is not the scheme's default.
fn authority(url: &Url) -> Option<String> {
    let host = host(url)?;
    Some(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

/// The path identifying the account that published `url`, without a leading slash.
///
/// `None` for an ordinary site, where the host is the whole identity.
pub fn account_path(url: &Url) -> Option<String> {
    let segments: Vec<_> = url
        .path_segments()?
        .filter(|segment| !segment.is_empty())
        .collect();
    let first = segments.first()?;

    // An `@handle` names an account wherever it appears, including on the federated hosts that
    // cannot be listed here.
    if first
        .strip_prefix('@')
        .is_some_and(|handle| !handle.is_empty())
    {
        return Some((*first).to_string());
    }

    let host = host(url)?;
    let prefixes = PLATFORMS
        .iter()
        .find(|(platform, _)| *platform == host)
        .map(|(_, prefixes)| *prefixes)?;
    if prefixes.is_empty() {
        return Some((*first).to_string());
    }
    if prefixes.contains(first) {
        return Some(segments.get(..2)?.join("/"));
    }
    None
}

/// The canonical name of whoever published `url`: the host, plus the account path when the host
/// is shared between publishers. This is what the reader shows and what a source's slug is
/// derived from, so the same publisher reads the same way everywhere.
pub fn canonical_name(url: &Url) -> Option<String> {
    let authority = authority(url)?;
    Some(match account_path(url) {
        Some(account) => format!("{authority}/{account}"),
        None => authority,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical name of a link, or an empty string when it is not a usable http(s) URL.
    fn name(value: &str) -> String {
        Url::parse(value)
            .ok()
            .as_ref()
            .and_then(canonical_name)
            .unwrap_or_default()
    }

    #[test]
    fn an_ordinary_site_is_named_by_its_domain_alone() {
        for (url, expected) in [
            ("https://www.DuckDB.org./news/article", "duckdb.org"),
            ("https://example.com/blog/feed.xml", "example.com"),
            (
                "https://news.example.co.uk/2026/09/post",
                "news.example.co.uk",
            ),
            // A project page's owner is already in the subdomain.
            ("https://alice.github.io/posts/one", "alice.github.io"),
            ("https://alice.substack.com/p/hello", "alice.substack.com"),
            ("https://example.com/", "example.com"),
        ] {
            assert_eq!(name(url), expected, "{url}");
        }
    }

    #[test]
    fn a_platform_keeps_the_path_that_names_the_account() {
        for (url, expected) in [
            (
                "https://www.youtube.com/@SomeChannel",
                "youtube.com/@SomeChannel",
            ),
            (
                "https://www.youtube.com/@SomeChannel/videos",
                "youtube.com/@SomeChannel",
            ),
            (
                "https://youtube.com/channel/UC123/videos",
                "youtube.com/channel/UC123",
            ),
            ("https://youtube.com/user/alice", "youtube.com/user/alice"),
            ("https://youtube.com/c/alice", "youtube.com/c/alice"),
            ("https://x.com/alice/status/1", "x.com/alice"),
            ("https://twitter.com/alice", "x.com/alice"),
            (
                "https://github.com/rust-lang/rust/releases",
                "github.com/rust-lang",
            ),
            (
                "https://www.reddit.com/r/rust/comments/1",
                "reddit.com/r/rust",
            ),
            (
                "https://bsky.app/profile/alice.bsky.social",
                "bsky.app/profile/alice.bsky.social",
            ),
            ("https://mastodon.social/@alice/1", "mastodon.social/@alice"),
        ] {
            assert_eq!(name(url), expected, "{url}");
        }
    }

    #[test]
    fn a_platform_path_that_names_no_account_falls_back_to_the_host() {
        for (url, expected) in [
            // A watch page is a video, not a channel.
            ("https://www.youtube.com/watch?v=123", "youtube.com"),
            (
                "https://www.youtube.com/feeds/videos.xml?channel_id=UC123",
                "youtube.com",
            ),
            ("https://youtu.be/123", "youtube.com"),
            ("https://www.youtube.com/", "youtube.com"),
            ("https://www.reddit.com/about", "reddit.com"),
        ] {
            assert_eq!(name(url), expected, "{url}");
        }
    }

    #[test]
    fn ports_survive_and_non_http_urls_have_no_name() {
        assert_eq!(name("http://localhost:8080/feed"), "localhost:8080");
        assert_eq!(
            name("https://example.test:8443/@alice/rss"),
            "example.test:8443/@alice"
        );
        // A default port is not part of the name.
        assert_eq!(name("https://example.test:443/feed"), "example.test");
        assert_eq!(name("javascript:alert(1)"), "");
        assert_eq!(name("not a url"), "");
    }

    #[test]
    fn an_empty_handle_is_not_an_account() {
        assert_eq!(name("https://example.com/@/posts"), "example.com");
    }
}
