# aggr

Git-native feed reader: a Rust CLI that turns `aggr.toml` into a static site, with every
fetched item stored on an append-only orphan git branch. Shipped as prebuilt binaries, a
composite GitHub Action (`action.yml`, install only) and a reusable workflow
(`.github/workflows/aggr.yml`) that runs sync → build → Pages deploy.

## Hard rules

- The data branch is append-only. Nothing may rewrite, squash or force-push it; every
  `blob/<sha>/…` URL must stay valid forever. Retention deletes files in normal commits only.
- A run that finds nothing new must leave no trace: no commit, no push, no state rewrite.
- Nothing is ever committed to `main` by the tool. Worktree (`.aggr/`) and output (`_site/`) go
  to `.git/info/exclude`.
- One failing source never fails the run; only config errors, git/IO errors and "every source
  failed" exit non-zero. Source errors surface through `status.toml` on transitions only.
  After the data branch is saved, an auxiliary recovery-pointer failure warns without blocking
  publication; never force-push an unrelated or newer pointer to make it succeed.
- Stored `.html` and rendered pages are safe by construction: `content.rs` strips scripts,
  handlers and `data:`/`javascript:` URLs before storage; ammonia sanitizes before display;
  comrak renders with raw HTML off. Never serve stored HTML unsanitized.
- No data leaves the user's repository except the fetches they configured.
- Fetching is the only step that touches the network: after a sync the build is hermetic, a pure
  function of the archive and the binary. Rendering must never make a request, so anything a page
  needs — article text, images, previews, durations, discussions, the search index — is resolved
  during the fetch and stored. A build that would need the network drops the feature instead.
- The CLI and static generator are Rust; the reader is hand-written HTML, CSS and JavaScript with
  no build step and no dependencies. Site generation stays entirely Rust: never execute
  JavaScript/SSR or invoke a frontend compiler from the CLI or Cargo build. Never reintroduce a
  frontend toolchain, a framework, or a vendored browser library. Async Rust
  work uses tokio (`JoinSet` + `Semaphore`); git is shelled out. Every CPU-bound stage is sized from
  the machine's parallelism, never a fixed slot count, and stages that run at the same time share it
  rather than each claiming the whole machine. Normal HTTP uses rustls with
  ring, installed via `http::install_crypto_provider()` before any client (tests too). The bounded
  challenge fallback uses wreq/BoringSSL; both transports share request and preservation limits.
- `config.default.toml` is the source of truth for defaults and must stay in sync with
  `config.rs` (it is embedded and parsed by a test).
- `aggr init` embeds `examples/starter.toml`: keep that small, explicit starter distinct from
  the full defaults. Retention bounds the current tree, never accumulated Git history.
- Search-engine indexing is opt-in with `[site] indexing = true` in release builds; development
  and previews remain noindex. Preserve local search and instance discovery regardless.
- Build budgets preserve all article text and leave archived media untouched. A build that fits records what it
  needed for everything but media, so the next one starts there instead of measuring the overflow
  with a whole extra build; the exact retry still decides whether that guess was right. The budget
  is a publishing limit: only a release build measures it, because measuring an over-budget archive
  costs a second complete build, and a development snapshot is never published. `dev --release`
  still applies it. Admit complete media
  families newest first, measure the complete output, and fail if text and required assets cannot
  fit. Include publication cache markers and recheck restored output. Cache compressed publication
  copies separately from sync state; see `docs/build-budget.md`.
- aggr is source-available under FSL-1.1-ALv2: each version becomes Apache-2.0 two years after
  its release. Prior MIT releases stay MIT.
  Preserve third-party notices and follow `CONTRIBUTING.md` for incoming contribution rights.
- `VERSION` and `Cargo.toml` `version` must agree; releases are tags `vX.Y.Z`. The stable reusable
  workflow (`@v1`) selects the greatest published binary in its major channel at run time; binary
  releases do not move the workflow tag. Distribute binaries through GitHub Releases and mise
  using its `github:aymericbeaumet/aggr` backend.

