# Themes

aggr renders local MiniJinja templates and static assets. Files in the project's `templates/` and
`static/` directories take precedence over the selected theme, which falls back to the embedded
default. Select another local directory with:

```toml
[site]
theme = "themes/my-reader"
```

Override templates individually. A static-file override replaces that whole file; it is not
appended to the default stylesheet or script. `aggr dev` watches theme changes.

## Template data and URLs

Every page receives `site`, `page`, and `build`. Collection pages receive `items` and paginator
metadata; an article page receives `item`, including its rendered `body_html`. The complete field
definitions live in [`src/site/context.rs`](../src/site/context.rs).

Items and recommendation links also expose `metadata`, the precomputed display view shared with search:
source identity, category, publication/update dates, reading statistics, resolved discussions, and
optional points/comments. The default
metadata partial uses this shared view; values must still be escaped normally.

`item.resources` contains validated `{ label, url }` links moved from an opening resource-only
paragraph into a compact row below the article header. Detection requires 2–8 short links to
recognizable model, dataset, repository, or paper destinations, with no surrounding prose. It
can follow up to three image-only hero paragraphs, whose images remain in the reader body. It
leaves author/social lists, tables of contents, code, later paragraphs, and ordinary prose alone.
Resources are separate from topic tags. Reader excerpts, search text, and reading statistics omit
that opening group; portable representations and feed content retain the links in their original
paragraph; plain-text and reStructuredText exports also list their destinations under Resources.
Stored Markdown and publisher metadata remain unchanged. Custom item templates should
render `item.resources` alongside `item.body_html`, escaping labels and URLs normally.

