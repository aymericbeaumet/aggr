# Interoperability

An aggr build is ordinary static files. Human-facing links and assets are relative, so the same
tree works at `/`, `/reader/`, or a deeper mount point. A release build also receives its public
URL; aggr uses that only where a protocol requires an absolute identity.

The data repository and generated site do not depend on the same provider. Standard Git stores and
replicates the append-only branch, while any static host can serve the rendered tree. GitHub adds a
turnkey workflow, Pages deployment, config discovery shortcuts, and commit-pinned web links around
that portable core.

## Source definitions and collections

`[[sources]].url` is the shared configuration boundary for local or remote websites, feeds,
aggr TOML, OPML subscriptions, and newline-separated URL lists. Ordinary remote feed/page URLs
perform no config-time requests; discovery belongs to the concurrent fetch pipeline. Local files,
remote `.toml`/`.opml`/`.txt` paths, and recognized GitHub config paths expand as collections.
An opaque collection URL requires `collection = true`; this flag applies only to the named resource,
not its flattened sources. Collection bytes determine their format. Strings and
arrays of strings share line splitting, trailing-comment removal, whitespace trimming, and
empty-line removal. A comment starts at the exact delimiter ` #` (an ASCII space followed by a
hash); `https://example.org/feed#section` preserves its fragment. This also applies to imported
newline-separated URL lists. Per-table options such as category, labels, headers, and content
mode are copied to every resulting source before shared resolution and deduplication.

Repository URLs infer an aggr data import: `https://github.com/owner/reader`,
`git@github.com:owner/reader.git`, and `ssh://git@github.com/owner/reader.git` identify the same
GitHub source. The first configured transport is retained for cloning, including SSH credentials.
Bare two-segment repository paths on GitLab, Bitbucket, and Codeberg are also recognized; other
hosts can use an explicit `.git` URL or SSH URL. Repository options `branch`, `sources`, and `limit`
apply to those URLs. Explicit `type` and `repo` fields are not supported.

To import a repository's subscriptions instead of its retained articles, link its config file:
`https://github.com/owner/reader/blob/main/aggr.toml` (or `/owner/reader/aggr.toml` to follow the
default branch). Existing bare GitHub collection URLs must add that config path; bare repository
URLs now import retained data and do not download the upstream root configuration.

Expansion preserves declaration order and reduces every form to the same `Source` model for
defaults, environment expansion, validation, deduplication, and discovery. Collection entries
resolve local paths relative to their file and inherit source options unless they override them.
Repeated resources and cycles are skipped. Remote collection chaining is controlled by the root
configuration's `fetch.allow_remote_source_chains`; nested files cannot grant themselves broader
access. Ordinary feed and website entries may still point to other origins. Inherited headers stay on
the collection's origin or repository; redirects cannot transfer collection credentials to children
on another origin.

Offline cleanup uses the same document parser to protect local dependencies through nested
collections, including environment-expanded paths, without requesting remote resources. New
collection formats must preserve this dependency tracking as well as normal loading.

Pinned archive builds use offline configuration loading. Dev starts from local configuration
and cached data, resolves declared remote collections in
the background, and reuses the resolved sources when only templates or styles change.

Instagram profile URLs are not reliable public feeds. The public page may expose a profile name
and post count while withholding all post data. aggr imports only post entries actually exposed in
the returned page; otherwise it reports a specific metadata-only, private-account, login, or
verification error instead of attempting generic feed endpoints. Configured headers are honored,
but aggr does not log in, complete challenges, or query third-party Instagram bridges. When a
profile cannot be read publicly, use the creator's website or feed instead. Meta's authorized
Instagram APIs are a separate integration and are not enabled by adding an ordinary profile URL.

## Source adapters

Every engine under `src/sources/` claims URLs with a predicate, and `sources/mod.rs` tries them
in this order before falling back to the generic feed path. A URL that no adapter claims is an
ordinary feed or website; nothing here is public configuration.

