# Client development and search

Svelte 5 and TypeScript in `web/` own the complete reader: static page markup, browser hydration,
search rows, and navigation. Rust prepares archive data and sanitizes article bodies. A single
component tree renders both the published HTML and its interactive browser version. Every page
works with JavaScript disabled. Search remains a build-time Pagefind index, with no search service.

## Build and runtime

`cargo build` exports the shared page types from `crates/aggr-ui-model` into ignored
`web/.generated/types/`, installs locked npm dependencies when needed, and runs `web/build.ts`.
Vite produces the browser bundles, bootstrap, service worker, and an isolated server renderer in
Cargo's `OUT_DIR`. Generated types and bundles are not committed. `mise.toml` and `mise.lock` pin
build tools; `Cargo.lock` and `web/package-lock.json` pin packages.

The binary embeds that output. Installed commands need neither Node nor frontend source files:
Rust calls the synchronous Svelte renderer in QuickJS with no network, filesystem, or process
bindings. Each rendering thread reuses its own runtime, with memory, stack, and execution limits.
The embedded asset fingerprint participates in publication caches and application versions.

## Page model and hydration

Every page embeds its typed model in `<script id="aggr-page">`. The contract includes head metadata,
page identity, prepared display data, and a discriminated view. Rust prepares URLs and display
values before the first paint. Article HTML is sanitized in Rust, passed separately to SSR, and
appears once in the document. Hydration claims that opaque body element instead of serializing
article text again. Browser-local preferences are applied after hydration; the small bootstrap
still applies visual settings before paint.

The browser hydrates the same components that produced the page. Navigation extracts the next
model and sanitized article body, then updates the component tree; Svelte owns head metadata and
page chrome. Search metadata carries typed rows, rendered with the same `Row.svelte` as feeds.
There are no parallel Rust templates, HTML search fragments, or template parity fixtures.
Malformed models and incompatible application versions fall back to ordinary navigation.
Persistent playback stays outside the changing page content.

## Development

```sh
make setup                         # install pinned build tools
cargo run -- dev                   # local config, or demo when no config exists
make dev                           # demo archive
AGGR_CONFIG=/path/to/aggr.toml make dev
make check                         # Rust, TypeScript, component and launcher checks
```

`cargo run -- dev` starts one Node supervisor with a stable Vite front door. Frontend edits use
Svelte HMR; Vite renders document requests from Rust's typed model. Rust edits compile a replacement
worker while the last successful generation continues serving. A failed compilation leaves that
worker available. Successful replacements retain archive snapshots safely and reload the browser.
Configuration and archive changes use the worker's snapshot/reload stream. Ctrl-C shuts down the
stack. `--port` selects the public port; `make dev ARGS="--port 3000"` forwards options.

Cached articles appear first, and completed sources become visible during cold sync. Development
uses an isolated cache and never commits or pushes the archive. `dev --release` exercises full
publication rendering. Release binaries include native preview and live reload. They use embedded
assets without Node; source checkouts can also launch Vite from optimized binaries.

`make client-check` first builds Rust to generate the contract, then runs Svelte/TypeScript checks,
Vitest, and supervisor tests. Rust tests exercise the embedded renderer and publication behavior;
frontend tests render the actual components. Browser contracts cover hydration and navigation.
The TypeScript checker writes temporary files under ignored `web/.svelte-check/`.
The native compiler uses TypeScript 7 through the `@typescript/native` npm alias. Svelte's checker
still needs the classic TypeScript 6 JavaScript compiler API and its supported peer range, so the
separate `typescript` dependency stays on that line until the checker supports TypeScript 7.

Vite hashes executable assets and lists their paths in `assets.json`; Rust publishes them verbatim.
HTML remains `no-store` in development. Navigation keeps a bounded cache and starts at most two
prefetches after pointer, focus or touch intent; there are no startup, idle or viewport page downloads.
Prefetches use low network priority and pause when Data Saver or a 2G connection is reported.
Requested navigation stays high priority. Its 15-second deadline includes the response body, so a
stalled transfer releases its slot and can fall back to ordinary navigation or a previously read page.
A prerendered page waits for activation before changing history or registering a worker.

Touch suggestions select on release and yield to scrolling. Their height follows the visible
viewport above the keyboard, including browser panning. Installed pull-to-refresh yields to nested
scrollers, controls, selection and zoom; the app's Search shortcut opens the focused feed search.

