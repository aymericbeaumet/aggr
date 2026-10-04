# The git model

aggr needs no database service: ordinary repository history is the durable source of truth. The
implementation uses standard Git rather than a hosting API, so the data repository may live on any
server that accepts the branch and refs described below. Local caches only accelerate fetching and
rendering and may be discarded at any time.

This is the contract: what goes where, what is never rewritten, and what is required to recover a
snapshot.

## Two branches, one set of refs

```text
primary branch    aggr.toml, optional provider CI
aggr              orphan, append-only, never force-pushed: the data
refs/aggr/*        lightweight pointers to data commits
```

The primary branch is often named `main`, but aggr does not depend on that name and never commits
to the branch holding the config. The whole `.aggr/` directory (the data worktree `.aggr/data`,
the build cache `.aggr/cache`, and the `aggr.lock` file described below) and the build output
(`_site/`) are added to `.git/info/exclude`, so `git status` stays clean without a `.gitignore`
entry on the primary branch.

The data branch shares no history with the primary branch; it starts from an orphan commit named
`aggr: init` and is only ever extended. An item in an older commit therefore remains addressable by
that commit object after it is changed or retained away from the current tree. This durability
lasts as long as the repository, the data branch, and their reachable history are preserved.
GitHub can turn such an address into a `github.com/…/blob/<commit>/…` page automatically; other
hosts expose the same Git objects through their own interfaces.

aggr fetches and pushes through a remote named `origin`. A hosted runner needs write access to
`refs/heads/aggr` and `refs/aggr/last-good`. No GitHub API is involved in these operations.

## The tree

```text
README.md                                              what this branch is, how to edit it by hand
.gitattributes                                         sources/*/seen.txt merge=union; LF everywhere
.gitignore                                             .tmp*: an interrupted atomic write is never committed
items/<source>/<yyyy>/<mm>/<yyyy-mm-dd>-<slug>.md      YAML front matter + readable Markdown
items/<source>/<yyyy>/<mm>/<yyyy-mm-dd>-<slug>.html    stripped HTML from which Markdown was derived
sources/<source>/state.toml                            identity, resolved_url, title/site URL, language, ETag, Last-Modified, body hash
sources/<source>/seen.txt                              "<key> <yyyy-mm-dd>" per line, append-only
status.toml                                            sources currently failing; absent when all is well
.aggr-sources.json                                     resolved source presentation metadata and remote collection digests
.aggr-discussions.json                                 captured discussion matches for local rendering
```

- **Bootstrap files are regenerated.** `README.md`, `.gitattributes`, and `.gitignore` are written
  when missing, each checked on its own, so a branch created by an older version gains the file it
  lacks the next time a command opens the archive. The `.gitignore` keeps `.tmp*`, the temporary
  file of an interrupted atomic write, out of `git add -A` and therefore out of history.
- **Item paths are identities.** Storage stays date-partitioned, while the public site uses
  `/items/<source>/<stem>/` plus `.md`, `.txt`, `.rst`, and `.json` representations. Search and
  alternate representations key on the path. The date is the upstream publication time, otherwise
  its update time, otherwise the time of first sight; the file is never moved. File names use
  lowercase ASCII slugs of at most 60 characters, with a stable item-derived hash on collision.
- **Bodies are written once.** A normal sync never rewrites an item's Markdown body and never
  moves its path. Enrichment fills in place and never removes existing content: a publisher feed
  can supply a missing audio URL or duration, and missing image and preview companions are
  repaired beside the item. A feed-only capture is upgraded to the original article when the page
  becomes available, keeping its path and dates. Hand edits to `hidden`, `labels`, `authors`, and
  `first_seen` win over all of these. `aggr sync --refresh` and `--reprocess` are the explicit
  exceptions that replace bodies, and the previous version remains in Git history.
- **State is regenerated.** `sources/<source>/state.toml` holds what the next fetch needs:
  `identity` is a hash of the unexpanded fetch inputs (credentials and `${ENV}` values never enter
  history) that invalidates the discovered `resolved_url` when the config changes, alongside the
  upstream title, site URL, and conditional-GET validators. Neither it nor `status.toml` holds
  anything a sync cannot rebuild.
