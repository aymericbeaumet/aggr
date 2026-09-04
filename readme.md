# aggr [![ci](https://github.com/aymericbeaumet/aggr/actions/workflows/ci.yml/badge.svg)](https://github.com/aymericbeaumet/aggr/actions/workflows/ci.yml)

Your own git-backed snapshot of the feeds you follow.

The web is full of great writing, but following it usually means renting your reading history from
an app. aggr takes a small `aggr.toml`, discovers feeds from ordinary website URLs, and saves each
new item once as readable Markdown plus stripped HTML when available on an append-only branch. It
then publishes a fast, self-contained static reader. There is no central aggr service, database,
account, or tracking layer: the repository and its history are yours.

Everyone builds an independent snapshot. Public instances expose the same discovery format, and
one instance can copy items from another, so useful parts of the web can survive in many ordinary
git repositories instead of one proprietary service. aggr's storage engine uses standard Git and
its output is static files, so the core is git-host and static-host agnostic. This project and its
turnkey workflow are hosted on GitHub because that is the clearest path for most users; the
provider-neutral path is documented below.

What you get:

- a responsive, installable PWA with a precached shell, home feed, and newest configured entries
  (30 by default); native pull-to-refresh where the platform provides it plus an always-available
  refresh control;
  one-minute deploy checks while running with catch-up checks on return or reconnect; tab-scoped
  new-item cues; and instant Swup navigation;
- build-time Pagefind full-text search over clean article prose, including best-effort lookup by a
  pasted original URL;
- a recent feed plus source, category, and `#tag` archives for every visible retained item;
- Atom, RSS 2.0, and JSON Feed streams for the reader and every source/category/tag;
- self-canonical snapshot pages, sitemaps, OpenSearch, a URL linkset, and an `aggr.json` instance
  descriptor for crawlers and other readers;
- original-page extraction by default, with the exact upstream URL and capture time kept as
  provenance;
- immutable item versions in append-only git history, for as long as that history is retained.

aggr is a readable-content archive, not a full-fidelity web crawler or WARC archive. It preserves
the extracted article or feed content, not scripts, styles, page chrome, response headers, or
downloaded media. If an original-page fetch fails, the saved item may contain feed content, a
summary, or only its metadata. These limits keep every stored and rendered page safe and small.

## Quick start on GitHub

