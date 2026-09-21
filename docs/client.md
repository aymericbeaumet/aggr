# Client development and search

Rust and MiniJinja render the complete static reader. The browser client is four hand-written
files with no build step and no dependencies: editing one and reloading is the whole loop.
Site generation never executes JavaScript or invokes a frontend compiler, so `cargo build`,
`aggr sync`, `aggr build` and `aggr dev` need no Node. Search is a build-time Pagefind index;
there is no search service.

## The client

| File | Loaded | What it owns |
|---|---|---|
| `themes/default/static/bootstrap.js` | render-blocking, every page | Applies saved preferences and rewrites dates before the first paint, so nothing flashes and no date column starts empty. The only blocking script, and deliberately small. |
| `themes/default/static/app.js` | deferred module, every page | Keyboard shortcuts and the selection cursor, feed page slicing, the preferences form, relative-date upkeep, the shortcut dialog, and the two lazy imports below. |
| `themes/default/static/search.js` | on first search intent | The query language, the completion menu, and the result list on top of Pagefind's low-level API. |
| `themes/default/static/media.js` | on article pages with media | The podcast player, provider video facades, and playback-time readouts. |

Everything else is the platform. Navigation is ordinary multi-page navigation, with
`<script type="speculationrules">` for prefetching and `@view-transition` for the transition.
The reading progress bar and the folding article header are scroll-driven animations. Rows open
through a stretched link, so modifier and middle clicks behave natively. Keyboard help is a
`<dialog>` opened by an invoker command. Anything without universal support sits behind
`@supports` or a feature check and degrades to plain HTML.

Each file opens with `// @ts-check`. `types/aggr.d.ts` declares the contracts they share with the
templates, and `jsconfig.json` makes editors type-check them with no toolchain installed. Hashed
asset names are never rewritten inside file contents, so the lazy modules are imported through
URLs the template resolves into `window.AGGR.assets`.

A prerendered page runs before anyone has seen it, so `app.js` waits for `prerenderingchange`
before writing session state, history or a worker registration.

## Preferences

`src/config/preferences.rs` holds one typed table. It produces the validation rules embedded as
JSON in `#aggr-preferences` and the grouped `<form>` on `/preferences/`. Nothing redeclares a
setting: adding one there adds its control, its validation and its default everywhere.

`bootstrap.js` reads the rules, applies stored values to `documentElement.dataset` before paint,
and exposes `window.AGGRPreferences`. `app.js` handles changes, cross-tab sync, and import and
export. Values live in `localStorage` under `aggr:<setting>`.

## Offline

A 55-line service worker keeps the shell installable, caches pages as the reader opens them, and
serves `offline.html` for anything unvisited. Content-addressed assets are served from the cache;
everything else is revalidated. Nothing is downloaded ahead of the reader, and there is no offline
search index.

## Browser regression tests

The browser suite uses a local ChromeDriver and a temporary, pinned article archive:

```sh
chromedriver --port=9515 --allowed-ips=127.0.0.1
# In another terminal:
AGGR_WEBDRIVER_URL=http://127.0.0.1:9515 cargo test --test browser -- --ignored
```

Set `AGGR_CHROME_BINARY` if Chrome is outside its usual location and `AGGR_BROWSER_TIMEOUT_SECS`
(default 45) when a loaded machine needs longer waits. CI installs matching browser and driver
versions; failure screenshots and logs are saved under `target/browser-artifacts/`.

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

A known qualifier without a value is an error; an unknown qualifier and any `http(s)://` URL stay
full text. The parser is `web/src/search/query.ts`.

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

## Current contracts

Custom templates must keep the documented markers and the preference bootstrap. Preference imports
require `{ "version": 1, "preferences": { ... } }`; shared links carry this envelope in
`#aggr-state=`. Versionless payloads, prefixed import keys, and query-string preference imports are
rejected. Stored Markdown is authoritative; builds do not replay historical renderers to repair it.
