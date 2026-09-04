# Interoperability

An aggr build is ordinary static files. Human-facing links and assets are relative, so the same
tree works at `/`, `/reader/`, or a deeper mount point. A release build also receives its public
URL; aggr uses that only where a protocol requires an absolute identity.

The data repository and generated site do not depend on the same provider. Standard Git stores and
replicates the append-only branch, while any static host can serve the rendered tree. GitHub adds a
turnkey workflow, Pages deployment, config discovery shortcuts, and commit-pinned web links around
that portable core.

## A local snapshot and its original

An item page identifies the readable copy held by one aggr instance. Its canonical URL therefore
points to itself, not to the upstream article. An upstream canonical would tell search engines to
consolidate the local page into the original and would work against finding the copy when the
original disappears.

Provenance is expressed separately and consistently:

- the exact original URL and first capture time are visible on the article page, along with the
  local replication time when it was copied from another aggr;
- HTML uses `rel=original` and `rel=via`, while the visible link has the microformats2
  `u-bookmark-of` property;
- Schema.org JSON-LD models the local page as a `WebPage` plus `ArchiveComponent`, joined to the
  upstream `CreativeWork` with `isBasedOn` and `archivedAt`;
- Atom and RSS use `rel=via`, while JSON Feed uses `external_url`;
- Markdown front matter keeps `link` and `first_seen` beside the saved content;
- `linkset.json` maps original URL anchors to their local copies and maps each copy to its
  alternate representations.

The original URL is deliberately not treated as another representation of the local page, nor is
it put in the sitemap: sitemap entries must be local public URLs. A normalized form helps local
lookup, but the exact URL recorded with the item remains the provenance source of truth.

The current item route is a stable reading page whose presentation can change when the site is
rebuilt. aggr does not call it a Memento: a conforming Memento also needs an immutable capture URI
and response headers such as `Memento-Datetime`, which a portable static deployment—particularly
GitHub Pages—cannot always provide.

## Representations and feeds

Every published, retained item is available as clean HTML plus `.md`, `.txt`, `.rst`, and `.json`.
The HTML page advertises its alternate representations, Open Graph metadata, Schema.org JSON-LD, and
microformats2 properties. The representations share one path-derived item identity while the
upstream URL remains provenance.

Every non-empty collection—the main feed, a source, a category, or a tag—has three equivalent
streams and advertises them from its HTML:

| Route | Media type | Format |
|---|---|---|
| `atom.xml` | `application/atom+xml` | Atom 1.0 |
| `rss.xml` | `application/rss+xml` | RSS 2.0 |
| `feed.json` | `application/feed+json` | JSON Feed 1.1 |

The same names live below `sources/<slug>/`, `categories/<slug>/`, and `tags/<slug>/`. Feed entries
point their primary URL at the local clean-reading page and carry the original URL through the
format's provenance field.

## Discovery and URL lookup

A release build emits standard, server-rendered pages; crawlers do not need JavaScript. It also
emits:

- `sitemap.xml`, or a sitemap index plus chunks for a large retained archive;
- `opensearch.xml` for the local Pagefind search UI;
- `linkset.json`, an RFC 9264-shaped set of typed relationships between originals, local copies,
  and alternate representations;
- `aggr.json`, the stable machine entry point for an aggr-aware client;
- `llms.txt`, a short inventory of the site's public resources.

HTML advertises the descriptor and linkset. `aggr.json` names the instance and generator, identifies
the aggr network, and enumerates feeds, collections, search, sitemap, PWA, and linkset endpoints.
When GitHub repository identity is known, it also points to the pinned root config and data tree.
The human `aggr.toml` navigation link opens GitHub's commit-pinned blob page, while machine
metadata uses the raw-content URL. On another host those GitHub-specific fields are omitted and
the colocated `aggr.toml` remains the configuration entry point.

Pagefind ranks titles and clean article prose while deliberately indexing exact and normalized
original URLs. Pasting a URL can therefore find a local copy on a best-effort basis without making
URL fragments pollute article snippets. `linkset.json` provides the non-interactive equivalent for
tools that already know an instance. Neither creates a global registry: a client must first
discover the public instance.

Ordinary search engines can crawl every published, retained item through archive pagination and the
sitemap, and the visible original URL gives them a truthful relationship to index. They may
nevertheless cluster similar copies, select the live original as the representative result, or
decline to crawl an unlinked site. The custom `aggr:*` metadata is for aggr-aware clients, not a
claim that general search engines understand the aggr network. Public links and optional sitemap
submission remain the reliable ways to seed discovery.

## The aggr network

`aggr` is the engine. An **aggr instance** is a repository containing a root config, data branch,
and the site they produce; provider workflow files are optional. Every instance is independent and
can identify itself as part of the same protocol without registering with a central service or
sending telemetry.

Every HTML page carries the semantic instance type and network identity, links `aggr.json` through
the registered `service-meta` relation, and links its root configuration with `via`. Schema.org
JSON-LD expresses the same membership with `WebSite.isPartOf`; `meta[name="generator"]` and
`meta[name="aggr:network"]` allow inexpensive recognition without JavaScript. The versioned
descriptor schema is [`aggr-instance.schema.json`](aggr-instance.schema.json).

The config link identifies the tracked root file used for the build when that identity is known. It
does not embed the full effective configuration: local included files, remote included bodies,
themes, environment expansion, and the binary version remain separate inputs. Pin or vendor remote
includes when repeatable rebuilding matters.

An instance can also consume another instance's retained data through a `type = "aggr"` source.
That copies readable item content and its ultimate original URL into a second repository. It is
decentralized replication of selected current content, not synchronization of the source
repository's complete Git history.

## Stable and portable URLs

- Internal links never assume an origin root. They resolve relative to the generated page.
- Canonical links, feed identifiers, linkset targets, OpenSearch templates, and sitemaps are emitted
  only when the public base URL is known from `--base-url`, or from `[site] url` during an
  `aggr build --release`.
- Moving a published site to a new origin or mount path requires one rebuild so absolute identities
  remain truthful.
- Item paths do not change after capture. Their current static pages remain published while the
  items remain in the data tree.
- Hashed immutable assets may be cached indefinitely. HTML, feeds, the linkset, manifest, and
  `sw.js` remain revalidatable so new builds take effect.
- `robots.txt` is emitted only for an origin-root deployment; a file under a project subpath cannot
  govern that host. Absence of that file does not block crawling, but the sitemap should be
  submitted directly when the host-level robots file cannot advertise it.

Retention removes an item from the current static site and discovery outputs. Append-only Git
history still contains the old object while its ancestor commits remain reachable, but search
engines are not a Git-history recovery interface. Keep the default unlimited store retention when
the live site is intended to remain a complete archive.

## Preservation boundary

aggr preserves a safe, readable snapshot: metadata, extracted or feed Markdown, and optionally the
stripped HTML used to derive it. It does not preserve the complete HTTP exchange, executable page,
stylesheet, fonts, video, or other linked assets, and a failed extraction can leave only feed
metadata. It is therefore not a pixel-perfect mirror or a WARC archive.

Protocols that require a receiving server, callback, provider-controlled response headers, or an
immutable capture endpoint are not advertised merely because aggr could emit a suggestive link.
Webmention, WebSub publication, and Memento support should be claimed only when a deployment can
complete their full contracts.
