# aggr [![ci](https://github.com/aymericbeaumet/aggr/actions/workflows/ci.yml/badge.svg)](https://github.com/aymericbeaumet/aggr/actions/workflows/ci.yml)

Your feeds, compiled into a fast website and kept forever in git.

The web is full of great writing, but following it usually means renting your reading history from
an app. aggr takes a small `aggr.toml`, discovers feeds from ordinary website URLs, stores new
items as Markdown on an append-only branch, and publishes a self-contained static reader. There is
no server, database, account, or tracking layer to maintain.

`aggr` is the engine. An **aggr instance** is a repository containing its workflow and root aggr
config, together with the data branch and site they produce. Instances are independent and publish
a common network identity, making every publicly linked instance recognizable to search engines
and other tools.

What you get:

- a responsive, installable PWA with offline reading and instant Swup navigation;
- build-time Pagefind full-text search over clean article prose;
- a recent feed plus complete source, category, and `#tag` archives;
- Atom, RSS 2.0, and JSON Feed streams for the reader and every source/category/tag;
- self-describing HTML, OpenSearch, sitemaps, and an `aggr.json` network descriptor so browsers,
  crawlers, and other readers can plug in;
- original-page extraction by default, with a lightweight feed-content mode when preferred;
- permanent `github.com/…/blob/<commit>/…` Markdown URLs for every saved item;

The fastest start is the working
[aggr-instance repository](https://github.com/aymericbeaumet/aggr-instance):
**[fork it](https://github.com/aymericbeaumet/aggr-instance/fork)**, enable Actions, edit
`aggr.toml`, and run the workflow. Your reader can be online in seconds. Any public reader can
then serve as a useful starting point for the next person.

## Start from scratch

Install the binary with either mise or Cargo:

```sh
mise use -g github:aymericbeaumet/aggr
# or
cargo install --git https://github.com/aymericbeaumet/aggr
```

Then initialize any GitHub repository:

```sh
aggr init --github
git add aggr.toml .github/workflows/aggr.yml
git commit -m "add aggr"
git push
```

Those are the only two files needed. The generated workflow runs on a schedule and after config
changes, appends new items to the orphan `aggr` branch, builds the site, and deploys it with GitHub
Pages. Forks must enable scheduled workflows once in the Actions tab. Private repositories need a
GitHub plan that supports private Pages; `pages: false` keeps the result as an Actions artifact.

The `@v1` reference is a stable workflow contract. On every run it resolves the greatest published
`v1.x.y` binary, so compatible engine releases reach every instance without changing its workflow.
Use an immutable `@vX.Y.Z` tag and exact `version` input only when you deliberately want to freeze
both pieces.

## Configure sources

Only write values you want to change. The commented
[`config.default.toml`](config.default.toml) is the complete configuration reference and the single
source of truth for every default.

An ordinary website URL is usually enough:

```toml
[site]
title = "My reads"

[[sources]]
url = "https://blog.rust-lang.org/"
category = "programming"
labels = ["rust", "language"]
```

Each item has at most one category and any number of labels. aggr tries the URL as a feed, follows
RSS/Atom/JSON Feed discovery metadata, probes conventional endpoints, and finally falls back to
conservative article discovery. In the default heavy content mode it downloads each original page
and extracts the main article with a Readability-style parser; use `content = "light"` on a source
to trust its feed content instead.

Split a growing collection by including a TOML file as a source. Its position is preserved,
and the category on the entry becomes the default for sources that do not set their own:

```toml
# aggr.toml
[[sources]]
include = "./aggr-ai.toml"
category = "ai"
```

```toml
# aggr-ai.toml
[[sources]]
url = "https://www.anthropic.com/news"

[[sources]]
url = "https://openai.com/news/"
category = "research" # an explicit category wins
```

Includes may be local paths/globs or remote aggr configs. A bare GitHub repository URL finds its
`aggr.toml` automatically, and GitHub-hosted configs can use relative wildcards just like local
ones:

```toml
[[sources]]
include = "https://github.com/aymericbeaumet/aggr-instance"

[[sources]]
include = "https://github.com/owner/reader/blob/main/topics/*.toml"
category = "community"
```

Imports are deterministic and cycle-safe: files and equivalent source endpoints are loaded once,
in declaration order, with the first declaration winning. Missing, malformed, and non-aggr targets
warn and are skipped. By default, a remote config may recurse only through relative paths inside
its own repository. The trusted root can opt into broader chains with
`[fetch] allow_remote_include_chains = true`. GitHub requests automatically use `GITHUB_TOKEN` or
`GH_TOKEN` when present; a private repository must grant that token read access.

Custom domains, network links, headers/secrets, PWA controls, retention, themes, and all other
options are documented directly in [`config.default.toml`](config.default.toml).

## Use the CLI

| Command | Purpose |
|---|---|
| `aggr init [--github] [--defaults]` | Write a minimal config, optionally the workflow; `--defaults` copies the full reference config. |
| `aggr sync [--fetch-only] [--dry-run]` | Fetch new items. Normally commit and push them; `--fetch-only` writes locally without either, while `--dry-run` writes nothing. |
| `aggr build [--release] [--out DIR]` | Sync, then build the static site. Exact build inputs are cached. |
| `aggr dev [--release] [--port 7319]` | Sync and build in an isolated system cache, serve entirely from memory, watch dependencies, and live-reload. It never commits, pushes, or pollutes the repository. |
| `aggr check` | Validate the config and probe every source. |
| `aggr completions <SHELL>` | Generate shell completions. |

`aggr sync` remains callable on its own and is also the required first stage of `build` and `dev`.
Build uses a repository-local cache; dev uses a separate OS-standard cache keyed by the canonical
config path. Repeated runs are designed to become nearly instant without mixing CI/build and local
development state.

## Git is the database

`main` keeps only your config, workflow, and optional theme. The unrelated `aggr` branch stores
small Markdown/HTML files, source validators, and append-only dedupe keys. A no-op sync creates no
commit, one broken source does not block healthy sources, and the data branch is never force-pushed.
Deleting or retaining items creates ordinary commits, so old blob URLs remain valid.

The precise branch, ref, dedupe, concurrency, and hand-editing contract is in
[`docs/git-model.md`](docs/git-model.md).

The generated routes, feed identity, discovery metadata, and subpath-portability contract are in
[`docs/interoperability.md`](docs/interoperability.md).

## Customize and contribute

A theme is a `templates/` plus `static/` directory rendered with
[MiniJinja](https://github.com/mitsuhiko/minijinja). Project-local files override a selected theme,
which overrides the embedded default, so changing one template does not require copying the rest.

For development:

```sh
make check
cargo run -- dev --config examples/aggr.toml
```

Issues and pull requests are welcome. If aggr improves your reading workflow, star the repository
and share your reader—the easiest way for someone else to begin is often to fork one that already
works.

MIT licensed.