## Preferences

`crates/aggr-lib/src/config/preferences.rs` holds one typed table. It produces the validation rules embedded as
JSON in `#aggr-preferences` and the form schema the client renders on `/preferences/`. Nothing
redeclares a setting: adding one there adds its control, its validation and its default everywhere.
The form is a static import of the preferences page, in the eager bundle with the shell. Opening
the tab paints the controls with the page; it does not fetch a chunk.

The pre-paint `bootstrap` script reads the rules and applies stored values to
`documentElement.dataset` before paint; the app's preferences state handles changes, cross-tab
sync, and import and export. Values live in `localStorage` under `aggr:<setting>`.

## Offline

The service worker separates automatically cached pages from selected offline downloads. Its
script carries the app shell, not the article catalog: `offline-catalog.json` is fetched only
when a download count is set, and individual collection pages are cached when opened. A new
worker copies shell files whose revision matches instead of downloading them again. The
offline preference saves complete article pages and their retained image renditions; readiness
requires every retained file. Updates stage immutable resource revisions, then publish the complete
selection in one cache write. A failed or canceled update keeps the previous complete downloads;
partial and quota failures remain visible. Uncommitted files are never served, and unused revisions
are removed after each update. Downloads made by older workers remain readable and move locally
into the revisioned cache on a successful update, without downloading unchanged files again.
Overlapping updates and clearing downloads share a queue and, where available, a browser Web Lock.
Any positive download
limit also saves the complete archive search manifest, including its versioned catalog, runtime
and fragments. Only a fully committed index becomes active, and a previous complete index remains
available during an update. Online/offline transitions select the matching catalog and runtime;
search results mark saved articles and omit previews that are unavailable offline.

Network responses stream immediately while cache writes and eviction finish under the fetch
event's lifetime. Only document requests can fall back to the HTML offline page. JSON, feeds and
stylesheets fall back to their exact cached URL, including its query, or return a network error.

## Browser regression tests

The browser suite uses a local ChromeDriver and a temporary, pinned article archive. `mise.toml`
pins ChromeDriver and Chrome for Testing to the same release. `make setup` installs both through
mise on supported desktop platforms. Point `AGGR_CHROME_BINARY` at the mise-installed browser:

```sh
mise exec -- chromedriver --port=9515 --allowed-ips=127.0.0.1
# In another terminal:
AGGR_CHROME_BINARY="$(mise where http:chrome-for-testing)/chrome-linux64/chrome" \
AGGR_WEBDRIVER_URL=http://127.0.0.1:9515 mise exec -- cargo test --locked --test browser -- --ignored
```

The example path is for Linux x64. On macOS, use
`$(mise where http:chrome-for-testing)/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing`
on Apple silicon, or replace `arm64` with `x64` on Intel. On Windows x64, the path is
`$(mise where http:chrome-for-testing)/chrome-win64/chrome.exe`. Set
`AGGR_BROWSER_TIMEOUT_SECS` (default 45) when a loaded machine needs longer waits. CI saves
failure screenshots and logs under `target/browser-artifacts/`.

### First-visit budget

The `default_visit` contract uses a fresh browser profile, 45 fixture articles, the default
feed page, remote media policies and zero selected offline downloads. On 2026-10-05,
Chrome 154 on macOS produced these measurements over local HTTP without compression:

| Measurement | Bytes |
|---|---:|
| Server response headers and bodies with `Cache-Control: no-store`, including service-worker installation | 937,331 |
| Same fixture with `public, max-age=31536000, immutable` on versioned assets | 698,712 |
| Regression budget for the `no-store` server total | 1,048,576 |
| Regression budget with cacheable versioned assets | 786,432 |

The sample ends 750 ms after worker control, before typing or navigation. It also verifies zero
external HTTP requests and zero automatic article, collection, search-index or offline-catalog
downloads. With cacheable assets, each asset is requested at most once: worker installation reuses
the browser's HTTP cache. Search works afterward. These are controlled fixture bytes, not
deployed-site timings; compression and hosting cache headers change the transfer size. The
samples predate the smaller install shell: collection pages and the article catalog are no
longer part of service-worker installation.

```sh
AGGR_WEBDRIVER_URL=http://127.0.0.1:9515 mise exec -- cargo test --locked --test browser default_visit -- --ignored --test-threads=1 --nocapture
```

