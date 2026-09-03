# The git model

aggr needs no database service: repository history is the source of truth. Local caches only
accelerate fetching and rendering and may be discarded at any time. This page is the contract —
what goes where, what is never rewritten, and what a run may or may not do.

## Two branches, one set of refs

```
main            aggr.toml, .github/workflows/aggr.yml, optional themes/ templates/ static/
aggr            orphan, append-only, never force-pushed: the data
refs/aggr/*     lightweight refs on data commits, pushed as pointers
```

`main` is yours. aggr never commits to it; the data worktree (`.aggr/data`) and the build
output (`_site/`) are added to `.git/info/exclude`, so `git status` stays clean without any
`.gitignore` entry.

The data branch shares no history with `main` (it starts from an orphan commit `aggr: init`)
and is only ever appended to. That single rule is what makes every `blob/<commit>/…` URL on
GitHub permanent: a commit that was reachable once stays reachable forever.

## The tree

```
README.md                                              what this branch is, how to edit it by hand
.gitattributes                                         sources/*/seen.txt merge=union; LF everywhere
items/<source>/<yyyy>/<mm>/<yyyy-mm-dd>-<slug>.md      YAML front matter + Markdown body
items/<source>/<yyyy>/<mm>/<yyyy-mm-dd>-<slug>.html    raw HTML the Markdown was derived from
sources/<source>/state.toml                            upstream title/site URL, ETag, Last-Modified, body hash
sources/<source>/seen.txt                              "<key> <yyyy-mm-dd>" per line, append-only
status.toml                                            sources currently failing; absent when all is well
```

- **Item paths are identities.** Storage stays date-partitioned, while the public site uses
  `/items/<source>/<stem>/` plus `.md`, `.txt`, `.rst`, and `.json` representations. Search
  index and alternate representations all key on the path. The date is `published` (or the
  time of first sight) at write time and is never moved. File names are lowercase ASCII slugs,
  ≤ 60 characters, safe on every OS; collisions in a directory get `-2`, `-3`.
- **Write once.** A sync never overwrites an existing `.md`; hand edits win. `aggr sync
  --refresh` is the explicit exception.
- **Deleted stays deleted.** Dedupe keys (`sha1` of the entry id, of the normalized link with
  tracking parameters removed, and of `title|published`) are appended to `seen.txt` when an item
  is written and are never removed, so deleting a file from the branch is final.
- **The raw `.html`** is stripped of `<script>`, `<style>`, inline SVG and `data:` URIs, capped
  at `[store] html_max_bytes`, and never served unsanitized.

## What a run does

```
fetch every source in parallel   →  write new items into the worktree
commit (if anything changed)     →  push, rebasing on rejection
update refs/aggr/last-good       →  only when every source succeeded
render the site from the pushed tip
post the digest (if due)         →  update refs/aggr/digest/<date>
```

- **No trace when nothing changed.** Conditional GET (`ETag` / `Last-Modified`) turns most
  fetches into a 304; a changed body with the same hash is treated the same. No new items and
  no status transition means no commit, no push, nothing. "Last updated" is the data tip's commit
  time; "last checked" is the build stamp.
- **Status transitions only.** `status.toml` is written when a source starts or stops failing,
  not on every failing run. One failing source never fails the run; only "every source failed",
  configuration errors and git/IO errors make `aggr sync` exit non-zero.
- **Push, never force.** A rejected push is followed by `git fetch` and `git rebase` onto
  `origin/aggr`. `seen.txt` files union-merge (`.gitattributes`), state files regenerate, so a
  laptop run and a CI run appending at the same time both land. A history that cannot be
  rebased is a hard error with instructions, never a force-push.

## Labels: trailers and refs

Every data commit carries structured trailers, so `git log` is the run log:

```
aggr: +12 items

rust-blog: +3
hn: +9
lobsters: error: 503 Service Unavailable

Aggr-Version: 1.0.0
Aggr-Config: 4f9c1a2…            # the main commit aggr.toml was read from
Aggr-Sources: 2 ok, 1 error
```

Subjects: `aggr: init` (first commit), `aggr: +N item(s)`, `aggr: status` (only a transition),
`aggr: update` (state only). `git log --grep='^aggr: +' --format=%s aggr` counts what arrived.

Refs are pointers with a meaning, visible in `git ls-remote`:

| Ref | Points to | Set when |
|---|---|---|
| `refs/aggr/last-good` | the data tip | a sync ended with zero source errors |
| `refs/aggr/digest/<yyyy-mm-dd>` | the data tip a digest was cut from | that day's digest issue was posted |

`aggr build --data-ref refs/aggr/last-good` reproduces the last fully healthy site; the number
of a digest is the number of `refs/aggr/digest/*` refs before its date plus one, and "since the
last digest" is the diff between those two commits.

## Reproducibility

Render-cache keys include the config files, data commit, theme, target URL, and binary renderer
sources. Given those inputs, `aggr build --data-ref <sha>` renders the same site anywhere.

## Editing the branch by hand

Everything on the branch is a text file, editable from GitHub's web editor:

- **Star or hide**: set `starred: true` or `hidden: true` in an item's front matter.
- **Delete**: remove the `.md` (and `.html`). It will not come back.
- **Fix a source**: `sources/<slug>/state.toml` may be deleted to force a full refetch; items
  already seen are still deduplicated through `seen.txt`.

Do not rebase, squash or force-push the branch. Retention (`[store] max_age_days`, `max_items`)
deletes files from the tree in a normal commit; history, and every permalink, stays intact.
