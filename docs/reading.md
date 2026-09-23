# The reader

The header links to **browse** (categories, sources, and tags), **preferences**, and `aggr.toml`.
On mobile, feed, browse, and preferences sit in the bottom tab bar; feed numbers are hidden.
Search sits above the first feed item and filters in place. Choosing a category, source, or tag
fills the search field on the main feed; its URL can be shared. Static archives remain available
without JavaScript.

Formulas written as dollar-delimited TeX (arXiv abstracts, for example) are shown as readable text:
common commands, Greek letters, relations, fractions, roots and scripts become Unicode at build
time, without a formula engine. A formula using notation the translator does not know is shown as
its TeX source in a code span instead of a partial translation, and dollar signs around ordinary
prose stay prose.

## Search

Combine full text with `source:`, `category:`, `tag:`, `date:`, `sort:`, quoted phrases, and
exclusions such as `-tag:sponsored`. Completion suggests real archive values and dates; hover over
the field for syntax help. See [client development](client.md) for the search contract.

## Sharing a passage

Selecting text inside an article offers a **Share** action above the selection. The link it copies
carries the selected word range in its fragment (`#selection=…`), so whoever opens it lands on the
article with the same words selected. Selecting is ephemeral: the address bar is never rewritten
while you select, so a shared link you arrived through stays shareable as it is, and only the link
the Share action copies describes your current selection. The toolbar answers your own gesture, so
opening a shared link shows the passage selected without it. Nothing is sent anywhere: the range is
resolved in the reader from the article's own text.

## Mobile

Feed, browse, and preferences sit in a compact bottom tab bar above the home indicator.
The feed search field stays pinned below the header while its results scroll.
The current tab has a rounded selected background. Links read by colour rather than an underline, and taps do
not flash the platform's default highlight.

Swipe left on article prose to open the next older article, or right for the previous newer one.
Moving past either end returns to the main feed. Links, media, code, and text selection keep their
own gestures.

## Preferences

Preferences controls theme (system/light/dark/sepia), text size and typeface, line width, line and
paragraph spacing, indentation, alignment, letter and word spacing, feed density and thumbnails,
dates, page size, motion, keyboard shortcuts, and d/u scroll distance. Paragraph indentation is
off by default. Settings stay on the device; all initial values can be set in `aggr.toml`:

```toml
[site.preferences]
paragraph_indent = false
font_family = "serif"
line_spacing = "relaxed"
scroll_amount = 10 # lines, capped at half the viewport
```

See [`config.default.toml`](../config.default.toml) for every setting.

Under **Transfer preferences**, share or copy a link, save a JSON file, or import one on another
device. Links and files include only these settings, not reading history, and imported settings
need confirmation before applying. Preference links keep their payload in the URL fragment so
it is not sent to the server. Older preference links still work. Reset restores the defaults
without clearing reading history. Resetting also restores the site's offline-download count.

## Offline reading

In **Offline reading**, choose 0–1000 recent articles to keep and wait for the available count to
finish. Their pages, previews, and retained image renditions are downloaded together, in browser
tabs and installed PWAs. Incomplete downloads are reported and retried when reconnecting. Browser
storage limits still apply; external images and embedded audio/video are not downloaded.
Any positive limit also saves the complete archive search index, with readiness reported
separately.

## Keyboard

| Keys | Action |
|---|---|
| `Cmd+K` / `Ctrl+K`, or `/` | Focus global search; the help names whichever modifier this platform presses. |
| `↑` / `↓`, then `Enter` in search | With suggestions open, choose and accept one; otherwise move the result cursor and open the selected result. `Escape` closes suggestions, then removes focus. |
| `j` / `k`, or `↓` / `↑`, in article lists | Select the next / previous item; the first press selects the first item. Article pages keep the arrows for scrolling. |
| `gg` / `G` | Select the first / last visible feed item; scroll to the top / bottom on article pages. |
| `o` / `Enter` in article lists | Open the selected item. |
| `j` / `k` in articles | Open the older / newer article. Moving past either end returns to the feed. |
| `O` | Open the original for the selected feed item or current article. |
| Uppercase network key | Open the selected/current item's matched discussion, or search that enabled network for its original URL. Built-ins use `H`, `R`, and `X`. |
| `u` / `d` in articles | Scroll up / down. |
| `g f`, `g l`, `g p` | Feed, browse, preferences. |
| `g 1` … `g 9` | Open one of the first nine feed entries. |
| `?` | Show keyboard help. |

List selection and reading position return with Back. Normal Tab and Enter behavior remains
available, including on original and discussion links. Single-key shortcuts are on by default
and can be disabled in Preferences; `Cmd+K` / `Ctrl+K` and standard keyboard controls still work.
Keyboard focus uses an underline or surface change, without a page-sized focus border.

## Updates and installation

The reader checks for deployments every 15 seconds while visible and online, and checks again when
it returns to the foreground or reconnects. New articles appear automatically in open feeds and
search results, preserving selection and reading position. An open article stays in place.
Application releases show a separate “Refresh to update” pill; feed updates continue while that
pill is visible. Browser refresh and the installed app's pull-to-refresh remain available.

New-item highlights and the title/favicon dot use tab-local state that disappears when the tab
closes. Normal browser tabs request a new tab for external links; installed apps hand out-of-scope
links to the platform's external or in-app browser UI. Presentation varies by operating system.
Installation and service-worker caching require HTTPS (localhost is the development exception).
A static aggr site cannot wake a closed app with background push. If an older installation used a
site under a path such as `/reads/`, removing and reinstalling it may be necessary to adopt the
corrected app identity and scope.
