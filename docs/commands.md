# Commands

| Command | Purpose |
|---|---|
| `aggr init [--github] [--defaults]` | Write the small starter config, optionally the GitHub workflow; `--defaults` copies the full reference config instead. |
| `aggr sync [--fetch-only] [--dry-run] [--refresh] [--reprocess]` | Fetch new items. Normally commit and push them; `--fetch-only` writes locally without either, while `--dry-run` writes nothing. |
| `aggr build [--release] [--out DIR] [--data-ref REF]` | Sync and render, or render a pinned data ref without fetches, commits, or pushes. |
| `aggr dev [--release] [--port 7319]` | Sync and build in an isolated cache, serve from memory, watch, and live-reload. Never commits or pushes. |
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

## Cache and cleanup

Build uses a repository-local cache; dev uses a separate OS-standard cache keyed by the config
path. Cleanup touches only targets it can prove disposable: archived articles, Git refs and
history, and hand-made files are never removed. Start with `aggr clean --dry-run` to inspect them.

`[store]` retention is different from cleanup. It removes articles from the current data tree
through ordinary commits; it does not reclaim the bytes in append-only Git history. See the
[Git model](git-model.md).
