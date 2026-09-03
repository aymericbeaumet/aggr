# Interoperability

An aggr build is ordinary static files. Human-facing links and assets are relative, so the same
tree works at `/`, `/reader/`, or a deeper mount point. A release build also receives its public
URL; aggr uses that only where a protocol requires an absolute identity.

## Representations and discovery

Every non-empty collection—the main feed, a source, a category, or a tag—has three equivalent
streams and advertises them from its HTML:

| Route | Media type | Format |
|---|---|---|
| `atom.xml` | `application/atom+xml` | Atom 1.0 |
| `rss.xml` | `application/rss+xml` | RSS 2.0 |
| `feed.json` | `application/feed+json` | JSON Feed 1.1 |

The same names live below `sources/<slug>/`, `categories/<slug>/`, and `tags/<slug>/`. Entries use
one path-derived URN across every format, point their primary URL at the local clean-reading page,
and preserve the upstream article as provenance (`rel=via`, `isBasedOn`, or `external_url`,
depending on the format).

Each item is also available as clean HTML plus `.md`, `.txt`, `.rst`, and `.json`. HTML pages expose
self-canonical links, alternate representations, Open Graph metadata, Schema.org JSON-LD, and
microformats2 `h-feed`/`h-entry` properties. `opensearch.xml` integrates the Pagefind index with
browser search, while `sitemap.xml` (or a split sitemap index for large archives) exposes public
pages to crawlers.

## Stable and portable URLs

- Internal links never assume an origin root. They resolve relative to the generated page.
- Canonical links, feed identifiers, OpenSearch templates, and sitemaps are emitted only when the
  public base URL is known (`aggr build --release` or `--base-url`).
- Moving a published site to a new origin or mount path requires one rebuild so those absolute
  identities remain truthful.
- Hashed immutable assets may be cached indefinitely. HTML, feeds, the manifest, and `sw.js` stay
  revalidatable so new builds take effect.
- `robots.txt` is emitted only for an origin-root deployment; a robots file under a subpath cannot
  govern that host.

Protocols that require a receiving server or callback, such as Webmention and WebSub publication,
are not advertised unless aggr can complete that protocol rather than merely emit a misleading
link.
