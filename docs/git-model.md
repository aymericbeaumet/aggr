# The git model

aggr needs no database service: ordinary repository history is the durable source of truth. The
implementation uses standard Git rather than a hosting API, so the data repository may live on any
server that accepts the branch and refs described below. Local caches only accelerate fetching and
rendering and may be discarded at any time.

This is the contract: what goes where, what is never rewritten, and what is required to recover a
snapshot.

## Two branches, one set of refs

```text
primary branch    aggr.toml, optional provider CI, themes/, templates/, static/
aggr              orphan, append-only, never force-pushed: the data
refs/aggr/*        lightweight pointers to data commits
```

The primary branch is often named `main`, but aggr does not depend on that name and never commits
to the branch holding the config. The data worktree (`.aggr/data`) and build output (`_site/`) are
added to `.git/info/exclude`, so `git status` stays clean without a `.gitignore` entry.

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
items/<source>/<yyyy>/<mm>/<yyyy-mm-dd>-<slug>.md      YAML front matter + readable Markdown
items/<source>/<yyyy>/<mm>/<yyyy-mm-dd>-<slug>.html    stripped HTML from which Markdown was derived
sources/<source>/state.toml                            upstream title/site URL, ETag, Last-Modified, body hash
sources/<source>/seen.txt                              "<key> <yyyy-mm-dd>" per line, append-only
status.toml                                            sources currently failing; absent when all is well
```

- **Item paths are identities.** Storage stays date-partitioned, while the public site uses
  `/items/<source>/<stem>/` plus `.md`, `.txt`, `.rst`, and `.json` representations. Search and
  alternate representations key on the path. The date is the upstream publication time, otherwise
  its update time, otherwise the time of first sight; the file is never moved. File names use
  lowercase ASCII slugs of at most 60 characters, with a stable item-derived hash on collision.
- **Write once.** A normal sync never overwrites an existing `.md`; hand edits win. `aggr sync
  --refresh` is the explicit exception, and the previous version remains in Git history.
- **Deleted stays deleted.** Dedupe keys derived from the entry id, normalized original URL, and
  `title|published` are appended to `seen.txt` when an item is written. They are never removed, so
  deleting a current item does not make a later fetch add it again.
- **HTML is safe storage, not a full web capture.** The `.html` sibling contains the extracted or
  feed HTML used to derive Markdown. Scripts, styles, inline SVG, embedded frames, event handlers,
  and active URL schemes are removed, and the file is capped at `[store] html_max_bytes`. It is
  sanitized again before display. Page chrome, HTTP response metadata, and media files are not
  archived, so this is not WARC-equivalent preservation.

The original URL and first capture time (`first_seen`) live in each Markdown file's front matter.
A copied aggr item also records `replicated_at`, the time it entered the current repository. A
heavy source attempts to extract the original article; if that request or extraction fails, aggr
keeps the feed body, summary, or metadata it has rather than failing an otherwise healthy source.

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
  configuration errors, and Git/IO errors make sync exit non-zero.
- **Push, never force.** A rejected push is followed by a fetch and rebase onto `origin/aggr`.
  `seen.txt` files union-merge through `.gitattributes`, while regenerated state files keep the
  current run's result. A history that cannot be rebased is a hard error with recovery instructions,
  never a force-push.

## Copying another instance

A `type = "aggr"` source performs a shallow checkout of another repository's current data branch
and republishes selected items into this one. Its Git URL may point to any supported HTTP(S) host.
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

With `--data-ref`, `build` skips source synchronization, discussion-network lookups, commits, and
pushes, then renders the selected stored tree. Unavailable feeds, article pages, and discussion
services therefore do not prevent this recovery build. Configuration loading can still fetch
remote `include` files, so this is fully offline only when the effective config is local. Nor does
the flag freeze every other build input: the current theme and binary affect HTML, and
time-dependent presentation can change between builds. The `Aggr-Config` trailer pins the tracked
root config commit only; remote included config bodies are cache inputs, not committed data. For a
repeatable recovery, keep all local includes and theme files in the primary branch, pin or vendor
remote includes, retain the matching aggr binary, and record the intended public base URL. The
render cache is an optimization, not part of the archive.

## Editing the branch by hand

Everything on the branch is text and can be edited in a normal checkout or a hosting provider's web
editor:

- **Hide:** set `hidden: true` in an item's front matter. Fetches do not overwrite it.
- **Delete:** remove the `.md` and `.html`. The dedupe keys keep it from returning.
- **Fix a source:** remove `sources/<slug>/state.toml` to force a full refetch. Existing items still
  deduplicate through `seen.txt`.

Do not rebase, squash, delete, or force-push the data branch. Retention (`[store] max_age_days` and
`max_items`) deletes files from its current tree in a normal commit, so older reachable commits keep
their contents. The generated site and sitemap contain only visible items in the current retained
tree; historical Git objects are durable but are no longer public article pages after retention
removes them.