| Adapter | Accepted URLs | Details |
|---|---|---|
| `instagram.rs` | `instagram.com`, `www.instagram.com`, or `m.instagram.com` with a profile name as the path | the paragraph above |
| `qwen.rs` | exactly `https://qwen.ai/blog` or `https://qwen.ai/research`; a trailing slash is accepted, any query string is not | below |
| `podcast.rs` | Apple Podcasts show URLs, `open.spotify.com/show/<ID>`, Deezer show URLs, and the hosting providers whose show URL maps to a known feed endpoint | [podcast sources](podcasts.md) |
| `feed.rs`, `html.rs` | every other `http(s)` URL: as a feed, then advertised and conventional endpoints, then the page's article list | [configuring sources](sources.md) |
| `aggr.rs` | repository URLs, selected at configuration time (`type = "aggr"` engine) | [source definitions](#source-definitions-and-collections), [the git model](git-model.md#copying-another-instance) |
| `youtube.rs` | not a dispatcher: it drops Shorts from every source's items, recognizes video URLs for inline players and posters, and reads public durations | [themes](themes.md#video-and-lead-media) |

### Qwen

Qwen's blog is rendered by JavaScript, so the adapter reads the site's public article endpoint
(`/api/v2/article/retrieval?type=qwen_ai&language=en-US`) with aggr's ordinary client, headers,
and conditional GET. The endpoint answers with a new request ID every time, so change detection
hashes the article list alone: an unchanged list is an unchanged source. Each article becomes one
item with its title, `https://qwen.ai/blog?id=<path>` as the link, its RFC 3339 date, author,
tags as labels, the small cover image as the preview candidate, and the article HTML (the first
of `.post-content`, `article`, `main`, or `body`) as content; the source is titled `Qwen` with
`https://qwen.ai/` as its site URL. An article without a path, title, or content is a source
error rather than an empty item. Because the API already carries the whole article, heavy mode
never requests the JavaScript shell for it, and the daily retry that upgrades feed-only captures
skips the source, as it does for light-mode and mirrored sources.

## Public threads

Heavy article extraction joins public posts by the original author. Mastodon/ActivityPub follows
same-origin ActivityStreams parents and replies. X/Twitter status URLs use public xcancel HTML;
stored attribution and post links use `x.com`, and images use their original media URLs. X source
credentials are never forwarded to xcancel. No login, browser cookies, or publisher scripts are used.

X traversal follows author continuations and cursor pages, deduplicates status IDs, and orders posts
chronologically while preserving each post's media sequence. It is bounded to 32 requests, 128 posts,
and 30 seconds; an unavailable continuation or exhausted budget is logged as a warning and the
retrieved posts are kept. Failure to retrieve the initial post leaves ordinary article extraction
available. Existing entries receive new thread extraction on explicit refresh; saved content is not
silently replaced.

A thread reads as one uninterrupted article: posts are concatenated without separators, per-post
links, or partial-thread notices, because the metadata original link already reaches the thread.
Trailing parenthesized or bracketed counters such as `(1/3)` and standalone `2/3` paragraphs are
removed per post before concatenation. Fractions inside prose, code, links, or nonterminal text stay
intact. Builds apply the same presentation cleanup to archived social Markdown, including earlier
captures that stored separators or generated links, across reader pages, search, feeds, and exports
without rewriting stored Markdown or HTML. Media remains at its post position; a video keeps its
poster image only; playback is not retained.

## Article images

Every raster format that decodes in pure Rust is archived and resized: JPEG, PNG, GIF, WebP, BMP,
ICO, TIFF and QOI, plus publisher SVG rasterized to a passive PNG master. AVIF needs an AV1
decoder, which is a system library rather than a crate, so it is archived undecoded: the response
is kept byte for byte as the master and served as-is, its size is read from the container's `ispe`
box so the space is reserved before it loads, and it carries a flat stand-in preview instead of a
ThumbHash. It gains no WebP renditions. A CDN that answers `auto=format` with AVIF regardless of
`Accept` — Linear's, for one — is archived this way rather than skipped.

## HTTP extraction and native builds

HTTP requests use reqwest with rustls first. A response explicitly marked
`cf-mitigated: challenge` permits one retry using a browser-compatible TLS and HTTP/2 profile.
The selected profile survives that request's redirects and transient retries on the same origin.
Successful choices are remembered in a bounded, in-memory origin cache for the run.
The retry preserves aggr's user agent and configured request headers, certificate verification,
redirect origin boundaries, host pacing, total timeout, and decompressed body limit. It shares the
ordinary response and article cache; challenge pages are not successful article captures.

This is an HTTP transport fallback implemented in Rust, with BoringSSL linked into the binary.
It does not run Python, a browser, JavaScript challenges, or an external proxy service, and does
not import browser cookies or obtain challenge cookies. Sites requiring interactive verification
can still refuse the request. Failed article extraction retains the available RSS/feed content,
as does a page whose server HTML is only a script's loading message (`loading…`, `reconnecting…`,
"enable JavaScript") outside its header, navigation and footer: aggr never runs the scripts that
would fill it, so the placeholder is not an article.