## Layout

```
src/main.rs, cli.rs                       entry + clap types; AGGR_CONFIG and AGGR_BASE_URL env defaults
src/commands/mod.rs                       Project = config + sources + repo; the .aggr/ lock and worktree
src/commands/sync.rs                      fetch, commit with trailers, push, move refs/aggr/last-good
src/commands/build.rs                     sync then render, or render a pinned --data-ref
src/commands/dev.rs                       isolated cache, in-memory snapshot, watch and live reload
src/commands/fetch.rs                     shared fetch stage: per-source workers, heavy content, persistence
src/commands/fetch/index.rs               one archive pass: stored URLs, paths, reconciliation indexes
src/commands/fetch/links.rs               cross-source URL reservations for overlapping feeds
src/commands/fetch/plan.rs                what an item becomes on disk, decided before any write
src/commands/fetch/transaction.rs         per-source file transaction with rollback
src/commands/fetch/repair.rs              post-feed repairs: missing media, feed-only capture upgrades
src/commands/fetch/tests.rs               the fetch stage's unit tests
src/commands/fetch/duration.rs            feed-parser receipts that gate conditional GET for durations
src/commands/fetch/openreview.rs          public paper metadata instead of the challenged web app
src/commands/fetch/podcast.rs             publisher-feed reconciliation of streaming-show items
src/commands/fetch/recording.rs           bounded recording-duration probes with daily backoff
src/commands/check.rs                     probe every source once; non-zero when any fails
src/commands/clean.rs                     provably disposable targets only
src/commands/init.rs                      aggr.toml and the optional GitHub workflow
src/commands/lock.rs                      non-blocking advisory lock (.aggr/aggr.lock, dev.lock)
src/commands/server.rs                    the dev HTTP server and its reload stream
src/config.rs, config/                    aggr.toml types, imports and collections, ${ENV}, BCP 47 tags, preferences, validation
src/git.rs                                worktree/orphan bootstrap, commit with trailers, push+rebase, refs
src/http.rs, http/transport.rs            reqwest client (UA, timeouts, size cap, conditional GET, retries) + wreq challenge fallback
src/sources/mod.rs                        engine dispatch by URL predicate: instagram, qwen, podcast, then feed
src/sources/feed.rs, html.rs              RSS/Atom/JSON via feed-rs; the discovery ladder from HTML
src/sources/aggr.rs                       another aggr repository as a source
src/sources/instagram.rs                  profile pages that expose post cards
src/sources/podcast.rs                    show pages resolved to publisher feeds
src/sources/qwen.rs                       Qwen's article API behind its JavaScript blog
src/sources/youtube.rs, youtube/          Shorts filter, video URLs, posters, durations
src/content.rs, content/                  strip → extract → Markdown → safe HTML; scan, cleanup, resources
src/content_highlight.rs                  bounded build-time syntax highlighting
src/media.rs, media/                      lossless article images; ThumbHash placeholders, srcset, SVG rasters, validation receipts
src/media_duration.rs                     strict recording durations shared by feeds and players
src/preview.rs, preview/pdf.rs            bounded thumbnails, PDF first pages
src/threads.rs, threads/x.rs              ActivityPub and X thread expansion
src/discussions.rs                        discussion-network lookups with cached backoff
src/cache.rs                              cache roots, the Namespace registry, the render-fingerprint classification
src/model.rs                              front matter, dedupe keys, link normalization, file names, blob sha
src/store/                                the branch tree: items, state, seen, status, retention, front matter
src/site/mod.rs                           build orchestration: phases, media windows, the timing line
src/site/assets.rs                        content-addressed media gathered on workers, published in item order
src/site/context.rs                       the template contract
src/site/render.rs                        minijinja env with the layered template/static lookup
src/site/outputs.rs                       feeds, OPML, discovery documents, sitemaps, redirect stubs
src/site/pagefind.rs                      the search index and its cache key
src/site/parallel.rs                      ordered map over at most 8 scoped workers; AGGR_BUILD_WORKERS
src/site/{related,display,document,interactive,item_type,native_media,video}.rs  navigation and presentation contexts
themes/default/                           embedded theme: templates/ plus static/ with the hand-written client
themes/default/static/{app,search,media,bootstrap}.js  the whole client: core, lazy search, lazy media, pre-paint
types/aggr.d.ts, jsconfig.json            editor-only type checking for the client; nothing to install
tests/cli.rs                              end-to-end: bare origin + clone + httpmock + the real binary
tests/clean.rs, local_sources.rs          cleanup and local-file source scenarios
tests/support/                            shared integration helpers: git and environment isolation
tests/browser/                            WebDriver contracts: harness.rs + one module per topic
tests/browser_performance.rs              non-gating client benchmark
docs/git-model.md                         the branch/ref contract; readme.md is a short user-facing entry
docs/*.md                                 the user-facing reference the readme links out to
```