The fastest start is the working
[aggr-instance repository](https://github.com/aymericbeaumet/aggr-instance):

1. **[Fork it](https://github.com/aymericbeaumet/aggr-instance/fork).**
2. Open the fork's **Actions** tab and enable workflows if GitHub asks.
3. Edit `aggr.toml` on the default branch (normally `main`).
4. Open **Actions → aggr → Run workflow** once.

The workflow enables GitHub Pages, appends fetched items to the orphan `aggr` branch, builds the
reader, and deploys it. Put the resulting Pages URL in the repository's About section and link it
where people can discover it. Scheduled workflows in a fork must remain enabled.

To add aggr to an existing GitHub repository, install the binary with either mise or Cargo:

```sh
mise use -g github:aymericbeaumet/aggr
# or
cargo install --git https://github.com/aymericbeaumet/aggr
```

Then, from the repository:

```sh
aggr init --github
$EDITOR aggr.toml
git add aggr.toml .github/workflows/aggr.yml
git commit -m "add aggr"
git push -u origin HEAD
```

The generated workflow runs every 30 minutes and after root or nested TOML configuration and theme
changes on the repository's actual default branch. Push events from other branches are ignored;
scheduled and manual runs remain available.

The installed reader attempts a deployment check once a minute while it is running and always
checks again when it returns to the foreground or reconnects; operating systems may suspend
background pages. A deployed update reloads automatically while preserving the reading position.
New-item highlights and the title/favicon dot use tab-local state that disappears when the tab
closes. Normal browser tabs request a new tab for external links; installed
apps hand out-of-scope links to the platform's external or in-app browser UI. Presentation varies
by operating system. Installation and service-worker caching require HTTPS (localhost is the
development exception). A static aggr site cannot wake a closed app with background push.

Private repositories need a GitHub plan that supports private Pages. To build without Pages and
keep the result as a workflow artifact, add the input to the generated job:

```yaml
jobs:
  aggr:
    uses: aymericbeaumet/aggr/.github/workflows/aggr.yml@v1
    with:
      pages: false
```

The `@v1` reference is a stable workflow contract. On every run it resolves the greatest published
`v1.x.y` binary, so compatible releases reach instances without a workflow edit. This reusable
workflow is deliberately a rolling major channel: pinning only its workflow tag does not also pin
the binary. A custom job can use the composite action at an immutable `@vX.Y.Z` tag with the same
exact `version` input when both pieces must be frozen.

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
conservative article discovery. In the default heavy content mode it attempts to download each
original page and extract its main article; use `content = "light"` on a source to trust its feed
content instead. The default first import considers the newest 100 entries from each feed, then
every later run adds newly observed entries.

Split a growing collection by including a TOML file as a source. Its position is preserved, and
the category on the entry becomes the default for sources that do not set their own:

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

Includes may be local paths/globs or direct HTTP(S) URLs. A bare GitHub repository URL finds its
`aggr.toml` automatically, and GitHub-hosted configs can use relative wildcards just like local
ones:

```toml
[[sources]]
include = "https://github.com/aymericbeaumet/aggr-instance"

[[sources]]
include = "https://github.com/owner/reader/blob/main/topics/*.toml"
category = "community"
```

Expansion is deterministic and cycle-safe: files and equivalent source endpoints are loaded once,
in declaration order, with the first declaration winning. Missing, malformed, and non-aggr targets
warn and are skipped. By default, a remote config may recurse only through relative paths on the
same HTTP origin—and, for GitHub URLs, within the same repository. The trusted root can opt into
broader chains with `[fetch] allow_remote_include_chains = true`. GitHub requests automatically use `GITHUB_TOKEN` or
`GH_TOKEN` when present; a private repository must grant that token read access.

Remote includes are live configuration dependencies, not part of the append-only data branch.
Pin their revision, or vendor them into the primary branch, if rebuilding the same site later
matters. Custom domains, network links, headers/secrets, PWA controls, retention, themes, and all
other options are documented directly in [`config.default.toml`](config.default.toml).

## Copy another aggr instance

An aggr instance can be a source for another one. The copied items keep their ultimate original
URLs and gain provenance pointing to the instance they came through. If both instances already
follow the same article, URL deduplication keeps one local copy.

GitHub repositories have a shorthand:

```toml
[[sources]]
type = "aggr"
repo = "friend/reads"
category = "friends"
```

For any public HTTP(S) Git remote, use its clone URL:

```toml
[[sources]]
type = "aggr"
url = "https://codeberg.org/friend/reads.git"
branch = "aggr"
category = "friends"
```

An aggr source copies every visible item retained in the other instance's current data tree by
default. Set `limit` only when you deliberately want the newest N items. Items that exist only in
older commits are not copied, so this is a useful content replica rather than a clone of the other
repository's complete history.

## Run on any Git and static host

The official releases live on GitHub, but the repository containing your config and data does not
have to. Initialize a repository on GitLab, Codeberg, Gitea, a self-hosted Git server, or another
standard Git service:

```sh
git clone https://git.example/YOU/READER.git
cd READER
aggr init
$EDITOR aggr.toml
git add aggr.toml
git commit -m "add aggr"
git push -u origin HEAD
```

Configure the provider's scheduler to run this from a normal Git checkout:

```sh
aggr build \
  --release \
  --base-url "https://reads.example.net/" \
  --out _site
```

Without `--data-ref`, `build` performs the sync first, including its commit and push, and then
renders `_site`; publish that directory with the provider's normal static-site step. The checkout
needs a writable remote named `origin`, and the server must accept `refs/heads/aggr` plus
`refs/aggr/last-good`. Set `[site] url` instead of passing `--base-url` when the public URL is
stable. Leave `[site] repository` unset outside GitHub: that field currently enables
GitHub-specific human blob/history/edit links and machine raw-config URLs only.

No GitHub API is involved in ordinary fetching, storage, rendering, or a direct HTTP(S) config
include. GitHub's reusable workflow, Pages deployment, repository shorthand, config wildcards,
and web permalinks are optional provider integrations around that portable core.

## Make a public snapshot discoverable

Release builds give every published, retained item an indexable page with a self-canonical URL. The
exact upstream URL remains visibly labelled as the original and is also expressed in structured
data, feeds, the Markdown and JSON representations, and the generated `linkset.json`. Keeping the
canonical on the local page is intentional: making the upstream page canonical would ask search
engines to discard the snapshot as a duplicate.

The sitemap exposes every published, retained item. `linkset.json` gives aggr-aware tools a direct
mapping from original URLs to local copies. Site search deliberately indexes exact and normalized
original URLs alongside article prose, so a pasted URL can find a local copy on a best-effort
basis. General search engines can still choose the live original—or another copy—as the
representative result. There is no central registry and no guarantee that an unlinked instance
will be crawled, so link the public reader and submit its
`sitemap.xml` through the search engines you care about.

For a public personal or organizational archive, add truthful ownership metadata rather than
making aggr guess an identity:

```toml
[site.identity]
type = "person" # or "organization"
name = "Your Name"
url = "https://example.com/about"
same_as = ["https://github.com/you"]
```

Retention changes what remains searchable on the live site. With the default unlimited store
retention, all published captured items stay in its archives and sitemap. If `[store] max_age_days` or
`max_items` removes one, its old git object remains reachable through append-only history but its
static article page is no longer published.

See [`docs/interoperability.md`](docs/interoperability.md) for the exact discovery and provenance
contract.

## Use the CLI

| Command | Purpose |
|---|---|
| `aggr init [--github] [--defaults]` | Write a minimal config, optionally the GitHub workflow; `--defaults` copies the full reference config. |
| `aggr sync [--fetch-only] [--dry-run]` | Fetch new items. Normally commit and push them; `--fetch-only` writes locally without either, while `--dry-run` writes nothing. |
| `aggr build [--release] [--out DIR] [--data-ref REF]` | Sync and render, or render a pinned data ref without source/discussion fetches, commits, or pushes. Matching renders are cached. |
| `aggr dev [--release] [--port 7319]` | Sync and build in an isolated system cache, serve entirely from memory, watch dependencies, and live-reload. It never commits, pushes, or pollutes the repository. |
| `aggr check` | Validate the config and probe every source. |
| `aggr completions <SHELL>` | Generate shell completions. |

Build uses a repository-local cache; dev uses a separate OS-standard cache keyed by the canonical
config path. Repeated runs are designed to become nearly instant without mixing CI/build and local
development state.

## Git is the database

Your primary branch keeps only the config, optional provider workflow, and optional theme. The
unrelated `aggr` branch stores Markdown, stripped HTML, source validators, and append-only dedupe
keys. A no-op sync creates no commit, one broken source does not block healthy sources, and the data
branch is never force-pushed. Deleting or retaining items creates ordinary commits, so versions in
older reachable commits remain intact.

The precise branch, ref, recovery, concurrency, and hand-editing contract is in
[`docs/git-model.md`](docs/git-model.md).

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