Readability receives explicit image-and-caption figure structure as article content before its
boilerplate filters run. This preserves charts whose IDs happen to contain words such as `replies`;
the enclosing semantic article receives the same signal so tied figure scores cannot drop a
neighboring figure. Ordinary comment containers and sidebars still follow the normal filtering rules. Extraction cache
versions change with these rules while raw HTTP responses remain reusable. A figure already absent
from both stored HTML and Markdown needs a fresh extraction; rendering cannot reconstruct it.

Charts that a page draws in the browser (Vega-Lite specifications streamed in a Next.js payload,
as on openai.com) leave an empty placeholder in the server HTML, and aggr never executes page
scripts. When the specification carries its data inline, the placeholder receives that data as a
table with the chart title and its caption, so the figures stay readable and searchable; the drawn
chart itself is not reproduced. Rows and columns are bounded.

An item that only kept feed content because the original page was unavailable (a Cloudflare
challenge, an outage, or a rate limit at capture time), or whose archived body is such a loading
placeholder, is retried on later runs: at most eight per source per run, once a day per page,
giving up after seven attempts. A successful retry rewrites
the body, retained HTML, and media while keeping the item's path, dates, labels, and authors.
Explicit refresh still forces a new capture regardless of this schedule.

Readability discards elements whose class or id mentions sharing. A wrapper that contains real
media and no sharing links (Apple Newsroom's `image-sharesheet` figures, for example) is renamed
before extraction so its picture survives; genuine share widgets with intent or social links are
still removed.

Shared boundary cleanup removes a compact leading `By Name Name MM.DD.YY` paragraph only when its
date matches the item's publication date. A standalone editorial note at either document
boundary such as `This article was updated on 08 September 2026.` is removed too, because the
item metadata already shows publication and update times; the same words inside prose stay. It preserves headings, quotations, code, normal prose,
later bylines, and existing front matter. Builds apply the same cleanup to older archived bodies
without rewriting their stored Markdown or HTML.

OpenReview papers can use verified public metadata and an identity-matched author preprint instead
of the challenged web application; see [OpenReview ingestion](openreview.md). Canvas applications
can retain their safe prose and automatically load the sandboxed live original inside the reader; see
[reader behavior](themes.md#reader-behavior). Neither route executes publisher scripts during fetching.

`wreq = 6.0.0-rc.31` and `wreq-util = 3.0.0-rc.14` are exact prerelease pins: both published
packages use Apache-2.0 and declare Rust 1.85 compatibility, preserving aggr's Rust 1.96 minimum.
The newer stable pair requires Rust 1.98; the older stable utility release has a different,
GPL-3.0 license. Review the actual package licenses, compiler requirements, and platform builds
before changing these pins.

Building from source additionally needs CMake 3.22+, a C/C++ toolchain, Git, and native libclang
for bindgen. Linux CI installs `cmake libclang-dev build-essential`; macOS uses Xcode tools and
Homebrew `cmake llvm`, with `LIBCLANG_PATH="$(brew --prefix llvm)/lib"`. Windows uses the native
Visual Studio environment, Ninja, and LLVM; `LIBCLANG_PATH` points to LLVM's `bin` directory.
Windows x64 also needs NASM. Windows ARM64 uses clang-cl's integrated assembler instead of NASM.
These are contributor and release build requirements; prebuilt binaries do not require them.
CI and release workflows keep native Linux GNU, macOS, and Windows runners for both architectures.
Windows ARM64 must pass those workflows; upstream's documented build matrix does not cover it.

## PDF preservation

In heavy content mode, direct PDF articles and explicitly resolved PDF alternates can retain an
item-owned document companion. Capture obeys `fetch.max_body_bytes` (10 MB by default) and accepts
at most 16 MiB. It requires a PDF signature and a compatible response type, and validates the
stored path and content hash before publication. HTML challenges never
become document companions. Fetching does not run the PDF or launch a browser.

Later syncs repair missing companions without replacing the article's body, HTML, or existing
media. Automatic repairs are bounded to eight attempts per source per run and share the capture
retry policy: once daily, stopping after seven failures. Explicit refresh bypasses that backoff.
A valid retained companion needs no repeat download. Copies followed from another aggr instance
retain the verified companion without fetching its publisher again.

The build publishes admitted documents under immutable same-origin URLs; oversized or unavailable
companions leave the original-link fallback. Store retention removes the current item's companion
through an ordinary commit, leaving its historical Git object intact. See
[PDF reader behavior](themes.md#pdf-documents) and [build budgets](build-budget.md).

## A local snapshot and its original

An item page identifies the readable copy held by one aggr instance. Its canonical URL therefore
points to itself, not to the upstream article. This identifies the snapshot; it does not claim
authorship or permission to republish. Pages use `noindex,follow` by default. Search-engine
indexing requires `[site] indexing = true` and a release build; development stays noindex.
See [publication and search indexing](hosting.md#publication-and-search-indexing).

Provenance is expressed separately and consistently:

- the exact original URL and first capture time are visible on the article page, along with the
  local replication time when it was copied from another aggr;
- HTML uses `rel=original` and `rel=via`, while the visible link has the microformats2
  `u-bookmark-of` property;
- Schema.org JSON-LD models the local page as a `WebPage` plus `ArchiveComponent`, joined to the
  upstream `CreativeWork` with `isBasedOn` and `archivedAt`;
- Atom and RSS use `rel=via`, while JSON Feed uses `external_url`;
- Markdown front matter keeps `link` and `first_seen` beside the saved content;
- `linkset.json` maps original URL anchors to their local copies and maps each copy to its
  alternate representations.

The original URL is deliberately not treated as another representation of the local page, nor is
it put in the sitemap: sitemap entries must be local public URLs. A normalized form helps local
lookup, but the exact URL recorded with the item remains the provenance source of truth.

The current item route is a stable reading page whose presentation can change when the site is
rebuilt. aggr does not call it a Memento: a conforming Memento also needs an immutable capture URI
and response headers such as `Memento-Datetime`, which a portable static deployment—particularly
GitHub Pages—cannot always provide.

## Representations and feeds

Every published, retained item is available as clean HTML plus `.md`, `.txt`, `.rst`, and `.json`.
The HTML page advertises its alternate representations, Open Graph metadata, Schema.org JSON-LD, and
microformats2 properties. The representations share one path-derived item identity while the
upstream URL remains provenance.

Every non-empty collection—the main feed, a source, a category, or a tag—has three equivalent
streams and advertises them from its HTML:

| Route | Media type | Format |
|---|---|---|
| `atom.xml` | `application/atom+xml` | Atom 1.0 |
| `rss.xml` | `application/rss+xml` | RSS 2.0 |
| `feed.json` | `application/feed+json` | JSON Feed 1.1 |

The same names live below `sources/<slug>/`, `categories/<slug>/`, and `tags/<slug>/`. The
human-facing `/sources/`, `/categories/`, and `/tags/` directories link to those stable
per-collection routes; `/browse/` combines the directories. Feed entries point their primary URL at the local
clean-reading page and carry the original URL through the format's provenance field.

The syndication surface is the same on every host: the three feed formats carry each episode or
video enclosure (`rel=enclosure` in Atom, `<enclosure>` in RSS, `attachments` in JSON Feed) with
its media type, byte size, and duration when the publisher declared them; `sources.opml` lists the
followed feeds as an OPML 2.0 subscription list; `llms.txt` inventories the public resources; and
`aggr.json` is the machine entry point. Every reading page advertises the root feeds and the OPML
list. The OPML output round-trips through aggr's own importer: pointing another instance's
`[[sources]].url` at `sources.opml` recreates the same feed URLs, names, and categories.

## Discovery and URL lookup

A release build emits standard, server-rendered pages; crawlers do not need JavaScript. It also
emits:

- `sitemap.xml`, or a sitemap index plus chunks for a large retained archive, only when
  `[site] indexing = true` and a public base URL is known;
- `opensearch.xml` for the local Pagefind search UI;
- `linkset.json`, an RFC 9264-shaped set of typed relationships between originals, local copies,
  and alternate representations;
- `aggr.json`, the stable machine entry point for an aggr-aware client;
- `llms.txt`, a short inventory of the site's public resources.

HTML advertises the descriptor and linkset. `aggr.json` names the instance and generator, identifies
the aggr network, and enumerates feeds, the collection directories and combined Browse directory,
search, PWA, and linkset endpoints, plus the sitemap when indexing is enabled.
When GitHub repository identity is known, it also points to the pinned root config and data tree.
The human `aggr.toml` navigation link opens GitHub's commit-pinned blob page, while machine
metadata uses the raw-content URL. On another host those GitHub-specific fields are omitted and
the colocated `aggr.toml` remains the configuration entry point.

Pagefind ranks titles and clean article prose while deliberately indexing exact and normalized
original URLs. Pasting a URL can therefore find a local copy on a best-effort basis without making
URL fragments pollute article snippets. `linkset.json` provides the non-interactive equivalent for
tools that already know an instance. Neither creates a global registry: a client must first
discover the public instance.

Pages remain publicly accessible when indexing is disabled; `noindex` asks cooperating search
engines not to include them in results and is not access control. When indexing is enabled,
engines can discover retained items through archive pagination and the sitemap. They may cluster
similar copies, select the live original as the representative result, or decline to crawl an
unlinked site. The custom `aggr:*` metadata is for aggr-aware clients, not a claim that general
search engines understand the aggr network.

## The aggr network

`aggr` is the engine. An **aggr instance** is a repository containing a root config, data branch,
and the site they produce; provider workflow files are optional. Every instance is independent and
can identify itself as part of the same protocol without registering with a central service or
sending telemetry.

Every HTML page carries the semantic instance type and network identity, links `aggr.json` through
the registered `service-meta` relation, and links its root configuration with `via`. Schema.org
JSON-LD expresses the same membership with `WebSite.isPartOf`; `meta[name="generator"]` and
`meta[name="aggr:network"]` allow inexpensive recognition without JavaScript. The versioned
descriptor schema is [`aggr-instance.schema.json`](aggr-instance.schema.json).

The config link identifies the tracked root file used for the build when that identity is known. It
does not embed the full effective configuration: local collection files, remote collection bodies,
themes, environment expansion, and the binary version remain separate inputs. Pin or vendor remote
collections when repeatable rebuilding matters.

An instance can also consume another instance's retained data through its repository URL.
That copies readable item content and its ultimate original URL into a second repository. It is
decentralized replication of selected current content, not synchronization of the source
repository's complete Git history.

## Stable and portable URLs

- Internal links never assume an origin root. They resolve relative to the generated page.
- Canonical links, feed identifiers, linkset targets, OpenSearch templates, and sitemaps are emitted
  only when the public base URL is known from `--base-url`, or from `[site] url` during an
  `aggr build --release`. Sitemaps additionally require the release indexing opt-in.
- Moving a published site to a new origin or mount path requires one rebuild so absolute identities
  remain truthful.
- Item paths do not change after capture. Their current static pages remain published while the
  items remain in the data tree.
- Hashed immutable assets may be cached indefinitely. HTML, feeds, the linkset, manifest, and
  `sw.js` remain revalidatable so new builds take effect.
- `robots.txt` is emitted only for an origin-root deployment; a file under a project subpath cannot
  govern that host. It allows crawling so engines can read each page's indexing directive, and
  advertises a sitemap only when indexing is enabled. Submit the sitemap directly for a subpath
  deployment if you choose to enable indexing.

Retention removes an item from the current static site and discovery outputs. Append-only Git
history still contains the old object while its ancestor commits remain reachable, but search
engines are not a Git-history recovery interface. Keep the default unlimited store retention when
the live site is intended to remain a complete archive.

## Preservation boundary

aggr preserves a safe, readable snapshot: metadata, extracted or feed Markdown, and optionally the
stripped HTML used to derive it. Optional image retention preserves safe raster masters and
lossless derivatives, including a metadata lead image omitted from extracted prose. SVG diagrams
are retained as passive rasters with their source URL; their original executable markup and
referenced resources are not archived. Standard SVG doctypes without internal subsets are ignored;
entity definitions and internal DTD subsets remain rejected. It does not preserve the complete HTTP exchange,
executable page, stylesheet, fonts, video streams, or arbitrary linked assets, and a failed
extraction can leave only feed metadata. It is therefore not a pixel-perfect mirror or a WARC archive.

Protocols that require a receiving server, callback, provider-controlled response headers, or an
immutable capture endpoint are not advertised merely because aggr could emit a suggestive link.
Webmention, WebSub publication, and Memento support should be claimed only when a deployment can
complete their full contracts.

Shared content cleanup runs during fetch and rendering, so retained archives benefit without
rewriting stored companions. Boundary removal targets standalone metadata, comment controls, and
accessibility labels such as “opens in new window”. A leading standalone “Advertisement” label
also removes its immediately following orphan bullet. Unrelated bullets, code, quotes, lists,
references, and interior prose remain intact. Provider-specific description
normalization stays scoped to that provider. Public YouTube captions are fetched with bounded
requests when advertised; unavailable captions leave the description intact and do not count as a
successfully cached transcript.

Subscription offers are not article bodies. aggr recognizes FT's subscription-plan page and
short, explicitly marked gate-only panels; subscription notices beside readable prose do not
trigger replacement. On a verified gate, capture checks the public Wayback availability API and
an exact-original archive.today lookup, without credentials or creating new snapshots. A recovered
copy must match both the original URL and article title and contain substantial readable prose.
The item's original link stays unchanged; `archive_url` records the public snapshot and
`archive_captured_at` retains Wayback's supplied `YYYYMMDDhhmmss` capture timestamp when available.

Archive requests share normal HTTP byte limits and the article cache. Recovery is limited to
three requests under fifteen seconds, with a six-second request deadline, a fifteen-minute failed
provider backoff, and a one-day retry delay per unsuccessful article. Archives may be unavailable,
rate-limited, missing, or contain the same gate; no recovery is promised. In those cases the reader
keeps a meaningful feed summary and offers the original and an exact-URL archive lookup. Older
captured offers receive the same cleanup during build without network requests or archive writes.
The existing bounded daily capture-repair pass also retries positively identified stored gates,
including entries no longer listed by the feed; successful recovery keeps the item path and dates.
See the [Wayback availability API](https://archive.org/help/wayback_api.php) for its response contract.

Aggregator feed bookkeeping is not article prose. Feed captures with a complete boundary group
of `Article URL` and a recognized Hacker News or Lobsters `Comments URL` are normalized when
captured and built; the article URL must match the item's original URL. Numeric points and comment
counts move to metadata, existing metadata wins, and a bookkeeping-only summary is discarded.
A PDF can therefore have an empty text body while retaining its document and caption. Meaningful
feed summaries, interior examples, code, quotes, and extracted publisher prose remain unchanged.
Build cleanup leaves archived Markdown and HTML untouched.

Standalone hashtag paragraphs at the beginning or end of an article become item labels, merged
with configured and feed labels using the same lowercase, sorted, deduplicated representation.
Interior hashtags, prose, code, lists, and quoted examples stay in the body. Builds apply this to
older Markdown without changing the archive; capture and explicit reprocessing persist the labels.
Original-page extraction also captures explicit `article:tag` metadata and article-scoped `rel=tag`
links before Readability removes them. Pure tag-link groups at article boundaries are removed from
the body; generic site keywords, navigation, and related-article links do not become labels. Tags
whose original-page metadata was already discarded require a new original-page capture.

Embedded X posts and standalone status-link paragraphs become quoted cards inside the article,
with canonical X attribution. Capture fetches at most four cards under one ten-second deadline,
using the shared bounded HTTP client and article cache; it preserves only the requested post,
not surrounding replies. Available images enter normal bounded media retention. Source-provided
quote text remains when the public mirror is unavailable or returns a shorter version. Inline
references stay links. Older standalone cards receive the same portable Markdown blockquote
format during build without network requests; downloading their media requires a new capture.

Publisher code-language hints are captured before Readability removes presentation classes, then
carried through stored HTML into Markdown fences. Explicit plain-text blocks remain plain text;
unknown language names are retained without promising a matching syntax grammar. Legacy `<tt>`
markup becomes inline code. Cloudflare email-protection payloads are decoded as escaped text,
including package versions mistakenly classified as email addresses, without running publisher
JavaScript. LWN's trailing article-index navigation is removed without removing prose tables.
Consecutive numbered citations stay inline, as does prose following a link. Intentional HTML
line breaks remain intact; action-link layout rules must not split citations or ordinary prose.

`--reprocess` can recover these semantics when the retained HTML still contains them. Language
metadata discarded by an older extraction needs an original-page extraction; it cannot be reliably
reconstructed from the stored code alone. Extraction cache versions change with these rules.
Reprocessing includes articles from renamed or removed sources and runs before source fetching.

Explicit `--refresh` may fill missing previews or article media while preserving existing
companions and historical blob URLs. To refresh existing dev items, run `dev --refresh` once and
then return to normal `dev`; leaving the flag enabled reprocesses feed-present entries on each
poll. Dev changes only its isolated cache.
