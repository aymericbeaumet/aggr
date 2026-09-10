import { dateMatches } from './query';
import { resolveFacet } from './facets';
import type { Facet, FacetKind, FilterKind, PagefindAPI, PagefindModule, PagefindMatches, Query, ResultData, ResultRef, SearchCatalog, SearchPage } from './types';

export function buildFilters(query: Query, manifest: SearchCatalog): Record<string, unknown> {
  const filters: Record<string, unknown> = {};
  for (const field of ['source', 'category', 'tag', 'type'] as FacetKind[]) {
    const clauses = query.clauses.filter(clause => clause.kind === 'facet' && clause.field === field);
    const resolve = (value: string) => resolveFacet(value, manifest.facets[field] || [], field).value;
    const any = [...new Set(clauses.filter(clause => !clause.exclude).flatMap(clause => resolve(clause.value)))];
    const none = [...new Set(clauses.filter(clause => clause.exclude).flatMap(clause => resolve(clause.value)))];
    if (any.length || none.length) filters[field] = { ...(any.length ? { any } : {}), ...(none.length ? { none } : {}) };
  }
  const dates = query.clauses.filter(clause => clause.kind === 'date');
  if (dates.length) {
    const days = (manifest.facets['published-day'] || []).map(facet => facet.value).filter(day => dates.every(clause => dateMatches(clause, day)));
    filters['published-day'] = { any: days.length ? days : ['__no_published_day__'] };
  }
  return filters;
}

const dataCaches = new WeakMap<PagefindAPI, Map<ResultRef, Promise<ResultData>>>();
const fragmentLoads = new WeakMap<PagefindAPI, Map<string, Promise<void>>>();
function snapshotResult(data: ResultData): ResultData {
  return {url:data.url,meta:{...data.meta},
    ...(data.excerpt !== undefined ? {excerpt:data.excerpt} : {}),
    ...(data.filters ? {filters:Object.fromEntries(Object.entries(data.filters).map(([field,values])=>[field,[...values]]))} : {})};
}
export function resultData(api: PagefindAPI, result: ResultRef): Promise<ResultData> {
  let cache = dataCaches.get(api);
  if (!cache) { cache = new Map(); dataCaches.set(api, cache); }
  let data = cache.get(result);
  if (!data) {
    let pending = fragmentLoads.get(api);
    if (!pending) { pending = new Map(); fragmentLoads.set(api,pending); }
    // Pagefind mutates one shared fragment for each query's excerpt. Snapshot it before
    // another query hydrates the same document; unrelated documents still load in parallel.
    data = (pending.get(result.id) ?? Promise.resolve()).then(()=>result.data()).then(snapshotResult)
      .catch(error => { if(cache!.get(result) === data) cache!.delete(result); throw error; });
    const settled = data.then(()=>{},()=>{});
    pending.set(result.id,settled);
    void settled.then(()=>{if(pending!.get(result.id) === settled) pending!.delete(result.id);});
    cache.set(result, data);
    if (cache.size > 256) cache.delete(cache.keys().next().value!);
  }
  return data;
}

const queries = new WeakMap<PagefindAPI, Map<string, Promise<PagefindMatches>>>();
function search(api: PagefindAPI, term: string | null, options: Record<string, unknown>) {
  let cache = queries.get(api);
  if (!cache) { cache = new Map(); queries.set(api, cache); }
  const key = JSON.stringify([term, options]);
  let pending = cache.get(key);
  if (!pending) {
    pending = api.search(term, options).catch(error => { cache!.delete(key); throw error; });
    cache.set(key, pending);
    if (cache.size > 32) cache.delete(cache.keys().next().value!);
  }
  return pending;
}

