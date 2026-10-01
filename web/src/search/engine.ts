import { offline } from '../state/offline.svelte';
import { resolveFacet } from './facets';
import { dateMatches, FACET_FIELDS, type FilterKind, type Query } from './query';
import type { Catalog, Facet, Matches, Pagefind, PagefindModule, PagefindResult, ResultRef } from './types';

/**
 * The index behind the field. Pagefind builds and serves it; this loads the catalogue, brings
 * the runtime up on demand, and applies every clause to complete result-ID sets before
 * anything is counted or paged.
 */

/**
 * The most index work search keeps in flight at once, whoever asks for it. Browsers already
 * queue past six connections to one origin over HTTP/1.1, so more would only move the queue out
 * of reach, and six keep small fragments moving over HTTP/2 as well.
 */
export const INDEX_REQUESTS = 6;
const SEARCH_CACHE = 32;
const HYDRATION_CACHE = 256;
const MEMBERSHIP_CACHE = 256;
const RETAINED_VERSIONS = 2;

export type Filters = Record<string, { any?: string[]; none?: string[] }>;

/** Turn the parsed query's facet and date clauses into Pagefind's filter shape. */
export function buildFilters(query: Query, catalogue: Catalog): Filters {
  const filters: Filters = {};
  for (const field of FACET_FIELDS) {
    const clauses = query.clauses.filter((clause) => clause.kind === 'facet' && clause.field === field);
    const resolve = (value: string) => resolveFacet(value, catalogue.facets[field] ?? [], field).value;
    const any = [...new Set(clauses.filter((clause) => !clause.exclude).map((clause) => resolve(clause.value)))];
    const none = [...new Set(clauses.filter((clause) => clause.exclude).map((clause) => resolve(clause.value)))];
    if (any.length || none.length) filters[field] = { ...(any.length ? { any } : {}), ...(none.length ? { none } : {}) };
  }
  const dates = query.clauses.filter((clause) => clause.kind === 'date');
  if (dates.length) {
    const days = (catalogue.facets['published-day'] ?? [])
      .map((facet) => facet.value)
      .filter((value) => dates.every((clause) => dateMatches(clause, value)));
    filters['published-day'] = { any: days.length ? days : ['__no_published_day__'] };
  }
  return filters;
}

/** Run tasks with at most `size` in flight, in the order they were asked for. */
export function limiter(size: number): <T>(task: () => Promise<T>) => Promise<T> {
  let active = 0;
  const waiting: (() => void)[] = [];
  const next = () => {
    while (active < size && waiting.length) {
      const task = waiting.shift();
      if (task) {
        active++;
        task();
      }
    }
  };
  return <T>(task: () => Promise<T>) =>
    new Promise<T>((resolve, reject) => {
      waiting.push(() => {
        Promise.resolve()
          .then(task)
          .then(resolve, reject)
          .finally(() => {
            active--;
            next();
          });
      });
      next();
    });
}

/** One page of hydrated results and the count they were cut from. */
export type SearchPage = { results: PagefindResult[]; total: number; page: number; pages: number; size: number };

/** One immutable catalogue and its lazily initialized runtime. */
export type Index = {
  readonly manifest: Catalog;
  /** No new work; the runtime is released once what is under way has finished. */
  retire(): void;
  /** Bring the runtime up for a query that is about to run; a filter-only query preloads its index too. */
  prepare(query: Query, signal?: AbortSignal): Promise<void>;
  run(query: Query, page: number, size: number, signal?: AbortSignal): Promise<SearchPage>;
  /** Counts for one facet after every other clause, without loading article bodies. */
  counts(query: Query, field: FilterKind, signal?: AbortSignal): Promise<Facet[]>;
};

export type ImportAPI = (url: string) => Promise<PagefindModule>;