- **Render metadata travels with the archive.** `.aggr-sources.json` captures resolved source
  names, categories, labels, public URLs and preservation options, plus collection identities and
  digests. A digest of the locally expanded source declarations prevents an offline build from
  silently restoring removed subscriptions or obsolete source options. It excludes fetching headers
  and credentials from the saved metadata. `.aggr-discussions.json` captures resolved
  discussion links. Sync writes these only when their contents change, so offline builds can use
  the selected commit's inputs without contacting collection or discussion providers.
- **Deleted stays deleted.** Dedupe keys derived from the entry id, normalized original URL, and
  `title|published` are appended to `seen.txt` when an item is written. They are never removed, so
  deleting a current item does not make a later fetch add it again.
- **HTML is safe storage, not a full web capture.** The `.html` sibling contains the extracted or
  feed HTML used to derive Markdown. Scripts, styles, inline SVG, embedded frames, event handlers,
  and active URL schemes are removed, and the file is capped at `[store] html_max_bytes`. It is
  sanitized again before display. Page chrome, HTTP response metadata, and media files are not
  archived, so this is not WARC-equivalent preservation.

Normalized URL keys retain nondefault ports: `example.com:8080` and `example.com:9090` are
separate endpoints. HTTP URLs with nondefault ports also keep their scheme. Older versions dropped
all ports from URL keys. Retained items still match through their stored original links, but an
already-deleted custom-port item without another matching entry-ID or title/date key may be
rediscovered because its old `seen.txt` hash cannot recover the port.

The original URL and first capture time (`first_seen`) live in each Markdown file's front matter.
A copied aggr item also records `replicated_at`, the time it entered the current repository. A
heavy source attempts to extract the original article; if that request or extraction fails, aggr
keeps the feed body, summary, or metadata it has rather than failing an otherwise healthy source.
Recoverable article failures appear in debug logs (`-vv`). An HTTP 401 or 403 affects only that
URL; an HTTP 429 pauses further article requests to that origin for the run. Interrupted response
bodies use the configured bounded retries before falling back to feed content.

## What a run does

```text
fetch every source in parallel   →  write new items into the data worktree
commit if anything changed       →  push, rebasing on rejection
update refs/aggr/last-good       →  only when every source succeeded
render the static site           →  from the selected data commit
```

- **No trace when nothing changed.** Conditional GET (`ETag` / `Last-Modified`) turns most fetches
  into a 304; a changed body with the same hash is treated the same. No new items and no status
  transition means no commit or push. "Last updated" is the data tip's commit time; "last checked"
  is the build stamp.
- **Status transitions only.** `status.toml` is written when a source starts or stops failing, not
  on every failing run. One failing source never fails the run; only every source failing,
  configuration errors, and data Git/IO errors make sync exit non-zero. After data is saved,
  an auxiliary recovery-pointer failure is reported as a warning and does not block rendering
  or deployment of the current snapshot.
- **One mutating command at a time.** `sync`, `build`, and `clean` hold an exclusive advisory lock
  on `.aggr/aggr.lock` while they run. A second one fails immediately, naming the holder's PID and
  command, instead of racing on `git worktree add`, `git worktree prune`, or the rebase. `clean
  --dry-run` only inspects the lock and creates neither the file nor `.aggr/`; `dev` guards its own
  cache with a `dev.lock` in the same way.
- **Push, never force.** A rejected push is followed by a fetch and rebase onto `origin/aggr`.
  `seen.txt` files union-merge through `.gitattributes`, while regenerated state files keep the
  current run's result. A history that cannot be rebased is a hard error with recovery instructions,
  never a force-push.

## Copying another instance

A repository URL source performs a shallow checkout of another repository's current data branch
and republishes selected items into this one. HTTPS, SSH, and SCP-style `git@host:path` URLs work;
Repository URLs select the aggr importer automatically. See [source normalization](interoperability.md) for
URL inference and importing subscriptions instead of articles.
The copied item keeps the ultimate original URL and records the intermediate repository as `via`,
so following the same article directly and through a friend still deduplicates locally.

This is replication of visible content retained in the current tree, not a mirror of the other
repository's complete history. All such items are considered by default; set `limit` only to bound
the copy to the newest N items. Items already absent from the other instance's current tree are not
recovered by its depth-one checkout.

## Commit labels and refs

Every data commit carries structured trailers, so `git log` is the run log:

