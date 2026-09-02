# aggr data branch

This branch is the database of an [aggr](https://github.com/aymericbeaumet/aggr) site. It is
append-only and never rewritten, so every commit stays reachable and every
`blob/<commit>/items/…` URL keeps working forever.

```
items/<source>/<yyyy>/<mm>/<yyyy-mm-dd>-<slug>.md     one item: YAML front matter + Markdown
items/<source>/<yyyy>/<mm>/<yyyy-mm-dd>-<slug>.html   the raw HTML that Markdown was derived from
sources/<source>/state.toml                           upstream title, ETag, Last-Modified, body hash
sources/<source>/seen.txt                             dedupe keys of everything ever stored
status.toml                                           sources currently failing (absent when all is well)
```

## Editing by hand

- **Star or hide an item**: edit its `.md` and set `starred: true` or `hidden: true` in the front
  matter. Fetches never overwrite an existing file.
- **Delete an item**: delete the `.md` (and `.html`). Its keys stay in `seen.txt`, so it will not
  come back.
- **Rename a source**: items live under the source slug, so set `slug` in `aggr.toml` before the
  first fetch if you care about the directory name.

Do not force-push or rebase this branch: permalinks pinned to old commits would break.
