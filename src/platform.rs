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

/// Front-matter extra holding the profile URL a page named for its own publisher, captured when
/// the article's URL does not name one itself.
pub const PUBLISHER_KEY: &str = "publisher_url";

/// A host shared between publishers, and how to find the account on it.
struct Platform {
    host: &'static str,
    /// Path segments that introduce an account; empty means the first segment is the account.
    prefixes: &'static [&'static str],
    /// Whether the account path is an opaque identifier. Those read better as the publisher's own
    /// name, which [`opaque`] lets the display layer substitute.
    opaque: bool,
    /// First segments that name one entry in the catalogue rather than whoever publishes it. An
    /// episode belongs to a show; it is not an account of its own.
    entries: &'static [&'static str],
}

const fn platform(host: &'static str, prefixes: &'static [&'static str]) -> Platform {
    Platform {
        host,
        prefixes,
        opaque: false,
        entries: &[],
    }
}

/// A directory whose paths are catalogue identifiers rather than names anyone would read.
const fn catalogue(host: &'static str) -> Platform {
    Platform {
        host,
        prefixes: &[],
        opaque: true,
        entries: &["episode", "episodes"],
    }
}

const PLATFORMS: &[Platform] = &[
    platform("bsky.app", &["profile"]),
    platform("codeberg.org", &[]),
    platform("github.com", &[]),
    platform("gitlab.com", &[]),
    platform("medium.com", &[]),
    platform("reddit.com", &["r", "user"]),
    platform("soundcloud.com", &[]),
    platform("twitch.tv", &[]),
    platform("vimeo.com", &[]),
    platform("x.com", &[]),
    platform("youtube.com", &["channel", "user", "c"]),
    catalogue("castbox.fm"),
    catalogue("overcast.fm"),
    catalogue("pocketcasts.com"),
    catalogue("podcasts.apple.com"),
    catalogue("spotify.com"),
];

fn platform_of(host: &str) -> Option<&'static Platform> {
    PLATFORMS.iter().find(|platform| platform.host == host)
}

/// The host as aggr names it: no trailing dot, no `www.`, and the platform's own domain rather
/// than one of its short, mobile or regional aliases.
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
        "open.spotify.com" => "spotify.com",
        "pca.st" | "play.pocketcasts.com" => "pocketcasts.com",
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

    let platform = platform_of(host(url)?)?;
    // A catalogue path is one opaque identifier however many segments it takes, and the locale
    // or section in front of it is part of reaching that entry.
    if platform.opaque {
        // A link to one episode names the episode, not a publisher: the show it belongs to is
        // elsewhere, and the host alone is the most that link can honestly claim.
        if platform.entries.contains(first) {
            return None;
        }
        return Some(segments.join("/"));
    }
    if platform.prefixes.is_empty() {
        return Some((*first).to_string());
    }
    if platform.prefixes.contains(first) {
        return Some(segments.get(..2)?.join("/"));
    }
    None
}

/// Whether this host names its publishers with identifiers rather than readable names, so a
/// display label does better to show the publisher's own title.
pub fn opaque(url: &Url) -> bool {
    host(url)
        .and_then(platform_of)
        .is_some_and(|platform| platform.opaque)
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

/// The channel a YouTube feed endpoint describes. Feed URLs state the transport; this is the
/// publisher behind it.
pub fn youtube_channel(url: &Url) -> Option<String> {
    if host(url)? != "youtube.com" || url.path().trim_end_matches('/') != "/feeds/videos.xml" {
        return None;
    }
    url.query_pairs()
        .find_map(|(key, value)| match key.as_ref() {
            "channel_id" => Some(format!("/channel/{value}")),
            "user" => Some(format!("/user/{value}")),
            _ => None,
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
    fn a_catalogue_keeps_its_whole_identifier_under_the_provider_domain() {
        for (url, expected) in [
            (
                "https://open.spotify.com/show/1sz1nhohqbpxbznlponfoz",
                "spotify.com/show/1sz1nhohqbpxbznlponfoz",
            ),
            (
                "https://podcasts.apple.com/us/podcast/some-show/id1234567",
                "podcasts.apple.com/us/podcast/some-show/id1234567",
            ),
            ("https://pca.st/abc123", "pocketcasts.com/abc123"),
            (
                "https://play.pocketcasts.com/podcasts/xyz",
                "pocketcasts.com/podcasts/xyz",
            ),
            (
                "https://overcast.fm/itunes123/a-show",
                "overcast.fm/itunes123/a-show",
            ),
            ("https://castbox.fm/channel/id42", "castbox.fm/channel/id42"),
        ] {
            assert_eq!(name(url), expected, "{url}");
        }
    }

    #[test]
    fn only_catalogue_hosts_ask_for_a_readable_name_instead_of_their_path() {
        let opaque = |value: &str| super::opaque(&Url::parse(value).unwrap());
        assert!(opaque("https://open.spotify.com/show/abc"));
        assert!(opaque("https://podcasts.apple.com/us/podcast/x/id1"));
        // A channel path already reads as a name, and an ordinary site has none to replace.
        assert!(!opaque("https://www.youtube.com/@SomeChannel"));
        assert!(!opaque("https://example.com/blog"));
    }

    #[test]
    fn a_youtube_feed_endpoint_resolves_to_the_channel_behind_it() {
        let channel = |value: &str| youtube_channel(&Url::parse(value).unwrap());
        assert_eq!(
            channel("https://www.youtube.com/feeds/videos.xml?channel_id=UC123").as_deref(),
            Some("/channel/UC123")
        );
        assert_eq!(
            channel("https://www.youtube.com/feeds/videos.xml?user=alice").as_deref(),
            Some("/user/alice")
        );
        assert_eq!(channel("https://www.youtube.com/@SomeChannel"), None);
        assert_eq!(
            channel("https://example.com/feeds/videos.xml?channel_id=UC1"),
            None
        );
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
