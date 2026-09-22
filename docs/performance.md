# Fetch and build performance

For a pinned public-instance snapshot, actual workflow durations, storage breakdown and
reproduction commands, see [archive and deployment measurements](benchmarks.md). Keep archive
files, historical Git storage, published-site size and cache space separate when planning capacity.
The [build budget](build-budget.md) limits published bytes through media selection and older-image
compression; it does not prune article text or reduce the archive's Git storage.

`sync`, `build`, and `dev` share the same fetch pipeline. Config loading performs no requests for
ordinary remote feed/page URLs; only known or explicitly declared collections expand before fetching. Sources use the configured
`fetch.concurrency` limit; article requests within each source use `fetch.article_concurrency`.
Network work uses Tokio and bounded semaphores. Git persistence and source transactions remain
ordered, so concurrent ingestion preserves rollback, deduplication, and no-op runs.

Beyond those two configured limits the fetch stage bounds itself per process: two articles are
converted at once and two are persisted at once (both stages are CPU-bound), eight
recording-duration probes may be in flight, a page's feed discovery considers at most 16 candidate
endpoints (advertised `<link>` elements first, so the cap only drops body anchors), one source's
whole discovery ladder shares a deadline of three request timeouts or 60 seconds, whichever is
longer, and one article's image downloads share a 120-second deadline.

The initial archive scan builds normalized deduplication links, per-source paths for explicit
refresh, and a compact Spotify episode index for publisher-feed reconciliation. Workers share these
indexes instead of rescanning the whole archive once per source. The indexes retain identifiers
and paths, not article bodies.

Ordinary runs also repair missing images in retained articles, including articles outside the latest
feed window and feeds returning 304. A compact candidate index and file checks avoid rereading good
masters. Repairs write only missing companions and preserve article bodies; unavailable images retry
after a one-hour backoff. A missing preview companion is regenerated from available local media; if
unavailable, only its unusable metadata pointer is omitted. It cannot roll back unrelated image
repairs, and other file-system errors still fail with the article path. Downloads allow up to 32 MiB per image and 256 MiB per article, with at most
512 retained images from 1,024 candidates. Raster decoding permits up to 200 megapixels and a 24,000px
axis, one article-image decoder at a time, with a 768 MiB decoder allocation ceiling. Masters above 32 megapixels keep their exact bytes and
bounded responsive renditions without a second full-resolution encode.

Source-set parsing preserves commas inside CDN URLs. Old truncated Substack transformation URLs
recover their safely decoded original image URL for downloading while retaining the archived alias
for body substitution; repair never rewrites the stored article text. A truncated transform such as
`q_auto:good` is recovered only when preserved source sets identify one unambiguous original image.
Same-document footnotes are excluded from image repair, including malformed images in older Markdown.

Thumbnail work shares in-flight requests and successful decoding results by URL, scoped headers,
and reusable image digest. A 64-entry per-run LRU bounds retention; failures remain retryable and
article-specific alternative text stays separate. Up to eight preview downloads overlap while
two decoder slots bound CPU work. Download permits are held while waiting for a decoder to bound
queued image bytes. Cancellation leaves blocking decoder permits held until decoding exits.
PDFs share these limits and cache keys distinguish PDF rendering from image decoding. Explicit
binary article links skip HTML extraction to avoid downloading the same original twice.

Build-time preview fallback reuses retained image renditions and memoizes up to 64 decoding
results. Dimensions from retained publisher HTML are inspected only for articles with image
markup; text-only articles avoid the additional read and sanitization. Render fingerprints
include every module that can change rendered bytes (a unit test classifies each source file as
fingerprinted or render-independent) and name loaded config files by their repository-relative
path, so the same checkout at another location shares a fingerprint.