The default theme explicitly opts into browser components using `data-preferences-root`,
`data-shortcut-help-root`, `data-connection-root`, and `data-audio-component`. These regions become
owned by Svelte after loading. Custom templates must provide these markers;
the default components exclusively own those regions. Site generation requires no JavaScript runtime. See [client ownership](client.md#component-and-service-ownership).

Article contexts also expose `item.word_count` and `item.reading_minutes`. Both are derived from
the visible Unicode text at build time, with reading time rounded up at 225 words per minute; an
empty/title-only body reports zero for each. The default theme shows reading time beside
publication, exposes the word count on hover, and emits `wordCount` plus an ISO 8601
`timeRequired` duration in structured data.

Every non-utility page advertises the site feeds: `page.feed_path` is the collection prefix (`""`
at the root) and `page.feed_title` the title to label its Atom, RSS and JSON Feed alternates with;
404 and offline pages carry neither. `site.og_locale` is the site language in Open Graph form
(`en_GB`). `sources[].feed_url` is the resolved feed endpoint when one is known, which the build
also publishes as `sources.opml` for other readers alongside `llms.txt` and `aggr.json`.

Use `url_for` for internal pages and assets so the same output works at `/` or under a nested
mount. The default base template supplies the page-relative `<base>`; static asset names are
content-hashed during the build. Public canonical and social URLs should use `site.base_url` only
when it is available.

```jinja
<a href="{{ item.url | url_for }}">{{ item.title }}</a>
<link rel="stylesheet" href="{{ 'assets/style.css' | url_for }}">
```

HTML templates escape values automatically. `item.body_html` is already sanitized and may be
rendered with `|safe`; do not apply that filter to arbitrary source text or serve stored HTML
without sanitizing it.

## Optional article previews

`item.preview` is absent when no local preview exists. Search display metadata exposes the same
optional object:

| Field | Value |
|---|---|
| `url` | Site-relative local image path, without a leading slash. |
| `width`, `height` | Intrinsic image dimensions in pixels. |
| `alt` | Optional image description. |
| `color` | Optional validated `#rrggbb` dominant color for a loading placeholder. |
| `placeholder.hash` | Base64 ThumbHash generated from the oriented image. |
| `placeholder.data_url` | Inline PNG decoded from the ThumbHash during the Rust build. |

```jinja
{% if item.preview %}
<span class="preview-media"
      data-thumbhash="{{ item.preview.placeholder.hash }}"
      style="--preview-color: {{ item.preview.color }}; --image-preview: url('{{ item.preview.placeholder.data_url }}')">
  <img src="{{ item.preview.url | url_for }}"
       width="{{ item.preview.width }}" height="{{ item.preview.height }}"
       alt="{{ item.preview.alt or '' }}" loading="lazy" decoding="async">
</span>
{% endif %}
```

Reserve the thumbnail's space before it loads and let rows without images use their full width.
The default theme displays an inline ThumbHash preview immediately, including before scripts run;
there is no placeholder network request or client decoder. The dominant color remains a fallback.
It uses the same placeholders in lists, search, article media, and podcast covers. Preview
URLs are content-addressed, lossless WebP assets.

## Archived article images

`item.body_html` already substitutes verified local article media when available. The portable
stored Markdown still contains the publisher URL; custom templates should render `body_html`
unchanged rather than trying to perform their own URL substitution. A safe static image with a
complete set of lossless renditions is emitted approximately as:

```html
<picture class="article-picture">
  <source type="image/webp" srcset="… 320w, … 640w, …" sizes="…">
  <img class="progressive-image" src="…"
       width="…" height="…" alt="…"
       style="--image-placeholder:#285a8c"
       loading="lazy" decoding="async" fetchpriority="low">
</picture>
```

The exact raster publisher master remains the `img` fallback and largest `srcset` candidate. SVG
diagrams use a passive local PNG master instead of publisher markup. Verified
WebP renditions let the browser choose smaller downloads without a quality ceiling. Large masters
skip expensive full-width WebP conversion while retaining bounded responsive copies. An animation
or profiled/high-bit-depth image keeps its master and a ThumbHash placeholder. The first image uses `loading="eager"`
and `fetchpriority="high"`; subsequent images use native lazy loading. Preserve
`.article-picture`, `.progressive-image`, intrinsic dimensions,
`data-thumbhash`, `--image-preview`, `--image-placeholder`, and loading attributes when styling or post-processing the generated
body.

Linked status badges keep compact intrinsic dimensions and never become article heroes or feed
previews. Pending progressive images suppress visible linked alt text until load or failure, while
screen-reader descriptions and readable error/no-script fallbacks remain available.

Both preview and body-image URLs are immutable local assets. Preferences selects 0–1000 recent
articles to download with all retained masters and responsive renditions. A separate protected
article cache keeps these assets out of the bounded runtime image cache. An article counts as
available only after every resource is saved; network and quota failures remain visible in
Preferences. Publisher-hosted fallback images and embedded audio/video are outside this guarantee.

## Reader behavior

The default theme uses plain links, a responsive header with visible navigation, system fonts, and a reading
column of approximately 65 characters. Its warm light/dark palettes, focus styling, and code
colors are defined in `static/style.css`. Code is highlighted during Rust rendering with `syntax-`
span classes and a small `.code-snippet[data-language]` label. Publisher language hints take
precedence over conservative detection; a snippet whose language stays ambiguous carries no
`data-language` attribute at all, so the label disappears rather than reading “Text”. Labels are
CSS-generated so copied code stays unchanged. No client-side highlighter is loaded.

A figure and its caption render as `figure.article-figure` wrapping the picture and a `figcaption`.
Tables are wrapped in `div.table-scroll`, which borrows the page margins and scrolls; the table
itself stays one layout box so its header and body columns line up, and a header row aggr supplied
for a source table that had none is hidden. Selecting article text mounts a
`.selection-share` toolbar; see [the reader](reading.md#sharing-a-passage).

When extending the default script, keep these relationships intact:

- `#swup` is the replaced main content. Persistent navigation and connection status stay outside
  it. `#aggr-page` supplies the current page root and kind after navigation.
- Primary navigation links use `data-route` and `data-kinds` for destination and active state.
  `browse`, `preferences`, and aggr.toml are visible in the desktop header. Mobile navigation uses
  feed, search, browse, and preferences tabs below the content. Search lives above the first feed entry, with visible scope
  qualifiers on category/source/tag feeds. The brand returns to the main feed. Facet links use the
  `facet_url` filter to open the main feed with a quoted search qualifier. Static archives remain at
  `sources/<slug>/`, `categories/<slug>/`, and `tags/<slug>/`; `/browse/` combines the directories. See [client development and search](client.md) for syntax and ownership.
- Article lists use `.rows .row`, with a `[data-row-open]` title link. `.is-selected` identifies the
  remembered keyboard cursor. Keep `_item.html` and the Svelte search result component aligned.
- List pagination exposes `data-page-previous` and `data-page-next`. The home feed's
  `data-feed-pager` also carries its generated page size, total, and static edge routes. The
  default script may divide one generated page into smaller `?feed-page=N` views; every row must
  remain in the HTML so no-JavaScript readers and crawlers see the complete static page. Article
  chronology uses `data-previous-url` and `data-next-url` on the article.
- Search results may arrive in stages. Preserve a focused title by URL when adding results;
  typing a new query must not move focus or let a stale response replace the current results.

Preferences uses one typed schema in the head of `base.html` (`window.AGGRPreferences`), before the
stylesheet, to apply saved appearance without a flash. The default script reuses that schema for
controls, validation, sharing, and import/export. Custom base templates using the default script
must retain that bootstrap. Controls identify their setting with `data-preference`; changing one
persists only the allowlisted `aggr:<setting>` local-storage key.

Site owners define initial values for every setting in `[site.preferences]`; the typed Rust schema
in `src/config/preferences.rs` supplies `site.preferences` as browser JSON. Device values take
precedence, and resetting preferences returns to the site's defaults. See
[`config.default.toml`](../config.default.toml) for all choices.

Root datasets cover theme (including sepia), text size, reading width, typeface, line and paragraph
spacing, optional paragraph indentation (off by default), alignment, letter and word spacing,
density, thumbnails, and motion. Feed page size, dates, shortcuts, `scroll-amount` (1–100 lines),
and `offline-items` (0–1000 articles) also use the shared schema. Reading settings affect prose,
not browser zoom or navigation. System reduced motion always wins. These controls follow familiar
reader features documented by [Apple Books](https://support.apple.com/en-ca/guide/books/ibks8923126d/mac).

Transfer actions share one versioned `{ "version": 1, "preferences": { … } }` JSON contract. Links
carry base64url JSON in `#aggr-state=…`, not a server-visible query; older `aggr-state` query links
remain importable. Files and links are size-bounded, allowlisted, and reviewed before explicit
application. Reading history and cached article contents are never exported. Resetting preferences also restores
the site's offline article count, which can resize automatic downloads.
Disabling single-key shortcuts leaves modifier shortcuts and native keyboard operation available.
Cmd/Ctrl+K focuses the shared global search field; Escape closes suggestions and removes focus.
On feeds and search results, `gg` selects and focuses the first visible item and `G` the last,
also scrolling to the corresponding page boundary. On article pages they scroll to the top and bottom instead.
Explicit focus keeps the current scroll position when the entire input is visible; otherwise it
scrolls instantly to the top, accounting for the sticky header and mobile viewport.
`O` opens the original and configured uppercase discussion shortcuts target the selected feed
result or current article. The separate
directories do not have local filters.
The [reader keyboard map](reading.md#keyboard) describes the controls.

Keep keyboard focus visible with an underline or contrasting surface. The default script marks
programmatic main focus with `data-navigation-focus`, suppressing only the large container
outline after a page change. Ordinary links, form controls, and the skip link must remain usable
with a keyboard.

Navigation replaces content without a document transition. Swup starts with pristine initial
HTML, before enhancement adds binding markers. Page requests share in-flight work; speculative
fetches are bounded, canceled when unrelated to navigation, and promoted when they become the
navigation target. Search display data is precomputed and hex-encoded because Pagefind indexes
even zero-weight metadata values; decoding it for display must not leak implementation keys into
search matches.

The service worker serves cached HTML immediately and refreshes it in the background. An explicit
reload requests fresh HTML. `updates.json` separates the application fingerprint (aggr version and
effective template/static files) from the content fingerprint. Browsers check it every 15 seconds while
visible, when returning to the app, and when reconnecting, including sites with PWA mode disabled.
New content refreshes the feed and search index without reloading the document or showing an app
update prompt, including while a separate application release awaits refresh. Returning to a feed
also checks its displayed content version so stale navigation entries are replaced automatically.
List selection and reading position are retained; an open article stays in place. Dev build events
check immediately and queue another check if an earlier request is still in flight.

An application release shows a clickable, keyboard-accessible “Refresh to update” pill in browsers
and installed apps on desktop and mobile. It reloads the same page, preserving its query, fragment,
and scroll position. A normal browser refresh also loads the new release; a hard refresh works too.
Browser refresh remains native; the installed app also supports pull-to-refresh. Dev disables
service workers so a previous deployment cannot mask a local snapshot. Loading, promoting, and
removing dev snapshots runs on blocking workers, so large asset trees cannot stop HTTP responses
while the previous in-memory snapshot remains available.

The worker coalesces offline-count changes, cancels superseded downloads, and uses six independent
slots without delaying activation. Failed replacements retain older complete articles within N. Shared
images reuse one request; verified content revisions avoid downloading unchanged resources across
builds. The offline fallback lists only fully saved articles on the current device. Downloads run
in ordinary secure browser tabs as well as installed PWAs; `pwa = false` disables both. Browser
storage eviction or quota limits can still remove/prevent downloads, so check the reported count.

Validate custom themes at narrow widths, with enlarged text, both color schemes, reduced motion,
and JavaScript disabled. The visible navigation links wrap on mobile;
there is no duplicate bottom navigation bar.

## Metadata and scrolling

`_metadata.html` supplies the feed and item metadata; search uses the same field order in
the Svelte search result component. Feed rows, search results, and article headers all begin their metadata with a
source link combining publisher identity with an optional italic `via` feed name. The navigation bar
omits individual source details; feed entries retain them. Only `.source-resolved` uses the palette's
orange; `via` keeps the muted metadata color.
Source names and article titles share `site::display::title` cleanup through the build context:
HTML, search, RSS, Atom, JSON Feed, Markdown, plain text, and reStructuredText all use those display
titles. Stored originals and article-body emoji remain intact. Profile paths distinguish channels
on shared hosts. Podcast platforms with opaque show IDs use compact provider/show-name display
labels derived from source metadata; actual destinations and archive IDs remain unchanged.
An optional linked `/category` follows
as its own field, then publication time, reading time, original link, and discussions.
Noninteractive field wrappers own evenly spaced middots, keeping them outside link underlines and
tooltips. The publication tooltip contains published and updated timestamps on separate lines;
equal instants omit the update. Exact dates are localized on interaction with cached formatters.
Feed, search, and article metadata share typography, field heights, and separator spacing in each
density and pointer mode. Dates use their natural text width. The `_dates.html` head bootstrap
formats dates during initial HTML parsing, before paint, and `app.js` uses the same formatter
thereafter; this avoids both empty date columns and media shifts during startup.
Tags use `#` and occupy the article header's second metadata line, with the same muted color and
font size as the metadata above. They are plain links without chip backgrounds or borders.

The article footer stacks compact “Coming next” and “Discover more” cards vertically at every
width. Each card fits its text and any actual preview; cards without thumbnails do not reserve
empty thumbnail height. Background clicks open their articles; hover underlines titles without recoloring cards.
They reuse feed metadata and preview geometry
through `_metadata.html` and `_related_item.html`, including source, category, date, reading time,
original link, and discussions. Thumbnail and excerpt preferences apply to these cards too.
Navigation and discovery exclude aliases of the current article and repeated full content.
Identity combines normalized original URLs, retained page paths, and exact whitespace-normalized
body matches of at least 64 words. Similar titles, shared topics, and short teasers do not establish
identity; a product page and its announcement can remain separate suggestions.

Canvas applications with interactive controls can offer the live original inside a reserved
viewer, alongside safely retained prose. The original loads automatically when the article opens and
runs in an opaque-origin `allow-scripts` sandbox with no referrer, forms, popups, or top navigation.
Loading does not move keyboard focus, and leaving the page removes the live frame. Its scripts
never run during ingestion. A visible original link remains available when framing,
network access, or browser graphics support prevents the interactive view from working.

Media placeholders use trusted inline PNG data URLs, independent of site base paths. Ordinary
local image `src` attributes continue to use the document's `<base>`.

Header folding follows vertical scroll directly. The title scales with fixed line wrapping and a
measured height so a two-line title cannot abruptly become one line. Slash-separated title terms
such as `km/h` stay together when they fit the reading width; oversized terms may wrap to avoid
overflow. Tags gradually disappear,
while the metadata remains. A 48px linear fade below the header softens content passing underneath;
the one-pixel separator fills left to right with linear page progress. Progress updates transform
only that bar, avoiding inherited style recalculation across the title and metadata. Header, prose, and
continuation cards use the same reading measure.

The first visible feed row is selected automatically. Desktop selection is an immediate warm left
bar; mobile hides it. The new-item background fades over five seconds as soon as it is displayed; opening the tab clears
the favicon badge immediately. Separator and highlight edges
share geometry, and rank-width metadata belongs to the whole list so single- and double-digit
ranks cannot move individual rules. Clicking row background opens its item, while individual links
retain their own destinations and text remains selectable.

## Video and lead media

List thumbnails and full article images serve different purposes. `item.article_preview` supplies
a retained lead image only when that image, or an equivalent retained master, is absent from the
rendered body. Equivalence uses the retained ThumbHashes, so an `og:image` served from another
CDN URL, size, or format than the first body image is still recognized as the same picture and
skipped. Leads narrower than 640px are skipped as well: social cards and small metadata images
upscale badly as a hero. Small feed previews must not replace full article masters.

Reader headings carry stable ids. A publisher heading that is one link to its own anchor or to
the article's own page becomes a plain heading with that anchor id; other ids come from the
heading text. Hovering shows a `#` marker in the margin without moving the title, and a plain click
updates the URL fragment and scrolls to the heading. Headings are never links in the reader;
portable outputs keep the original Markdown.

On screens of at least 2dppx, body pictures display at most two thirds of their intrinsic width
so a modest master is not stretched to double its pixels; large masters still fill the measure.
Interactive and PDF viewers caption only the original link.

Provider players are facades: nothing is requested from YouTube, Vimeo, or Twitch until the reader
activates the poster, and activation starts playback in one click. A body paragraph that is only a
link to a supported video (a bare URL, a text link, or a linked thumbnail) renders as the same facade
so videos play inside aggr; its poster is the linked picture or the archived provider thumbnail.
Portable outputs keep the plain link. Embed URLs come from validated provider identifiers; YouTube uses its privacy-enhanced
domain, Vimeo uses DNT, and Twitch requires the current hostname as `parent`. These options do not
guarantee that providers never set cookies or show recommendations. A creator's published poster
can differ from the frame visible after playback begins.

## PDF documents

Direct PDF article URLs (a `.pdf` path or `filename=*.pdf`) render a native PDF embed inside the
article, with a permanent Open PDF fallback link. The viewer uses the original HTTP(S) URL and its
query/fragment, without a third-party viewer. Browser PDF support and publisher embedding policies
still apply; unsupported viewers use the fallback. PDF links in article prose remain ordinary links.
When preferred previews are absent, fetching can rasterize the first page to a retained thumbnail;
the full PDF still loads from its publisher and is not retained for offline access.

## Stable media loading

Images use retained intrinsic dimensions first, then sanitized publisher dimensions, then a fixed
16:9 fallback. The fallback stays fixed after loading and contains the image without distortion.
Lead images, native video, provider players, audio controls, and PDF viewers also reserve their
space before fetching. PDFs keep a document placeholder behind the native viewer; an embed load
event does not establish that the browser successfully rendered the document. Provider posters
remain visible until the frame loads, including with reduced motion. Twitch reserves its 4:3
viewport before activation so the provider minimum size cannot enlarge the article.

Podcast audio enclosures and direct audio/video file URLs stay publisher-hosted, without autoplay
or preload. Audio progressively enhances into a player with an uncropped cover in a full-height
artwork area beside controls in golden-ratio columns (38.2% artwork, 61.8% controls), including on
mobile. The artwork is inset on the player's own background rather than a separate panel. Controls
stay compact within the right column and offer play/pause, seeking, symmetric 30- and 15-second
skips either side of play, speed, and mute/volume; muting lowers the volume control to zero and
unmuting restores the previous level. Native controls remain
usable without JavaScript and appear automatically if enhanced playback fails; there is no separate
browser-controls toggle. Navigation pauses audio and releases its resource, timers, and listeners.
The initial card reserves the enhanced height, including wrapped touch controls.

Shared metadata uses `consumption` to distinguish reading, listening, and watching. A reliable
recording duration produces “10 min listen” or “10 min watch”; unknown media durations use “Listen”
or “Watch”, never the read time of show notes or a transcript. Feed-provided durations are retained
as `extra.duration_seconds`, and loaded native media can correct the article's displayed duration.
The Rust fetch/build path also reads matching YouTube player and primary AudioObject/VideoObject
metadata before playback. RSS durations share a strict parser for seconds, MM:SS, HH:MM:SS, and
ISO 8601; byte lengths, active broadcasts, and unrelated recommended videos are never durations.
Missing retained durations reuse cached original pages, with bounded requests and a one-day retry
interval for pages without usable metadata. Corrections preserve article bodies and companions;
a disposable parser receipt allows one feed reparse without rewriting unchanged data-branch state.
Finite, actively playing recordings show “Ends at …” using remaining time and playback speed.
Paused, buffering, live, or inaccessible provider players do not promise a completion time. Timing
hosts reserve a line even while empty. Provider events must match both the expected origin and
iframe window; playback metadata never requires downloading a player SDK.
Previews prefer supplied images and video posters,
then usable retained images; direct image links can supply their own preview. Older retained
images can supply a build-time thumbnail without changing the archive. A video without artwork
or a published poster does not currently have a decoded frame thumbnail.
