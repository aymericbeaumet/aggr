# Client development and search

Rust and MiniJinja render the complete static reader. The browser client is a Svelte 5 +
TypeScript application in `web/`, compiled ahead of time by Vite and committed under
`themes/default/static/app/`, where the binary embeds it like any other static file. Site
generation never executes JavaScript or invokes a frontend compiler, so `cargo build`, `aggr sync`,
`aggr build` and `aggr dev` need no Node. Search is a build-time Pagefind index; there is no
search service.

## The page model

Every HTML page embeds the model it was rendered from in `<script id="aggr-page">`:
`ClientPage` in [`src/site/client.rs`](../src/site/client.rs), with the site, the build versions
and one of five views: `list` (rows, paginator, scope), `article` (header row, authors, resources,
access notice, neighbours and recommendations), `preferences`, `offline` and `static`. Field
names are snake_case like the templates, URLs are site paths the client joins onto `base`, and
`cargo test` exports the TypeScript types with ts-rs into `web/src/generated/` (CI fails when
they are stale). The article body is not in the model: it stays in the HTML as
`<div data-article-content>`, together with the media figure.

## Boot and navigation

The static page is the first paint. The deferred module reads the model, captures the article
content (or a static page's `<main>`), mounts `App` into a detached `<div id="app">`, swaps it
over the rendered one in the same task and flushes effects, so nothing paints in between; the
captured element is moved into the new tree, never re-parsed. Lists and articles are rendered by
components; browse and error pages are adopted whole; preferences and offline pages exist only in
the client. If `#aggr-page` is missing or malformed the page stays static.

Navigation fetches the target page, extracts its model, its content element and its `<head>`
metadata with `DOMParser`, and re-renders `<main>` from state while the header and tab bar
persist. A link inside the archive is fetched when it is pressed, focused or hovered (60 ms),
with at most two intent prefetches in flight and a 24-page, five-minute cache. No pages are fetched
just because their links are visible, and unvisited destinations are not refreshed periodically.
Intent prefetch is off on Save-Data, slow connections, offline, or hidden tabs; following a link
still uses ordinary navigation when necessary. Scroll positions are kept per history entry, so
Back returns to the same place and list cursor. A non-HTML response, a page without a model or one built by another app version
falls back to ordinary navigation, and without JavaScript every link is an ordinary link.

## Components and parity

Components that mirror a MiniJinja partial (`Header`, `ListHead`, `Toolbar`, `Row`, `Metadata`,
`Pager`, `RelatedCard`, `ArticleHeader`, `ArticleFooter`, and `App` for whole pages) must render
exactly what the partial renders. `cargo test` writes fixtures to `web/test/parity/` (a props
JSON and the partial's HTML per case; `AGGR_UPDATE_PARITY=1 cargo test --bin aggr parity`
refreshes them after a template or model change) and vitest renders each component with
`svelte/server`, normalises both sides (comments, attribute order, whitespace, hashed asset
names) and compares. These components carry no `<style>`: `style.css` is the only stylesheet,
so class names stay identical on both sides. Search results reuse `Row` from Pagefind's opaque
per-document metadata, which is the same `ClientRow` JSON.

Everything else is the platform. The reading progress bar and the folding article header are
scroll-driven animations, with a small script standing in where scroll timelines or `calc-size()`
are missing. Rows open through a stretched link, so modifier and middle clicks behave natively.
Keyboard help is a `<dialog>` opened by an invoker command. Anything without universal support
sits behind `@supports` or a feature check and degrades to plain HTML.

## Development

```sh
mise install                      # node
npm ci --prefix web
make client-check                 # svelte-check + vitest
make client-build                 # rebuild the committed bundle under themes/default/static/app/
make client-dev                   # Vite dev server with HMR
AGGR_VITE_URL=http://127.0.0.1:5173 cargo run -- dev --config examples/aggr.toml
```

With `AGGR_VITE_URL` set, the development server replaces the compiled module tag in its HTML
with Vite's client and the source entry, so component edits hot-swap; static HTML, media and data
stay served by aggr. The setting accepts a local HTTP origin only and has no effect on builds.
`make check` runs the frontend checks alongside the Rust ones; CI also rebuilds the bundle and
fails if the committed output or the generated types differ.
The frontend installs TypeScript 6 for `tsc` and an aliased TypeScript 7 for Svelte's `--tsgo`
checker; both are needed until `svelte-check` can use TypeScript 7 alone. The checker writes
temporary files under ignored `web/.svelte-check/`.

Vite content-hashes the client bundle and lists it in `static/app/.vite/manifest.json`; Rust
publishes those files verbatim (chunk names are referenced inside the bundle) and resolves the
entry, its stylesheet and its eager imports through the manifest. Every other static file is
hashed by Rust. Hashed names are never rewritten inside file contents.

A prerendered page runs before anyone has seen it, so the client waits for `prerenderingchange`
before writing session state, history or a worker registration.

## Preferences

`src/config/preferences.rs` holds one typed table. It produces the validation rules embedded as
JSON in `#aggr-preferences` and the form schema the client renders on `/preferences/`. Nothing
redeclares a setting: adding one there adds its control, its validation and its default everywhere.

The pre-paint `bootstrap` script reads the rules and applies stored values to
`documentElement.dataset` before paint; the app's preferences state handles changes, cross-tab
sync, and import and export. Values live in `localStorage` under `aggr:<setting>`.

## Offline

The service worker separates automatically cached pages from selected offline downloads. The
default offline-download count is zero. Installation caches the home/offline shell, small icons and
the client's eager dependencies; collection pages and optional search, media and preferences
chunks load when used. The worker references a versioned offline catalogue instead of embedding
its article families; only an enabled offline preference downloads it, checking its byte count,
SHA-256 digest, item count and local URLs before use. Saved user preferences still take precedence.
Large app-installation icons remain available on demand.
Content-addressed shell assets reuse the browser's HTTP cache during installation; pages and
mutable manifests still request fresh responses. This avoids downloading the same client assets
twice when the host permits caching.

The offline preference saves complete article pages and their retained image renditions; readiness
requires every retained file. Partial and quota failures remain visible. Any positive download
limit also saves the complete archive search manifest, including its versioned catalogue, runtime
and fragments. Only a fully committed index becomes active, and a previous complete index remains
available during an update. Online/offline transitions select the matching catalogue and runtime;
search results mark saved articles and omit previews that are unavailable offline.

The optional `[site.params] introduction = true` introduction uses native `<details>` outside the Svelte
root, so collapsing it survives navigation within the document. It appears only on the feed and
adds no script or network dependency. Browse and source directories expose subscription downloads
and the reading-list configuration; source/category query URLs remain shareable.

## Browser regression tests

The browser suite uses a local ChromeDriver and a temporary, pinned article archive. `mise.toml`
pins the `chromedriver` npm package to the installed Chrome's major version; if its binary is
missing after `mise install`, run `node install.js` inside that package once to download it.

```sh
mise exec -- chromedriver --port=9515 --allowed-ips=127.0.0.1
# In another terminal:
AGGR_WEBDRIVER_URL=http://127.0.0.1:9515 cargo test --test browser -- --ignored
```

Set `AGGR_CHROME_BINARY` if Chrome is outside its usual location and `AGGR_BROWSER_TIMEOUT_SECS`
(default 45) when a loaded machine needs longer waits. CI installs matching browser and driver
versions; failure screenshots and logs are saved under `target/browser-artifacts/`.

### First-visit budget

The `default_visit` contract uses a fresh browser profile, 45 fixture articles, the default
25-entry feed, remote media policies and zero selected offline downloads. On 2026-10-04,
Chrome 154 on macOS produced these measurements over local HTTP without compression:

| Measurement | Bytes |
|---|---:|
| Server response headers and bodies with `Cache-Control: no-store`, including service-worker installation | 833,136 |
| Same fixture with `public, max-age=31536000, immutable` on versioned assets | 595,352 |
| Browser navigation/resource `transferSize` with `no-store`, which omits worker requests | 390,218 |
| Regression budget for the `no-store` server total | 1,048,576 |
| Regression budget with cacheable versioned assets | 786,432 |

The sample ends 750 ms after worker control, before typing or navigation. It also verifies zero
external HTTP requests and zero automatic article, collection, search-index or offline-catalogue
downloads. With cacheable assets, each asset is requested at most once: worker installation reuses
the browser's HTTP cache. Search works afterward. These are controlled fixture bytes, not
deployed-site timings; compression and hosting cache headers change the transfer size.

```sh
AGGR_WEBDRIVER_URL=http://127.0.0.1:9515 cargo test --test browser default_visit -- --ignored --test-threads=1 --nocapture
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
use the hostname identifier, for example `source:"hnrss.org"`; friendly source names are presentation
labels in source directories and optional manual aliases. Publisher and `via` labels beneath article
titles use the same canonical hostname, never subscription paths. Other facets can insert readable unique labels. Duplicate labels
or labels colliding with another identifier retain their stable identifiers; exact identifiers win,
and ambiguous manual aliases produce an explanation.
Source includes both the publisher and every configured or retained feed that supplied the article.
A canonical article has denormalized source memberships: it appears once globally and once in each
matching hostname collection. Publisher and feed IDs both use the lowercase/punycode hostname,
excluding trailing dots, conventional `www.`, paths, and ports. Other subdomains remain distinct;
provider aliases do not merge actual domains. All subscriptions or accounts on one host aggregate
into one source filter, and publisher/feed memberships collapse when their hosts match. Feed hosts
come from configured URLs or persisted endpoint/site metadata, never archived slugs. An origin with
no trusted HTTP(S) metadata contributes no guessed membership; a known article publisher still does.
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
`make client-test` exercises the query and session contracts with Node’s built-in test runner;
no runtime dependency or browser bundle is introduced.
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

Completion caches normalized aliases and ranking by immutable catalogue identity; detecting a
finished facet token does not build or sort a suggestion list. A persistent search session coalesces
catalogue requests and keeps two recently used independent Pagefind instances across navigation.
Evicted instances remain alive only while their queries, hydration, preload, or facet requests finish;
the last operation releases their worker resources and cached results. Application disposal cancels
shared catalogue requests and retires instances after their active operations finish. JavaScript module code remains subject to
the browser's import cache. Content updates and online/offline changes revalidate the catalogue;
failed content revalidation retains the usable index, while network transitions reselect the current
online index or the worker's committed offline index. Page disposal cancels only its own work. Result hydration is cached by
query-specific result reference, not document ID. Pagefind can mutate and return the same fragment
for different queries, so aggr copies an immutable display snapshot
before another query can hydrate that document. Unrelated documents still hydrate in parallel.
Display preparation is memoized by snapshot identity, avoiding repeated metadata parsing without
reusing highlights from a previous query.

## Current contracts

Custom templates must keep the documented markers and the preference bootstrap. Preference imports
require `{ "version": 1, "preferences": { ... } }`; shared links carry this envelope in
`#aggr-state=`. Versionless payloads, prefixed import keys, and query-string preference imports are
rejected. Stored Markdown is authoritative; builds do not replay historical renderers to repair it.
