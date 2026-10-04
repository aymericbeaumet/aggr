import { tick } from 'svelte';
import { navigation } from '../navigation';
import { preferences } from '../state/preferences.svelte';
import { search, type ResultPage, type SearchDriver } from '../state/search.svelte';
import { acceptCompletion, complete, completionContext, isCompletingFacet, type Completion } from './completion';
import { isClientRow, resultRow } from './display';
import { createSession, type Index, type Session } from './engine';
import { readableQuery, resolveFacet } from './facets';
import { isFacetKind, parseQuery, QueryError, queryURL, stripSearch, type FilterKind } from './query';
import type { Catalog, Facet } from './types';

/**
 * The search controller: what happens between a keystroke and the rows. It owns nothing in the
 * DOM; the components bind to the state and call the driver, and the driver changes the state.
 * Sessions (catalogue, runtimes) outlive pages; everything else follows the list on screen.
 */

/**
 * Typing pause before a query runs. Long enough to coalesce a burst, short enough to feel live:
 * the index is already warm, stale runs are discarded by generation, and repeats are cached.
 */
export const DEBOUNCE = 60;
const SNAPSHOT_LIMIT = 512 * 1024;

const sessions = new Map<string, Session>();
let session: Session | null = null;
let catalogue: Catalog | undefined;
let menu: 'idle' | 'open' | 'dismissed' = 'idle';
/** The reader chose a suggestion by hand; keep it under the highlight as the list changes. */
let moved = false;
/** The text has been edited on this page, so an arriving catalogue may not rewrite it. */
let edited = false;
let debounce: ReturnType<typeof setTimeout> | undefined;
let work: AbortController | undefined;
let contextWork: AbortController | undefined;
let context: { key: string; field: FilterKind; values: Facet[] } | undefined;
let catalogueGeneration = 0;
/** What the results on screen answer. */
let shownFor: { text: string; page: number } | null = null;

const field = (): HTMLInputElement | null => {
  const element = document.getElementById('q');
  return element instanceof HTMLInputElement ? element : null;
};

const now = () => Date.now();

/** The session of the site on screen; a page arriving revalidates its catalogue. */
function sessionFor(): Session {
  const root = search.root;
  let existing = sessions.get(root);
  if (!existing) {
    existing = createSession(root);
    sessions.set(root, existing);
  } else existing.stale();
  existing.network();
  session = existing;
  return existing;
}

function cancelQuery(): void {
  clearTimeout(debounce);
  search.invalidate();
  work?.abort();
}

function closeMenu(): void {
  menu = 'dismissed';
  contextWork?.abort();
  search.open = false;
}

function hideResults(): void {
  search.results = null;
  search.status = '';
  shownFor = null;
}

function syncLocation(): void {
  if (!search.attached) return;
  try {
    navigation.replaceAddress(search.active ? queryURL(search.href, search.text, search.page, now()) : stripSearch(search.href));
  } catch {
    /* Incomplete queries are not shareable yet. */
  }
}

/** Suggestions for the token under the cursor, scoped to the rest of the query when it has one. */
function suggest(): void {
  if (!search.attached || search.composing) return;
  let facets = catalogue?.facets;
  let scoped = false;
  try {
    const editing = completionContext(search.text, search.cursor, now());
    if (catalogue && editing && editing.rest && menu === 'open' && editing.query.clauses.some((clause) => clause.kind !== 'sort')) {
      scoped = true;
      const key = JSON.stringify([catalogue.version, editing.field, editing.rest]);
      if (context?.key !== key) {
        contextWork?.abort();
        contextWork = new AbortController();
        const request = contextWork;
        const pending = { key, field: editing.field, values: [] as Facet[] };
        context = pending;
        void session
          ?.load()
          .then((index) => index.counts(editing.query, editing.field, request.signal))
          .then((values) => {
            if (request.signal.aborted || context !== pending || !search.attached) return;
            pending.values = values;
            suggest();
          })
          .catch((failure: unknown) => {
            if (!request.signal.aborted && search.attached) {
              search.error = failure instanceof Error ? failure.message : 'Search suggestions are unavailable.';
            }
          });
      }
      facets = { ...catalogue.facets, [editing.field]: context.values };
    }
    if (!scoped) {
      contextWork?.abort();
      context = undefined;
    }
    const selected = moved ? search.completions[search.highlighted]?.id : undefined;
    const completions = complete(search.text, search.cursor, facets, now(), catalogue?.facets, scoped);
    search.completions = completions;
    search.highlighted = selected ? Math.max(0, completions.findIndex((option) => option.id === selected)) : 0;
    search.open = menu === 'open';
  } catch {
    search.completions = [];
    contextWork?.abort();
    context = undefined;
    search.open = false;
  }
}