Stored-image validation receipts live under the existing build/dev cache in `validated-images-v2`.
They reuse verified rendition choices, derived colors, and ThumbHashes across builds
and restarts. Keys cover every input byte, image metadata, and the compiled media implementation
and dependency lockfile. Changed inputs, corrupt receipts, collisions, and unavailable caches fall
back to full decoding and pixel comparison. Fresh bounded file reads and identity/size checks still
run on hits; receipts never modify the archive. After the full-byte SHA-256 key matches, receipts
reuse verified SHA-1 identities and transfer rendition buffers without extra hashing or cloning. There are 16,384 slots of at most 8 KiB each, capped
at 128 MiB, and normal cache cleanup removes them. Each bucket has four candidate slots so common
collisions do not repeatedly evict one another. Empty or corrupt slots are reclaimed first; a full
bucket replaces its oldest receipt. Cache hits never rewrite timestamps. Warm hits reconstruct
the small inline PNG from its validated hash without decoding the master. The first validation
remains a cold operation, and large cold images are deliberately validated one at a time.

`build_max_bytes` is what a host will accept for a published site, so only a publishing build
measures it. A development snapshot (`aggr dev` without `--release`) is served from a local cache
and skips the check: when an archive is larger than the limit, measuring it costs a second complete
build, which on a 3,000-item instance was 205 s followed by 319 s. `aggr dev --release` still
applies the budget, and its snapshot keeps every archived rendition rather than the published
selection, so the local cache holds more bytes than the site would.

Every stage that is CPU-bound runs on the machine's parallelism rather than a fixed slot count.
Reading the archive (one file read and parse per item) and re-deriving stored bodies both run on the
shared worker pool. `sync` and `build` own the machine while they fetch, so article conversion and
persistence take a slot per worker; `dev` fetches in the background while it rebuilds, so it takes
half and leaves the rest to the build a reader is waiting for. Sizing both to the full worker count
measured worse on an eight-core machine: the background sync and the rebuild oversubscribed it and
the build phase grew from 167 s to 322 s.

Re-deriving stored bodies (`--reprocess`) reads each retained HTML companion and converts it again.
The conversion is pure CPU per item and runs on the shared worker pool; applying the results stays
on the calling thread in archive order, so the transaction and the written bytes never depend on
scheduling. On a 3,318-item archive the conversion alone measured 48 s of single-threaded work.

Static article pages and portable representations render with at most eight scoped CPU workers,
limited by available parallelism or by `AGGR_BUILD_WORKERS`. `parallel::map` hands inputs to those
workers through a shared claim cursor (each thread takes the next unclaimed input, so a few slow
articles never leave the others idle) and writes results into preallocated slots, so the output
keeps input order whatever the thread count; fewer than eight inputs run on the calling thread.
The media phase gathers archived media (reads, validation, decoding) on the same workers one
window at a time, about two items per worker and never below eight, and publishes each window on
the build thread in item order, so the dedupe map, memo state, and output bytes never depend on
timing while the assets held in memory stay bounded. Outputs and sitemap order remain
deterministic; all workers join before errors propagate. The existing rendered-site,
Pagefind, article-response, extraction, and dev caches remain in use. Offline catalogs hash each
shared asset once per build. Worker downloads reuse verified revisions across deployments.

Shared template data is converted to MiniJinja values once per build. Article templates still
receive the complete archive, including custom themes, but rendering every article no longer
serializes that archive again. Each article is parsed and highlighted once by bounded workers. Its prepared HTML and plain text
serve reading metrics, excerpts, every collection feed, portable representations, article image
substitution, and search indexing. Shared immutable image/preview paths are written once per build;
verified asset hashes are reused instead of rereading the same bytes for naming.

Dev snapshots share immutable asset bytes across responses and rebuilds. Content-addressed image,
theme, and versioned search files reuse their existing buffers when their lengths still match;
mutable pages and manifests are reread, and removed paths disappear from the next snapshot. Up to
four workers read remaining files, with replacement loading off the async runtime thread. An
in-flight response keeps its original bytes while the next complete snapshot is promoted.

