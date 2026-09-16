# aggr [![ci](https://github.com/aymericbeaumet/aggr/actions/workflows/ci.yml/badge.svg)](https://github.com/aymericbeaumet/aggr/actions/workflows/ci.yml)

Your own git-backed snapshot of the feeds you follow.

Point aggr at the websites you read. It discovers their feeds, saves every new article once as
readable Markdown on an append-only git branch, and publishes a fast, searchable, installable
static reader. No service, no database, no account, no tracking: the repository and its history
are yours.

## Try it in two minutes

[Fork the working instance](https://github.com/aymericbeaumet/aggr-instance/fork), then:

1. Open the fork's **Actions** tab and enable workflows if GitHub asks.
2. Edit `aggr.toml` on the default branch.
3. Open **Actions → aggr → Run workflow**.

That's it. The workflow enables GitHub Pages, appends fetched items to the orphan `aggr` branch,
builds the reader, and deploys it. Scheduled workflows in a fork must remain enabled.

Prefer to run it locally first?

```sh
mise use -g github:aymericbeaumet/aggr   # or grab a binary from Releases
mkdir reads && cd reads && git init
aggr init
aggr dev                                  # http://127.0.0.1:7319
```

## Write your reading list

A website URL is enough — no need to hunt for the feed:

```toml
[site]
title = "My reads"

[[sources]]
category = "programming"
url = """
https://blog.rust-lang.org
https://martinfowler.com
https://simonwillison.net
https://www.youtube.com/@ThePrimeTimeagen
"""

[[sources]]
category = "news"
url = "https://hnrss.org/frontpage?points=100"
```

aggr tries the URL as a feed, follows feed-discovery metadata, probes the conventional endpoints,
and falls back to reading the page's article list. Podcast and video show pages (Apple Podcasts,
Spotify, Deezer, YouTube, SoundCloud, and more) resolve to the publisher's real RSS. OPML files,
URL lists, other aggr instances, and other `aggr.toml` files all work as sources too.

Options like `name`, `labels`, `images`, and `previews` go in the same table. See
[configuring sources](docs/sources.md) and the commented
[`config.default.toml`](config.default.toml), which is the complete reference.

## What you get

- a responsive, installable PWA with unified search, comfortable typography, keyboard navigation,
  and a precached shell; updates arrive without interrupting reading;
- build-time full-text search over clean article prose, including lookup by a pasted original URL;
- a recent feed plus source, category, and `#tag` archives, each with Atom, RSS, and JSON Feed;
- original-page extraction by default, keeping the upstream URL and capture time as provenance;
- local, lossless copies of safe article images, with responsive renditions and instant previews;
- selectable offline reading of the newest articles, images included;
- immutable item versions in append-only git history, for as long as that history is retained.

aggr preserves readable article or feed content rather than a complete copy of the original
website. Scripts, styles, page chrome, and response headers are excluded. If an original-page fetch
fails, the saved item may contain feed content, a summary, or only its metadata.

Everyone builds an independent snapshot. Public instances expose the same discovery format, and
one instance can copy items from another, so useful parts of the web can survive in many ordinary
git repositories instead of one proprietary service.

## Commands

| Command | Purpose |
|---|---|
| `aggr init [--github] [--defaults]` | Write a minimal config, optionally the GitHub workflow; `--defaults` copies the full reference config. |
| `aggr sync [--fetch-only] [--dry-run] [--refresh] [--reprocess]` | Fetch new items. Normally commit and push them; `--fetch-only` writes locally without either, while `--dry-run` writes nothing. |
| `aggr build [--release] [--out DIR] [--data-ref REF]` | Sync and render, or render a pinned data ref without fetches, commits, or pushes. |
| `aggr dev [--release] [--port 7319]` | Sync and build in an isolated cache, serve from memory, watch, and live-reload. Never commits or pushes. |
| `aggr clean [--dry-run] [--out DIR]` | Remove disposable dev state, build cache, and owned output. `--dry-run` lists exact targets. |
| `aggr check` | Validate the config and probe every source. |
| `aggr completions <SHELL>` | Generate shell completions. |

`--reprocess` re-derives stored bodies from the HTML retained beside them, so content cleanup
added since an item was captured reaches the archive without refetching. Use it after upgrading.

Build uses a repository-local cache; dev uses a separate OS-standard cache keyed by the config
path, so repeated runs are nearly instant. Cleanup only ever touches targets it can prove
disposable: archived articles, git refs and history, and hand-made files are never removed.

## Git is the database

Your primary branch keeps only the config, optional provider workflow, and optional theme. The
unrelated `aggr` branch stores Markdown, stripped HTML, source validators, and append-only dedupe
keys. A no-op sync creates no commit, one broken source does not block healthy sources, and the
data branch is never force-pushed. Deleting or retaining items creates ordinary commits, so
versions in older reachable commits remain intact.

The precise branch, ref, recovery, concurrency, and hand-editing contract is in
[the git model](docs/git-model.md).

## Documentation

| | |
|---|---|
| [Configuring sources](docs/sources.md) | source syntax, discovery, podcasts, images, previews, collections, mirroring |
| [The reader](docs/reading.md) | search, sharing a passage, preferences, offline, keyboard |
| [Hosting](docs/hosting.md) | GitHub Pages, scheduling, other git/static hosts, discoverability |
| [Themes](docs/themes.md) | template contract and reader behaviour |
| [Git model](docs/git-model.md) | branch, ref, recovery, and hand-editing contract |
| [Interoperability](docs/interoperability.md) | discovery, provenance, and preservation boundaries |
| [Performance](docs/performance.md) | fetch/build pipeline limits and caches |
| [Client development](docs/client.md) | frontend commands, ownership, and search contracts |

## Contributing

```sh
npm ci --prefix web
make check
cargo run -- dev --config examples/aggr.toml
```

A theme is a `templates/` plus `static/` directory rendered with
[MiniJinja](https://github.com/mitsuhiko/minijinja); project-local files override the embedded
default, so changing one template does not require copying the rest. The reader uses Svelte,
TypeScript, and Vite, while Rust generates the complete static site. Frontend contributors rebuild
the committed assets with `make client-build`, or use `make client-dev` with
`AGGR_VITE_URL=http://127.0.0.1:5173` for live updates. Normal Cargo builds and published binaries
use embedded assets and require no Node runtime.

The browser regression suite uses a local ChromeDriver and a temporary, pinned article archive:

```sh
chromedriver --port=9515 --allowed-ips=127.0.0.1
# In another terminal:
AGGR_WEBDRIVER_URL=http://127.0.0.1:9515 cargo test --test browser -- --ignored
```

Set `AGGR_CHROME_BINARY` if Chrome is outside its usual location. CI installs matching browser
and driver versions; failure screenshots and logs are saved under `target/browser-artifacts/`.

Issues and pull requests are welcome. If aggr improves your reading workflow, star the repository
and share your reader—the easiest way for someone else to begin is often to fork one that already
works.

MIT licensed.
