# Reader feature parity

The reference is commit `50d91c2`, the last audit of the first Svelte reader on 2026-09-16. The
current reader is again a Svelte application, now rendering from the typed page model that every
static page embeds instead of from the DOM; Rust still renders every page. Later publisher
identities, native navigation, and responsive layout changes remain in place.

Historical paths below are relative to that commit; the current client lives in `web/src/`.
Behavioral tests run with `make client-check`; the browser contracts under `tests/browser/` use
the generated site and a real Chrome session.

| Contract | Reference implementation | Current implementation and evidence |
|---|---|---|
| In-place navigation, metadata replacement, keyboard cursor, Back position and progressive links | `web/src/reader.ts`, `navigation.ts`, `selection.ts` | `web/src/navigation/`, `app/boot.ts`, `state/selection.svelte.ts`; `tests/browser/navigation.rs`, `article.rs`, `no_js.rs` |
| Bounded prefetch with slow-connection opt-out; fixed mobile controls and keyboard clearance | `navigation.ts`, `reader.ts` | `web/src/navigation/speculation.ts`, `app/platform.ts`; `tests/browser/mobile.rs`, `navigation.rs` |
| Feed slicing, page-size preference and URL-preserving pagination | `reader/pagination.ts` | `web/src/feed/paging.ts`, `feed/Pager.svelte`; `tests/browser/article.rs`, `feed.rs` |
| Full query grammar; phrases and exclusions applied before counting/pagination | `search/query.ts`, `search/engine.ts` | `web/src/search/query.ts`, `engine.ts`; `web/src/search/*.test.ts`, `tests/browser/search.rs` |
| Incomplete qualifiers hide stale rows; completion has scoped counts and stable option identities | `search/index.ts`, `completion.ts`, `completion-context.ts` | `web/src/search/completion.ts`, `Completions.svelte`; completion tests and browser incomplete-query/context tests |
| Catalogue-only startup; shared two-version runtime cache; pending queries outlive eviction safely | `search/engine.ts` | `web/src/search/engine.ts` session leases; engine tests |
| Search keyboard selection, history restoration, disposal and content refresh | `search/index.ts`, `query.ts` | `web/src/search/controller.ts`, `Results.svelte`, `state/selection.svelte.ts`; browser search history/popstate contracts |
| Search optional point/comment counts, including zero | `search/Metadata.svelte`, `display.ts` | `web/src/feed/Metadata.svelte` from the `ClientRow` both sides share (`src/site/client.rs`); parity fixtures |
| Preferences before paint, typed defaults, cross-tab sync, import review, export/share/reset | `preferences/`, inline bootstrap | `web/src/bootstrap.ts`, `preferences/`, `state/preferences.svelte.ts`, `src/config/preferences.rs`; `tests/browser/preferences.rs` |
| Passage links use the same half-open word-range fragment and restore the selection | `share-selection.ts` | `web/src/article/selection-share.ts`; its unit tests and `tests/browser/restoration.rs` |
| Selecting is ephemeral: the toolbar copies a link without changing the address bar; arrival does not display the toolbar | `share-selection.ts`, `docs/reading.md` | `web/src/article/selection-share.ts`, `SelectionShare.svelte`; browser restoration contract. Scroll only repositions the toolbar. |
| Native audio transport, four skip buttons, seek, speeds, mute restoration and device-volume fallback | `audio.ts`, `AudioControls.svelte` | `web/src/media/audio.ts` bound to the static item controls; `media/audio.dom.test.ts`, `tests/browser/media.rs` |
| Finite recording durations, speed-adjusted finish times, paused remaining time; buffering/live streams have no false finish time | `media-timing.ts` | `web/src/media/timing.ts`; timing tests |
| Provider facades activate once, keep poster geometry, preserve modifier clicks, and support Space/Enter | `media.ts` | `web/src/media/facades.ts`; browser media geometry/provider contracts |
| Player telemetry is bound to the exact origin/window; timers, audio/video and observers end with the page | `media.ts`, `media-timing.ts` | `web/src/media/` disposed with the article's enhancer; `article/enhance.dom.test.ts` |
| Safe PDF and interactive originals with visible fallbacks | `interactive.ts`, item template | Static item template and Rust document context; browser PDF/interactive contracts |
| Reading progress, folding article header, image placeholders and responsive footnotes | `reader/article-header.ts`, `margin-notes.ts`, `media.ts` | CSS/platform rendering, Rust margin-note generation, `web/src/article/{readingHeader,footnotes}.ts`; article/media browser contracts |
| All-page deployment polling every 15 seconds while visible and online, plus reconnect/foreground checks | `build-watcher.ts` | `web/src/updates/poll.ts`; browser restoration polling contract |
| Application-release pill is separate from content updates; refresh preserves scroll/focus | `updates.ts`, `ConnectionStatus.svelte`, `reader.ts` | `web/src/state/versions.svelte.ts`, the status bar in `app/`; browser restoration contract |
| Browser-native PWA installation (the reference had no custom install prompt/button) | Manifest template, `reader.ts` app-installed event | Manifest template, `web/src/offline/client.svelte.ts`; browser manifest identity checks |
| Installed-app pull-to-refresh and appropriate external-link behavior | `reader/pull-refresh.ts`, `reader.ts` | `web/src/offline/pull-refresh.svelte.ts`, `article/external-links.ts`; mobile/browser contracts |
| New-item state survives pagination; hidden tabs show title/favicon badges and visible rows acknowledge pending items | `reader/entries.ts`, `reader.ts` | `web/src/updates/badges.ts` scoped session state; article/feed browser contracts |
| Offline preference selects 0–1000 recent complete articles, with status and a saved-article directory | `offline-client.ts`, `offline.ts`, `preferences/Panel.svelte` | `web/src/offline/`, `preferences/Form.svelte`; offline worker and browser tests |
| Selected offline pages, all retained image renditions and local PDF companions are separate from evictable runtime caches | Worker template, offline catalogue | `src/site/offline.rs`, `web/src/sw/offline.ts`; worker tests, PDF build test, browser offline contract |
| Complete offline search manifest verifies sizes/hashes, commits atomically, resumes, and retains the previous complete index during replacement | Worker template, `search/engine.ts` | `web/src/sw/search.ts`, `search/engine.ts`; worker integrity tests and search offline-transition tests |
| Offline search readiness does not imply article/image availability; saved badges and unavailable previews reflect actual downloads | `search/Results.svelte`, `offline.ts` | `web/src/search/Results.svelte`, `state/offline.svelte.ts`; offline/status and search transition tests |
| Bounded collection-page shell caching, site-scoped worker cleanup, required shell integrity, visited-page fallbacks and quota recovery | `offline-client.ts`, worker template | `web/src/sw/{caches,routing}.ts`, `offline/client.svelte.ts`; worker tests, Rust bounded source/category/tag output and browser registration tests |

The rewrite had explicitly retired automatic offline downloads in its implementation. Feature
parity restores the preference and complete downloads; it retains the newer worker's navigation
preload, network timeout, server-error fallback and content-addressed runtime cache behavior.
A downloaded article means its complete published local resource family, not a cached search
fragment. External media streams and third-party embeds still require a connection.

The mobile search tab is a deliberate later navigation change: search lives above the feed, while
the bottom bar contains feed, browse and preferences. Passage selection never changes the current
fragment merely because the reader selects different words; only the Share action creates a link.
