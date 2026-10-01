import { afterEach, describe, expect, it, vi } from 'vitest';
import { offline } from '../state/offline.svelte';
import { buildFilters, createIndex, createSession, limiter, type ImportAPI } from './engine';
import { parseQuery } from './query';
import type { Catalog, Matches, Pagefind, PagefindResult, ResultRef } from './types';

const manifest: Catalog = {
  version: 'v1',
  base: 'pagefind/v1/',
  docs: 4,
  facets: {
    category: [{ value: 'world-news', label: 'World News', count: 2 }, { value: 'rust', label: 'Rust', count: 1 }],
    source: [{ value: 'blog', label: 'Blog', count: 4 }],
    tag: [{ value: 'ads', label: 'Ads', count: 1 }],
    type: [{ value: 'podcast', label: 'Podcast', count: 2 }],
    'published-day': ['2026-09-07', '2026-09-08', '2026-09-09'].map((value) => ({ value, label: value, count: 1 })),
  },
};

type Fake = Partial<Pagefind> & { search: Pagefind['search'] };

/** A Pagefind module whose instance is `api`, counting how often it is imported and destroyed. */
function fakeModule(api: Fake) {
  const destroy = vi.fn(async () => {});
  const init = vi.fn(async () => {});
  const importAPI = vi.fn<ImportAPI>(async () => ({
    createInstance: () => ({ init, destroy, preload: async () => {}, ...api }),
  }));
  return { importAPI, destroy, init };
}

const refs = (ids: string[], loaded: string[] = []): ResultRef[] =>
  ids.map((id) => ({
    id,
    data: async () => {
      loaded.push(id);
      return { url: id, meta: { title: id } };
    },
  }));

describe('filters', () => {
  it('resolves unique aliases, rejects ambiguity, and gives exact identifiers precedence', () => {
    const catalogue: Catalog = {
      ...manifest,
      facets: { ...manifest.facets, source: [{ value: 'spotify', label: 'Underscore_', count: 2 }, { value: 'youtube', label: 'UNDERSCORE_', count: 2 }] },
    };
    expect(() => buildFilters(parseQuery('source:underscore_'), catalogue)).toThrow(/Ambiguous source/);
    expect(buildFilters(parseQuery('source:spotify type:Podcast'), catalogue)).toEqual({ source: { any: ['spotify'] }, type: { any: ['podcast'] } });
    catalogue.facets.source = [...(catalogue.facets.source ?? []), { value: 'underscore_', label: 'Other', count: 1 }];
    expect(buildFilters(parseQuery('source:underscore_'), catalogue)).toEqual({ source: { any: ['underscore_'] } });
    expect(() => buildFilters(parseQuery('-tag:missing'), manifest)).toThrow(/Unknown tag/);
  });

  it('uses OR within facets and AND across them, with exclusions and UTC day ranges', () => {
    expect(buildFilters(parseQuery('category:"World News" category:rust source:blog -tag:ads date:>=2026-09-08'), manifest)).toEqual({
      category: { any: ['world-news', 'rust'] },
      source: { any: ['blog'] },
      tag: { none: ['ads'] },
      'published-day': { any: ['2026-09-08', '2026-09-09'] },
    });
    expect(buildFilters(parseQuery('date:2020-01-01'), manifest)['published-day']).toEqual({ any: ['__no_published_day__'] });
  });
});