/** Whether the reader is still choosing a facet value rather than searching. */
function completing(): boolean {
  return catalogue !== undefined && isCompletingFacet(search.text, search.cursor, catalogue.facets, now());
}

async function loadCatalogue(refresh = false): Promise<Index | undefined> {
  const engine = session ?? sessionFor();
  const token = ++catalogueGeneration;
  const index = await engine.load(refresh);
  if (!search.attached || token !== catalogueGeneration) return undefined;
  catalogue = index.manifest;
  if (!edited && search.active) {
    const readable = readableQuery(search.text, catalogue.facets);
    if (readable !== search.text) {
      search.text = readable;
      search.cursor = readable.length;
    }
  }
  suggest();
  return index;
}

const snapshotKey = () => `aggr:search-snapshot:${encodeURIComponent(new URL(search.root).pathname)}`;

function remember(results: ResultPage): void {
  try {
    const saved = JSON.stringify({ query: search.text, page: results.page, results });
    if (saved.length <= SNAPSHOT_LIMIT) sessionStorage.setItem(snapshotKey(), saved);
  } catch {
    /* Storage is optional. */
  }
}

function show(results: ResultPage): void {
  search.page = results.page;
  search.address = queryURL(search.href, search.text, results.page, now());
  search.results = results;
  search.status = `${results.total} ${results.total === 1 ? 'article' : 'articles'}`;
  shownFor = { text: search.text, page: results.page };
}

/** Put back what this query and page last showed: the rows still held, else the saved snapshot. */
function restore(): boolean {
  if (search.results && shownFor && shownFor.text === search.text && shownFor.page === search.page) {
    show(search.results);
    return true;
  }
  try {
    const saved = JSON.parse(sessionStorage.getItem(snapshotKey()) || 'null') as {
      query?: unknown;
      page?: unknown;
      results?: { rows?: unknown; total?: unknown; page?: unknown; pages?: unknown; size?: unknown };
    } | null;
    if (!saved || saved.query !== search.text || saved.page !== search.page || !Array.isArray(saved.results?.rows)) return false;
    const { rows, total, page, pages, size } = saved.results;
    if (![total, page, pages, size].every((value) => typeof value === 'number')) return false;
    show({ rows: rows.filter(isClientRow), total: total as number, page: page as number, pages: pages as number, size: size as number });
    return true;
  } catch {
    return false;
  }
}

function warmQuery(): void {
  const token = search.generation;
  const request = new AbortController();
  work = request;
  void loadCatalogue()
    .then((index) => {
      if (!index || token !== search.generation || request.signal.aborted || completing()) return undefined;
      return index.prepare(parseQuery(search.text, now()), request.signal);
    })
    .catch(() => {});
}

/** The page of results for the text as it stands; an older run never replaces a newer one. */
async function run(keepRows = false, refresh = false): Promise<void> {
  if (!search.attached || search.composing) return;
  cancelQuery();
  const token = search.generation;
  const request = new AbortController();
  work = request;
  if (!search.active) {
    reset();
    return;
  }
  if (!keepRows) hideResults();
  search.error = '';
  search.busy = true;
  if (!search.results) search.status = 'Searching…';
  try {
    const index = await loadCatalogue(refresh);
    if (!index || token !== search.generation) return;
    if (completing()) {
      search.status = '';
      search.busy = false;
      return;
    }
    const query = parseQuery(search.text, now());
    const scope = search.scope;
    if (scope && isFacetKind(scope.kind)) {
      const kind = scope.kind;
      const within = query.clauses.some(
        (clause) =>
          clause.kind === 'facet' && clause.field === kind && !clause.exclude && resolveFacet(clause.value, index.manifest.facets[kind] ?? [], kind).value === scope.slug,
      );
      // A query that leaves the scoped list belongs to the main feed.
      if (!within) {
        void navigation.go(queryURL(search.root, search.text, search.page, now()));
        return;
      }
    }
    const outcome = await index.run(query, search.page, preferences.feedPageSize, request.signal);
    if (token !== search.generation) return;
    const rows = outcome.results.flatMap((result) => {
      const row = resultRow(result);
      if (!row) console.warn('aggr: search result without display data', result.url);
      return row ? [row] : [];
    });
    const results: ResultPage = { rows, total: outcome.total, page: outcome.page, pages: outcome.pages, size: outcome.size };
    show(results);
    search.busy = false;
    syncLocation();
    remember(results);
  } catch (failure) {
    if (token !== search.generation) return;
    hideResults();
    search.busy = false;
    search.error =
      failure instanceof QueryError
        ? failure.message
        : navigator.onLine
          ? 'Search is unavailable right now. Try again in a moment.'
          : 'Offline search is unavailable until the complete index is saved.';
  }
}