## Query syntax

```text
"memory safety" category:programming tag:rust date:>=2026-09-01
source:"Underscore_" type:podcast -tag:sponsored sort:newest
date:2026-08-01..2026-08-31 -"sponsored post"
```

| Clause | Meaning |
|---|---|
| `word` | Full-text match in the indexed title and prose. |
| `"exact phrase"` | The words together; a backslash escapes a quote or a backslash, and an unclosed quote is an error. |
| `-word`, `-"phrase"`, `-tag:x` | Exclude what the clause matches; a minus inside quotes is text. |
| `source:`, `category:`, `tag:`, `type:` | Facet by stable identifier or unique label; quote a value with spaces. |
| `date:2026-09-08` | Published on that UTC day. |
| `date:2026-08-01..2026-08-31` | Inclusive range; either side can be left open (`..2026-08-31`). |
| `date:>=2026-09-01`, `date:>`, `date:<`, `date:<=`, `date:=` | Comparisons; `since:`, `after:`, `before:`, and `until:` are the same with a plain date. |
| `date:today`, `yesterday`, `week`/`last7d`, `month`/`last30d`, `year` | Shortcuts, rewritten to absolute dates in the shared URL. |
| `sort:relevance`, `sort:newest`, `sort:oldest` | Result order; the one clause that cannot be negated. |

A known qualifier without a value offers completion while it is being edited; other invalid
recognized clauses show an error. An unknown qualifier and any `http(s)://` URL stay full text.
The parser and controller live in `web/src/search/`.

Unqualified words search the indexed title and full text. Quoted phrases match together; a leading
minus excludes a word, phrase, or facet. `category:`, `source:`, `tag:`, and `type:` accept stable
identifiers or unique matching labels. Source links, completion, and canonicalized queries always
use canonical publisher identifiers, for example `source:"hnrss.org"` or
`source:"youtube.com/@Micode"`. Friendly source names are presentation labels and optional manual
aliases. `via` uses the same normalized label as the source directory: a resolved Spotify show
named `Underscore_` reads as `spotify.com/underscore`, while its query and collection identity
remains `open.spotify.com/show/<id>`. Without a resolved name, the catalog path remains visible.
Other facets can insert readable unique labels. Duplicate labels or labels colliding with another
identifier retain their stable identifiers; exact identifiers win, and ambiguous manual aliases
produce an explanation.
Source includes both the publisher and every configured or retained feed that supplied the article.
A canonical article appears once globally and once in each matching publisher collection.
Ordinary publisher IDs use the lowercase/punycode hostname without trailing dots, conventional
`www.`, or ports. Other subdomains remain distinct. Shared platforms also keep the account path;
platform aliases resolve through `crates/aggr-lib/src/platform.rs`, while opaque catalog IDs retain the host
that issued them. Subscriptions belonging to one publisher share a source filter, and duplicate
publisher/feed memberships collapse. Feed identities come from configured URLs or persisted
endpoint/site metadata, never archived slugs. An origin with no trusted HTTP(S) metadata contributes
no guessed membership; a known article publisher still does.
Archived source IDs, article paths, original URLs, and discussion provenance remain unchanged.
Repeated positive values
within a facet mean any of those values; different facets combine with AND. Exclusions remove matches.
Content types are `article`, `podcast`, `video`, `audio`, `image`, and `document`; they describe the
primary content, not incidental images embedded in an article.

`date:` uses the published UTC calendar day. It accepts exact dates, `<`, `<=`, `>`, `>=`, and
inclusive `start..end` ranges. Date shortcuts insert absolute dates so shared queries remain stable.
`sort:` accepts `relevance`, `newest`, or `oldest`; text searches default to relevance and filter-only
queries default to newest. Ordinary URLs and unknown colon-containing terms remain full text.
An empty facet qualifier at the cursor (such as `source:`) hides previous results and offers
values from the current index instead of a validation error, including when opened from a shared
query URL. Invalid recognized qualifiers keep results hidden and show an explanation.
Queries are limited to 4096 characters and 16 clauses, with explicit errors rather than truncation.

The syntax reminder under the field is a pointer affordance for someone already typing: it appears
only while the field has focus and the pointer is over it, and never on touch or from the keyboard
alone. It overlays the page without moving the feed, and `aria-describedby` reaches it regardless.

