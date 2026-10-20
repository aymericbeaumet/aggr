# Commands

| Command | Purpose |
|---|---|
| `aggr init [--github] [--defaults]` | Write the small starter config, optionally the GitHub workflow; `--defaults` copies the full reference config instead. |
| `aggr sync [--fetch-only] [--dry-run] [--refresh]` | Fetch new items. Normally commit and push them; `--fetch-only` writes locally without either, while `--dry-run` writes nothing. |
| `aggr build [--release] [--out DIR] [--data-ref REF]` | Sync and render, or render a pinned data ref without fetches, commits, or pushes. |
| `cargo run -- dev [--release] [--port 7319]` | Start the Node-managed contributor stack, sync into an isolated cache, and preview pages. Debug builds only; never commits or pushes. |
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

`cargo run -- dev` starts Vite HMR and automatic Rust restarts. `cargo run --release -- dev` also
works. With no local configuration, `dev` tries the embedded two-source starter in an isolated
cache; set `AGGR_CONFIG=/path/to/aggr.toml` to preview another instance.
Node and npm are source-build tools: Cargo compiles and embeds the reader automatically.
Installed release binaries include `dev` and serve the embedded reader without Node. Use
`aggr dev --native` to choose that mode in a source checkout, or `aggr dev --demo --native` to
try the product without creating a configuration or Git repository in your current directory.

The Rust preview backend prepares the archive's shared display metadata, then renders each page only when it is
requested. Opening the feed reads its thumbnails; opening an article loads that article's media.
Completion uses a small metadata catalog; the full-text index is built only when a search needs
its runtime, and reuses the persistent Pagefind cache.
Vite handles Svelte, TypeScript and CSS updates. Rust changes compile while the previous worker
continues serving. During replacement preparation, the public server retains complete responses
for recently visited pages and assets, then reloads after successful startup. Saves that supersede
an in-flight compilation wait for the latest build instead of promoting intermediate workers.
Completed sources become visible while slower remote sources continue; local reprocessing also
publishes before network refresh completes. Each preview owns immutable archive inputs. A template syntax error keeps the last working
snapshot available. Requested output lives in disposable per-generation directories; the archive
and fetch caches survive restart, and nothing is committed or pushed.

Use `cargo run -- dev --release` to exercise the complete publication build, including its media budget
and PWA outputs. Ordinary lazy dev omits PWA outputs; the dev server unregisters its scoped worker
to keep browser caches from hiding edits.
`mise run dev` is a shortcut for the same contributor stack.
`mise run check` runs the Rust and frontend checks. See [client development](client.md).

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