/** No query: the feed is back, and the address says so. */
function reset(): void {
  cancelQuery();
  contextWork?.abort();
  context = undefined;
  search.page = 1;
  closeMenu();
  hideResults();
  search.busy = false;
  search.error = '';
  search.address = '';
  syncLocation();
}

function changed(delay = DEBOUNCE): void {
  if (!search.attached) return;
  cancelQuery();
  edited = true;
  search.page = 1;
  search.revealed = true;
  menu = 'open';
  moved = false;
  if (!search.text.trim() && !search.composing) {
    reset();
    return;
  }
  hideResults();
  search.error = '';
  suggest();
  if (search.composing) return;
  warmQuery();
  debounce = setTimeout(() => void run(), delay);
}

function choose(completion: Completion): void {
  if (!search.attached || menu !== 'open') return;
  const offered = search.completions.some(
    (option) => option.id === completion.id && option.insert === completion.insert && option.start === completion.start && option.end === completion.end,
  );
  if (!offered) return;
  const accepted = acceptCompletion(search.text, completion);
  search.text = accepted.query;
  search.cursor = accepted.cursor;
  changed(0);
  if (!completion.insert.endsWith(':')) closeMenu();
  void tick().then(() => {
    const input = field();
    if (!input || search.text !== accepted.query) return;
    input.focus({ preventScroll: true });
    input.setSelectionRange(accepted.cursor, accepted.cursor);
  });
}

function refresh(): void {
  if (!session) return;
  contextWork?.abort();
  context = undefined;
  catalogueGeneration++;
  session.stale();
  if (!search.attached) return;
  if (search.active && !search.composing) void run(search.ready, true);
  else void loadCatalogue(true).catch(() => {});
}

export const driver: SearchDriver = {
  arrive() {
    sessionFor();
    cancelQuery();
    contextWork?.abort();
    context = undefined;
    menu = document.activeElement === field() ? 'open' : 'idle';
    moved = false;
    edited = false;
    search.completions = [];
    search.open = false;
    search.busy = false;
    search.error = '';
    if (search.active) void run(restore());
    else {
      hideResults();
      search.address = '';
      // The catalogue loads when the field is used. The articles on screen get the connection first.
    }
  },
  leave() {
    cancelQuery();
    contextWork?.abort();
    context = undefined;
    catalogueGeneration++;
    menu = 'idle';
    search.open = false;
    search.busy = false;
  },
  changed() {
    changed();
  },
  moved() {
    suggest();
  },
  focus() {
    if (!search.attached || document.activeElement !== field()) return;
    if (menu === 'idle') menu = 'open';
    suggest();
    if (!catalogue) void loadCatalogue().catch(() => {});
  },
  blur() {
    setTimeout(() => {
      if (search.attached && document.activeElement !== field()) {
        menu = 'idle';
        search.open = false;
      }
    }, 120);
  },
  choose,
  highlight(index) {
    if (search.highlighted === index) return;
    search.highlighted = index;
    moved = true;
  },
  close() {
    const suggesting = search.suggesting;
    closeMenu();
    return suggesting;
  },
  submit() {
    closeMenu();
    void run();
  },
  clear() {
    if (!search.attached) return;
    if (search.scope) {
      void navigation.go(search.root);
      return;
    }
    search.text = '';
    search.cursor = 0;
    edited = true;
    reset();
    field()?.focus({ preventScroll: true });
  },
  turn(delta) {
    if (!search.attached || !search.results) return;
    search.page = Math.max(1, Math.min(search.results.pages, search.page + delta));
    void run();
    document.querySelector('[data-search-results]')?.scrollIntoView?.({ block: 'start', behavior: 'instant' });
  },
  refresh,
};

if (typeof window !== 'undefined') {
  const network = () => {
    if (session?.network()) refresh();
  };
  window.addEventListener('online', network);
  window.addEventListener('offline', network);
  window.addEventListener('pagehide', (event) => {
    if (event.persisted) return;
    for (const engine of sessions.values()) engine.dispose();
    sessions.clear();
    session = null;
  });
}
