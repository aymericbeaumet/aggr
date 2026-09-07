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
empty/title-only body reports zero for each. The default theme presents them in the article
metadata and emits `wordCount` plus an ISO 8601
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
  unified source/category/tag directory at `/library/`. Individual collection archives stay at
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
`/` opens global search from the river when no local filter is present; Library and individual
collection archives keep filtering local. The [README keyboard map](../readme.md#read-comfortably)
describes the controls.

Keep keyboard focus visible with an underline or contrasting surface. The default script marks
programmatic main focus with `data-navigation-focus`, suppressing only the large container
outline after a page change. Ordinary links, form controls, and the skip link must remain usable
with a keyboard.

Native View Transitions provide a short main-content fade when supported. They leave persistent
navigation stationary, honor reduced motion, and do not animate history traversal. Browser
refresh remains native; the installed app adds pull-to-refresh. A service-worker update shows a
ready state and takes effect at the next deliberate navigation or refresh without interrupting
the current article.

Validate custom themes at narrow widths, with enlarged text, both color schemes, reduced motion,
and JavaScript disabled. Reserve the bottom bar's full height and device safe area so it cannot
cover content or keyboard focus.
