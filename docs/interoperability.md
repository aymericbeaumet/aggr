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

## aggr network

`aggr` is the engine. An **aggr instance** is a repository containing a workflow and root aggr
config, together with the data branch and site they produce. Every aggr instance is independent
and identifies itself as a member of the same aggr network. It does not register with a central
service or send telemetry. Instead, every HTML page carries the same semantic instance type and
network identity, links its metadata with the registered
`service-meta` relation, and links the exact TOML that produced it with `via`.

The root `aggr.json` is the stable machine entry point. It names the instance and generator,
identifies the network, points to its pinned configuration and git data when known, and enumerates
feeds, archives, search, sitemap, and PWA endpoints. Its versioned schema is
[`aggr-instance.schema.json`](aggr-instance.schema.json). Schema.org JSON-LD expresses the same
membership with `WebSite.isPartOf`, while `meta[name="generator"]` and
`meta[name="aggr:network"]` make inexpensive crawler recognition possible. Canonical links and
sitemaps make each publicly linked instance crawlable by ordinary search engines; the shared type,
profile and network identifiers then let search tools recognize and consume it without executing
JavaScript. The protocol deliberately defines identity and interoperability rather than claiming a
central, exhaustive registry.

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
