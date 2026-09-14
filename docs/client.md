# Client development and search

Rust and MiniJinja render the complete static reader. Svelte and TypeScript run only in the browser,
where they own search, preferences, shortcut help, connection/update status, and podcast controls.
Swup owns page navigation. Site generation never executes JavaScript or invokes a frontend compiler:
`cargo build`, `aggr sync`, `aggr build`, and normal `aggr dev` use embedded assets without Node.
There is no SvelteKit server, JavaScript SSR engine, or external search service.

## Development

Frontend source lives in `web/`. Install its locked dependencies with `npm ci --prefix web`.
`make client-check` runs type checks and tests; `make client-build` writes the committed
`themes/default/static/app.js` and `client.css`. Rebuild these files after source changes. CI
rebuilds and compares them, while normal Cargo builds embed the existing files without invoking npm.

For immediate frontend feedback, run `make client-dev`, then run the reader with:

```sh
AGGR_VITE_URL=http://127.0.0.1:5173 cargo run -- dev --config examples/aggr.toml
```

The development server replaces the compiled client references in its HTTP responses with Vite's
module entry and HMR client. Static HTML, media, and data remain served by aggr. The environment
setting accepts a local HTTP origin only and has no effect on production builds. Svelte component
edits update through HMR; changes to document-level reader services reload the page.

The production bundle is self-contained, with one separate component stylesheet. Keep imported
images and internal runtime chunks out of the bundle: Rust hashes and renames static files without
rewriting references inside their contents. Pagefind is a separate, versioned runtime loaded by URL.

## Component and service ownership

The client has two lifetimes: a persistent application scope for preferences, update/offline services,
connection controls and shortcut help; and a replaceable page scope for search, the preferences panel,
and media. Each scope cancels pending work synchronously and awaits all disposers before replacement.
Swup and live feed replacement share that contract. A fetched feed rechecks the page, version, and
scope after disposal so a concurrent navigation cannot install obsolete HTML.

The preference service wraps the prepaint bootstrap schema and validator; it does not define another
set of defaults. Svelte subscriptions update controls after local edits, imports, and cross-tab storage
events. Theme changes do not reload the search index; page-size changes refresh results and date
changes update the component model. File and clipboard results cannot affect a disposed panel.

Navigation canonicalization goes through one adapter, updating the address bar, Swup history, and
selection URL together. The actual Swup instance is stored explicitly: before initialization,
`window.swup` can refer to the named `<main>` element. Selection persists by article URL, with Svelte rendering search selection
and the static adapter handling ordinary rows. Search owns its date text and localized tooltip;
minute/visibility ticks update its model instead of replacing Svelte's text nodes.

Application-release state, content versions, and offline download state are distinct. Typed watchers
coalesce update checks, ignore stale completions, and release timers/listeners on disposal. A pending
release does not stop content refresh. While search is active, its hidden static fallback keeps its
original content version; clearing the search refreshes that fallback when necessary.

Interactive templates require `data-preferences-root`, `data-shortcut-help-root`,
`data-connection-root`, and `data-audio-component`. Components mount into owned regions; they do not
hydrate MiniJinja output. Native media nodes are restored on disposal. Article content stays statically rendered and
native audio keeps its source, controls, and original-link fallback.

Rust precomputes a shared `metadata` view for static feeds, articles, recommendations, and opaque
search display data. `Metadata.svelte` renders the same source/category/date/reading/discussion/points/
comments fields. Static and Svelte markup remain separate;
parity tests cover their shared data, optional fields, escaping, and separators.

## Ownership and navigation

The shared header contains `feed | browse | preferences`, with `aggr.toml` on the right and
regular-weight, full-opacity labels and an underline for the selected section. Article pages select
no section: feed is current only on the feed itself. At widths up to
40rem, only the brand and aggr.toml remain at the top. A persistent four-tab bar provides feed,
search, browse, and preferences at the viewport bottom. Browse groups categories, sources, and tags;
Search focuses the shared field and preserves its query. Mobile feed rows omit numbers and use the
full available width. Mobile metadata follows the title and uses the full row width; search
excerpts show at most two lines below it. Pagination stays on one line with plain text controls
and 44px touch targets. The bar's background reaches the screen
edge; the home-indicator inset is applied once inside the bar. Content clearance uses its measured
height plus 12px, with a matching CSS fallback before JavaScript. The software keyboard hides the
bar without moving article content. Tapping the current tab dismisses an editor and returns to the
top without opening search or starting another navigation. Pressed feedback changes only the surface
color, with no transform, delay, or layout shift. Sources remain visible beneath individual article titles.
Browser click tests must scroll offscreen controls using native `scrollIntoView` before clicking:
WebDriver can consider a target visible even when the fixed bar covers it. Assert clearance rather
than hiding the bar; native scrolling and focus already honor the root scroll padding.