Completion replaces only the token at the cursor. It suggests qualifier names, source/category/tag/type
values with contextual counts and date shortcuts. Articles appear exclusively in the result list;
completing or submitting free text never opens an article suggestion. Value lists
are scrollable without an arbitrary cutoff; completed values stop suggesting themselves. Keyboard
and touch selection are supported; Escape closes open suggestions, and a further Escape blurs the
search field, never clearing the query. Composition input does not run partial searches. Enter and Tab accept the
highlighted stable option identity, even when labels coincide or asynchronous counts reorder values.
Accepting a qualifier leaves the menu open on the values it accepts, so `sour` and Enter reach the
sources in two keystrokes. The arrows walk open suggestions and the results the rest of the time,
with the keyboard staying in the field either way; the first result is selected by default and Enter
opens it once the token at the cursor has nothing left to complete. Results are the feed filtered
and share its one cursor: the first is selected as soon as they render, leaving the field returns
them to the ordinary `j`/`k`/`o` shortcuts, and clearing the query hands the cursor back to the
feed. The static feed waits hidden behind the results, so a cursor may only ever rest on a row
whose container is on screen.
Opening a result and coming back restores the last rendered page from `sessionStorage`, scoped
to the site path and exact query/page. The live query still runs; background refreshes preserve
existing rows, while changing the query immediately hides obsolete results and cancels its work.
Suggestions and counts honor every remaining clause after removing the edited token. While that
context loads, unrelated archive-wide values remain hidden. Obsolete requests cannot replace a newer
context, survive clearing/navigation, or apply to another index version.
Load the completion vocabulary independently of query generations: typing an incomplete qualifier
while the manifest loads must not discard the vocabulary needed to complete that qualifier.
The vocabulary comes from `search-catalog.json`, which contains only the index version, base,
document count, and facets. Focusing the
field or completing an unresolved qualifier does not initialize Pagefind or fetch its index chunks.
`make client-check` exercises the query and session contracts with vitest and runs the Node
development launcher tests alongside them.
Once a valid query is entered, runtime initialization overlaps the existing typing delay. Fully
resolved filter queries also use Pagefind's supported preload API; partial text does not speculate
on potentially broad prefix matches. Clearing, replacing, or abandoning the query cancels pending
warm-up before additional shared runtime work starts. Already-started shared imports remain reusable.

The parser produces token positions and a query AST shared by completion and execution. Exact
source, type, and published-day facets are generated in Rust. Every phrase intersection and exclusion is
applied to complete Pagefind result-ID sets before counting or pagination; only visible result
fragments are loaded. Completion uses Pagefind's filtered counts for a single search and bounded
filter-membership intersections for mixed phrases/exclusions, without loading article bodies.
Display metadata remains opaque to Pagefind's text tokenizer. Archived points and comment counts
are shared by static and search metadata, including explicit zero values; missing counts stay absent.
Hacker News points, comment counts, and the discussion score are omitted from that shared view.

Completion caches normalized aliases and ranking by immutable catalog identity; detecting a
finished facet token does not build or sort a suggestion list. A persistent search session coalesces
catalog requests and keeps two recently used independent Pagefind instances across navigation.
Evicted instances remain alive only while their queries, hydration, preload, or facet requests finish;
the last operation releases their worker resources and cached results. Application disposal cancels
shared catalog requests and retires instances after their active operations finish. JavaScript module code remains subject to
the browser's import cache. Content updates and online/offline changes revalidate the catalog;
failed content revalidation retains the usable index, while network transitions reselect the current
online index or the worker's committed offline index. Page disposal cancels only its own work. Result hydration is cached by
query-specific result reference, not document ID. Pagefind can mutate and return the same fragment
for different queries, so aggr copies an immutable display snapshot
before another query can hydrate that document. Unrelated documents still hydrate in parallel.
Display preparation is memoized by snapshot identity, avoiding repeated metadata parsing without
reusing highlights from a previous query.

## Current contracts

The page keeps the preference bootstrap. Preference imports
require `{ "version": 1, "preferences": { ... } }`; shared links carry this envelope in
`#aggr-state=`. Versionless payloads, prefixed import keys, and query-string preference imports are
rejected. Stored Markdown is authoritative; builds do not replay historical renderers to repair it.