Pagefind output is reused by its input fingerprint. Upstream Pagefind can order internal metadata
differently on a cold rebuild, producing different bytes for an equivalent index. Preserving the
cache keeps no-op index and worker versions stable; a cold rebuild may change the content index
version, but never the application-release fingerprint.

## Caches on GitHub Actions

`src/cache.rs` is the registry of every `.aggr/cache/build-v1` namespace; `Namespace::ci_cached`
decides which ones the reusable workflow carries between runners, and a unit test holds the
workflow's `actions/cache` paths to that list. Only derived state about bytes the site already
publishes qualifies, because it invalidates itself: `deployment-media-v1` (validated compressed
publication copies), `validated-images-v2` (content-keyed receipts),
`pagefind-v1` (index keyed by its input fingerprint), `feed-parsing` (parser-version receipts that
gate conditional GET), and the timestamped backoff markers in `discussions-v1`, `image-failures-v1`,
`capture-retries-v1` and `recording-duration-v1`.

Compressed media uses its own `aggr-media-v1-…` Actions cache, separate from the smaller mutable
state in `aggr-state-v1-…`. After restore and after a successful build, the workflow hashes the
sorted relative file paths and exact contents of `deployment-media-v1`, reading in bounded chunks.
It saves a new media entry only when those bytes changed or no entry was restored; an empty cache
is not uploaded. File timestamps and changes to source backoffs do not trigger media uploads.
The smaller derived-state cache still saves after each successful build. Both restore the newest
matching entry on the next run; neither saves a failed build's state.