```text
aggr: +12 items

rust-blog: +3
hn: +9
lobsters: error: 503 Service Unavailable

Aggr-Version: 1.0.0
Aggr-Config: 4f9c1a2…            # tracked primary-branch commit containing the root config
Aggr-Sources: 2 ok, 1 error
```

Subjects are `aggr: init` for the first commit, `aggr: +N item(s)` for additions, `aggr: status`
for an error transition, and `aggr: update` for other committed changes such as state or retention.
This command counts addition runs:

```sh
git log --grep='^aggr: +' --format=%s aggr
```

Refs are pointers with a meaning, visible through standard Git:

| Ref | Points to | Set when |
|---|---|---|
| `refs/aggr/last-good` | the data tip | a sync ended with zero source errors |

Pointer pushes fetch missing history and retry when a shallow checkout cannot establish ancestry.
A newer pointer published by another run is retained. An unrelated pointer is left unchanged and
reported as a warning; aggr never force-pushes it. In that case `--data-ref refs/aggr/last-good`
still selects the older history, while normal builds use the current data branch.

## Recovery and reproducibility

The data does not need an upstream feed, aggr binary, or hosting UI to remain readable. From any
clone, fetch and export the branch with ordinary Git:

```sh
git fetch origin refs/heads/aggr:refs/remotes/origin/aggr
git log --oneline origin/aggr
git archive --format=tar --output=aggr-data.tar origin/aggr
```

Every Markdown item and stripped HTML sibling in that archive can be opened directly. Copying or
mirroring the Git repository is therefore the minimum viable backup of an instance.

To render the last fully healthy snapshot, fetch its auxiliary ref when it exists. `--data-ref`
then pins which data tree aggr renders:

```sh
git fetch origin refs/aggr/last-good:refs/aggr/last-good
aggr build --data-ref refs/aggr/last-good \
  --release --base-url "https://reads.example.net/"
```

With `--data-ref`, `build` resolves only locally available Git objects and materializes a temporary
snapshot without fetching, updating the data checkout, or invoking checkout hooks and filters.
It skips synchronization, live discussion lookups, commits and pushes. `--offline` selects the local
data-branch tip with the same contract. Missing local objects fail instead of triggering a fetch.
Remote collection expansion uses the selected commit's captured source metadata; an older archive
without that metadata requires a fully local effective source configuration or a prior sync.

The render clock is the selected commit's timestamp, or an explicitly supplied `SOURCE_DATE_EPOCH`.
The current binary, root configuration, local imports and public base URL are also inputs. The
`Aggr-Config` trailer identifies the tracked root configuration commit; retain its matching local
files and the binary when reproducing a build. Search-index serialization can still differ between
cold builds, so this is not a promise of byte-identical output. Offline execution does not make
publisher-hosted media available offline; `--hermetic` adds checks for automatically loaded external
resources. See [commands](commands.md#offline-and-hermetic-builds).

The render cache is an optimization, not part of the archive. `status.toml` and
`sources/<source>/state.toml` are regenerated and self-healing: a malformed or missing file is read
as absent, logged, and rewritten by the next sync that has something to record, so neither needs
restoring from history.

## Editing the branch by hand

Everything on the branch is text and can be edited in a normal checkout or a hosting provider's web
editor:

- **Hide:** set `hidden: true` in an item's front matter. Fetches do not overwrite it.
- **Delete:** remove the `.md` and `.html`. The dedupe keys keep it from returning.
- **Fix a source:** remove `sources/<slug>/state.toml` to force a full refetch. Existing items still
  deduplicate through `seen.txt`.

Do not rebase, squash, delete, or force-push the data branch. Per-feed retention (`[defaults] max_age_days`,
`max_items`, `max_bytes`, and `since`) deletes files from its current tree in a normal commit, so older reachable commits keep
their contents. The generated site and sitemap contain only visible items in the current retained
tree; historical Git objects are durable but are no longer public article pages after retention
removes them.

Policy eviction removes the complete article family, including obsolete image, preview, and PDF
companions. Compact `sources/<slug>/evicted.json` records distinguish policy evictions from manual
deletions: a changed policy can admit those articles again when they remain available upstream.
Ordinary same-policy syncs leave these records unchanged. See [storage limits](storage.md).
