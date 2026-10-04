# Storage and cache limits

Measure the archive, published site, disposable caches, and historical Git objects separately:

```sh
aggr storage inspect
aggr storage inspect --json
```

Inspection reads the existing local archive and configuration without fetching, opening a data
worktree, or changing files. It reports logical current-tree bytes, referenced original images and
responsive renditions, the configured output directory, each build-cache namespace, and loose and
packed Git object storage. Git's object accounting is repository-wide; it is not a fresh-clone
transfer estimate. Missing local output or cache directories count as zero.

## Choose what future syncs retain

The default keeps article text and leaves images, previews, and documents at their publisher URLs.
Use explicit preservation policies when local media is valuable:

```toml
[defaults]
content = "light"       # feed content; "heavy" also extracts article pages
media = "remote"        # alternatively "compressed" or "local"
max_items = 250
max_age_days = 730
max_bytes = 1_000_000_000
# since = 2026-01-01

[[sources]]
url = "https://example.com/feed.xml"
content = "heavy"
media = "compressed"
max_items = 100
```

Each source inherits omitted fields individually. `compressed` keeps a bounded reading copy of
images; `local` retains original image and PDF bytes. Responsive copies are generated for
publication in disposable caches; neither adds responsive copies to new archive commits.
Media settings affect future capture and publication, without rewriting retained masters.
See [source policies](sources.md).

## Bound each feed independently

Count, date, and byte limits apply together. The default retains at most 250 articles per feed,
no older than 730 days, with at most 1 GB of current article files per feed. `since` is an inclusive
UTC date; if both date limits are set, the later cutoff wins. Dates use publication time, then
updated time, then first capture. Set a numeric limit to `0` to disable it; `since = false` clears
an inherited date.

Expired items are removed first, then the oldest excess items until both count and size fit.
Bytes include Markdown, saved HTML, PDFs, image masters, and obsolete owned companions, even
under a remote-media policy. Cleanup removes entire article families only from the feed that
exceeds its allowance. A single article larger than its feed's budget cannot be retained.
Limits run before archive enrichment and after capture, including media-only backfills; temporary downloads and source transaction
backups can exceed the final retained-byte cap during a sync. Count and date also filter incoming
entries before article or media requests. Offline builds select the same retained corpus without
changing the archive.

These are current article-file limits, not Git repository quotas. Normal deletion commits preserve
historical blobs and permalinks. Source state and compact deduplication/eviction records remain;
manual deletions stay suppressed. Widening a policy permits previously policy-evicted items to be
captured again if the upstream still supplies them. Repeating the same policy does not repeatedly
download and discard oversized articles. Old deletions without eviction records remain suppressed.
Disposable caches have separate bounds below.

Remote media reduces future archive growth and ingestion work. The reader still requests those
resources from their publishers. An offline build can contain remote image URLs;
[`build --hermetic`](commands.md#offline-and-hermetic-builds) additionally requires automatically
loaded reader resources to be local.

## Remove old optional renditions

```sh
aggr storage prune-renditions --dry-run
aggr storage prune-renditions --apply
```

Omitting both flags is a dry run. The command requires an existing, clean checkout of the configured
data branch. It verifies each original image's bytes, content address, format and dimensions before
planning removal of that image's optional rendition files. Missing or corrupt originals keep their
renditions. It refuses unsafe filesystem paths and unrelated pending archive changes.

Apply removes the optional files and their metadata references in one ordinary data-branch commit,
then pushes when a remote is configured. Article paths, dates, original image bytes, and the exact
Markdown body remain unchanged. Other YAML values are retained, although formatting can change.
Failure before a commit restores the planned changes; a failed push leaves the local commit for
the next sync to publish. Repeating the command after a successful migration creates no commit or
push.

This shrinks the current checkout and future deployment inputs. **It does not reclaim historical
Git storage:** old commits still contain the removed files, and their blob URLs continue working.
The command never rewrites history, force-pushes, deletes originals, or runs Git garbage collection.

## Bound disposable caches

```toml
[cache]
media_max_bytes = 2147483648     # 2 GiB, compressed publication copies and responsive renditions
response_max_bytes = 268435456  # 256 MiB, cached original-page responses and extractions
```

These positive byte limits apply separately to each repository build cache and each isolated dev
cache. Successful sync/build boundaries enforce them. Obsolete media-processing generations are
removed first; inactive entries are evicted before those used during the current process, oldest
first. If the active set exceeds its limit, it is also evictable. Cache hits do not rewrite files
or timestamps. A missing or evicted entry is regenerated from the retained input when needed.

Cache maintenance never prunes the archive. Media encoding keys track the processing policy and
codec dependencies, so changing only aggr's release version does not invalidate every image.
The media and small-state caches remain separate on Actions; raw page-response caches are private
to the machine that fetched them. See [performance](performance.md).
