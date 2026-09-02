# aggr [![ci](https://github.com/aymericbeaumet/aggr/actions/workflows/ci.yml/badge.svg)](https://github.com/aymericbeaumet/aggr/actions/workflows/ci.yml)

Git-native feed reader. `aggr.toml` in, static site out, history on a branch.

Created and maintained by [Aymeric Beaumet](https://github.com/aymericbeaumet). If aggr earns a
place in your reading workflow, [star it on GitHub](https://github.com/aymericbeaumet/aggr) and
share your reader—each public reader is also a ready-to-fork starting point for the next one.

A GitHub repository with two files becomes a reader: a workflow fetches your feeds every half
hour, appends every new item as a Markdown file to an append-only branch, renders a static site
from that branch, and publishes it on free GitHub Pages. Every item has a permanent
`github.com/…/blob/<commit>/…` URL, a run that finds nothing new leaves no trace, and the daily
digest arrives as a GitHub issue (GitHub emails it to you). No server, no database, no account
anywhere but GitHub.

See it live at <https://aggr.aymericbeaumet.com>. Its complete setup is the deliberately small
[aggr starter repository](https://github.com/aymericbeaumet/aggr.aymericbeaumet.com): one config
and one workflow, with real RSS, Atom and HTML-scraped sources.

## Install: two files

The quickest start is to **[fork the working starter](https://github.com/aymericbeaumet/aggr.aymericbeaumet.com/fork)**.
Edit its single `aggr.toml`, enable workflows in the fork's Actions tab, run `aggr` once, and your
reader is live at `https://<you>.github.io/<repo>/`. No build tooling or local installation is
required. Keep your fork public and its own visitors can fork it in turn.

From scratch, in any repository:

```sh
aggr init --github      # writes aggr.toml and .github/workflows/aggr.yml
git add -A && git commit -m "aggr" && git push
```

Or write them by hand:

```toml
# aggr.toml
[site]
title = "My reads"

[digest]
at = "08:00"

[[sources]]
url = "https://blog.rust-lang.org/feed.xml"
category = "rust"
```

```yaml
# .github/workflows/aggr.yml
name: aggr
on:
  schedule: [{ cron: "*/30 * * * *" }]
  push: { branches: [main], paths: ["*.toml", themes/**, templates/**, static/**] }
  workflow_dispatch:
permissions: { contents: write, pages: write, id-token: write, issues: write }
jobs:
  aggr:
    uses: aymericbeaumet/aggr/.github/workflows/aggr.yml@v1
```

The first run creates the `aggr` branch, switches Pages to "GitHub Actions" and deploys to
`https://<user>.github.io/<repo>/`. Then it runs on the cron, whenever `aggr.toml` (or a theme
file) is pushed, and on demand from the Actions tab.

Good to know:

- Pushes to the data branch never retrigger the workflow (bot pushes do not fire `on: push`).
- The bot commits keep the repository active, so GitHub does not disable the cron after 60 days.
- Pages on a **private** repository needs GitHub Pro/Team; the branch and `aggr build` work
  anywhere. Pass `with: { pages: false }` to keep the site as a run artifact instead.
- A run takes about a minute. Every 30 minutes is ~1,500 minutes a month; on a private repository
  under the free plan, use `0 * * * *`.
- Forks must enable scheduled workflows manually.
- If enabling Pages fails on the first run, enable it once in Settings → Pages (source: GitHub
  Actions) and re-run.
- A custom domain is set in Settings → Pages; the workflow builds for whatever URL Pages reports.
  For other hosts, set `[site] url` and `aggr build --release` writes the matching `CNAME`.

The reusable workflow takes inputs `config`, `version`, `pages`, `digest`. To compose your own
pipeline (deploy `_site/` somewhere else, add steps), use the install action directly:

```yaml
- uses: aymericbeaumet/aggr@v1
- run: aggr build --release --base-url https://reads.example.com/
```

## The binary

```sh
brew install aymericbeaumet/tap/aggr          # macOS / Linux
mise use -g github:aymericbeaumet/aggr        # prebuilt binaries, any platform
cargo install --git https://github.com/aymericbeaumet/aggr
```

Release archives for Linux, macOS and Windows (amd64, arm64) with `SHA256SUMS` are on the
[releases page](https://github.com/aymericbeaumet/aggr/releases).

| Command | What it does |
|---|---|
| `aggr init [--github] [--defaults]` | Write a commented `aggr.toml` (every option with `--defaults`), plus the workflow with `--github`. |
| `aggr sync [--dry-run]` | Fetch every source, write new items into the data worktree, commit, push, update `refs/aggr/last-good`. `build` invokes this first. |
| `aggr fetch` | `sync` without the commit and push. |
| `aggr build [--release] [--out dir] [--base-url url] [--data-ref ref]` | Sync, then render the site. `--release` builds for `[site] url` (or `--base-url`) and writes `CNAME`; `--data-ref` renders any data commit after syncing. |
| `aggr dev [--port 7319]` | Sync, build, serve, and live-reload from an ephemeral workspace; it never commits, pushes, or writes data/build output into the repository. |
| `aggr digest [--dry-run] [--force]` | Post today's digest issue when it is due. |
| `aggr check` | Validate the configuration and probe every source. |
| `aggr completions <shell>` | Shell completions. |

Locally, `aggr dev` is the whole loop. It serves an atomic in-memory site snapshot; private build,
store, and cache staging lives in an OS temporary directory that is removed on exit. Iterating from
`aggr.toml` therefore leaves the repository untouched. `aggr sync` and `aggr build` use the
persistent data worktree by design.

## Configuration

Only what differs from the defaults needs to be in `aggr.toml`; unknown keys are errors.
[`config.default.toml`](config.default.toml) lists every option with its default and a comment.

```toml
include = ["aggr-*.toml"]          # topic files: sources only, optionally one `category` for all

[site]
title = "My reads"
timezone = "Europe/Paris"          # digest schedule, date display
theme = "themes/mine"              # a directory with templates/ and static/; "default" otherwise
# url = "https://reads.example.com"  # custom domain: build target and CNAME

[fetch]
content = "heavy"                  # default: fetch + clean original pages; "light" trusts feeds

[digest]
at = "08:00"                       # one GitHub issue a day, when something is new

[[sources]]
url = "https://blog.rust-lang.org/feed.xml"
name = "Rust Blog"                 # display name; the feed's title when unset
category = "rust"

[[sources]]
url = "https://api.example.com/feed"
headers = { Authorization = "Bearer ${API_TOKEN}" }   # ${VAR} expands from the environment

[[sources]]
type = "html"                      # no feed? scrape the listing page with CSS selectors
url = "https://cognition.com/blog"
items = "li > a[href^='/blog/']"   # one match per entry
title = "h2"                       # `css` = text, `css@attr` = attribute, `@attr` = the entry's own
link = "@href"
date = "span"
date_format = "%m.%d.%y"           # only needed when the date is ambiguous
summary = "p"

[[sources]]
type = "aggr"                      # another aggr repository is a source too
repo = "friend/reads"
category = "friends"
```

```toml
# aggr-ai.toml — an included topic file
category = "ai"

[[sources]]
url = "https://simonwillison.net/atom/everything/"
name = "Simon Willison"
```

Any RSS, Atom or JSON Feed URL is a source. Sites that speak feeds already: YouTube channels,
GitHub releases (`…/releases.atom`), Reddit (`…/.rss`), Mastodon (`https://<instance>/@user.rss`),
Bluesky (`https://bsky.app/profile/<handle>/rss`), Hacker News through [hnrss.org](https://hnrss.org),
Lobsters (`https://lobste.rs/rss`), arXiv, Substack, Medium. For the rest there is `type = "html"`;
`aggr check` prints how many entries the selectors find and how many carry a date.

## The site

Hacker News density, a periwinkle (`#8ea1ff`) palette, responsive layout, and light, dark or
automatic color mode. The feed is paginated and sorted by the source's creation date, newest
first. Every source, category and tag has a generated collection page under `/sources/`,
`/categories/` and `/tags/`. An item has one category and any number of labels. Full-text search
is precomputed at build time by [Pagefind](https://pagefind.app/) and runs instantly over titles,
article text, categories and labels without downloading the whole archive;
Atom (`atom.xml` and the compatibility `feed.xml`), RSS 2.0 (`rss.xml`) and JSON Feed 1.1
(`feed.json`) re-syndicate the aggregate. Every category and tag page emits the same three
formats from its own directory. Keyboard: `j`/`k` move, `o` opens the original, and `Enter`
opens the item page.

Immutable CSS, JavaScript and image assets carry content hashes. Every internal HTML link is
relative to the generated page, so the same self-contained output can be mounted at `/`,
`/a/b/c/`, or any other HTTP path without rebuilding.

All browseable routes are rendered ahead of time. The vendored, MIT-licensed
[Swup](https://swup.js.org/) navigation layer swaps those server-rendered pages, preserves browser
history, and caches visits; it progressively falls back to ordinary links without JavaScript.

Theme state stays in `localStorage` without changing normal URLs. **copy state** under `/settings/`
copies a one-time URL containing every local key/value; opening it imports the state and cleans
the payload from the address bar. Hacker News discussion search is on by default; Reddit, X, or
any other service can be added with `[[site.discussions]]` URL templates using `{url}` and
`{title}`.

It is a **PWA**: "Install" it from the browser menu and it opens as an app with its own icon
and a search shortcut. A service worker precaches the feed, the lists and
the newest `[site] offline_items` item pages at every build, and remembers whatever you open,
so the reader works on the train; an "Updated — reload" toast appears when a new build is
live. `pwa = false` turns it off (an installed worker unregisters itself on the next visit).

In the default `heavy` mode, each new item downloads the original page, extracts its main article
with a Readability-style parser, then runs the result through aggr's sanitizer and Markdown
pipeline. Extraction failures safely fall back to the feed. Set `[fetch] content = "light"` or
`content = "light"` on one source to avoid the extra request and reuse feed content.

Each item has a clean `/items/<source>/<slug>/` page, an `original` link, and sibling `.md`,
`.txt`, `.rst`, and `.json` representations. The page also links to the GitHub permalink pinned to
the commit that introduced it, plus the file's history and an edit link (set `hidden: true` in the front
matter from GitHub's editor; fetches never overwrite an existing file). Items outside the site
window (`[site] max_items`, `max_age_days`) become 200-byte redirect stubs to their permalink,
so old URLs keep working.

### Themes

A theme is a directory with `templates/` and `static/`; a `templates/` or `static/` directory
next to `aggr.toml` overrides individual files of any theme, which overrides the embedded
default. Templates are [minijinja](https://github.com/mitsuhiko/minijinja); the context
(`site`, `page`, `items`, `item`, `sources`, `categories`, `build`) and the filters (`url_for`,
`domain`, `date`, `excerpt`, `json`) are the ones the default theme in
[`themes/default/`](themes/default) uses — copy it and start from there.

## The digest

With a `[digest]` section, the first workflow run after `at` (in `[site] timezone`) opens one
issue titled `Digest #12 · 2026-09-02 · 8 new` listing what arrived since the previous digest,
grouped by source, with links to the site and to the Markdown files. Issues are assigned to the
repository owner by default, so GitHub emails them; the previous digest is closed; days with
nothing new are skipped. Each digest leaves a `refs/aggr/digest/<date>` ref on the repository,
which is how the numbering stays monotonic without any state file.

## The git model

Read [`docs/git-model.md`](docs/git-model.md). In short: `main` holds your files, the orphan
branch `aggr` holds the data, and `refs/aggr/*` are pointers. Nothing is ever rewritten.

```
items/<source>/<yyyy>/<mm>/<yyyy-mm-dd>-<slug>.md     front matter + Markdown
items/<source>/<yyyy>/<mm>/<yyyy-mm-dd>-<slug>.html   raw HTML
sources/<source>/state.toml                           ETag, Last-Modified, body hash
sources/<source>/seen.txt                             dedupe keys, append-only, union-merged
status.toml                                           failing sources, present only while some fail
```

## Development

```sh
make check          # fmt-check, clippy -D warnings, tests (hermetic: no network)
make run ARGS="dev"
```

`aggr dev` syncs and builds in a disposable workspace, watches the config, includes, templates and
static files, then resyncs/rebuilds and reloads open browsers after each successful change. It
prefers port 7319 and automatically picks a free localhost port when that is busy; use `--port` to
choose one. `aggr build` performs the same required sync persistently and exits after rendering.

Releases: bump `version` in `Cargo.toml` and `VERSION`, commit, tag `vX.Y.Z`, push the tag.

MIT.
