# Commands

| Command | Purpose |
|---|---|
| `aggr init [--github] [--defaults]` | Write the small starter config, optionally the GitHub workflow; `--defaults` copies the full reference config instead. |
| `aggr sync [--fetch-only] [--dry-run] [--refresh]` | Fetch new items. Normally commit and push them; `--fetch-only` writes locally without either, while `--dry-run` writes nothing. |
| `aggr build [--out DIR] [--data-ref REF]` | Sync and render, or render a pinned data ref without fetches, commits, or pushes. |
| `cargo run -- dev [--port 7319]` | Start the native contributor stack, sync into an isolated cache, and preview pages; never commits or pushes. |
| `aggr clean [--dry-run] [--out DIR]` | Remove disposable dev state, build cache, and owned output. `--dry-run` lists exact targets. |
| `aggr storage inspect [--json]` | Report current archive, local media, generated output, cache and Git object bytes without changing them. |
| `aggr storage prune-renditions [--dry-run\|--apply]` | Verify originals, then explicitly remove optional archived renditions in an ordinary commit. Dry run is the default. |
| `aggr check` | Validate the config and probe every source. |
| `aggr completions <SHELL>` | Generate shell completions. |

Run `aggr <command> --help` for its full options. Global `--config PATH` selects a configuration;
`-v` and `-vv` increase logging.

## Starting small

`aggr init` copies [the starter configuration](../examples/starter.toml): two sources using feed
content and remote media. `[defaults]` bounds each feed to 250 retained items, 730 days and
1,000,000,000 bytes, including companions. Source tables can override individual limits; zero
disables a numeric limit. `aggr init --defaults` copies
[`config.default.toml`](../config.default.toml) with all general defaults instead. See
[source configuration](sources.md#capture-defaults-and-per-source-limits).

`[site] build_max_bytes` defaults to 1,000,000,000 bytes. Article text takes priority over local
media; `media_full_quality_days = 30` controls when published images are compressed. These
settings affect publication, never the archived originals. See [build budgets](build-budget.md)
and [storage measurements](benchmarks.md).

## Refreshing and reprocessing

`--refresh` refetches items still listed by configured sources using the current capture policies
and fills missing media companions. It can replace stored bodies and discard hand edits; use
`--backfill-media` when only media should change.

Sync automatically re-derives retained article bodies from their complete local HTML when the
extraction implementation, dependencies, or stored inputs change. This includes the sync before
ordinary builds and development runs, and requires no additional requests. It skips missing or
truncated HTML. Reprocessing can replace hand-edited bodies; unchanged extraction inputs and
outputs write nothing. Disposable cache receipts remember processed inputs without adding metadata
or commits to the data branch. Pinned and offline builds only render their existing stored bodies.

## Local development

`mise run dev` and `cargo run -- dev` start the native compiler and preview supervisor.
`cargo run --release -- dev` uses the same behavior with an optimized executable. With no local
configuration, `dev` tries the embedded starter in an isolated cache; set `AGGR_CONFIG` to preview
another instance. Installed binaries serve their embedded reader. `aggr dev --no-watch` selects
that frozen preview in a source checkout; `aggr dev --demo` needs no configuration or repository.

The preview prepares shared archive metadata, then renders pages, media and search on demand.
UI and template edits compile with Rolldown and reload the page without restarting Rust. Rust edits
compile in the background while the previous worker serves requests; successful replacements
reload the browser. Failed compilations preserve the last working generation. Data and search
caches survive UI edits and worker restarts. Nothing is committed or pushed.

Development uses the same renderer, assets and configured PWA behavior as publication. The preview
adds an HTTP noindex header and a reload stream, preserving canonical page bytes and offline hashes.
Large offline catalogues are prepared once on demand. `aggr build` materializes the complete site and
checks its publication budget. See [the native toolchain](native-toolchain.md) for details.

`mise run check` runs the Rust, type and browser unit checks. See [client development](client.md).

## Cache and cleanup

Build uses a repository-local cache; dev uses a separate OS-standard cache keyed by the config
path. Cleanup touches only targets it can prove disposable: archived articles, Git refs and
history, and hand-made files are never removed. Start with `aggr clean --dry-run` to inspect them.

Media and response caches have configurable byte caps. `aggr storage inspect` shows their current
size; [storage](storage.md) documents the caps and the explicit rendition migration.

The per-feed limits in `[defaults]` and `[[sources]]` control retention separately from cleanup.
They remove articles and companions from the current data tree through ordinary commits without
reclaiming the bytes in append-only Git history. See the
[Git model](git-model.md).