/** Retired runtimes finish their leases before they are destroyed. */
export function createIndex(base: string, manifest: Catalog, importAPI: ImportAPI, lifetime: AbortSignal): Index {
  let api: Promise<Pagefind> | undefined;
  let filtersLoaded: Promise<unknown> | undefined;
  const searches = new Map<string, Promise<Matches>>();
  const hydrated = new Map<ResultRef, Promise<PagefindResult>>();
  const loading = new Map<string, Promise<void>>();
  const memberships = new Map<string, Promise<Set<string>>>();
  const limit = limiter(INDEX_REQUESTS);
  let leases = 0;
  let retired = false;
  let destroyed = false;

  function release(): void {
    if (!retired || leases || destroyed) return;
    destroyed = true;
    void api?.then((client) => client.destroy()).catch(() => {});
    searches.clear();
    hydrated.clear();
    memberships.clear();
  }

  function instance(): Promise<Pagefind> {
    api ??= (async () => {
      const bundle = new URL(manifest.base, base);
      const module = await importAPI(new URL('pagefind.js', bundle).href);
      lifetime.throwIfAborted();
      const client = module.createInstance({
        basePath: bundle.pathname,
        baseUrl: new URL(base).pathname,
        excerptLength: 28,
        ranking: {
          termFrequency: 0.65,
          termSimilarity: 1,
          pageLength: 0.35,
          termSaturation: 0.8,
          metaWeights: { title: 12, source: 2, date: 0, aggr_display: 0 },
        },
      });
      try {
        await client.init();
      } catch (error) {
        await client.destroy().catch(() => {});
        throw error;
      }
      return client;
    })().catch((error: unknown) => {
      api = undefined;
      throw error;
    });
    return api;
  }

  async function use<T>(task: (client: Pagefind) => Promise<T>, signal?: AbortSignal): Promise<T> {
    signal?.throwIfAborted();
    if (retired) throw new Error('Search index is retired.');
    leases++;
    try {
      const client = await instance();
      signal?.throwIfAborted();
      lifetime.throwIfAborted();
      return await task(client);
    } finally {
      leases--;
      release();
    }
  }

  /** Repeat searches within a session are common; keep a bounded cache of their ID sets. */
  function search(client: Pagefind, term: string | null, options: Record<string, unknown>): Promise<Matches> {
    const key = JSON.stringify([term, options]);
    let pending = searches.get(key);
    if (!pending) {
      pending = limit(() => client.search(term, options)).catch((error: unknown) => {
        searches.delete(key);
        throw error;
      });
      searches.set(key, pending);
      if (searches.size > SEARCH_CACHE) searches.delete(searches.keys().next().value ?? '');
    }
    return pending;
  }

  /**
   * Pagefind reuses one mutable fragment per document, so copy an immutable snapshot before
   * another query can hydrate the same document. Unrelated documents still load in parallel.
   */
  function hydrate(result: ResultRef): Promise<PagefindResult> {
    let data = hydrated.get(result);
    if (data) return data;
    const previous = loading.get(result.id) ?? Promise.resolve();
    data = previous
      .then(() => limit(() => result.data()))
      .then((value) => ({ url: value.url, meta: { ...value.meta }, ...(value.excerpt !== undefined ? { excerpt: value.excerpt } : {}) }))
      .catch((error: unknown) => {
        if (hydrated.get(result) === data) hydrated.delete(result);
        throw error;
      });
    const settled = data.then(
      () => {},
      () => {},
    );
    loading.set(result.id, settled);
    void settled.then(() => {
      if (loading.get(result.id) === settled) loading.delete(result.id);
    });
    hydrated.set(result, data);
    if (hydrated.size > HYDRATION_CACHE) {
      const oldest = hydrated.keys().next().value;
      if (oldest) hydrated.delete(oldest);
    }
    return data;
  }

  /**
   * Every phrase intersection and exclusion is applied to complete result-ID sets, so counts and
   * paging can never depend on a first-page sample.
   */
  async function matching(client: Pagefind, query: Query) {
    const terms = query.clauses.filter((clause) => clause.kind === 'text' && !clause.exclude);
    const plain = terms.filter((clause) => !clause.quoted).map((clause) => clause.value).join(' ');
    const phrases = terms.filter((clause) => clause.quoted).map((clause) => '"' + clause.value.replace(/"/g, '') + '"');
    const negative = query.clauses
      .filter((clause) => clause.kind === 'text' && clause.exclude)
      .map((clause) => (clause.quoted ? '"' + clause.value.replace(/"/g, '') + '"' : clause.value));
    // A filter-only query has no relevance to sort by, so it falls back to newest.
    const sort = query.sort === 'relevance' && !terms.length ? 'newest' : query.sort;
    const options: Record<string, unknown> = { filters: buildFilters(query, manifest) };
    if (sort !== 'relevance') options.sort = { date: sort === 'oldest' ? 'asc' : 'desc' };
    const positive: (string | null)[] = [...(plain ? [plain] : []), ...phrases];
    if (!positive.length) positive.push(null);
    const matches = await Promise.all([...positive, ...negative].map((term) => search(client, term, options)));
    const required = matches.slice(1, positive.length).map((match) => new Set(match.results.map((result) => result.id)));
    const excluded = new Set(matches.slice(positive.length).flatMap((match) => match.results.map((result) => result.id)));
    const results = matches[0].results.filter((result) => required.every((ids) => ids.has(result.id)) && !excluded.has(result.id));
    return {
      results,
      filters: matches.length === 1 ? matches[0].filters : undefined,
      // Each positive term's own counts. The results are a subset of every one of those
      // searches, so a value any of them never saw cannot occur among the results.
      bounds: matches.slice(0, positive.length).map((match) => match.filters),
    };
  }

  /** The IDs of every document carrying one facet value, fetched once per page. */
  function membership(client: Pagefind, field: string, value: string): Promise<Set<string>> {
    const key = field + '\u0000' + value;
    let members = memberships.get(key);
    if (!members) {
      // Straight to the index rather than through `search`: kept as bare IDs here, a membership
      // would only evict the query results that cache exists for.
      members = limit(() => client.search(null, { filters: { [field]: value } }))
        .then((found) => new Set(found.results.map((result) => result.id)))
        .catch((error: unknown) => {
          memberships.delete(key);
          throw error;
        });
      memberships.set(key, members);
      if (memberships.size > MEMBERSHIP_CACHE) memberships.delete(memberships.keys().next().value ?? '');
    }
    return members;
  }

  /** Counts come with a search only for the filters whose index is loaded: load them all, once. */
  function loadFilters(client: Pagefind): Promise<unknown> {
    filtersLoaded ??= Promise.resolve()
      .then(() => (client.filters ? client.filters() : undefined))
      .catch((error: unknown) => {
        filtersLoaded = undefined;
        throw error;
      });
    return filtersLoaded;
  }

  return {
    manifest,
    retire() {
      retired = true;
      release();
    },
    async prepare(query, signal) {
      buildFilters(query, manifest);
      return use(async (client) => {
        if (!query.clauses.some((clause) => clause.kind === 'text')) await client.preload(null, { filters: buildFilters(query, manifest) });
      }, signal);
    },
    run(query, requestedPage, size, signal) {
      return use(async (client) => {
        const { results } = await matching(client, query);
        signal?.throwIfAborted();
        const pages = Math.max(1, Math.ceil(results.length / size));
        const page = Math.max(1, Math.min(pages, requestedPage));
        return {
          results: await Promise.all(results.slice((page - 1) * size, page * size).map((result) => hydrate(result))),
          total: results.length,
          page,
          pages,
          size,
        };
      }, signal);
    },
    counts(query, field, signal) {
      return use(async (client) => {
        await loadFilters(client);
        signal?.throwIfAborted();
        const matches = await matching(client, query);
        signal?.throwIfAborted();
        const facets = manifest.facets[field] ?? [];
        const counts = matches.filters?.[field];
        if (counts) return facets.map((facet) => ({ ...facet, count: counts[facet.value] || 0 })).filter((facet) => facet.count > 0);
        const ids = new Set(matches.results.map((result) => result.id));
        if (!ids.size) return [];
        const bounds = matches.bounds.flatMap((filters) => (filters?.[field] ? [filters[field]] : []));
        const candidates = facets.filter((facet) => bounds.every((known) => (known[facet.value] || 0) > 0));
        const counted = await Promise.all(
          candidates.map(async (facet) => {
            signal?.throwIfAborted();
            const members = await membership(client, field, facet.value);
            const [small, large] = members.size < ids.size ? [members, ids] : [ids, members];
            let count = 0;
            for (const id of small) if (large.has(id)) count++;
            return { ...facet, count };
          }),
        );
        return counted.filter((facet) => facet.count > 0);
      }, signal);
    },
  };
}

/** A session keeps catalogue and runtime versions coherent across page navigation. */
export type Session = {
  load(refresh?: boolean): Promise<Index>;
  /** The content may have changed: the next load revalidates the catalogue. */
  stale(): void;
  /** The network came or went: returns whether that changed which index is current. */
  network(online?: boolean): boolean;
  dispose(): void;
};

/** Node has no `onLine`; anywhere it is unknown, the network is assumed. */
export const isOnline = (): boolean => typeof navigator === 'undefined' || navigator.onLine !== false;

const defaultImport: ImportAPI = (url) => import(/* @vite-ignore */ url) as Promise<PagefindModule>;

export function createSession(base: string, importAPI: ImportAPI = defaultImport, online = isOnline()): Session {
  const versions = new Map<string, Index>();
  let current: Index | undefined;
  let pending: Promise<Index> | undefined;
  let generation = 0;
  let stale = false;
  const lifetime = new AbortController();

  function load(refresh = false): Promise<Index> {
    if (lifetime.signal.aborted) return Promise.reject(new Error('Search session is disposed.'));
    if (pending) return pending;
    if (current && !refresh && !stale) return Promise.resolve(current);
    const token = generation;
    pending = Promise.resolve().then(async () => {
      try {
        const saved = offline.search;
        if (!online && (!saved?.activeVersion || !saved.base)) throw new Error('A complete offline search index is not saved.');
        const catalogueBase = online ? base : new URL(saved?.base ?? '', base).href;
        const response = await fetch(new URL('search-catalog.json', catalogueBase).href, { cache: 'no-store', signal: lifetime.signal });
        if (!response.ok) throw new Error('Search catalogue unavailable.');
        const manifest = (await response.json()) as Catalog;
        lifetime.signal.throwIfAborted();
        if (token !== generation) return load();
        const bundle = new URL(manifest.base, base);
        const site = new URL(base);
        if (
          bundle.origin !== site.origin ||
          !bundle.pathname.startsWith(site.pathname) ||
          typeof manifest.version !== 'string' ||
          !manifest.facets ||
          (!online && manifest.version !== saved?.activeVersion)
        ) {
          throw new Error('Invalid search catalogue.');
        }
        const key = JSON.stringify([manifest.version, bundle.href]);
        current = versions.get(key) ?? createIndex(base, manifest, importAPI, lifetime.signal);
        versions.delete(key);
        versions.set(key, current);
        stale = false;
        if (versions.size > RETAINED_VERSIONS) {
          const oldest = versions.keys().next().value;
          if (oldest !== undefined) {
            versions.get(oldest)?.retire();
            versions.delete(oldest);
          }
        }
        return current;
      } catch (error) {
        // A failed revalidation keeps the usable index; only a first load has nothing to keep.
        if (token === generation && current && !lifetime.signal.aborted) return current;
        throw error;
      } finally {
        if (token === generation) pending = undefined;
      }
    });
    return pending;
  }

  return {
    load,
    stale() {
      generation++;
      pending = undefined;
      stale = true;
    },
    network(now = isOnline()) {
      if (online === now) return false;
      online = now;
      generation++;
      pending = undefined;
      current = undefined;
      stale = true;
      return true;
    },
    dispose() {
      lifetime.abort();
      generation++;
      pending = undefined;
      current = undefined;
      for (const index of versions.values()) index.retire();
      versions.clear();
    },
  };
}