Swup navigations skip animation frames and transition delays. Intent from touch, pointer, or focus
promotes queued requests; idle work warms tab destinations and adjacent recommendations. Speculation
is bounded to two active requests, eight queued URLs, and 32 cached pages, and respects offline and
data-saving modes. Cached destinations can render immediately; uncached pages still require a
network response. Components and index fragments load only when needed.

Article recommendations stack vertically at every viewport width, with Coming next above Discover
more. Cards retain their natural heights and the shared feed metadata and previews. Card backgrounds
open the article with the same modifier-click behavior as feed rows; hovering underlines the title
without changing the surface color. Metadata links keep their own destinations and hover feedback.

Each feed renders a search form above its first article. Svelte mounts into `data-search-root` and
`data-search-results`; it does not hydrate Rust HTML. The original `data-static-feed` remains
available until a search begins. Query changes hide both the original feed and previous search
results immediately; only the current completed response becomes visible. Background index refreshes
keep the current keyed rows mounted until replacement results arrive, preserving keyboard focus. Clearing the query
restores the feed. Article pages and no-JavaScript navigation
stay statically functional. Full-text search requires JavaScript.

Dispose search components and media listeners before Swup or a live feed update replaces content.
Preserve keyboard selection by article URL, preserve focus while results change, and never allow
an older request to overwrite a newer query. Components must not attach listeners to an abandoned
page. Keep application-release refresh state separate from content and index changes.
When canonicalizing a query, update Swup's history-state URL along with the address bar. Only an
actual query edit cancels pending Back-navigation row restoration; passive canonicalization waits
for the matching result rows before restoring their saved selection and focus.

The site title returns home without focusing search; reselecting the current page scrolls to its top.
Cmd/Ctrl+K focuses the feed search, navigating home if necessary. Explicit focus scrolls to the top
only when the input is not completely visible between the sticky header and the viewport/tab bar.
Restoring an editor after a live feed update preserves its saved scroll position. Mouse hover reveals syntax help
without moving the articles; focus and touch do not open it. The grey input has the same geometry
before and after mounting, with an inline clear control and no search icon. Category, source, and tag links
open the main feed with a quoted qualifier in `?q=`. Their directory pages explain each grouping;
scoped archive URLs render static feeds with the corresponding qualifier visible. Focusing a
scoped archive field preserves its URL until editing. Removing the scope or clearing the entire
field returns to global search/the main feed. Active searches use `?q=` and
`search-page` in a shareable URL, with the existing feed-size preference controlling pagination.
Search URLs use the main feed and `?q=` exclusively; separate facet query parameters are not supported.

## Query syntax

```text
"memory safety" category:programming tag:rust date:>=2026-09-01
source:"Underscore_" type:podcast -tag:sponsored sort:newest
date:2026-08-01..2026-08-31 -"sponsored post"
```

Unqualified words search the indexed title and full text. Quoted phrases match together; a leading
minus excludes a word, phrase, or facet. `category:`, `source:`, `tag:`, and `type:` accept stable
identifiers or unique matching labels. Completion inserts a quoted readable label when it differs
from the identifier; labels equivalent to the identifier retain its spelling. Duplicate labels or
labels colliding with another identifier use the stable identifier, shown in the suggestion for
disambiguation. Exact identifiers take precedence; ambiguous labels produce an explanation instead
of silently including several sources. Initial shared queries also replace eligible internal IDs
with readable labels after the manifest loads, unless the user has already edited the query.
Source means the configured or retained feed, including aggregator feeds. Repeated positive values
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

