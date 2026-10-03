# Commands

| Command | Purpose |
|---|---|
| `aggr init [--github] [--defaults]` | Write the small starter config, optionally the GitHub workflow; `--defaults` copies the full reference config instead. |
| `aggr sync [--fetch-only] [--dry-run] [--refresh] [--reprocess]` | Fetch new items. Normally commit and push them; `--fetch-only` writes locally without either, while `--dry-run` writes nothing. |
| `aggr build [--release] [--out DIR] [--data-ref REF]` | Sync and render, or render a pinned data ref without fetches, commits, or pushes. |
| `aggr dev [--release] [--port 7319]` | Sync into an isolated cache, render requested pages, watch, and live-reload. Never commits or pushes. |
| `aggr clean [--dry-run] [--out DIR]` | Remove disposable dev state, build cache, and owned output. `--dry-run` lists exact targets. |
| `aggr check` | Validate the config and probe every source. |
| `aggr completions <SHELL>` | Generate shell completions. |

Run `aggr <command> --help` for its full options. Global `--config PATH` selects a configuration;
`-v` and `-vv` increase logging.

## Starting small

`aggr init` copies [the starter configuration](../examples/starter.toml): two sources, at most ten
recent entries considered per source per run. Image archiving and unlimited store retention
remain enabled. `aggr init --defaults` copies [`config.default.toml`](../config.default.toml)
with all general defaults instead.

`[site] build_max_bytes` defaults to 1,000,000,000 bytes. Article text takes priority over local
media; `media_full_quality_days = 30` controls when published images are compressed. These
settings affect publication, never the archived originals. See [build budgets](build-budget.md)
and [storage measurements](benchmarks.md).

## Refreshing and reprocessing

`--refresh` fills missing media companions on existing articles. `--reprocess` re-derives stored
bodies from the HTML retained beside them, so content cleanup added since capture reaches the
archive without refetching. This pass covers all retained articles, including renamed or removed
sources, and skips truncated HTML. It runs once before the normal configured-source sync; repeating
it without content changes writes nothing. Hand-edited bodies are replaced. Use it after upgrading
when you want those extraction changes applied.

## Local development

`aggr dev` prepares the archive's shared display metadata, then renders each page only when it is
requested. Opening the feed reads its thumbnails; opening an article loads that article's media.
Completion uses a small metadata catalogue; the full-text index is built only when a search needs
its runtime, and reuses the persistent Pagefind cache.
Template, CSS and JavaScript edits reuse normalized bodies and parsed Markdown, replace the current
snapshot atomically, and trigger a browser reload. A template syntax error keeps the last working
snapshot available. Requested output lives in disposable per-generation directories; the archive
and fetch caches survive restart, and nothing is committed or pushed.

Use `aggr dev --release` to exercise the complete publication build, including its media budget
and PWA outputs. Ordinary lazy dev omits PWA outputs; the dev server unregisters its scoped worker
to keep browser caches from hiding edits.
For client code, run `make client-dev` alongside dev with `AGGR_VITE_URL` set; `make check` runs
the Rust and frontend checks. See [client development](client.md).

## Cache and cleanup

Build uses a repository-local cache; dev uses a separate OS-standard cache keyed by the config
path. Cleanup touches only targets it can prove disposable: archived articles, Git refs and
history, and hand-made files are never removed. Start with `aggr clean --dry-run` to inspect them.

`[store]` retention is different from cleanup. It removes articles from the current data tree
through ordinary commits; it does not reclaim the bytes in append-only Git history. See the
[Git model](git-model.md).