## Commands

```sh
make check                                   # fmt, clippy and the full test suite
cargo test --test cli                        # end-to-end only
make run ARGS="dev --port 3000"              # dogfood examples/aggr.toml
cargo run -- sync --dry-run -vv              # fetch without writing, with debug logs
```

## Conventions

- Pure functions for anything decidable (dedupe, file names, retention plans, status
  transitions, commit messages); IO at the edges in `commands/`, `store/`,
  `git.rs`. Add the unit test next to the function; add a `tests/cli.rs` scenario when git or
  the CLI surface is involved.
- `anyhow` with `.context()`; no `unwrap` outside tests.
- Browser fixtures expecting next/discovery cards need distinct substantial article bodies; changing
  only URLs or titles does not bypass content-identity deduplication.
- httpmock serves the first registered matching mock: `delete()` the old one before adding a
  new mock for the same path.
- minijinja: `trim_blocks`/`lstrip_blocks` are on, autoescape follows the `.html` extension, and
  the custom formatter escapes `& < > " '` only.
- A listing that is only the page it came from is not a listing: probe the conventional endpoints
  before accepting a single-page app's own shell, and prefer any feed that parses. Keep the
  subscribed origin's endpoints in the probe set when a response redirects away from it, and say in
  `check` whether items came from a feed or from cards read off a page.
- A normal source URL is intentionally enough: keep HTML heuristics internal and remember the
  discovered feed endpoint. `type = "html"` and site-specific selectors are not public config.
  An origin that publishes nothing itself is read from the section that does (`/blog`, `/news`,
  `/posts`) once every feed endpoint has failed, so the subscription can be the site; a URL that
  already names a section has said where to look and keeps it.
  A listing URL names a section, so probe conventional endpoints relative to it with or without a
  trailing slash, never at the root, and only on first resolution. A discovered feed with no
  entries loses to the listing that does have them.
- Article ingestion must not launch a browser or embed Python. Detect challenges by the
  `cf-mitigated: challenge` response header, never by scripts also present in real articles.
  Preserve aggr's identity and configured headers when changing HTTP transports; see
  [interoperability](docs/interoperability.md) for native build requirements.
- `sync` persists to git; `build` runs that same sync first; `dev` runs it against a namespaced
  OS cache and serves an atomic in-memory snapshot without committing or pushing.