Completion replaces only the token at the cursor. It suggests qualifier names, source/category/tag/type
values with contextual counts and date shortcuts. Articles appear exclusively in the result list;
completing or submitting free text never opens an article suggestion. Value lists
are scrollable without an arbitrary cutoff; completed values stop suggesting themselves. Keyboard
and touch selection are supported; Escape closes open suggestions, and a further Escape blurs the
search field, never clearing the query. Composition input does not launch partial searches. Enter and Tab accept the
highlighted stable option identity, even when labels coincide or asynchronous counts reorder values.
Suggestions and counts honor every remaining clause after removing the edited token. While that
context loads, unrelated archive-wide values remain hidden. Obsolete requests cannot replace a newer
context, survive clearing/navigation, or apply to another index version.
Load the completion vocabulary independently of query generations: typing an incomplete qualifier
while the manifest loads must not discard the vocabulary needed to complete that qualifier.
The vocabulary comes from `search-catalog.json`, which contains only the index version, base,
document count, and facets. Focusing the
field or completing an unresolved qualifier does not initialize Pagefind or fetch its index chunks.
Once a valid query is entered, runtime initialization overlaps the existing typing delay. Fully
resolved filter queries also use Pagefind's supported preload API; partial text does not speculate
on potentially broad prefix matches. Clearing, replacing, or abandoning the query cancels pending
warm-up before additional shared runtime work starts. Already-started shared imports remain reusable.

The parser produces token positions and a query AST shared by completion and execution. Exact
source, type, and published-day facets are generated in Rust. Every phrase intersection and exclusion is
applied to complete Pagefind result-ID sets before counting or pagination; only visible result
fragments are loaded. Completion uses Pagefind's filtered counts for a single search and bounded
filter-membership intersections for mixed phrases/exclusions, without loading article bodies.
Display metadata remains opaque to Pagefind's text tokenizer.

Completion caches normalized aliases and ranking by immutable catalogue identity; detecting a
finished facet token does not build or sort a suggestion list. A persistent search session coalesces
catalogue requests and keeps two recently used independent Pagefind instances across navigation.
Evicted instances remain alive only while their queries, hydration, preload, or facet requests finish;
the last operation releases their worker resources and cached results. Application disposal cancels
shared catalogue requests and awaits instance retirement. JavaScript module code remains subject to
the browser's import cache. Content updates and online/offline changes revalidate the catalogue;
failed content revalidation retains the usable index, while network transitions reselect the current
online index or the worker's committed offline index. Page disposal cancels only its own work. Result hydration is cached by
query-specific result reference, not document ID. Pagefind can mutate and return the same fragment
for different queries, so aggr copies an immutable display snapshot
before another query can hydrate that document. Unrelated documents still hydrate in parallel.
Display preparation is memoized by snapshot identity, avoiding repeated metadata parsing without
reusing highlights from a previous query.

## Offline search

Enabling automatic offline articles also downloads the complete archive search index. The stable
`search-manifest.json` describes its version, relative base URL, document count, byte total, facet
vocabulary, and each required file's size and SHA256 digest. Immutable `pagefind/<version>/` URLs
keep the runtime and chunks together. The build publishes no unversioned runtime aliases.
The compact catalogue is network-first. Offline, the worker derives it from the same verified
manifest that selected the complete index, so cached newer vocabulary cannot select an incomplete
replacement. It does not require a separately retained catalogue file.

The worker downloads into a protected cache with two concurrent slots and commits readiness only
after all required files have been verified. Progress and article readiness are reported separately.
An incomplete update retains the previous complete index. Interrupted downloads resume, quota
failures are reported, and ordinary runtime eviction does not trim complete indexes. A missing
resource invalidates readiness instead of turning an incomplete search into a successful zero count.

Changing a positive article limit resizes article saving without downloading an unchanged index
again. Setting the limit to zero cancels automatic downloads and removes their protected caches;
ordinary browsing caches remain bounded independently.

The full index contains searchable text from all retained articles. That does not make every reader
page or its media available offline: results use the worker's verified saved-article list to distinguish
saved pages. Search must not download every result page or media asset to make these annotations.

## Current contracts

Custom templates must provide the documented Svelte roots and preference bootstrap. Preference
imports require `{ "version": 1, "preferences": { ... } }`; shared links carry this envelope in
`#aggr-state=`. Versionless payloads, prefixed import keys, and query-string preference imports are
rejected. Stored Markdown is authoritative; builds do not replay historical renderers to repair it.

