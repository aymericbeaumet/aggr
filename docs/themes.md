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

Article contexts also expose `item.word_count` and `item.reading_minutes`. Both are derived from
the visible Unicode text at build time, with reading time rounded up at 225 words per minute; an
empty/title-only body reports zero for each. The default theme shows reading time beside
publication, exposes the word count on hover, and emits `wordCount` plus an ISO 8601
`timeRequired` duration in structured data.

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

```jinja
{% if item.preview %}
<span class="preview-media"
      {% if item.preview.color %}style="--preview-color: {{ item.preview.color }}"{% endif %}>
  <img src="{{ item.preview.url | url_for }}"
       width="{{ item.preview.width }}" height="{{ item.preview.height }}"
       alt="{{ item.preview.alt or '' }}" loading="lazy" decoding="async">
</span>
{% endif %}
```

Reserve the thumbnail's space before it loads and let rows without images use their full width.
The default theme uses `--preview-color` as the placeholder background and reveals a loaded image
only when motion is allowed. It shows previews only in article lists and search results. Preview
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

The exact publisher master is the `img` fallback. aggr emits `picture` only when a verified
full-width WebP rendition prevents a high-density client from hitting a quality ceiling. An
existing WebP master can provide that full-width candidate directly; other conversions are used
only when the full-width result is smaller than the master. An animation or profiled/high-bit-depth
image therefore has only the master `img`. The first image in an article uses `loading="eager"`
and `fetchpriority="high"`; subsequent images use native lazy loading. Preserve
`.article-picture`, `.progressive-image`, intrinsic dimensions,
`--image-placeholder`, and these loading attributes when styling or post-processing the generated
body.

Both preview and body-image URLs are immutable local assets. The service worker keeps them in a
separate bounded image cache so they cannot evict the application shell. Only media selected by
the build's optional offline byte budget is guaranteed to be available on a first offline visit;
other images become available offline after a successful online request.

## Reader behavior

The default theme uses plain links, a mobile bottom navigation bar, system fonts, and a reading
column of approximately 65 characters. Its warm light/dark palettes, focus styling, and code
colors are defined in `static/style.css`. Syntax-highlighted code uses `syntax-` span classes;
plain code needs no client-side highlighter.

When extending the default script, keep these relationships intact:

- `#swup` is the replaced main content. Persistent navigation and connection status stay outside
  it. `#aggr-page` supplies the current page root and kind after navigation.
- Primary navigation links use `data-route` and `data-kinds` for their destination and active
  state. The mobile bar has Feed, Search, Library, and Preferences. `library.html` renders the
  unified source/tag/category directory at `/library/`. Individual collection archives stay at
  `sources/<slug>/`, `categories/<slug>/`, and
  `tags/<slug>/`.
- Article lists use `.rows .row`, with a `[data-row-open]` title link. `.is-selected` identifies the
  remembered keyboard cursor. Keep `_item.html` and `renderResult()` in `static/app.js` aligned.
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
persists only the allowlisted `aggr:<setting>` local-storage key. Existing theme, date-format, and
single-key-shortcuts keys remain compatible.

Root datasets are `theme` (auto/light/dark), `textSize` (default/large/largest), `readingWidth`
(standard/wide), `density` (compact/comfortable), `thumbnails` (show/hide), and `motion` (auto/off).
`feed-page-size` is a typed preference without a root dataset; it slices only the home feed and
keeps navigation continuous across generated static pages.
Article-size preferences affect prose, not browser zoom or navigation. Hidden thumbnails leave
article-body images intact. Turning motion off disables transitions even without an OS preference;
system reduced motion always wins.

Transfer actions share one versioned `{ "version": 1, "preferences": { … } }` JSON contract. Links
carry base64url JSON in `#aggr-state=…`, not a server-visible query; older `aggr-state` query links
remain importable. Files and links are size-bounded, allowlisted, and reviewed before explicit
application. Reading history and offline cache are never exported or reset with preferences.
Disabling single-key shortcuts leaves modifier shortcuts and native keyboard operation available.
`/` opens global search; Library and individual collection archives do not have local filters.
The [README keyboard map](../readme.md#read-comfortably) describes the controls.

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
reload requests fresh HTML. The update pill reloads the same page, preserving its query, fragment,
and scroll position. Browser refresh remains native; the installed app also supports pull-to-refresh.
Dev disables service workers so a previous deployment cannot mask a local snapshot.

Validate custom themes at narrow widths, with enlarged text, both color schemes, reduced motion,
and JavaScript disabled. Reserve the bottom bar's full height and device safe area so it cannot
cover content or keyboard focus.

## Metadata and scrolling

`_metadata.html` supplies the feed and item metadata; search uses the same field order in
`renderResult()`. The source link combines publisher identity with an optional italic `via` feed
name. Profile paths distinguish channels on shared hosts. An optional linked `/category` follows
as its own field, then publication time, item-only reading time, original link, and discussions.
Noninteractive field wrappers own evenly spaced middots, keeping them outside link underlines and
tooltips. The publication tooltip contains published and updated timestamps on separate lines;
equal instants omit the update. Exact dates are localized on interaction with cached formatters.
Tags use `#` and occupy the article header's second metadata line.

Header folding follows vertical scroll directly. The title scales with fixed line wrapping and a
measured height so a two-line title cannot abruptly become one line. Tags gradually disappear,
while the metadata remains. A 48px linear fade below the header softens content passing underneath;
the one-pixel separator fills left to right with linear page progress. Header, prose, and
continuation cards use the same reading measure.

The first visible feed row is selected automatically. Desktop selection is an immediate warm left
bar; mobile hides it. The new-item background fades independently. Separator and highlight edges
share geometry, and rank-width metadata belongs to the whole list so single- and double-digit
ranks cannot move individual rules. Clicking row background opens its item, while individual links
retain their own destinations and text remains selectable.

## Video and lead media

List thumbnails and full article images serve different purposes. `item.article_preview` supplies
a retained lead image only when that image, or an equivalent retained master, is absent from the
rendered body. This covers metadata images that article extraction omits without duplicating an
existing body image. Small feed previews must not replace full article masters.

YouTube item pages initialize a paused native player without autoplay. Twitch and Vimeo load after
activation. Embed URLs come from validated provider identifiers; YouTube uses its privacy-enhanced
domain, Vimeo uses DNT, and Twitch requires the current hostname as `parent`. These options do not
guarantee that providers never set cookies or show recommendations. A creator's published poster
can differ from the frame visible after playback begins.
