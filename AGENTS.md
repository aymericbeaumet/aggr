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
- The CLI and static generator are Rust; the reader uses Svelte and TypeScript built with Vite.
  Site generation stays entirely Rust: never execute JavaScript/SSR or invoke a frontend compiler
  from the CLI or Cargo build. Compile Svelte only for browser assets during frontend development;
  embed committed, self-contained bundles so running or building aggr never requires Node. Async Rust
  work uses tokio (`JoinSet` + `Semaphore`); git is shelled out. Normal HTTP uses rustls with
  ring, installed via `http::install_crypto_provider()` before any client (tests too). The bounded
  challenge fallback uses wreq/BoringSSL; both transports share request and preservation limits.
- `config.default.toml` is the source of truth for defaults and must stay in sync with
  `config.rs` (it is embedded and parsed by a test).
- `VERSION` and `Cargo.toml` `version` must agree; releases are tags `vX.Y.Z`. The stable reusable
  workflow (`@v1`) selects the greatest published binary in its major channel at run time; binary
  releases do not move the workflow tag. Distribute binaries through GitHub Releases and mise
  using its `github:aymericbeaumet/aggr` backend.

## Layout

```
src/main.rs, cli.rs            entry + clap types
src/commands/*.rs              one file per subcommand; Project = config + sources + repo
src/config.rs, config/         aggr.toml types, safe source collections, ${ENV}, validation
src/git.rs                     worktree/orphan bootstrap, commit with trailers, push+rebase, refs
src/http.rs                    reqwest client: UA, timeouts, size cap, conditional GET, retries
src/sources/{mod,feed,html,aggr}.rs source dispatch, automatic feed/HTML discovery, aggr engine
src/content.rs                 strip → sanitize → Markdown; html_to_text, excerpt
src/model.rs                   front matter, dedupe keys, link normalization, file names, blob sha
src/store/                     the branch tree: items, state, seen, status, retention, front matter
src/site/                      build orchestration, template context, minijinja env, outputs
themes/default/                embedded theme (templates/, static/)
tests/cli.rs                   end-to-end: bare origin + clone + httpmock + the real binary
docs/git-model.md              the branch/ref contract; readme.md is user-facing
```

## Commands

```sh
make check                                   # Rust checks and frontend type/tests; npm ci --prefix web first
make client-build                            # rebuild committed embedded client assets after frontend edits
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
- `themes/default/static/swup.js` is the vendored Swup 4 UMD build; keep `swup.LICENSE` beside it
  and keep navigation progressively functional without JavaScript.
- A normal source URL is intentionally enough: keep HTML heuristics internal and remember the
  discovered feed endpoint. `type = "html"` and site-specific selectors are not public config.
- Article ingestion must not launch a browser or embed Python. Detect challenges by the
  `cf-mitigated: challenge` response header, never by scripts also present in real articles.
  Preserve aggr's identity and configured headers when changing HTTP transports; see
  [interoperability](docs/interoperability.md) for native build requirements.
- `sync` persists to git; `build` runs that same sync first; `dev` runs it against a namespaced
  OS cache and serves an atomic in-memory snapshot without committing or pushing.

## Reader and ingestion invariants

- Normalize source syntax into `SourceConfig` before shared resolution and deduplication; reuse
  its strict option schema. Remove ` #` comments from raw lines before trimming whitespace.
  Scope inherited collection headers to the declaring origin or repository. Ordinary remote source
  URLs must not trigger config-time network probes; opaque collection endpoints use `collection = true`.
- Preserve existing body, HTML, and preview companions when explicit refresh fills missing media;
  apply shared boundary cleanup during both fetch and rendering so old archives benefit safely.
  Remove compact bylines only from a leading prose paragraph with a matching publication date.
- Preserve explicitly captioned image figures before Readability classifies incidental IDs such as
  `replies.png` as boilerplate, and rename share-named wrappers that hold media but no share links. Bump the extraction-cache version when extraction semantics change;
  missing content already absent from stored HTML requires a fresh extraction.
  Publisher-feed reconciliation enriches media in place and records canonical dedupe aliases; keep article paths
  and hand-edited content, reject ambiguous matches, and leave repeats unchanged.
