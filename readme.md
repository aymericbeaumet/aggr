# aggr [![ci](https://github.com/aymericbeaumet/aggr/actions/workflows/ci.yml/badge.svg)](https://github.com/aymericbeaumet/aggr/actions/workflows/ci.yml)

Git-native feed reader. `aggr.toml` in, static site out, history on a branch.

A GitHub repository with two files becomes a reader: a workflow fetches your feeds every half
hour, appends every new item as a Markdown file to an append-only branch, renders a static site
from that branch, and publishes it on free GitHub Pages. Every item has a permanent
`github.com/…/blob/<commit>/…` URL, a run that finds nothing new leaves no trace, and the daily
digest arrives as a GitHub issue (GitHub emails it to you). No server, no database, no account
anywhere but GitHub.

Live example: <https://aggr.aymericbeaumet.com> — built from
[aymericbeaumet/aggr.aymericbeaumet.com](https://github.com/aymericbeaumet/aggr.aymericbeaumet.com),
a repository holding nothing but `aggr.toml`, a few topic files and the workflow below. The demo
at <https://aymericbeaumet.github.io/aggr/> is built from [`examples/aggr.toml`](examples/aggr.toml).

## Install: two files

The quickest start is to **fork
[aymericbeaumet/aggr.aymericbeaumet.com](https://github.com/aymericbeaumet/aggr.aymericbeaumet.com)**:
edit `aggr.toml` (drop `[site] url`, or point it at your own domain), enable workflows in the
fork's Actions tab, run the `aggr` workflow once, and your reader is live at
`https://<you>.github.io/<repo>/`. Delete the topic files you do not want and add your own.

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
| `aggr sync [--source slug…] [--dry-run]` | Fetch every source, write new items into the data worktree, commit, push, update `refs/aggr/last-good`. `build` and `dev` invoke this first. |
| `aggr fetch` | `sync` without the commit and push. |
| `aggr build [--release] [--out dir] [--base-url url] [--data-ref ref]` | Sync, then render the site. `--release` builds for `[site] url` (or `--base-url`) and writes `CNAME`; `--data-ref` renders any data commit after syncing. |
| `aggr dev [--port 7319]` | Sync, build, serve, then rebuild and live-reload as config or theme files change. |
| `aggr digest [--dry-run] [--force]` | Post today's digest issue when it is due. |
| `aggr check` | Validate the configuration and probe every source. |
| `aggr completions <shell>` | Shell completions. |

Locally, `aggr dev` is the whole loop. The data branch lives in a worktree at
`.aggr/data` and the site in `_site/`; both are added to `.git/info/exclude`, so `main` stays
clean without touching your `.gitignore`.

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

Theme state stays in `localStorage` without changing normal URLs. **copy state** in the top bar
copies a one-time URL containing every local key/value; opening it imports the state and cleans
the payload from the address bar. Discussion links such as Hacker News and X are configured with
`[[site.discussions]]` URL templates using `{url}` and `{title}`.

It is a **PWA**: "Install" it from the browser menu and it opens as an app with its own icon
and a search shortcut. A service worker precaches the feed, the lists and
the newest `[site] offline_items` item pages at every build, and remembers whatever you open,
so the reader works on the train; an "Updated — reload" toast appears when a new build is
live. `pwa = false` turns it off (an installed worker unregisters itself on the next visit).

Each item page renders the Markdown and links to the raw `.md`, a sanitized view of the original
HTML, the original article, and the GitHub permalink pinned to the commit that introduced it,
plus the file's history and an edit link (set `hidden: true` in the front
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

`aggr dev` syncs and builds once, watches the config, includes, templates and static files, and
reloads open browsers after each successful rebuild. It listens on port 7319 by default; use
`--port` to choose another. `aggr build` performs the same required sync but exits after rendering.

Releases: bump `version` in `Cargo.toml` and `VERSION`, commit, tag `vX.Y.Z`, push the tag.

MIT.