async function matchingResults(api: PagefindAPI, query: Query, manifest: SearchCatalog): Promise<PagefindMatches> {
  const terms = query.clauses.filter(clause => clause.kind === 'text' && !clause.exclude);
  const plain = terms.filter(clause => !clause.quoted).map(clause => clause.value).join(' ');
  const phrases = terms.filter(clause => clause.quoted).map(clause => '"' + clause.value.replace(/"/g, '') + '"');
  const negative = query.clauses.filter(clause => clause.kind === 'text' && clause.exclude).map(clause => clause.quoted ? '"' + clause.value.replace(/"/g, '') + '"' : clause.value);
  const sort = query.sort === 'relevance' && !terms.length ? 'newest' : query.sort;
  const options: Record<string, unknown> = { filters: buildFilters(query, manifest) };
  if (sort !== 'relevance') options.sort = { date: sort === 'oldest' ? 'asc' : 'desc' };
  const positive: (string | null)[] = [...(plain ? [plain] : []), ...phrases];
  if (!positive.length) positive.push(null);
  // Each Pagefind call returns its complete ID set. Hydration happens only after all
  // intersections and exclusions, so page counts cannot depend on a first-page sample.
  const matches = await Promise.all([...positive, ...negative].map(term => search(api, term, options)));
  const required = matches.slice(1, positive.length).map(match => new Set(match.results.map(result => result.id)));
  const excluded = new Set(matches.slice(positive.length).flatMap(match => match.results.map(result => result.id)));
  const results = matches[0].results.filter(result => required.every(ids => ids.has(result.id)) && !excluded.has(result.id));
  return { results, filters: matches.length === 1 ? matches[0].filters : undefined };
}

export async function executeQuery(api: PagefindAPI, query: Query, manifest: SearchCatalog, requestedPage: number, size: number): Promise<SearchPage> {
  const { results } = await matchingResults(api, query, manifest);
  const pages = Math.max(1, Math.ceil(results.length / size));
  const page = Math.max(1, Math.min(pages, requestedPage));
  return { results: await Promise.all(results.slice((page - 1) * size, page * size).map(result => resultData(api, result))), total: results.length, page, pages, size };
}

export async function executeFacetCounts(api: PagefindAPI, query: Query, manifest: SearchCatalog, field: FilterKind, signal?: AbortSignal): Promise<Facet[]> {
  const matches = await matchingResults(api, query, manifest);
  const facets = manifest.facets[field] || [];
  if (matches.filters?.[field]) {
    return facets.map(facet => ({...facet, count: matches.filters![field][facet.value] || 0})).filter(facet => facet.count > 0);
  }
  const ids = new Set(matches.results.map(result => result.id));
  if (!ids.size) return [];
  const counted: Facet[] = new Array(facets.length);
  let cursor = 0;
  // Compound text queries have no single Pagefind count map. Intersect cached filter
  // memberships instead of hydrating every matching article merely to count facets.
  await Promise.all(Array.from({length: Math.min(4, facets.length)}, async () => {
    while (cursor < facets.length) {
      signal?.throwIfAborted();
      const index = cursor++, facet = facets[index];
      const members = await search(api, null, {filters: {[field]: facet.value}});
      counted[index] = {...facet, count: members.results.reduce((count, result) => count + Number(ids.has(result.id)), 0)};
    }
  }));
  return counted.filter(facet => facet.count > 0);
}

const filterLoads = new WeakMap<PagefindAPI, Promise<unknown>>();
function loadFilters(api: PagefindAPI): Promise<unknown> {
  let pending = filterLoads.get(api);
  if (!pending) {
    pending = Promise.resolve().then(() => api.filters()).catch(error => {filterLoads.delete(api);throw error;});
    filterLoads.set(api,pending);
  }
  return pending;
}

class LoadedIndex {
  private phase: 'retained' | 'retired' | 'disposed' = 'retained';
  private leases = 0;
  private initialized?: Promise<PagefindAPI>;
  private instance?: PagefindAPI;
  private retirement?: Promise<void>;
  private finishRetirement?: () => void;
  constructor(readonly manifest: SearchCatalog, private initialize: () => Promise<PagefindAPI>) {}

  async use<T>(work: (api: PagefindAPI) => Promise<T>): Promise<T> {
    if (this.phase !== 'retained') throw new Error('Search index is retired.');
    this.leases++;
    try {
      this.initialized ??= this.initialize().then(api => {this.instance=api;return api;})
        .catch(error=>{this.initialized=undefined;throw error;});
      return await work(await this.initialized);
    } finally {this.leases--;this.drain();}
  }

  retire(): Promise<void> {
    if (!this.retirement) {
      this.retirement = new Promise(resolve=>{this.finishRetirement=resolve;});
      this.phase='retired';
      this.drain();
    }
    return this.retirement;
  }

  private drain() {
    if(this.phase !== 'retired' || this.leases) return;
    this.phase='disposed';
    const api=this.instance;
    this.instance=undefined;this.initialized=undefined;
    if(api) {
      dataCaches.delete(api);fragmentLoads.delete(api);queries.delete(api);filterLoads.delete(api);
      void api.destroy().catch(()=>{}).then(()=>this.finishRetirement?.());
    } else this.finishRetirement?.();
  }
}

async function fetchCatalog(base: string, signal: AbortSignal): Promise<SearchCatalog> {
  const response = await fetch(new URL('search-catalog.json', base), { cache: 'no-store', signal });
  if (!response.ok) throw new Error('Search index is unavailable. Try again when connected.');
  const manifest: SearchCatalog = await response.json();
  if (!manifest || !manifest.version || !manifest.base || !manifest.facets) throw new Error('The search index is not ready yet.');
  return manifest;
}

export class SearchEngine {
  private current?: LoadedIndex;
  private loading?: Promise<LoadedIndex>;
  private generation = 0;
  private stale = false;
  private online?: boolean;
  private versions = new Map<string, LoadedIndex>();
  private retirements = new Set<Promise<void>>();
  private disposal?: Promise<void>;
  private lifetime = new AbortController();
  constructor(private base: string, private importAPI: (url: string) => Promise<PagefindModule> = url => import(/* @vite-ignore */ url)) {}

  invalidate() {
    this.markStale();
    this.current = undefined;
  }

  markStale() {
    this.generation++;
    this.stale = true;
    this.loading = undefined;
  }

  setOnline(online: boolean) {
    if (this.online !== undefined && this.online !== online) this.invalidate();
    this.online = online;
  }

  private retire(index: LoadedIndex) {
    const pending=index.retire();
    this.retirements.add(pending);
    void pending.then(()=>this.retirements.delete(pending));
  }

  dispose(): Promise<void> {
    if(this.disposal) return this.disposal;
    this.lifetime.abort(new Error('Search session is disposed.'));
    this.invalidate();
    for(const index of this.versions.values()) this.retire(index);
    this.versions.clear();
    this.disposal=Promise.all(this.retirements).then(()=>{});
    return this.disposal;
  }

  async load(refresh = false): Promise<LoadedIndex> {
    this.lifetime.signal.throwIfAborted();
    if (this.loading) return this.loading;
    if (this.current && !refresh && !this.stale) return this.current;
    const epoch = this.generation;
    this.loading = (async () => {
      try {
        const manifest = await fetchCatalog(this.base, this.lifetime.signal);
        this.lifetime.signal.throwIfAborted();
        if(epoch !== this.generation) return this.load();
        const bundle = new URL(manifest.base, this.base);
        if (bundle.origin !== new URL(this.base).origin || !bundle.pathname.startsWith(new URL(this.base).pathname)) throw new Error('Invalid search index location.');
        const key = JSON.stringify([manifest.version, bundle.href]);
        const cached = this.versions.get(key);
        if (cached) {
          if (epoch === this.generation) {
            this.versions.delete(key); this.versions.set(key, cached);
            this.current = cached; this.stale = false;
          }
          return cached;
        }
        const initialize = async () => {
          this.lifetime.signal.throwIfAborted();
          const module=await this.importAPI(new URL('pagefind.js', bundle).href);
          this.lifetime.signal.throwIfAborted();
          const api=module.createInstance({basePath:bundle.pathname,baseUrl:new URL(this.base).pathname,excerptLength:28,
            ranking:{termFrequency:0.65,termSimilarity:1,pageLength:0.35,termSaturation:0.8,metaWeights:{title:12,source:2,date:0,aggr_display:0}}});
          try {
            await api.init();
            this.lifetime.signal.throwIfAborted();
            return api;
          } catch(error) {
            await api.destroy().catch(()=>{});
            if (this.current === loaded) this.current = undefined;
            throw error;
          }
        };
        const loaded = new LoadedIndex(manifest,initialize);
        if (epoch === this.generation) {
          this.current = loaded; this.stale = false;
          this.versions.set(key, loaded);
          if (this.versions.size > 2) {
            const oldest=this.versions.keys().next().value!;
            this.retire(this.versions.get(oldest)!);
            this.versions.delete(oldest);
          }
        }
        return loaded;
      } catch (error) {
        if (epoch === this.generation && this.current) return this.current;
        throw error;
      } finally { if (epoch === this.generation) this.loading = undefined; }
    })();
    return this.loading;
  }

  async search(query: Query, page: number, size: number): Promise<SearchPage> {
    const loaded = await this.load();
    return loaded.use(api=>executeQuery(api,query,loaded.manifest,page,size));
  }

  async prepare(query: Query, signal?: AbortSignal): Promise<void> {
    signal?.throwIfAborted();
    if (!query.clauses.length) return;
    const loaded=await this.load();
    signal?.throwIfAborted();
    const filters=buildFilters(query,loaded.manifest);
    await loaded.use(async ready=>{
      signal?.throwIfAborted();
      // A completed filter query is definite intent; partial text should not preload
      // broad prefix matches that the next keystroke may immediately discard.
      if (!query.clauses.some(clause=>clause.kind==='text')) await ready.preload(null,{filters});
    });
  }

  async facetCounts(query: Query, field: FilterKind, signal?: AbortSignal): Promise<Facet[]> {
    const loaded = await this.load();
    signal?.throwIfAborted();
    return loaded.use(async ready=>{
      await loadFilters(ready);
      signal?.throwIfAborted();
      return executeFacetCounts(ready, query, loaded.manifest, field, signal);
    });
  }
}

export type SearchSession = Pick<SearchEngine, 'load' | 'search' | 'prepare' | 'facetCounts' | 'markStale' | 'setOnline' | 'dispose'>;

export function createSearchSession(base: string, online: boolean): SearchSession {
  const session = new SearchEngine(base);
  session.setOnline(online);
  return session;
}