The compression cache is keyed by source content and encoding policy, so unchanged older media
can reuse validated publication copies without repeating compression on a fresh runner. A corrupt,
missing or evicted entry is recomputed from the stored master. The age cutoff and deployment budget
decide which copies to publish, independently of the reusable encoded bytes. See
[build-budget caching](build-budget.md#reusing-compressed-media).

Actions caches are evictable performance aids, never the archive or a backup. New media entries
are complete snapshots, not incremental uploads; a changed snapshot still transfers its cache
contents, and retained snapshots consume the repository's cache quota. Separating media avoids
copying it merely because a small backoff marker changed. The before/after fingerprints also read
all cached media bytes twice, so cache transfer, hashing and encoding should be measured separately.

`articles-v1` holds raw original-page responses. It is private to the machine that fetched it and is
never uploaded. `render-v1` is not cached on Actions either: the fingerprint folds each item's age
band (1 h, 3 h, 24 h) into the generation, so any item under a day old changes it, and a full site
of a 1,100-item instance is about 2.4 GB per entry. Uploading one per run exhausted the 10 GB
repository cache quota for entries that almost never hit.

Nothing in these caches expires by age except the backoff markers, so `aggr sync` sweeps the build
cache at most once a day (`cache::sweep`, remembered in `.aggr/cache/build-v1/.last-sweep`; a dry
run never sweeps). Under `articles-v1` it removes every `bodies/<sha1>.html` that no `entries/*.toml`
references any more (a page that changed leaves its previous body behind) and every
`extracted/<version>/` directory of an earlier extractor version. Bodies themselves have no TTL: a
body stays as long as one entry points at it, however old, because it is what conditional GET
revalidates against. The same pass can drop `image-failures-v1/<generation>/` directories of earlier
media implementations, since `sync` passes the current generation. The
sweep is best-effort: a path that cannot be removed is logged at debug and retried on the next pass,
and no sweep failure ever fails the sync. The marker lives in the git-excluded cache, so a run that
finds nothing new still leaves no trace in the repository.

A warm run on Actions therefore still renders every page, but reuses unchanged image compression,
skips cold image validation and Pagefind indexing, fetches feeds conditionally, and honours the image, capture and duration
backoffs. Expect rendering to dominate the build; the cold image validation that took about seven
minutes on a 1,120-item instance disappears once the receipt cache restores. The first run after a
cache eviction is cold again.

These changes remove repeated work and allow independent work to overlap. They do not change
storage retention or guarantee a fixed speedup; source latency, image dimensions, CPU availability,
and warm versus cold caches determine the result.

## Reader startup and search

The reader is four hand-written files embedded in the Rust binary, with no build step. Only the
small pre-paint bootstrap blocks rendering; the core module is deferred, and search and media are
fetched on demand. The complete static HTML remains the first paint and the no-JavaScript fallback.

Search completion uses `search-catalog.json` (version, base, document count, and facets). Pagefind's
runtime and its filter files wait until an actual query needs them, so a reader who never searches
downloads none of it. See [client development](client.md) for the search contract.

Prefetching is the browser's: a `speculationrules` document rule with moderate eagerness, which
lets the browser spend its own budget and cancel work the reader moved away from.

## Measuring changes

Every build logs one info line (`-v`) of the form `build: <phase> Xs, …, total Xs (N pages, M
items)`, naming the seven phases in build order: archive preparation, media and item metadata,
feed and directory pages, article pages and representations, feeds and static assets, search index,
and offline catalog and worker. Under GitHub Actions the same line is also printed as a `::notice`,
so it reaches the run summary without debug logging. The search phase is where a cold run
legitimately differs from a warm one: an index rebuilt from scratch can differ byte-for-byte
because upstream Pagefind fills its word and filter maps from hash maps, which the reused
`pagefind-v1` entry masks. Use `RUST_LOG=aggr=debug` on builds or dev to see archive, media,
page-rendering, index, and offline preparation timings without verbose dependency logs affecting
the measurements. Compare cold caches,
warm rebuilds, and no-op builds separately. Avoid simultaneous source
edits and competing CPU-heavy jobs during timing comparisons.

`cargo test benchmark_prepared_template_context -- --ignored --nocapture` compares repeated and
shared context conversion for 1,000 pages with 1,000 archive entries. A development-profile run
measured 14.5 seconds versus 8.9 milliseconds; this isolates template preparation, not the full build.

In an isolated localhost production-client check with PWA downloads and images disabled, opening
source suggestions changed from 11 requests / 434,216 bytes to one request / 33,937 bytes. These
figures measure completion traffic, not a complete full-text search. The first actual search still
loads the versioned Pagefind runtime and its required index data.

On the same 1,519-image archive (1.27 GiB), warm validation fell from 109–114 seconds to
17–18 seconds after four-way receipt reuse and optimized SHA-256. Repeated decoding fell from
384–473 images to 13. These development-profile measurements exclude article rendering; cold
validation still reads and decodes every uncached input. SHA-1 was subsequently optimized after
profiling the remaining warm path. Keep first-time PWA downloads separate from interactive search
measurements: background article and index retention can compete with a first uncached query.

A subsequent dev rebuild of 970 retained items completed in 52 seconds with validated-image
receipts available: archive preparation 2.0 s, media and metadata 21.8 s, feed/directory pages
7.0 s, article representations 2.1 s, feeds/assets 2.6 s, search 14.2 s, and offline output 2.4 s.
This is a single-machine measurement, not a before/after total-build comparison. The previous
snapshot stayed available while its replacement was built. An unchanged Cargo build took about
one second once the final profile and embedded assets had compiled.

An isolated 987-item archive comparison using the same frozen theme and cached search index measured
37.0 seconds before shared article preparation and receipt identity reuse, and 23.8 seconds after
(a 36% reduction in build time). Media/metadata fell from 21.0 to 15.7 s, feed/directory pages from
6.3 to 2.4 s, article representations from 3.0 to 1.5 s, and feed/static output from 2.8 to 0.1 s.
All 6,069 published asset files had identical paths and bytes across those runs. These are single
development-profile runs, not release benchmarks. The first run populating the new
receipt cache took 292 s, including 273 s validating images; it is not comparable to a warm rebuild.

Completion benchmarks with 1,000 sources measured about 0.10 ms per suggestion pass and 0.0005 ms
for a completed-token probe. A regression test bounds normalization work to linear cold preparation
and constant warm alias lookup; a previous 1,000-source pass normalized over two million values.

The mobile browser contract checks cached navigation at 390px and 360px, including a simulated
32px home-indicator inset and keyboard resizing. Five tab destinations took 4.5–17.5 ms per visit
in a localhost production-bundle run, with no navigation animation hooks. This measures already
cached pages; it does not predict uncached network latency or replace testing on physical iPhones.

A final five-run browser comparison used fresh sessions, localhost, HTTP cache disabled, blocked
images, and PWA downloads off. Mobile emulated a 390px viewport with 4× CPU throttling. Median
milliseconds were:

| Operation | Desktop before → after | Mobile before → after |
| --- | ---: | ---: |
| First contentful paint | 56 → 64 | 176 → 144 |
| Client ready | 59 → 58 | 175 → 190 |
| Open preferences | 50 → 17 | 86 → 70 |
| Return to feed | 50 → 17 | 50 → 22 |
| First full-text search | 310 → 306 | 353 → 340 |

Navigation benefits from removing frame waits. Startup and first-search measurements remain in a
similar range; this does not establish an across-the-board speedup. The browser performance test
writes individual samples to `target/browser-artifacts/client-performance.json`.

## Rust development builds

The development profile keeps basic optimization for aggr itself and stronger optimization for
image decoding (including PNG filtering), regular-expression matching, and SHA-1/SHA-256. Other dependencies keep Cargo's fast
unoptimized development build. Warm asset validation still reads and SHA-256 hashes retained bytes; receipts reuse the previously
verified SHA-1 asset identities without a second byte pass.

The profile emits line tables instead of full DWARF. Backtraces and profiles keep file and line
information, while the debug information that dominated linking is gone: a one-line edit to
`src/main.rs` rebuilds in about 6 s instead of about 19 s on an M-series laptop, which is what
`cargo run -- dev` pays on every restart. Use `RUSTFLAGS="-g"` for a session that needs full
variable inspection under a debugger.

Keep the local `target/` directory between edits. Cargo enables incremental compilation for the
application by default; changing compiler flags, profiles, or target directories can discard that
benefit. `make timings` writes Cargo's HTML timing report under `target/cargo-timings/`. Compare a
cached no-op build separately from a representative source edit, and avoid concurrent source or
embedded-asset writes while measuring. See [Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html).

`make check` verifies formatting, then runs frontend checks alongside Rust validation. Rust linting
finishes before tests begin, avoiding competing Cargo processes and target-directory locks.
Compiler parallelism remains under Cargo's control. Build, run, lint, and test Make targets use
`--locked` so verification cannot silently update dependency versions.

CI preserves completed dependency builds even when later checks fail, including the native TLS
build. The existing compiler, platform, and environment cache keys remain separate. Workspace and
incremental artifacts are not uploaded by default; local incremental compilation remains enabled.
See [rust-cache](https://github.com/Swatinem/rust-cache). A global compiler wrapper is not required:
[sccache cannot cache linked binaries or incremental crates](https://github.com/mozilla/sccache#rust),
so it does not directly accelerate aggr's normal incremental application rebuild.

A later 390px, 4× CPU navigation profile used the production bundle against the same local archive.
Returning to the feed fell from 120–122 ms to 97–101 ms of browser task time. Layout passes fell
from four to two; combined style/layout work fell from 45–51 ms to 24–28 ms. Unchanged preferences
no longer rewrite root attributes or force a theme-color read, and normal page focus avoids mobile
keyboard geometry reads. Initial layout of the larger Qwen article remained about 115 ms; removing
one synchronous read moved that necessary work rather than eliminating it.

A three-run mobile scroll probe over 240 frames reduced warm style recalculation from 245–248 ms
to 100–109 ms after isolating the progress bar transform. Both versions generally sustained 16.7 ms
frames in that localhost probe. These CPU-throttled desktop-browser measurements are scoped to the
profiled pages, not physical-device or network latency guarantees.