describe('the index', () => {
  it('intersects entire phrase result sets and subtracts exclusions before paging, hydrating only the page', async () => {
    const loaded: string[] = [];
    const matches: Record<string, string[]> = { rust: ['1', '2', '3', '4'], '"borrow checker"': ['4', '2', '3'], old: ['2'] };
    const search = vi.fn(async (term: string | null): Promise<Matches> => ({ results: refs(matches[term || ''] || [], loaded) }));
    const { importAPI } = fakeModule({ search });
    const index = createIndex('https://reader.test/', manifest, importAPI, new AbortController().signal);
    expect(importAPI).not.toHaveBeenCalled();
    const page = await index.run(parseQuery('rust "borrow checker" -old'), 2, 1);
    expect(page).toMatchObject({ total: 2, page: 2, pages: 2, size: 1 });
    expect(page.results.map((result) => result.url)).toEqual(['4']);
    expect(loaded).toEqual(['4']);
    expect(search.mock.calls.map((call) => call[0])).toEqual(['rust', '"borrow checker"', 'old']);
    expect(importAPI).toHaveBeenCalledExactlyOnceWith('https://reader.test/pagefind/v1/pagefind.js');
    // The same query again is answered from the cache.
    await index.run(parseQuery('rust "borrow checker" -old'), 1, 1);
    expect(search).toHaveBeenCalledTimes(3);
  });

  it('sorts filter-only queries newest first and preloads only those', async () => {
    const search = vi.fn(async (): Promise<Matches> => ({ results: [] }));
    const preload = vi.fn(async () => {});
    const { importAPI } = fakeModule({ search, preload });
    const index = createIndex('https://reader.test/', manifest, importAPI, new AbortController().signal);
    await index.prepare(parseQuery('rust'));
    expect(preload).not.toHaveBeenCalled();
    await index.prepare(parseQuery('source:blog'));
    expect(preload).toHaveBeenCalledExactlyOnceWith(null, { filters: { source: { any: ['blog'] } } });
    await expect(index.prepare(parseQuery('source:nope'))).rejects.toThrow(/Unknown source/);
    await index.run(parseQuery('category:rust date:2020-01-01'), 1, 25);
    expect(search).toHaveBeenCalledWith(null, { filters: { category: { any: ['rust'] }, 'published-day': { any: ['__no_published_day__'] } }, sort: { date: 'desc' } });
  });

  it('snapshots the fragment Pagefind mutates so two queries hydrate one document separately', async () => {
    const raw: PagefindResult = { url: '/article/', excerpt: '<mark>first</mark>', meta: { title: 'Original' } };
    let release!: () => void;
    const first: ResultRef = {
      id: 'same',
      data: async () => {
        await new Promise<void>((resolve) => (release = resolve));
        raw.excerpt = '<mark>first</mark>';
        return raw;
      },
    };
    const second: ResultRef = {
      id: 'same',
      data: async () => {
        raw.excerpt = '<mark>second</mark>';
        raw.meta.title = 'Replacement';
        return raw;
      },
    };
    const other: ResultRef = { id: 'other', data: async () => ({ url: '/other/', meta: {} }) };
    const search = vi.fn(async (term: string | null): Promise<Matches> => ({ results: term === 'a' ? [first, other] : [second] }));
    const { importAPI } = fakeModule({ search });
    const index = createIndex('https://reader.test/', manifest, importAPI, new AbortController().signal);
    const a = index.run(parseQuery('a'), 1, 25);
    const b = index.run(parseQuery('b'), 1, 25);
    await new Promise((resolve) => setTimeout(resolve, 0));
    release();
    const [pageA, pageB] = await Promise.all([a, b]);
    expect(pageA.results[0]).toEqual({ url: '/article/', excerpt: '<mark>first</mark>', meta: { title: 'Original' } });
    expect(pageB.results[0]).toEqual({ url: '/article/', excerpt: '<mark>second</mark>', meta: { title: 'Replacement' } });
    expect(pageA.results[1].url).toBe('/other/');
  });

  it('counts a facet after every other clause: Pagefind counts for one search, memberships otherwise', async () => {
    const filters = vi.fn(async () => ({}));
    const search = vi.fn(async (term: string | null, options?: Record<string, unknown>): Promise<Matches> => {
      const source = (options?.filters as Record<string, unknown> | undefined)?.source;
      const ids =
        source === 'blog' ? ['2', '3'] : source === 'two' ? ['4', '5'] : source === 'empty' ? ['8']
        : term === 'rust' ? ['1', '2', '3', '4', '5'] : term === '"borrow checker"' ? ['2', '3', '4'] : term === 'old' ? ['3'] : ['1', '2'];
      return { results: refs(ids), filters: { source: { blog: 90, two: 80, empty: 10 }, category: { rust: 2 } } };
    });
    const catalogue: Catalog = {
      ...manifest,
      facets: { ...manifest.facets, source: [{ value: 'blog', label: 'Blog', count: 100 }, { value: 'two', label: 'Two', count: 100 }, { value: 'empty', label: 'Empty', count: 10 }] },
    };
    const { importAPI } = fakeModule({ search, filters });
    const index = createIndex('https://reader.test/', catalogue, importAPI, new AbortController().signal);
    expect(await index.counts(parseQuery('category:rust'), 'source')).toEqual([
      { value: 'blog', label: 'Blog', count: 90 }, { value: 'two', label: 'Two', count: 80 }, { value: 'empty', label: 'Empty', count: 10 },
    ]);
    expect(filters).toHaveBeenCalledTimes(1);
    expect(await index.counts(parseQuery('rust "borrow checker" -old category:rust'), 'source')).toEqual([
      { value: 'blog', label: 'Blog', count: 1 }, { value: 'two', label: 'Two', count: 1 },
    ]);
    expect(filters).toHaveBeenCalledTimes(1);
  });

  it('retires only once its leases have finished, and refuses new work afterwards', async () => {
    let finish!: (matches: Matches) => void;
    const search = vi.fn(() => new Promise<Matches>((resolve) => (finish = resolve)));
    const { importAPI, destroy } = fakeModule({ search });
    const index = createIndex('https://reader.test/', manifest, importAPI, new AbortController().signal);
    const running = index.run(parseQuery('rust'), 1, 25);
    await vi.waitFor(() => expect(search).toHaveBeenCalled());
    index.retire();
    expect(destroy).not.toHaveBeenCalled();
    await expect(index.run(parseQuery('rust'), 1, 25)).rejects.toThrow(/retired/);
    finish({ results: [] });
    expect((await running).total).toBe(0);
    await vi.waitFor(() => expect(destroy).toHaveBeenCalledTimes(1));
  });

  it('keeps at most the configured number of index requests in flight', async () => {
    const limit = limiter(2);
    let active = 0;
    let peak = 0;
    const task = () =>
      limit(async () => {
        active++;
        peak = Math.max(peak, active);
        await new Promise((resolve) => setTimeout(resolve, 1));
        active--;
      });
    await Promise.all([task(), task(), task(), task(), task()]);
    expect(peak).toBe(2);
  });
});