- Every `src/sources/*` adapter documents its accepted URL contract in
  [interoperability](docs/interoperability.md#source-adapters); `sources/mod.rs` dispatches on
  those predicates in order.
- Every new `src/**/*.rs` file is classified in `src/cache.rs`: list it in
  `render_implementation_sources()` when it can change rendered bytes, otherwise in
  `RENDER_INDEPENDENT_SOURCES`. The classification unit test fails until it is.
- Cache directories are registered in `cache::Namespace`; a unit test pins the reusable workflow's
  `actions/cache` path list to that registry, so a new namespace changes both.
- The browser suite (`tests/browser/`) is one module per topic, and every test builds its own
  fixture site and Chrome session. `AGGR_BROWSER_TIMEOUT_SECS` lengthens its waits on a loaded
  machine; `AGGR_BUILD_WORKERS` pins the build's worker count.

## Reader and ingestion invariants

- Normalize source syntax into `SourceConfig` before shared resolution and deduplication; reuse
  its strict option schema. Remove ` #` comments from raw lines before trimming whitespace.
  Scope inherited collection headers to the declaring origin or repository. Ordinary remote source
  URLs must not trigger config-time network probes; opaque collection endpoints use `collection = true`.
- Cleanup that only needs the Markdown body belongs in the build, where it also reaches archives
  written by an older version; cleanup that needs the original HTML belongs in capture. A build
  never rewrites a stored body, not even for whitespace. `--reprocess` is the one path that
  re-derives stored bodies from retained HTML: explicit, idempotent, and never shortening an item
  whose companion was truncated.
- Every cleaning rule carries a test built from the markup that motivated it, named in a comment,
  plus the neighbouring case it must not touch.
- Rebuild document semantics Markdown cannot express before conversion, not after: numbered
  listing tables become code blocks, endnote lists and their references become Markdown footnotes,
  figure captions become a hard break the renderer regroups into `<figure>`, and a headerless table
  gains an empty header so it stays a table. An `<audio>` element has no player in the reader, so
  drop it with the chrome around it. Leave a code block unlabelled rather than guessing its
  language; a publisher's own name for a language outranks the grammar used to colour it.
- Preserve existing body, HTML, and preview companions when explicit refresh fills missing media;
  apply shared boundary cleanup during both fetch and rendering so old archives benefit safely.
  The leading-metadata walk steps over a publisher's hero pictures rather than into them, so page
  chrome below one is still reachable and the picture is never the price of reaching it. A follow
  widget's label survives its button as a colon introducing nothing; a label above the list, quote,
  picture or link it announces is doing its job and stays.
  Remove compact bylines only from a leading prose paragraph with a matching publication date.
- Preserve explicitly captioned image figures before Readability classifies incidental IDs such as
  `replies.png` as boilerplate, and rename share-named wrappers that hold media but no share links.
  Script-drawn charts with inline data become tables; feed-only captures are retried with a daily,
  bounded backoff and upgraded in place when the original page becomes available. Bump the extraction-cache version when extraction semantics change;
  missing content already absent from stored HTML requires a fresh extraction.
  A formula is a formula wherever a page put it: MathML with a TeX annotation, a `math` class, or
  `$…$` in the prose. One command the bounded translator does not know leaves the whole formula as
  its source, so a command it can read as text belongs in `src/content/math.rs`.
  Markdown must be able to say what the HTML said: an inline wrapper never holds a block, emphasis
  markers flank their content or move aside, and markers with nothing to mark are dropped rather
  than written into the prose. `src/content/markdown/fuzz.rs` generates documents to hold the
  conversion to that, and points the same oracle at a real archive through `AGGR_CORPUS`.
  An article URL that cannot name its own account (a YouTube watch URL) has one read from its page
  during the fetch it already makes and stored beside the item; a captured account on another host
  is discarded, never shown. Markdown link labels are inline: an anchor wrapping blocks keeps them
  and links only its leading run.
  Notes the prose cites by number are content however link-dense they are; sibling note blocks
  with ids (not only `<li>` lists) become Markdown footnotes, and an unreferenced or empty note
  leaves its reference an ordinary link. A note's links back to the places citing it are
  navigation the footnote already carries, whether labelled by return glyph or by number
  (`↑ 1.00 1.01 …`); links pointing anywhere else are what the note says. A page whose files are line tables (GitHub gists) is
  archived as those files, never as the discussion under them.
  Reject positively identified subscription offers as article bodies. Public archive recovery requires
  matching original URL, title, and readable content; keep requests bounded and backed off. Preserve
  original provenance, reject archive lookup pages as snapshots, and show an honest fallback on failure.
  Capture code-language hints before Readability strips classes; preserve explicit plain text and
  legacy inline code. Decode publisher email-protection payloads into escaped text, never markup.
  Publisher-feed reconciliation enriches media in place and records canonical dedupe aliases; keep article paths
  and hand-edited content, reject ambiguous matches, and leave repeats unchanged.
- Canonical publisher IDs are normalized article hostnames: lowercase/punycode, without trailing dots
  or conventional `www.`, ports, or cross-domain provider aliases. Preserve other subdomains. A host
  shared between publishers identifies none of them, so there the account path is part of the ID,
  the page (`sources/youtube.com/@channel/`) and the filter value: the label a reader clicks and the
  source they land on are the same publisher. An article URL settles the account where it names one;
  where it does not, the source's own resolved metadata does, and only for that article's host.
  Preserve configured/persisted feed identities and profile names as provenance; never migrate
  stored source IDs to publisher IDs. Canonical articles carry deduplicated publisher/feed
  memberships and stay unique globally. Visible labels use a canonical name: a hostname alone, plus
  the account path only on the platform hosts listed in
  `src/platform.rs`, where one domain is shared between unrelated publishers. `src/platform.rs` is
  the only place that knows host aliases, account path shapes, or which hosts use opaque
  identifiers; never re-derive any of that elsewhere. A publisher label names the account the
  article itself identifies, so it reads the same whoever linked it; only where nothing names one
  does the subscription that carried it, and then the bare host, stand in. A `via` label names the
  subscription. Grouping an item under its publisher host must not overwrite its label with the
  bare host. A derived source slug
  is that same canonical name, disambiguated by the differing feed path when two sources collide. Exact source IDs win manual alias collisions. See [client development](docs/client.md).
  A readable alias never replaces a source in a query: its canonical name is already readable.
  The source directory lists whole sites and feeds: an account nobody configured keeps its page and
  its filter value but is not listed (`sources[].listed`).
  Public source IDs and filter values use canonical publisher names only; group same-publisher subscriptions
  while preserving their archived IDs, article paths, and individual OPML endpoints. Display names
  must never replace hostnames in generated queries.
  Keep upstream or configured labels distinct from categories; do not invent tags for unlabelled items.
  Model, code, and paper buttons are resource links, not topic tags; retain their destinations when
  separating them from the reader body and keep them in portable exports.
- A shared link unfurls as the article: `og:`/`twitter:` descriptions carry the excerpt, while the
  archival framing stays in `description` for crawlers. An aggregator's machine summary
  (`Article URL: … Points: …`) describes the submission rather than the article and is dropped
  however the body was captured, so it never stands in for an excerpt.
- Apply title presentation rules once in the build context and reuse them in every published
  representation, including feeds and Markdown; preserve stored originals and article bodies.
  A publisher writing in Markdown can emit the markers with its headline: emphasis wrapping a whole
  title comes off, while markers around part of one are the author's own and stay.
  A leading heading that restates the title is a duplicate: aggregators reword what they syndicate,
  so a longer title may differ by a word in four, while short ones must still match word for word.
  A document's own heading may also run the title through a subtitle or a year marker; a deeper
  heading has to match outright, and a heading holding a destination stays whatever it says.
- Article suggestions are resolved once, and every one of them is meant to be shown: never filter
  them again downstream. They skip the chronological neighbours the page already links, and avoid
  pointing back at an article that already suggests them so browsing never closes a two-page loop.
- A lead image must not repeat a body picture (compare ThumbHashes, not only URLs) and must be at
  least 640px wide. Reader headings are id anchors, never links; portable outputs keep links.
- A publisher that flattens an embedded post writes it as loose paragraphs: the poster's avatar
  linking to the post, their name, what they said, any post they quoted, and the provider's
  counters. Read those back as the quote they were, nested quote included, and leave the avatars
  and counters behind; the prose after them is the publisher's again.
- Expand public social threads using only the original author's posts; preserve post/media order,
  strip terminal thread counters, and keep X links canonical even when xcancel supplies the data.
  Concatenate posts without separators, per-post links, or partial-thread notices: the metadata
  original link is the only pointer. Keep traversal bounded and log incomplete continuations.
- New items arrive without a reload: a list page polls `updates.json`, on an interval and whenever
  the window is activated, and swaps its rows for the current ones when the content version moves.
  The swap waits for the top of the list unless the reader has just come back to the window, so it
  never moves the ground under them. The page carries the version it was built from.
- Following a footnote, or its way back, marks both halves: the note and the reference. `:target`
  carries the half the fragment names without JavaScript; the reader pairs the other, which is the
  margin note when the notes list is off screen.
- A source chip navigates to the collection page the build already wrote, not to a query the
  browser has to answer: same list, no index to download. Modules a page can use load with the
  page, and the search engine warms its index on mount rather than on the first keystroke.
- Keep source names visible below titles in feed, search, and item metadata; tags appear only
  on item pages. The feed toolbar's omission of sources does not apply to individual feed entries.
  Put separators outside links and hover targets. Share metadata typography and spacing; format
  dates before the first paint instead of reserving empty date columns.
- Use recording duration for watch/listen metadata; never substitute transcript or show-note read
  time. Completion estimates require finite active playback and must account for playback speed.
- Reserve media geometry before loading or player activation, including failure and reduced-motion
  paths. Use validated dimensions or a stable fallback ratio; keep posters until players are ready.
  Generate ThumbHashes from decoded local images and embed their validated tiny PNG previews in
  HTML; placeholders must not wait for JavaScript or a separate network request. Archive every
  raster format that decodes without a system library. AVIF needs an AV1 decoder and is archived
  undecoded instead: exact bytes, the size its `ispe` box states, and a flat stand-in preview, so
  the picture and its geometry survive without a new build dependency. Preserve intact
  local image masters when repairing missing companions, and back off failed downloads.
  Parse `srcset` using URL and descriptor boundaries: CDN URLs may contain literal commas.
  Preserve exclamations before links as prose when converting HTML to Markdown; article footnotes
  are not image URLs. Recover malformed archived CDN aliases only from unambiguous saved source sets.
  Preserve fractional, untransformed header heights; integer measurements can shift media on load.
  Animate reading progress on its own transform; avoid per-frame inherited variables on the header.
- Offline readiness means an article page and all its retained image renditions are cached.
  Keep selected downloads separate from evictable runtime caches and report partial/quota failures.
- Precompute display data during builds. Keep Pagefind display metadata opaque: even zero-weight
  metadata can pollute search results. Bound navigation caches and speculative requests.
  Prepare shared template values once per build; preserve full archive access for custom themes.
  Completion uses the compact catalogue; initialize the full search index only when needed.
- Preserve pristine page HTML before client enhancement, and restore keyboard selection by URL
  across navigation and staged search results. Avoid layout shifts when selection or headers change.
- Keep mobile navigation fixed to the viewport bottom. Apply the safe-area inset once inside the
  bar and derive content clearance from its measured height; never stack extra safe-area spacers.
  Skip navigation animations and bound intent/idle prefetch; uncached navigation remains progressive.
  Keep feed search pinned below the measured header, with results outside the sticky wrapper.
  Touch tabs activate once on release with immediate contact feedback. Article swipes must yield to
  selection, vertical scroll, pinch zoom, controls, horizontal scrollers, and browser edge gestures.
  Never apply `touch-action: pan-y` to an ancestor containing horizontal scrollers.
  Share keyboard/swipe article destinations; a missing neighbor returns to the main feed.
- The client is four hand-written files edited in place: `bootstrap.js` (pre-paint preferences and
  dates, the only render-blocking script), `app.js` (every page), and `search.js` and `media.js`,
  imported on demand. Each opens with `// @ts-check`; `types/aggr.d.ts` declares the contracts they
  share with the templates. Never add a bundler, a framework, an npm dependency, or a vendored
  browser library, and never import across files by anything but a `url_for`-resolved URL from
  `window.AGGR.assets`: hashed asset names are not rewritten inside file contents.
- Reach for the platform before JavaScript: speculation rules and ordinary navigation, view
  transitions, scroll-driven animations, `<dialog>` with invoker commands, stretched links, CSS
  counters. Wrap anything not universally supported in `@supports` or a feature check that degrades
  to plain HTML. Generated HTML must stay usable with JavaScript disabled.
- A prerendered page runs before anyone sees it: gate session storage, history writes and worker
  registration behind `document.prerendering`.
- Preferences live in one typed table in `src/config/preferences.rs`, which produces both the
  browser validation rules and the rendered form. Never redeclare a setting in a template or a
  script.
- Documents carry no `<base>` element. `url_for` and `facet_url` resolve from the page being
  rendered; `site_path` keeps the root-relative form for data attributes the client resolves
  against its known root, and `item.body_html | rebase` points body media at the page. Fragment
  links such as footnotes must stay `#id` so they navigate within the current document.
- Apply every search clause before counting or pagination. Bind runtime, chunks, and completion
  vocabulary to one index version. Offline search is ready only after its complete manifest is
  cached; index fragments never establish article/media readiness.
- Completion suggests only query operators and filter values; article hits belong exclusively to
  the result list.
- Scope completion counts to the other active search clauses and keep readable source aliases
  unambiguous. Infer searchable item types from primary content, not incidental article media.
- Keep reading and supported media inside aggr whenever practical, with accessible original-link
  fallbacks when a provider or browser prevents embedding. Activating a provider facade plays at
  once: ask the player directly instead of trusting an `autoplay` parameter.
- Keep PDFs as bounded, validated item companions; preserve them across refresh and replication.
  Admit same-origin document URLs through the media budget and include retained copies in offline
  resources. Serve `application/pdf`, including in dev previews. Preserve the publisher fallback
  and portable caption links; see
  [PDF preservation](docs/interoperability.md#pdf-preservation).
- A shared selection addresses words, not DOM offsets, so the link survives a rebuild. Keep the
  range in the fragment, update it live while the selection changes, and clear it when it empties.
  The toolbar answers the reader's own gesture, never a restored selection, and scrolling with a
  selection repositions it without re-deriving it: index the article once per page scope.
- Mobile is a platform surface: a compact tab bar above the home indicator, colour rather than
  underline for the current tab and for links, and no default tap highlight.
  A page whose figures are mounted by scripts has no pictures in its HTML either: two or more
  media-less figures naming a module are read the same way as a canvas application.
  Detect interactive canvas capabilities before stripping source HTML; retain only a metadata
  marker and load live originals automatically in an opaque-origin sandbox when the reader opens
  the article, never during ingestion. Release live frames when their page scope is disposed.
  Public paper alternates must match the requested publication identity; preserve provenance and
  never replace readable content with a challenged document embed. See [OpenReview](docs/openreview.md).
- Keep provider embed URLs validated and permissions minimal; do not cache missing captions as a
  successful transcript or promise privacy controls that the provider does not support.

Podcast URL resolution and provider limits are documented in [podcast sources](docs/podcasts.md);
shared pipeline limits and cache boundaries are in [performance](docs/performance.md).

Source configuration is documented in [sources](docs/sources.md), the reading experience in
[reading](docs/reading.md), and deployment in [hosting](docs/hosting.md).

See [the theme contract](docs/themes.md) for reader behavior and
[interoperability](docs/interoperability.md) for source and preservation boundaries.
See [client development](docs/client.md) for frontend commands, ownership, and search contracts.
