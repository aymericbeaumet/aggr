# Published-site budget

aggr preserves article text while fitting the generated site to a configured byte budget. The
default is 1,000,000,000 bytes, matching a conservative decimal interpretation of GitHub Pages'
1 GB published-site limit. The Git archive and its original image masters remain unchanged.

```toml
[site]
build_max_bytes = 1000000000
media_full_quality_days = 30
```

`build_max_bytes` must be positive. It counts logical file lengths across the complete output:
article pages and portable representations, feeds, search, theme assets, manifests, worker and
published media. It is not the compressed upload size, filesystem allocation or Git repository
size. A successful build must fit; if article text and required assets alone exceed the limit,
the build fails with an explanation instead of silently discarding articles.

## Media priority

All retained articles remain available to read and search. Media is processed in article order,
newest first. Recent articles retain full-quality published media when it fits. Articles older
than `media_full_quality_days` use smaller compressed publication copies where possible. Age uses
the article's publication time, falling back to its updated or first-seen time, so importing an
old article does not make it recent. Setting the window to zero makes past articles eligible for
compression; future-dated articles remain recent.

Older eligible images are resized to at most 1,600 pixels per axis. Opaque images use JPEG
quality 72; transparent images use PNG. A replacement is kept only when it is smaller. Animation,
embedded color profiles and high-bit-depth images keep their masters instead of being flattened;
they can still be omitted if they do not fit. Small feed previews retain their existing encoding.

The budget counts shared content-addressed files once. When media does not fit, aggr leaves it
out of the generated site and keeps the publisher URL in the article. Feed previews can also be
omitted. Logs report the budget outcome. Full quality is a priority, not a promise to exceed the
configured limit for recent images.
Retained PDF companions use the same allowance and newest-first priority. Their bytes are
preserved exactly, without lossy compression. A published copy loads from the reader's own
origin and joins its offline resources; an omitted copy falls back to the publisher URL.
When a recent image's master fits but its responsive copies do not, aggr publishes the exact
master alone before considering omission. The browser scales that full-quality image itself.
An oversized image family can be skipped while smaller later images still fit. This is
newest-first admission, not a strict date cutoff that removes every older image together.

The original Markdown, PDF companions, and retained image masters stay in the data branch. Publication compression
does not rewrite them. A larger future budget can include omitted media again; the age window
still decides whether to publish full quality or a compact copy. Omitted publisher-hosted
images may disappear or fail to load and are not guaranteed offline. The offline manifest lists
only the local resources the build actually publishes.

## Enforcing the limit

The default theme loads one shared, minified bootstrap for preferences and dates before paint,
instead of repeating those scripts inline. This removes about 6,445 bytes per page and adds one
4,537-byte content-addressed asset. JavaScript and CSS files are published once and reused across
pages. These figures describe the current bootstrap refactoring, not total page size; the budget
measures actual output files and does not assume compressed transfers or duplicate `.gz` sidecars.

The build renders the complete output and measures its logical bytes. If it is over budget, it
reduces the media allowance and rerenders with the same captured content. Retries are bounded,
with a final no-local-media attempt to distinguish a media overrun from an archive whose text and
required assets already exceed capacity. Ingestion and Git publication do not repeat during these
render retries. A failed build never becomes a successful cached output.

Media selection happens before pages, search metadata and offline manifests are generated. aggr
does not delete image files after writing pages that reference them. Crossing the age threshold
changes the render-cache generation even when no article has arrived; the output's content version
also tracks the media selected for publication. These content updates do not themselves count as
an application release.

The Pages workflow retains a separate 1,000,000,000-byte check before upload. Raising the build
budget cannot raise GitHub's hosting limit. For a larger deployment, use a suitable host and set
the reusable workflow's `pages: false` input to keep its site artifact without a Pages deployment.
See [hosting](hosting.md) and [GitHub Pages limits](https://docs.github.com/en/pages/getting-started-with-github-pages/github-pages-limits).

## Reusing compressed media

Compressed publication copies are derived cache entries under
`.aggr/cache/build-v1/deployment-media-v1`. Their identity includes source content and the encoding
policy. Unchanged older images reuse validated encoded bytes; missing or invalid entries are
recomputed from the archived master. Rebuilding, adjusting the site budget, or crossing an age
threshold does not require recompressing an already cached copy with the same inputs.
Receipts also remember images that cannot be compacted, without copying their original masters.

The reusable workflow restores this directory through a separate Actions media cache. It compares
file paths and contents before and after the build, saving only changed media or a newly populated
cache. Changes to smaller parser and backoff caches do not re-upload unchanged media. These caches
are evictable and subject to storage quotas; correctness and recovery depend on the Git archive,
not on their availability. Raw publisher responses and complete rendered sites are excluded from
Actions cache uploads. See [performance](performance.md#caches-on-github-actions) for the transfer
tradeoffs.

Individual entries are bounded and validated, but this cache has no total-size cap or automatic
expiration. Older encoding-policy entries can remain until cache cleanup or host eviction. It
stores reusable compressed copies and receipts, not another copy of every full-quality master.

## Archive growth

This budget controls deployment size, not acquisition or repository growth. Preserving image
masters still consumes Git storage, and append-only history retains earlier objects. Measure
repository bytes separately; [the public-instance baseline](benchmarks.md) illustrates the cost.

Leave `[store] max_items` and `max_age_days` unset to retain all captured articles in the current
archive. Explicit store retention is independent: it removes current articles through ordinary
commits and cannot reclaim their historical Git objects. The build budget never enables retention
or removes articles to make room for media.
