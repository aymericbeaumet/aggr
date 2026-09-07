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

- a responsive, installable PWA with bottom navigation on mobile, comfortable article typography,
  keyboard navigation, and a precached shell, home feed, and newest configured entries (30 by
  default); updates arrive without interrupting reading;
- build-time Pagefind full-text search over clean article prose, including best-effort lookup by a
  pasted original URL;
- a recent feed plus source, category, and `#tag` archives for every visible retained item;
- Atom, RSS 2.0, and JSON Feed streams for the reader and every source/category/tag;
- self-canonical snapshot pages, sitemaps, OpenSearch, a URL linkset, and an `aggr.json` instance
  descriptor for crawlers and other readers;
- original-page extraction by default, with the exact upstream URL and capture time kept as
  provenance;
- local, lossless copies of safe article images by default, plus optional list/search previews;
- immutable item versions in append-only git history, for as long as that history is retained.

aggr preserves readable article or feed content rather than a complete copy of the original
website. Scripts, styles, page chrome, and response headers are excluded. By default, safe raster
images in newly captured articles are stored locally while their publisher URLs remain in the
portable Markdown. Images that cannot be archived safely keep using those publisher URLs. If an
original-page fetch fails, the saved item may contain feed content, a summary, or only its
metadata.

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

To add aggr to an existing GitHub repository, install a prebuilt binary with
[mise's GitHub backend](https://mise.jdx.dev/dev-tools/backends/github.html):

```sh
mise use -g github:aymericbeaumet/aggr
aggr --version
```

Or download the archive for your OS and architecture from
[GitHub Releases](https://github.com/aymericbeaumet/aggr/releases/latest), verify it against the
release's `SHA256SUMS`, and put the extracted `aggr` executable on your `PATH`. Release binaries
are available for Linux, macOS, and Windows on amd64 and arm64. To select a particular release
with mise, append its version, for example `github:aymericbeaumet/aggr@1.6.0`.

Then, from the repository:

```sh
aggr init --github
$EDITOR aggr.toml
git add aggr.toml .github/workflows/aggr.yml
git commit -m "add aggr"
git push -u origin HEAD
```

The generated workflow requests a run at minutes 7 and 37 of every hour (UTC), and after root or
nested TOML configuration and theme changes on the repository's actual default branch. Push events
from other branches are ignored; scheduled and manual runs remain available.

GitHub's scheduler can [delay or drop scheduled runs under load](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#schedule).
Avoiding the start of the hour reduces a documented source of contention, but the cron interval
is not a freshness guarantee. Artificial commits do not have a documented scheduling benefit;
aggr leaves the data branch unchanged when there is nothing new. The separate 60-day inactivity
rule can disable a public repository's schedule; check the workflow's state and
[re-enable it](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/disable-and-enable-workflows)
if needed.

For more predictable trigger timing, an external scheduler can invoke the existing manual trigger
without creating commits. For example, run this from an authenticated host every 15 or 30 minutes
(replace `OWNER/REPO` with the instance repository):

```sh
gh workflow run aggr.yml --repo OWNER/REPO
```

Give the host's repository-scoped token Actions write permission. Monitor successful deployments
and retry failed dispatches: an accepted dispatch still depends on runner availability and a
successful build. The existing GitHub schedule can remain as a fallback.

The reusable workflow already serializes sync and Pages deployment across scheduled, manual, and
push-triggered runs. Its [concurrency group](https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/control-workflow-concurrency)
keeps one active run and one pending run; newer arrivals replace the pending run. Extra caller
concurrency or duplicate scheduled workflow files are unnecessary for that protection.

When diagnosing freshness, compare gaps between successful deployments, including the longest
gap, over complete days. Workflow creation time modulo the cron interval measures only a phase:
a five-minute delay and a 65-minute delay look identical on a 30-minute schedule. Daily run counts
show a delivery shortfall but cannot distinguish delayed events from dropped ones.

The reader checks for deployments once a minute while visible and online, and checks again when
it returns to the foreground or reconnects. An update takes effect on your next navigation or
refresh; it never reloads the article you are reading. Use your browser's refresh command
(`Cmd+R` or `Ctrl+R`) or native pull-to-refresh where available. The installed app provides its own
pull-to-refresh gesture.

New-item highlights and the title/favicon dot use tab-local state that disappears when the tab
closes. Normal browser tabs request a new tab for external links; installed
apps hand out-of-scope links to the platform's external or in-app browser UI. Presentation varies
by operating system. Installation and service-worker caching require HTTPS (localhost is the
development exception). A static aggr site cannot wake a closed app with background push.
If an older installation used a site under a path such as `/reads/`, removing and reinstalling it
may be necessary to adopt the corrected app identity and scope.

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

## Read comfortably

Mobile navigation keeps Feed, Search, Library, and Preferences within reach. The `/library/` hub
groups source, category, and tag archives on one filterable page. Preferences groups appearance, article text size and line width,
feed density, page size and preview visibility, date formats, and keyboard controls. Changes save automatically
in this browser. Compact feeds are the default; choose Comfortable for more space. Page transitions
follow your device's reduced-motion setting and can also be switched off.
Article headers show the visible Unicode word count and a reading estimate rounded up at 225 words
per minute; the same values are available to themes and machine-readable article metadata.

Under **Transfer preferences**, share or copy a link, save a JSON file, or import one on another
device. Links and files include only these settings, not reading history, and imported settings
need confirmation before applying. Preference links keep their payload in the URL fragment so
it is not sent to the server. Older preference links still work. Reset restores the defaults
without clearing reading history or offline articles.

| Keys | Action |
|---|---|
| `Cmd+K` / `Ctrl+K` | Open global search. |
| `/` | Local search. |
| `j` / `k` in article lists | Select the next / previous item; the first press selects the first item. |
| `o` / `Enter` in article lists | Open the selected item. |
| `j` / `k` in articles | Open the older / newer article. At either end, the key is a no-op. |
| `O` in articles | Open the original article. |
| Uppercase network key in articles | Open the matched discussion, or search that enabled network for the original URL. Built-ins use `H`, `R`, and `X`. |
| `u` / `d` in articles | Scroll up / down. |
| `g f`, `g /`, `g l`, `g p` | Feed, global search, Library, preferences. |
| `g 1` … `g 9` | Open one of the first nine feed entries. |
| `?` | Show keyboard help. |

List selection and reading position return with Back. Normal Tab and Enter behavior remains
available, including on original and discussion links. Single-key shortcuts are on by default
and can be disabled in Preferences; `Cmd+K` / `Ctrl+K` and standard keyboard controls still work.
Keyboard focus uses an underline or surface change, without a page-sized focus border.

## Configure sources

Only write values you want to change. The commented
[`config.default.toml`](config.default.toml) is the complete configuration reference and the single
source of truth for every default.

An ordinary website URL is usually enough. Each `[[sources]]` table has a `url` field and can
appear between any other TOML sections:

```toml
[site]
title = "My reads"

[[sources]]
url = "https://blog.rust-lang.org/"
name = "Rust Blog"
labels = ["rust", "language"]

[[sources]]
category = "programming"
url = """
https://example.com/feed.xml
https://example.org/rss
"""

[fetch]
content = "light"

[[sources]]
category = "science"
url = ["https://example.net/feed.xml", "https://example.edu/rss"]
```

`url` accepts a string, a multiline string, or an array of strings, including multiline
strings within an array. Each string is split into lines, surrounding whitespace is trimmed,
and empty lines are discarded. URLs retain their
fragments. Put comments and options in the TOML table, outside the strings; use another
`[[sources]]` table to start a group with different options.

Existing configs can keep `include` as an alias for `url` and
`[fetch].allow_remote_include_chains` as an alias for `allow_remote_source_chains`.
Do not set both names of an alias in the same table.

The table's options apply to every source it expands. Collections inherit these defaults unless
their entries set explicit options. Complete source tables can be mixed with other sections
anywhere in the file; their fields must stay inside their own table.

All forms reduce to the same source model before validation and discovery, using the same
global defaults, `${ENV}` expansion, collection expansion, and deduplication. Sources are expanded
in declaration order, with the first declaration of an equivalent endpoint winning.

Each item has at most one category and any number of labels. aggr tries the URL as a feed, follows
RSS/Atom/JSON Feed discovery metadata, probes conventional endpoints, and finally falls back to
conservative article discovery. In the default heavy content mode it attempts to download each
original page and extract its main article; use `content = "light"` on a source to trust its feed
content instead. The default first import considers the newest 100 entries from each feed, then
every later run adds newly observed entries.

Articles are deduplicated across sources by normalized original URL, so following both Hacker
News and a publisher keeps one copy. Existing archived duplicates appear once in the reader;
their previous item URLs redirect to the selected copy.

YouTube videos use their feed descriptions instead of article extraction. YouTube Shorts links
are always excluded, including from other feeds and mirrored archives, with no setting to enable
them. Previously archived Shorts are omitted when the site is rebuilt.

Extraction retains article images, inline links, and code indentation, with syntax colors for
recognized code languages. When retained HTML can recover code formatting lost in an older
capture, builds improve the displayed article without rewriting the saved item or its git history.
Custom source headers stay on the configured origin; redirected or linked article and image
requests on other origins do not receive those headers.

In heavy mode, a public ActivityPub/Mastodon status is expanded into its public same-author
self-reply thread when the page advertises ActivityStreams data. Parent and reply traversal stays
on the status origin, is tightly bounded, and falls back to ordinary article extraction if the
server withholds or rejects any required data. aggr does not scrape X/Twitter threads; its official
thread APIs require authentication and mutable deletion handling that is incompatible with the
append-only archive.

Article-image archiving is on by default. The explicit form, including a per-source opt-out, is:

```toml
[fetch]
images = true

[[sources]]
url = "https://blog.rust-lang.org/"

[[sources]]
url = "https://example.com/feed.xml"
images = false # leave this source's body images at their publisher URLs
```

For every accepted JPEG, PNG, GIF, or WebP image, aggr keeps the exact publisher response as the
master. When the bounded media budget permits it, safe static 8-bit images also get responsive
320, 640, 960, 1280, and 1600-pixel renditions where useful, plus a full-width rendition. These
are resized once with a high-quality filter, encoded as lossless WebP, then decoded and
pixel-compared before aggr offers them to the browser. The exact master always remains the
fallback, so choosing a smaller rendition introduces no lossy compression and a high-density
display is never forced to upscale it. A rendition is retained only when it is smaller than the
master. For a non-WebP master, aggr offers the responsive set only when its verified full-width
WebP is also smaller; an existing WebP master fills that full-width slot without being duplicated.

Animated GIF/WebP, images with an ICC color profile, and high-bit-depth images keep only their
exact master. SVG is left remote because it can contain active content. Other unsupported or
invalid media also keeps its safe publisher URL. Image work is failure isolated: a timeout,
decode error, size rejection, or failed download never prevents the article from being saved.

Acquisition is intentionally bounded per article: at most 24 candidates are inspected and 12
images retained; one response or rendition may use at most 10 MiB; downloaded and retained media
each have a 32 MiB cumulative budget; and decoded images may not exceed 32 megapixels or 12,000
pixels on either axis. Decoding runs at most two images concurrently with a 15-second per-image
limit. These media limits are conservative implementation safeguards rather than configuration
knobs.

Rendered pages use intrinsic dimensions and a dominant-color placeholder to avoid layout jumps.
The first body image loads eagerly at high priority; later images use native lazy loading and
asynchronous decoding. Local files are content-addressed and immutable. In the PWA, media for the
newest offline articles is cached opportunistically within a separate byte budget; other local
media enters a bounded cache after a successful view. Offline text therefore remains available
even when an optional image did not fit the precache. A publisher-hosted fallback is never
guaranteed offline.

Image archiving applies to newly discovered items only; enabling it does not fetch media for old
captures. Exact masters and responsive renditions increase the append-only data branch and Git
history, and normal retention cannot reclaim historical objects. Before publishing an archive,
make sure storing and redistributing a source's images is compatible with its terms and your local
law. Use `images = false` for sources whose media you should not retain.

Small local previews are enabled by default. They can be disabled globally or per source:

```toml
[fetch]
previews = true

[[sources]]
url = "https://blog.rust-lang.org/"

[[sources]]
url = "https://example.com/feed.xml"
previews = false # override the default for one source
```

aggr downloads and resizes a suitable image to at most 256 pixels per side, encodes it as lossless WebP (up to 384 KiB), and records its intrinsic dimensions, alt text,
and dominant color. The color reserves a calm placeholder while the thumbnail loads. A missing or
unusable preview never prevents saving the article. Previews appear in feed and search rows
and YouTube article pages. Enabling previews applies to new items; run with `--refresh` to fill
missing previews on existing items without replacing stored previews.

In heavy mode, YouTube articles also include a timed transcript when the public video page
advertises accessible captions. Timestamp links open the video at that point. Captions are grouped
into short readable paragraphs and fetched directly from YouTube, with no third-party transcript
service or credentials. Videos without accessible captions retain their description and preview;
YouTube may withhold or rate-limit captions. Light mode does not request video pages or transcripts.

The same `url` field also accepts local paths and remote URLs for aggr TOML, OPML subscription
lists, newline-separated URL lists, RSS, Atom, and JSON Feed. aggr detects the format from the
resource's content. A feed document registers one source; a collection expands into its listed
sources at that position. The table's options become defaults for those sources:

```toml
# aggr.toml
[[sources]]
url = ["./aggr-ai.toml", "./subscriptions.opml", "./feeds.txt"]
category = "ai"
```

```toml
# aggr-ai.toml
[[sources]]
url = """
https://www.anthropic.com/news
https://openai.com/news/
"""

[[sources]]
url = "https://example.com/research.xml"
category = "research" # an explicit category wins
```

Local paths resolve relative to the file that names them. Local globs and direct HTTP(S) URLs
are supported. A bare GitHub repository URL finds its `aggr.toml` automatically, and GitHub-hosted
configs can use relative wildcards just like local ones. Collections and feeds can share a table:

```toml
[[sources]]
url = """
https://github.com/aymericbeaumet/aggr-instance
https://example.com/subscriptions.opml
https://example.org/feed.xml
"""
category = "community"

[[sources]]
url = "https://github.com/owner/reader/blob/main/topics/*.toml"
```

Expansion is deterministic and cycle-safe: resources and equivalent source endpoints are loaded
once, in declaration order, with the first declaration winning. Missing, malformed, and unsupported
collections warn and are skipped. By default, a remote collection may expand only relative paths
on the same HTTP origin—and, for GitHub URLs, within the same repository. Its feed and website
entries may use other origins. The trusted root can opt into broader collection chains with
`[fetch] allow_remote_source_chains = true`. GitHub requests automatically use `GITHUB_TOKEN` or
`GH_TOKEN` when present; a private repository must grant that token read access.

Remote collections are live configuration dependencies, not part of the append-only data branch.
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
When previews or article images are enabled for the mirror source, existing local companions are
copied with the article without contacting the publisher. Disabled media stays omitted.

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
| `aggr sync [--fetch-only] [--dry-run] [--clean]` | Fetch new items. Normally commit and push them; `--fetch-only` writes locally without either, while `--dry-run` writes nothing. `--clean` discards rebuildable state first. |
| `aggr build [--release] [--out DIR] [--data-ref REF] [--clean]` | Sync and render, or render a pinned data ref without source/discussion fetches, commits, or pushes. Matching renders are cached; `--clean` starts without that cache or prior owned output. |
| `aggr dev [--release] [--port 7319] [--clean]` | Sync and build in an isolated system cache, serve entirely from memory, watch dependencies, and live-reload. It never commits or pushes; `--clean` recreates its isolated snapshot. |
| `aggr clean [--dry-run] [--out DIR]` | Remove this config's disposable dev state, repository build cache, and owned generated output. `--dry-run` lists exact targets. |
| `aggr check` | Validate the config and probe every source. |
| `aggr completions <SHELL>` | Generate shell completions. |

Build uses a repository-local cache; dev uses a separate OS-standard cache keyed by the canonical
config path. Repeated runs are designed to become nearly instant without mixing CI/build and local
development state. Dev renders retained articles before syncing, reports each source as it
finishes, and publishes new articles before refreshing optional discussion links. Theme edits
reuse cached discussions without network requests.

Cleanup removes the whole aggr-owned repository cache (including obsolete
namespace versions) and this config's dev snapshot, but only after proving each target disposable. Generated
output needs its `.aggr-site` ownership marker; an unmarked output directory is kept. Protected,
tracked, escaping, or symlinked targets are rejected, and cleanup stops if the same dev workspace
is active. The append-only data worktree under `.aggr/data`, archived articles, Git refs/history,
and hand-made files are never cleanup targets.

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
See [`docs/themes.md`](docs/themes.md) for the template contract, preview fields, and navigation
hooks to preserve when customizing the reader.

For development:

```sh
make check
cargo run -- dev --config examples/aggr.toml
```

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