describe('the session', () => {
  afterEach(() => {
    vi.restoreAllMocks();
    offline.status = null;
  });

  /** The worker's report with a complete index of `activeVersion` saved under `base`. */
  const saved = (activeVersion: string, base: string) => {
    offline.status = {
      type: 'AGGR_OFFLINE_STATUS',
      requested: 1,
      total: 1,
      saved: [],
      failed: 0,
      downloading: false,
      search: {
        phase: 'ready',
        activeVersion,
        targetVersion: activeVersion,
        base,
        downloadedFiles: 1,
        totalFiles: 1,
        downloadedBytes: 1,
        totalBytes: 1,
        error: null,
      },
    };
  };

  function serve(served: () => Catalog | Response) {
    return vi.spyOn(globalThis, 'fetch').mockImplementation(async () => {
      const value = served();
      return value instanceof Response ? value : new Response(JSON.stringify(value));
    });
  }

  it('fetches the catalogue uncached and imports Pagefind only for a query', async () => {
    const fetcher = serve(() => manifest);
    const { importAPI } = fakeModule({ search: async () => ({ results: [] }) });
    const session = createSession('https://reader.test/', importAPI, true);
    const index = await session.load();
    expect(index.manifest.facets.source?.[0].label).toBe('Blog');
    expect(fetcher).toHaveBeenCalledWith('https://reader.test/search-catalog.json', { cache: 'no-store', signal: expect.any(AbortSignal) });
    expect(importAPI).not.toHaveBeenCalled();
    expect(await session.load()).toBe(index);
    expect(fetcher).toHaveBeenCalledTimes(1);
    await index.run(parseQuery('rust'), 1, 25);
    expect(importAPI).toHaveBeenCalledTimes(1);
  });

  it('reports an unavailable or foreign catalogue', async () => {
    serve(() => new Response('gone', { status: 404 }));
    await expect(createSession('https://reader.test/', vi.fn(), true).load()).rejects.toThrow(/unavailable/);
    vi.restoreAllMocks();
    serve(() => ({ ...manifest, base: 'https://elsewhere.test/pagefind/' }));
    await expect(createSession('https://reader.test/', vi.fn(), true).load()).rejects.toThrow(/Invalid/);
  });

  it('keeps the usable index when a revalidation fails, and keeps two versions alive', async () => {
    let version = 'v1';
    const fetcher = serve(() => ({ ...manifest, version, base: `pagefind/${version}/` }));
    const { importAPI, destroy } = fakeModule({ search: async () => ({ results: [] }) });
    const session = createSession('https://reader.test/', importAPI, true);
    const first = await session.load();
    session.stale();
    fetcher.mockRejectedValueOnce(new Error('disconnected'));
    expect(await session.load()).toBe(first);
    await first.run(parseQuery('rust'), 1, 25);
    version = 'v2';
    session.stale();
    const second = await session.load();
    expect(second.manifest.version).toBe('v2');
    await second.run(parseQuery('rust'), 1, 25);
    version = 'v1';
    session.stale();
    expect(await session.load()).toBe(first);
    expect(importAPI).toHaveBeenCalledTimes(2);
    version = 'v3';
    session.stale();
    await (await session.load()).run(parseQuery('rust'), 1, 25);
    // Three versions seen: the least recently used one (v2) is gone.
    await vi.waitFor(() => expect(destroy).toHaveBeenCalledTimes(1));
    session.dispose();
    await expect(session.load()).rejects.toThrow();
  });

  it('selects the saved offline index when the network is gone', async () => {
    const fetcher = serve(() => ({ ...manifest, version: 'saved', base: 'pagefind/saved/' }));
    const session = createSession('https://reader.test/', vi.fn(), false);
    await expect(session.load()).rejects.toThrow(/offline search index/);
    saved('saved', 'offline/');
    expect((await session.load()).manifest.version).toBe('saved');
    expect(fetcher).toHaveBeenLastCalledWith('https://reader.test/offline/search-catalog.json', expect.anything());
    saved('other', 'offline/');
    expect(session.network(true)).toBe(true);
    expect(session.network(true)).toBe(false);
    expect((await session.load()).manifest.version).toBe('saved');
  });
});