- Derive publisher/profile identity from configured or persisted source metadata, never from slugs.
  Keep upstream or configured labels distinct from categories; do not invent tags for unlabelled items.
  Model, code, and paper buttons are resource links, not topic tags; retain their destinations when
  separating them from the reader body and keep them in portable exports.
- Apply title presentation rules once in the build context and reuse them in every published
  representation, including feeds and Markdown; preserve stored originals and article bodies.
- A lead image must not repeat a body picture (compare ThumbHashes, not only URLs) and must be at
  least 800px wide. Reader headings are id anchors, never links; portable outputs keep links.
- Expand public social threads using only the original author's posts; preserve post/media order,
  strip terminal thread counters, and keep X links canonical even when xcancel supplies the data.
  Concatenate posts without separators, per-post links, or partial-thread notices: the metadata
  original link is the only pointer. Keep traversal bounded and log incomplete continuations.
- Keep source names visible below titles in feed, search, and item metadata; tags appear only
  on item pages. The feed toolbar's omission of sources does not apply to individual feed entries.
  Put separators outside links and hover targets. Share metadata typography and spacing; format
  dates before the first paint instead of reserving empty date columns.
- Use recording duration for watch/listen metadata; never substitute transcript or show-note read
  time. Completion estimates require finite active playback and must account for playback speed.
- Reserve media geometry before loading or player activation, including failure and reduced-motion
  paths. Use validated dimensions or a stable fallback ratio; keep posters until players are ready.
  Generate ThumbHashes from decoded local images and embed their validated tiny PNG previews in
  HTML; placeholders must not wait for JavaScript or a separate network request. Preserve intact
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
- Keep application and content versions separate. Feed changes update lists automatically, even
  while a release refresh is pending; only binary or effective template/static changes offer an
  app refresh. Verify background polling without synthetic navigation/reconnect events.
- Develop the client in `web/`; rebuild its committed assets instead of editing compiled `app.js`
  or `client.css`. Svelte owns explicit interactive roots; Swup owns navigation and history.
  Await page-scope disposal before content replacement, then recheck ownership after every await.
  Keep persistent controls outside the page scope. Support only the current Svelte mount contract;
  do not add legacy DOM adapters, old config aliases, or obsolete asset routes.
  Keep the real Swup instance separate from window named properties such as `<main id="swup">`.
  Svelte owns search dates/selection; static DOM helpers must not rewrite component-bound nodes.
  Keep generated HTML usable without JavaScript.
- Apply every search clause before counting or pagination. Bind runtime, chunks, and completion
  vocabulary to one index version. Offline search is ready only after its complete manifest is
  cached; index fragments never establish article/media readiness.
- Completion suggests only query operators and filter values; article hits belong exclusively to
  the result list.
- Scope completion counts to the other active search clauses and keep readable source aliases
  unambiguous. Infer searchable item types from primary content, not incidental article media.
- Keep reading and supported media inside aggr whenever practical, with accessible original-link
  fallbacks when a provider or browser prevents embedding.
  Detect interactive canvas capabilities before stripping source HTML; retain only a metadata
  marker and load live originals automatically in an opaque-origin sandbox when the reader opens
  the article, never during ingestion. Release live frames when their page scope is disposed.
  Public paper alternates must match the requested publication identity; preserve provenance and
  never replace readable content with a challenged document embed. See [OpenReview](docs/openreview.md).
- Keep provider embed URLs validated and permissions minimal; do not cache missing captions as a
  successful transcript or promise privacy controls that the provider does not support.

Podcast URL resolution and provider limits are documented in [podcast sources](docs/podcasts.md);
shared pipeline limits and cache boundaries are in [performance](docs/performance.md).

See [the theme contract](docs/themes.md) for reader behavior and
[interoperability](docs/interoperability.md) for source and preservation boundaries.
See [client development](docs/client.md) for frontend commands, ownership, and search contracts.
