# Configuring sources

Only write values you want to change. The commented
[`config.default.toml`](../config.default.toml) is the complete configuration reference and the
single source of truth for every default.

## Source tables

An ordinary website URL is usually enough. Each `[[sources]]` table has a `url` field and can
appear between any other TOML sections:

```toml
[site]
title = "My reads"

[[sources]]
url = "https://blog.rust-lang.org"
name = "Rust Blog"
labels = ["rust", "language"]

[[sources]]
category = "programming"
url = """
https://example.com
https://example.org/blog
"""

[fetch]
content = "light"

[[sources]]
category = "science"
url = ["https://example.net", "https://example.edu/news"]
```

`url` accepts a string, a multiline string, or an array of strings, including multiline
strings within an array. All forms share one normalization path: split lines, remove a trailing
comment beginning with exactly ` #`, trim whitespace, discard empty entries, then parse and expand
sources. `https://example.com/feed#section` retains its hash; `https://example.com/feed # note`
removes the comment. Use another `[[sources]]` table to start a group with different options.

The table's options apply to every source it expands. Collections inherit these defaults unless
their entries set explicit options. Complete source tables can be mixed with other sections
anywhere in the file; their fields must stay inside their own table.

All forms reduce to the same source model before validation and discovery, using the same
global defaults, `${ENV}` expansion, collection expansion, and deduplication. Sources are expanded
in declaration order, with the first declaration of an equivalent endpoint winning.

## Source names in URLs

Each source gets a directory under `sources/`, named after its `slug`. Set `slug` to choose it;
otherwise it comes from the source's `name`, and failing that from its canonical name: the
publisher's domain, since everything under one domain is normally the same publisher.

```toml
[[sources]]
url = "https://www.example.com/blog/feed.xml"   # -> sources/example-com/
```

Some hosts carry thousands of unrelated publishers, so there the account's path is part of who is
publishing and stays in the name. That list lives in `src/platform.rs` and covers YouTube, X,
GitHub, GitLab, Codeberg, Reddit, Medium, Bluesky, Twitch, Vimeo and SoundCloud, plus the `@handle`
convention on any host.

```toml
[[sources]]
url = "https://www.youtube.com/@SomeChannel"    # -> sources/youtube-com-somechannel/
```

Two feeds from one publisher arrive at the same name, so the path that differs is added to tell
them apart (`example-com` and `example-com-notes`). Set `slug` yourself when you want a specific
name; the same slug written twice is an error rather than something aggr renames for you.

## How a URL is resolved

aggr tries the URL as a feed, follows RSS/Atom/JSON Feed discovery metadata, probes the
conventional endpoints under that section (`feed.xml`, `rss.xml`, `atom.xml`, `index.xml`, `feed`,
`rss`), and finally falls back to conservative article discovery on the page itself. A section URL
is treated as a directory, so `https://example.com/blog` and `https://example.com/blog/` probe the
same endpoints. A discovered feed that publishes no entries is skipped in favour of the listing.
The endpoint that answered is remembered, so the probe happens once per source.

In the default heavy content mode aggr downloads each original page and extracts its main article;
use `content = "light"` on a source to trust its feed content instead. The default first import
considers the newest 100 entries from each feed, then every later run adds newly observed entries.

Articles are deduplicated across sources by normalized original URL, so following both Hacker
News and a publisher keeps one copy. Existing archived duplicates appear once in the reader;
their previous item URLs redirect to the selected copy.

Ordinary remote feed/page URLs are resolved without config-time downloads. Remote `.toml`, `.opml`,
and `.txt` paths expand automatically. For an opaque collection endpoint, add `collection = true`
to its `[[sources]]` table. This flag is not inherited by the contained sources.

## Keeping an archive current with aggr itself

Two different things can be out of date. Items the source still lists are re-fetched by
`sync --refresh`, which rewrites them from the live page. Items the source has stopped listing can
still be improved from what aggr already retained: `sync --reprocess` re-derives every stored body
from its retained HTML companion, so content cleanup added in a later release reaches the archive
without a single extra request. Both discard hand edits, and both write ordinary commits.

`--reprocess` recovers anything still present in the retained HTML. It cannot recover content that
article extraction discarded before storing — that needs the original page, and so needs the item
to be listed again.

## Podcasts and video

Public show URLs work for Apple Podcasts, Spotify, Deezer, YouTube, SoundCloud, Podbean,
Buzzsprout, Spreaker, Acast, Libsyn, and Simplecast. They resolve to publisher RSS where available.
Spotify and Deezer can discover a full publisher feed through Apple's public catalog, verifying
show identity and matching episodes before using it; their public episode listings remain a
fallback. Podcast enclosures play inside articles. See [podcast sources](podcasts.md) for accepted
URLs and provider limits. Episode text and artwork are archived; audio/video playback is not
available offline through these adapters.

