import { mount, tick } from 'svelte';
import { writable } from 'svelte/store';
import Controls from './Controls.svelte';
import { revealSearchInput } from './focus';
import Results from './Results.svelte';
import { mountOver } from '../mount-over';
import { acceptCompletion, complete, completionMenu, isCompletingFacet, stableCompletions, type Completion, type CompletionMenu } from './completion';
import { completionContext, LatestFacets } from './completion-context';
import { readableQuery, resolveFacet } from './facets';
import { parseQuery, queryFromLocation, queryURL, quoteValue } from './query';
import { displayData, safeURL } from './display';
import type { Facet, FacetKind, SearchHandle, SearchCatalog, SearchOptions } from './types';
import type { ViewState } from './state';

export type { SearchOptions, SearchHandle } from './types';
export { facetURL } from './query';
export { createSearchSession, type SearchSession } from './engine';

/** A handle that does nothing: what the page keeps when the search controls could not be mounted. */
export function inertSearchHandle(): SearchHandle {
  return { async destroy() {}, focus() {}, refresh() {}, isActive: () => false, updateOfflineStatus() {}, updateDates() {}, select() {} };
}

export function mountSearch(options: SearchOptions): SearchHandle {
  const { root, resultsRoot, staticFeed, base } = options;
  const scopeKind = root.dataset.scopeKind, scopeValue = root.dataset.scopeValue;
  const scope = scopeKind && scopeValue ? `${scopeKind}:${quoteValue(scopeValue)}` : '';
  const initialURL = new URL(location.href);
  const locationQuery = queryFromLocation(initialURL);
  const initialQuery = locationQuery || scope;
  const size = () => {
    const preferred = Number(options.preferences.values['feed-page-size']);
    return [10, 25, 50].includes(preferred) ? preferred : 50;
  };
  let state: ViewState = {
    query: initialQuery, cursor: initialQuery.length, active: !!locationQuery, ready: false,
    busy: false, error: '', offline: '', suggestions: [], open: false,
    page: { results: [], total: 0, page: 1, pages: 1, size: size() },
    selectedURL: '', now: Date.now(), dateFormat: options.preferences.values['date-format'], base,
    isOffline: !navigator.onLine, hasOfflineStatus: false, savedURLs: new Set(), cachedPreviews: new Set()
  };
  const model = writable(state);
  const update = (changes: Partial<ViewState>) => {
    if (destroyed) return;
    state = { ...state, ...changes }; model.set(state);
  };
  const engine = options.session;
  engine.setOnline(navigator.onLine);
  let manifest: SearchCatalog | undefined;
  let normalizeInitial = true;
  const latestFacets = new LatestFacets();
  let contextual: {key: string; values: Facet[]} | undefined;
  let queryWarmup: AbortController | undefined;
  let generation = 0, catalogGeneration = 0, destroyed = false, composing = false;
  let menu: CompletionMenu = 'idle';
  let restoringFocus = false;
  let online = navigator.onLine;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const bindings = new AbortController();
  const staticList = staticFeed?.querySelector<HTMLElement>('#list');
  const staticID = staticList?.id;
  const wasHidden = staticFeed?.hidden ?? false;
  const input = () => root.querySelector<HTMLInputElement>('#q');

  function showResults(show: boolean) {
    if (staticFeed) staticFeed.hidden = show || wasHidden;
    if (staticList) {
      if (show) staticList.removeAttribute('id');
      else if (staticID) staticList.id = staticID;
    }
  }
  function rememberSelection() {
    const selectedURL = resultsRoot.querySelector<HTMLAnchorElement>('.row.is-selected [data-row-open]')?.href || state.selectedURL;
    update({ selectedURL });
  }
  function hidePendingResults() {
    rememberSelection();
    showResults(true);
    update({ ready: false });
  }
  function suggestions() {
    let facets = manifest?.facets;
    let scoped = false;
    try {
      const context = completionContext(state.query, state.cursor);
      if (manifest && context && context.query.clauses.some(clause => clause.kind !== 'sort') && menu === 'open' && !composing) {
        scoped = true;
        const key = JSON.stringify([manifest.version, context.field, context.query.raw]);
        facets = {...manifest.facets, [context.field]: contextual?.key === key ? contextual.values : []};
        latestFacets.request(key, signal => engine.facetCounts(context.query, context.field, signal), values => {
          contextual = {key, values};
          suggestions();
        }, error => {
          contextual = {key, values: []};
          update({suggestions: [], error: error instanceof Error ? error.message : 'Search suggestions are unavailable.'});
        });
      } else latestFacets.clear();
    } catch { facets = undefined; latestFacets.clear(); }
    const next = stableCompletions(state.suggestions, complete(state.query, state.cursor, facets, Date.now(), manifest?.facets, scoped));
    const open = menu === 'open';
    if (next !== state.suggestions || open !== state.open) update({ suggestions: next, open });
  }
  function dismiss() {
    latestFacets.clear();
    menu = completionMenu(menu, 'dismiss');
    update({ open: false });
  }
  function loadFacets(refresh = false) {
    const epoch = ++catalogGeneration;
    void engine.load(refresh).then(loaded => {
      if (!destroyed && epoch === catalogGeneration) { receiveManifest(loaded.manifest); suggestions(); warmCurrentQuery(); }
    }).catch(() => {});
  }
  function warmCurrentQuery() {
    queryWarmup?.abort();
    if (!manifest || composing || !state.query.trim() || isCompletingFacet(state.query,state.cursor,manifest.facets)) return;
    try {
      const query=parseQuery(state.query);
      queryWarmup=new AbortController();
      void engine.prepare(query,queryWarmup.signal).catch(()=>{});
    } catch { /* Incomplete syntax needs only the completion catalogue. */ }
  }
  function receiveManifest(loaded: SearchCatalog) {
    manifest = loaded;
    if (normalizeInitial) {
      normalizeInitial = false;
      const query = readableQuery(state.query, manifest.facets);
      if (query !== state.query) update({query, cursor: query.length});
    }
  }
  function offlineStatus() {
    const status = options.getOfflineStatus() as { saved?: { url: string }[]; search?: { phase?: string; activeVersion?: string; error?: string } } | null;
    update({ isOffline: !navigator.onLine, hasOfflineStatus: !!status?.saved,
      savedURLs: new Set((status?.saved || []).map(entry => safeURL(entry.url, base))),
      offline: navigator.onLine ? '' : status?.search?.activeVersion ? 'Offline search' : 'Offline · some search data may be unavailable' });
    void verifyPreviews();
    if (online !== navigator.onLine) {
      online = navigator.onLine;
      clearTimeout(timer); generation++;
      engine.setOnline(online);
      queryWarmup?.abort();
      catalogGeneration++;
      latestFacets.clear(); contextual = undefined;
      manifest = undefined;
      if (state.active && !composing) void perform(state.page.page);
      else loadFacets();
    }
  }
  async function verifyPreviews() {
    if (navigator.onLine || !('caches' in window)) return;
    const epoch = generation;
    const urls = state.page.results.flatMap(entry => {
      const preview = displayData(entry).preview;
      return preview ? [safeURL(preview.url, base)] : [];
    });
    const saved = await Promise.all(urls.map(async url => {
      try {
        const response = await caches.match(url);
        return response?.ok && response.headers.get('content-type')?.startsWith('image/') ? url : null;
      } catch { return null; }
    }));
    if (!destroyed && epoch === generation) update({ cachedPreviews: new Set(saved.filter((url): url is string => url !== null)) });
  }
  function publishLocation(query: string, page: number) {
    const url = queryURL(base, query, page);
    if (location.href !== url) {
      options.replaceLocation(url);
    }
  }
  async function perform(page = 1, refresh = false) {
    if (destroyed) return;
    clearTimeout(timer);
    const epoch = ++generation;
    let query = state.query;
    if (!query.trim()) { reset(); return; }
    const focused = resultsRoot.contains(document.activeElement) && document.activeElement instanceof HTMLAnchorElement ? document.activeElement.href : '';
    if (refresh && state.ready) rememberSelection();
    else hidePendingResults();
    try {
      update({ busy: true, active: true, error: '' });
      const loaded = await engine.load(refresh);
      if (destroyed || epoch !== generation) return;
      receiveManifest(loaded.manifest);
      query = state.query;
      suggestions();
      if (isCompletingFacet(query, state.cursor, loaded.manifest.facets)) {
        update({ busy: false });
        return;
      }
      const parsed = parseQuery(query);
      if (scope && !parsed.clauses.some(clause => clause.kind === 'facet' && clause.field === scopeKind && resolveFacet(clause.value, loaded.manifest.facets[scopeKind as FacetKind] || [], scopeKind as FacetKind).value === scopeValue)) {
        options.navigate(queryURL(base, query, page));
        return;
      }
      const result = await engine.search(parsed, page, size());
      if (destroyed || epoch !== generation) return;
      const selected = state.selectedURL;
      const selectedURL = selected && result.results.some(entry => safeURL(entry.url, base) === selected) ? selected : safeURL(result.results[0]?.url, base);
      showResults(true);
      update({ page: result, ready: true, busy: false, selectedURL, dateFormat: options.preferences.values['date-format'] });
      publishLocation(query, result.page);
      await tick();
      if (destroyed || epoch !== generation) return;
      options.onRowsChanged();
      void verifyPreviews();
      if (focused) [...resultsRoot.querySelectorAll<HTMLAnchorElement>('[data-row-open]')].find(link => link.href === focused)?.focus({ preventScroll: true });
    } catch (error) {
      if (!destroyed && epoch === generation) update({ error: error instanceof Error ? error.message : 'Search is unavailable. Please try again.', busy: false });
    }
  }
  function reset() {
    queryWarmup?.abort();
    clearTimeout(timer); generation++;
    dismiss();
    update({ query: '', cursor: 0, ready: false, active: false, busy: false, error: '', open: false, suggestions: [] });
    showResults(false);
    if (scope || !staticFeed) options.navigate(base);
    else { publishLocation('', 1); void tick().then(() => { if (!destroyed) options.onRowsChanged(); }); }
  }
  function change(query: string, cursor: number, isComposing: boolean) {
    if (destroyed) return;
    normalizeInitial = false;
    composing = isComposing;
    options.onQueryChanged();
    clearTimeout(timer); generation++;
    if (!query.trim() && !isComposing) { reset(); return; }
    hidePendingResults();
    update({ query, cursor, active: !!query.trim(), busy: !isComposing, error: '' });
    menu = completionMenu(menu, 'input');
    suggestions();
    warmCurrentQuery();
    if (!manifest) loadFacets();
    if (!isComposing) timer = setTimeout(() => { void perform(); }, 180);
  }
  function choose(completion: Completion) {
    if (destroyed) return;
    if (menu !== 'open' || !state.suggestions.some(item => item.id === completion.id && item.insert === completion.insert && item.start === completion.start && item.end === completion.end)) return;
    const accepted = acceptCompletion(state.query, completion);
    change(accepted.query, accepted.cursor, false);
    if (!completion.insert.endsWith(':')) dismiss();
    void tick().then(() => {
      if (destroyed) return;
      const field = input();
      field?.focus({ preventScroll: true });
      field?.setSelectionRange(accepted.cursor, accepted.cursor);
    });
  }
  function revealInput() {
    if (!destroyed && !restoringFocus) revealSearchInput(input());
  }
  function focusField({ restore = false }: { restore?: boolean } = {}) {
    if (destroyed) return;
    if (!restore) revealInput();
    restoringFocus = true;
    try { input()?.focus({ preventScroll: true }); } finally { restoringFocus = false; }
  }
  function focusInput() {
    if (destroyed) return;
    revealInput();
    menu = completionMenu(menu, 'focus');
    suggestions();
    if (!manifest) loadFacets();
  }

  // The static form comes back if either component fails to mount, and again when the page is disposed.
  const overlay = mountOver(root, [
    () => mount(Controls, { target: root, props: {
      model, change, choose, submit: () => { if (!composing) { dismiss(); void perform(); } },
      move: (direction: number) => !composing && state.ready && state.page.results.length > 0 && options.moveSelection(direction),
      open: () => !composing && state.ready && !state.busy && state.page.results.length > 0 && options.openSelected(),
      clear: () => { options.onQueryChanged(); reset(); }, caret: (cursor: number) => { if (cursor !== state.cursor) { update({ cursor }); suggestions(); } }, focusInput, revealInput,
      dismiss, blur: () => { menu = completionMenu(menu, 'blur'); update({ open: false }); }
    } }),
    () => mount(Results, { target: resultsRoot, props: { model, pageChanged: (page: number) => { void perform(page); } } })
  ]);
  window.addEventListener('popstate', () => {
    const url = new URL(location.href), query = queryFromLocation(url);
    dismiss();
    update({ query, cursor: query.length, open: false });
    if (query) void perform(Number(url.searchParams.get('search-page')) || 1);
    else reset();
  }, { signal: bindings.signal });
  window.addEventListener('online', offlineStatus, { signal: bindings.signal });
  window.addEventListener('offline', offlineStatus, { signal: bindings.signal });
  offlineStatus();
  if (locationQuery) void perform(Number(initialURL.searchParams.get('search-page')) || 1);
  if (initialURL.searchParams.has('focus-search')) void tick().then(() => focusField());

  let disposal: Promise<void> | undefined;
  return {
    focus: focusField,
    refresh() {
      if (destroyed) return;
      if (composing) {
        loadFacets(true);
        return;
      }
      if (state.active) void perform(state.page.page, true);
      else loadFacets(true);
    },
    isActive: () => state.active,
    updateOfflineStatus() { if (!destroyed) offlineStatus(); },
    updateDates() { if (!destroyed) update({ now: Date.now(), dateFormat: options.preferences.values['date-format'] }); },
    select(selectedURL: string) { if (!destroyed && state.selectedURL !== selectedURL) update({ selectedURL }); },
    destroy() {
      if (disposal) return disposal;
      destroyed = true; generation++; clearTimeout(timer); bindings.abort();
      latestFacets.clear();
      queryWarmup?.abort();
      disposal = overlay.restore().then(() => showResults(false));
      return disposal;
    }
  };
}
