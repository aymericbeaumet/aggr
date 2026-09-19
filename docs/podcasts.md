# Podcast sources

Use a public show URL in an ordinary source. Direct RSS URLs work too.

```toml
[[sources]]
url = "https://podcasts.apple.com/us/podcast/lex-fridman-podcast/id1434243584"
category = "podcasts"
```

These ten services cover major listening platforms and widely used podcast hosts. They are
not a universal popularity ranking: listening platforms and hosting providers measure different
things. YouTube, Spotify and Apple lead the US listening-platform figures in
[Edison's 2025 research](https://www.edisonresearch.com/wp-content/uploads/2025/12/Edison-Researchs-Top-10-Findings-of-2025-deck-1.pdf).

| Service | Accepted public URL and resolution |
| --- | --- |
| Apple Podcasts | `podcasts.apple.com/<country>/podcast/<name>/id<ID>`; Apple's catalog lookup returns the matching show's publisher RSS feed. If the requested storefront omits the show, the default catalog is checked with the same ID. Episode query parameters still subscribe to the show. |
| Spotify | `open.spotify.com/show/<ID>` (including localized `intl-…` URLs); advertised RSS takes priority, then the public show identity is matched to a verified publisher feed through Apple's catalog. Public page episode metadata remains available when no verified feed can be found. |
| YouTube | `youtube.com/channel/<ID>` and `youtube.com/playlist?list=<ID>` resolve to channel/playlist feeds. Handles use ordinary advertised-feed discovery. Shorts remain excluded. |
| SoundCloud | `soundcloud.com/<creator>`; the page's public app-link user ID identifies its podcast RSS feed. Only tracks the creator includes in RSS appear. |
| Podbean | `<show>.podbean.com`; resolves to `feed.podbean.com/<show>/feed.xml`. |
| Buzzsprout | `buzzsprout.com/<show-ID>`; resolves to `feeds.buzzsprout.com/<show-ID>.rss`. Custom show domains use advertised-feed discovery. |
| Spreaker | `spreaker.com/show/<ID>` or `spreaker.com/podcast/<name>--<ID>`; resolves to the show's episode RSS feed. |
| Acast | `shows.acast.com/<show>`; resolves to `feeds.acast.com/public/shows/<show>`. |
| Libsyn | `<show>.libsyn.com`; resolves to the show's `/rss` endpoint. |
| Simplecast | `<show>.simplecast.com` and custom show domains; advertised opaque `feeds.simplecast.com/<ID>` links and embedded RSS metadata identify the feed. |

Discovery follows publisher-advertised RSS URLs, including embedded JSON fields such as
`feedUrl`, `rssFeedUrl` and `rss_feed_url`. A discovered endpoint is remembered with HTTP
validators, so subsequent syncs fetch the feed directly and can use conditional GET. Failed
remembered endpoints trigger rediscovery. Configured request headers retain their existing
origin scope. Apple show URLs
use Apple's own [catalog lookup](https://developer.apple.com/library/archive/documentation/AudioVideo/Conceptual/iTuneSearchAPI/LookupExamples.html).

For Spotify shows without an advertised feed, aggr submits only the public show title and
publisher to [Apple's podcast search](https://developer.apple.com/library/archive/documentation/AudioVideo/Conceptual/iTuneSearchAPI/Searching.html).
It never sends source headers or cookies to that catalog. A result must uniquely match both
show and publisher after normalization, and its RSS must contain at least two distinct
episodes matching Spotify's titles and publication dates. Ambiguous results, mismatches and
lookup failures keep the public Spotify episode listing. Once verified, the publisher RSS
endpoint is remembered; later syncs request it directly with conditional GET.

An RSS source exposes the episodes its publisher currently includes. Spotify's public page
exposes a recent subset, not a complete historical catalog; repeated syncs preserve episodes as
they appear. Missing, empty or changed Spotify metadata produces a source error instead of a
successful empty import. Spotify account-only content and private catalogs require the
publisher's accessible RSS feed. Spotify-hosted shows must
[enable RSS distribution](https://support.spotify.com/us/creators/article/finding-and-enabling-your-rss-feed/)
before that feed is available outside Spotify. Apple can also omit RSS for shows whose
publishers disable [catalog distribution](https://podcasters.apple.com/support/897-submit-a-show).

The archive retains episode text, artwork and public links. RSS audio enclosure URLs are kept
as `extra.audio_url`, with the enclosure's declared media type as `extra.audio_type` and its
byte size, when the feed states a positive one, as `extra.audio_length`; the site's Atom, RSS,
and JSON Feed outputs republish the three as enclosure metadata. An enclosure is also the item
link when the feed supplies no episode page. Audio/video files are not downloaded by these adapters. Offline article storage therefore
does not promise offline podcast playback or private media access.

For example, the Underscore_ Spotify show resolves to
[Micode's Acast feed](https://feeds.acast.com/public/shows/43160adc-fadd-4764-b93f-aa1ca93f22bf).
Lenny's Apple Podcasts show resolves to
[its Substack feed](https://api.substack.com/feed/podcast/10845.rss).
Both publisher feeds provide full episode audio enclosures. Direct RSS URLs also avoid a
dependency on catalog discovery when configuring another reader.

When a Spotify subscription switches from its public episode listing to verified publisher RSS,
existing entries are matched within that source by normalized title and UTC publication date.
Unique matches gain missing audio enclosures and canonical deduplication aliases while retaining
their article URLs, body, HTML, artwork, and other metadata. Ambiguous matches are not merged;
repeating a completed reconciliation leaves the archive unchanged.

## Optional aggregation services

[RSSHub](https://github.com/DIYgod/RSSHub) is an open-source service that can expose additional
sites as RSS. A self-hosted route can be configured as an ordinary feed URL; aggr does not
automatically route subscriptions through a public instance.

RSSHub's current [Spotify route](https://github.com/DIYgod/RSSHub/blob/master/lib/routes/spotify/show.ts)
requires a Spotify client ID and secret and uses `audio_preview_url` for enclosures. Spotify
documents that field as a deprecated, nullable
[30-second preview](https://developer.spotify.com/documentation/web-api/reference/get-a-show).
It does not recover full publisher audio. Its
[Apple route](https://github.com/DIYgod/RSSHub/blob/master/lib/routes/apple/podcast.ts) generates
a feed from a limited episode listing. Direct publisher RSS is preferable for these two
integrations; an aggregation service is useful when another source has no accessible feed.

Provider references: [Spreaker RSS](https://developers.spreaker.com/api/show-rss-metadata/),
[Acast RSS](https://learn.acast.com/en/articles/3386270-what-is-my-acast-rss-feed-url),
[Podbean RSS](https://help.podbean.com/support/solutions/articles/25000005057-what-is-my-podbean-feed-url),
[Buzzsprout RSS](https://www.buzzsprout.com/help/103-rss-feed),
[Libsyn RSS](https://help.libsynsupport.com/hc/en-us/articles/360041220931-Using-SSL-with-Your-RSS-Feeds),
[Simplecast RSS](https://help.simplecast.com/hc/en-us/articles/21953701477917-How-do-I-find-my-Simplecast-RSS-Feed-URL),
and [SoundCloud podcast support](https://help.soundcloud.com/hc/en-us/sections/45260205368347-Podcasts-on-SoundCloud).