YouTube videos use their feed descriptions instead of article extraction. YouTube Shorts links
are always excluded, including from other feeds and mirrored archives, with no setting to enable
them. Previously archived Shorts are omitted when the site is rebuilt.

In heavy mode, YouTube articles also include a timed transcript when the public video page
advertises accessible captions. Timestamp links open the video at that point. Captions are grouped
into short readable paragraphs and fetched directly from YouTube, with no third-party transcript
service or credentials. Videos without accessible captions retain their description and preview;
YouTube may withhold or rate-limit captions. Light mode does not request video pages or transcripts.

## What extraction keeps

Extraction retains article images, figure captions, inline links, footnotes, tables, and code
indentation, with syntax colors for recognized code languages. A code block whose language cannot
be determined is left unlabelled rather than guessed at. Cleanup that only needs the stored
Markdown is reapplied on every build, so an archive written by an older release reads correctly;
cleanup that needs the original HTML reaches an existing item through `--reprocess`. Custom source
headers stay on the configured origin; redirected or linked article and image requests on other
origins do not receive those headers.

In heavy mode, a public ActivityPub/Mastodon status is expanded into its public same-author
self-reply thread when the page advertises ActivityStreams data. Parent and reply traversal stays
on the status origin, is tightly bounded, and falls back to ordinary article extraction if the
server withholds or rejects any required data. X/Twitter status links use publicly available
xcancel pages to collect the author's replies, with original links pointing to `x.com`. Posts and
their media keep their original order and read as one article: no separators, per-post links, or
trailing thread counters such as `(1/3)`. The original link in the article metadata reaches the
thread; unavailable continuations are logged. See [public threads](interoperability.md#public-threads).

## Article images

Article-image archiving is on by default. The explicit form, including a per-source opt-out, is:

```toml
[fetch]
images = true

[[sources]]
url = "https://blog.rust-lang.org"

[[sources]]
url = "https://example.com/feed.xml"
images = false # leave this source's body images at their publisher URLs
```

For every accepted JPEG, PNG, GIF, or WebP image, aggr keeps the exact publisher response as the
master. When the bounded media budget permits it, safe static 8-bit images also get responsive
320, 640, 960, 1280, and 1600-pixel renditions where useful, plus a full-width rendition. These
are resized once with a high-quality filter, encoded as lossless WebP, then decoded and
pixel-compared before aggr offers them to the browser. The exact master always remains the
fallback, so choosing a smaller rendition introduces no lossy compression and a high-density
display is never forced to upscale it. A rendition is retained only when it is smaller than the
master. Masters above 32 megapixels skip the full-width encode and pair bounded responsive copies
with the original as the largest candidate; an existing WebP master is not duplicated.

Animated GIF/WebP, images with an ICC color profile, and high-bit-depth images keep only their
exact master. SVG diagrams become passive local PNGs with a bundled font; scripts, external images,
and embedded resources are ignored, and original SVG markup is never published. Other unsupported or
invalid media also keeps its safe publisher URL. Image work is failure isolated: a timeout,
decode error, size rejection, or failed download never prevents the article from being saved.

Every local image gets an inline ThumbHash preview at build time, visible before scripts or image
requests finish. Ordinary sync/build/dev runs repair missing images in retained articles, reuse
existing masters, and retry failed downloads after an hour.

Acquisition is intentionally bounded per article: at most 1,024 candidates are inspected and 512
images retained; one response or rendition may use at most 32 MiB; downloaded and retained media
each have a 256 MiB cumulative budget; and decoded images may not exceed 200 megapixels or 24,000
pixels on either axis. SVG parsing accepts up to 2 MiB and 20,000 XML nodes, rasterized to at most
1,600 pixels per axis. Decoding runs one article image at a time with a 15-second per-image
limit. These media limits are conservative implementation safeguards rather than configuration
knobs.

Rendered pages use intrinsic dimensions and an inline ThumbHash preview to avoid layout jumps.
The first body image loads eagerly at high priority; later images use native lazy loading and
asynchronous decoding. Local files are content-addressed and immutable. In the PWA, media for the
selected recent articles is downloaded with every retained image rendition into a dedicated cache;
other local media enters a bounded cache after a successful view. Preferences reports an article
ready only when its full retained resource set is saved. A publisher-hosted fallback is never
guaranteed offline.

Enabling image archiving also fills missing media in retained captures. Exact raster masters and
responsive renditions increase the append-only data branch and Git history, and normal retention
cannot reclaim historical objects. Before publishing an archive, make sure storing and
redistributing a source's images is compatible with its terms and your local law. Use
`images = false` for sources whose media you should not retain.

## Previews

Small local previews are enabled by default. They can be disabled globally or per source:

```toml
[fetch]
previews = true

[[sources]]
url = "https://blog.rust-lang.org"

[[sources]]
url = "https://example.com/feed.xml"
previews = false # override the default for one source
```

aggr downloads and resizes a suitable image to at most 256 pixels per side, encodes it as lossless
WebP (up to 384 KiB), and records its intrinsic dimensions, alt text, and dominant color. The color
reserves a calm placeholder while the thumbnail loads. A missing or unusable preview never prevents
saving the article. Previews appear in feed and search rows and article pages. If the publisher
supplies no usable preview, aggr tries video posters and article images; a direct PDF can supply
its first page. PDF rasterization uses a bounded input and output size with two concurrent
decoders; rendering itself has no hard CPU or allocation limit. Existing retained images also
supply missing previews during builds without modifying the archive. Videos without published
artwork or a poster do not yet provide frame thumbnails. Enabling remote previews applies to new
items; run with `--refresh` to fill missing previews on existing items without replacing stored
previews.

## Collections

The same `url` field also accepts local paths and remote URLs for aggr TOML, OPML subscription
lists, newline-separated URL lists, RSS, Atom, and JSON Feed. aggr detects the format from the
resource's content. A feed document registers one source; a collection expands into its listed
sources at that position. The table's options become defaults for those sources:

```toml
# aggr.toml
[[sources]]
url = ["./aggr-ai.toml", "./subscriptions.opml", "./feeds.txt"]
category = "ai"
```

```toml
# aggr-ai.toml
[[sources]]
url = """
https://www.anthropic.com/news
https://openai.com/news
"""

[[sources]]
url = "https://example.com/research.xml"
category = "research" # an explicit category wins
```

Local paths resolve relative to the file that names them. Local globs and direct HTTP(S) URLs
are supported. Link to the repository's `aggr.toml` file explicitly; GitHub-hosted configs can use
relative wildcards just like local ones. Collections and feeds can share a table:

```toml
[[sources]]
url = """
https://github.com/aymericbeaumet/aggr-instance/blob/main/aggr.toml
https://example.com/subscriptions.opml
https://example.org/feed.xml
"""
category = "community"

[[sources]]
url = "https://github.com/owner/reader/blob/main/topics/*.toml"
```

Expansion is deterministic and cycle-safe: resources and equivalent source endpoints are loaded
once, in declaration order, with the first declaration winning. Missing, malformed, and unsupported
collections warn and are skipped. By default, a remote collection may expand only relative paths
on the same HTTP origin—and, for GitHub URLs, within the same repository. Its feed and website
entries may use other origins. The trusted root can opt into broader collection chains with
`[fetch] allow_remote_source_chains = true`. GitHub requests automatically use `GITHUB_TOKEN` or
`GH_TOKEN` when present; a private repository must grant that token read access.

Remote collections are live configuration dependencies, not part of the append-only data branch.
Pin their revision, or vendor them into the primary branch, if rebuilding the same site later
matters. Custom domains, network links, headers/secrets, PWA controls, retention, themes, and all
other options are documented directly in [`config.default.toml`](../config.default.toml).

## Copying another aggr instance

An aggr instance can be a source for another one. The copied items keep their ultimate original
URLs and gain provenance pointing to the instance they came through. If both instances already
follow the same article, URL deduplication keeps one local copy.

Use the repository URL; HTTPS, SSH, and `git@host:path` clone addresses are recognized:

```toml
[[sources]]
url = "https://github.com/friend/reads"
category = "friends"
```

For example, an SSH clone URL can name the same source:

```toml
[[sources]]
url = "git@github.com:friend/reads.git"
branch = "aggr"
category = "friends"
```

Equivalent GitHub URL spellings share one source identity. For a custom Git host, use an explicit
`.git` or SSH URL. Sources use `url`; explicit `type` and `repo` fields are not supported.
To import subscriptions instead of stored articles, name the repository's `aggr.toml` file
explicitly (for example, `https://github.com/friend/reads/blob/main/aggr.toml`). Bare repository
URLs import data.

An aggr source copies every visible item retained in the other instance's current data tree by
default. Set `limit` only when you deliberately want the newest N items. Items that exist only in
older commits are not copied, so this is a useful content replica rather than a clone of the other
repository's complete history. When previews or article images are enabled for the mirror source,
existing local companions are copied with the article without contacting the publisher. Disabled
media stays omitted.
