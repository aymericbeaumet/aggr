import { afterEach, describe, expect, it, vi } from 'vitest';
import { buildFilters, executeQuery, executeFacetCounts, resultData, SearchEngine } from './engine';
import { parseQuery } from './query';
import type { PagefindAPI, PagefindModule, ResultRef, SearchCatalog } from './types';

const moduleImport = (load:(url:string)=>Promise<Omit<PagefindAPI,'destroy'>>) => async (url:string):Promise<PagefindModule> => {
  const api=await load(url);
  return {createInstance:()=>({...api,destroy:async()=>{}})};
};

const manifest: SearchCatalog = {
  version: 'v1', base: 'pagefind/v1/', docs: 4,
  facets: { category: [{ value: 'world-news', label: 'World News', count: 2 }, { value: 'rust', label: 'Rust', count: 1 }], source: [{ value: 'blog', label: 'Blog', count: 4 }], tag: [{ value: 'ads', label: 'Ads', count: 1 }],
    'published-day': ['2026-09-07', '2026-09-08', '2026-09-09'].map(value => ({ value, label: value, count: 1 })) }
};
describe('query execution', () => {
  it('resolves unique aliases, rejects ambiguity, and gives exact IDs precedence', () => {
    const catalog = { ...manifest, facets: { ...manifest.facets, source: [
      { value: 'spotify', label: 'Underscore_', count: 2 },
      { value: 'youtube', label: 'UNDERSCORE_', count: 2 }
    ], type: [{ value: 'podcast', label: 'Podcast', count: 2 }] } };
    expect(() => buildFilters(parseQuery('source:underscore_'), catalog)).toThrow(/Ambiguous source/);
    expect(buildFilters(parseQuery('source:spotify type:Podcast'), catalog)).toEqual({source:{any:['spotify']}, type:{any:['podcast']}});
    catalog.facets.source = [...catalog.facets.source, {value:'underscore_',label:'Other',count:1}];
    expect(buildFilters(parseQuery('source:underscore_'), catalog)).toEqual({source:{any:['underscore_']}});
  });
  it('reports unknown facet values instead of silently returning no results', () => {
    expect(() => buildFilters(parseQuery('source:typo'), manifest)).toThrow(/Unknown source/);
    expect(() => buildFilters(parseQuery('-tag:missing'), manifest)).toThrow(/Unknown tag/);
  });
  it('uses OR within facets and AND across facets, including exclusions and UTC ranges', () => {
    expect(buildFilters(parseQuery('category:"World News" category:rust source:blog -tag:ads date:>=2026-09-08'), manifest)).toEqual({
      category: { any: ['world-news', 'rust'] }, source: { any: ['blog'] }, tag: { none: ['ads'] },
      'published-day': { any: ['2026-09-08', '2026-09-09'] }
    });
  });
  it('intersects entire phrase result sets and subtracts exclusions before counting or paging', async () => {
    const loaded: string[] = [];
    const refs = (ids: string[]): ResultRef[] => ids.map(id => ({ id, data: async () => { loaded.push(id); return { url: id, meta: { title: id } }; } }));
    const matches: Record<string, string[]> = { rust: ['1', '2', '3', '4'], '"borrow checker"': ['4', '2', '3'], old: ['2'] };
    const search = vi.fn(async (query: string | null) => ({ results: refs(matches[query || ''] || []) }));
    const api = { search } as unknown as PagefindAPI;
    const result = await executeQuery(api, parseQuery('rust "borrow checker" -old'), manifest, 2, 1);
    expect(result.total).toBe(2);
    expect(result.results.map(item => item.url)).toEqual(['4']);
    expect(loaded).toEqual(['4']);
    expect(search.mock.calls.map(call => call[0])).toEqual(['rust', '"borrow checker"', 'old']);
  });
  it('makes an impossible date range match nothing, and sorts facet-only searches newest first', async () => {
    const search = vi.fn(async () => ({ results: [] }));
    await executeQuery({ search } as unknown as PagefindAPI, parseQuery('category:rust date:2020-01-01'), manifest, 1, 25);
    expect(search).toHaveBeenCalledWith(null, { filters: { category: { any: ['rust'] }, 'published-day': { any: ['__no_published_day__'] } }, sort: { date: 'desc' } });
  });
});

describe('query-specific result hydration', () => {
  it('caches each result reference and snapshots fields that Pagefind mutates for another query', async () => {
    const api={} as PagefindAPI;
    const raw={url:'/article/',excerpt:'<mark>first</mark> query',meta:{title:'Original title'},filters:{source:['original']}};
    const first={id:'same-document',data:vi.fn(async()=>raw)};
    const before=await resultData(api,first);
    raw.excerpt='<mark>second</mark> query';raw.meta.title='Replacement title';raw.filters.source[0]='replacement';
    const second={id:'same-document',data:vi.fn(async()=>raw)};
    const after=await resultData(api,second);
    expect(after.excerpt).toBe('<mark>second</mark> query');
    expect(before).toEqual({url:'/article/',excerpt:'<mark>first</mark> query',meta:{title:'Original title'},filters:{source:['original']}});
    expect(await resultData(api,first)).toBe(before);
    expect(first.data).toHaveBeenCalledTimes(1);
    expect(second.data).toHaveBeenCalledTimes(1);
  });
  it('isolates concurrent queries for one document while unrelated documents remain parallel', async () => {
    const api={} as PagefindAPI;
    const raw={url:'/same/',excerpt:'',meta:{title:'Shared fragment'}};
    let release!: ()=>void;
    const started:string[]=[];
    const first={id:'same',data:async()=>{started.push('first');await new Promise<void>(resolve=>{release=resolve;});raw.excerpt='first';return raw;}};
    const second={id:'same',data:async()=>{started.push('second');raw.excerpt='second';return raw;}};
    const other={id:'other',data:async()=>{started.push('other');return {url:'/other/',meta:{}};}};
    const a=resultData(api,first),b=resultData(api,second),c=resultData(api,other);
    await c;
    expect(started).toEqual(['first','other']);
    release();
    expect((await a).excerpt).toBe('first');
    expect((await b).excerpt).toBe('second');
  });
  it('allows queued hydration and retries after a failed fragment fetch', async () => {
    const api={} as PagefindAPI;
    const failed={id:'same',data:vi.fn().mockRejectedValueOnce(new Error('temporary')).mockResolvedValue({url:'/fixed/',meta:{}})};
    const next={id:'same',data:vi.fn(async()=>({url:'/next/',meta:{}}))};
    const a=resultData(api,failed),b=resultData(api,next);
    await expect(a).rejects.toThrow('temporary');
    expect((await b).url).toBe('/next/');
    expect((await resultData(api,failed)).url).toBe('/fixed/');
  });
});

describe('contextual facet counts', () => {
  it('uses Pagefind counts for the complete filtered query without loading article fragments', async () => {
    const search = vi.fn(async () => ({ results: [], filters: { source: { blog: 2 }, category: { rust: 2 } } }));
    const result = await executeFacetCounts({search} as unknown as PagefindAPI, parseQuery('category:rust'), manifest, 'source');
    expect(result).toEqual([{value:'blog',label:'Blog',count:2}]);
    expect(search).toHaveBeenCalledTimes(1);
    expect(search).toHaveBeenCalledWith(null, expect.objectContaining({filters:{category:{any:['rust']}}}));
  });
  it('counts exact phrase intersections and exclusions across every matching ID', async () => {
    const data = vi.fn(async () => ({url:'never',meta:{}}));
    const refs = (ids: string[]): ResultRef[] => ids.map(id => ({id,data}));
    const catalog = {...manifest, facets:{...manifest.facets, source:[{value:'one',label:'One',count:100},{value:'two',label:'Two',count:100},{value:'empty',label:'Empty',count:10}]}};
    const search = vi.fn(async (term: string | null, options?: Record<string, unknown>) => {
      const filters = options?.filters as Record<string, unknown>;
      const source = filters?.source;
      const ids = source === 'one' ? ['2','3'] : source === 'two' ? ['4','5'] : source === 'empty' ? ['8'] : term === 'rust' ? ['1','2','3','4','5'] : term === '"borrow checker"' ? ['2','3','4'] : term === 'old' ? ['3'] : [];
      return {results:refs(ids),filters:{source:{one:90,two:80,empty:10}}};
    });
    const result = await executeFacetCounts({search} as unknown as PagefindAPI,parseQuery('rust "borrow checker" -old category:rust'),catalog,'source');
    expect(result).toEqual([{value:'one',label:'One',count:1},{value:'two',label:'Two',count:1}]);
    expect(data).not.toHaveBeenCalled();
  });
});

describe('search index replacement', () => {
  it('warms a valid query once without searching, and skips cancelled or unresolved intent', async () => {
    const fetcher=vi.spyOn(globalThis,'fetch').mockImplementation(async()=>new Response(JSON.stringify(manifest)));
    const search=vi.fn(async()=>({results:[]}));
    const preload=vi.fn(async()=>{});
    const imported=vi.fn(async()=>({init:async()=>{},options:async()=>{},filters:async()=>({}),search,preload}));
    const engine=new SearchEngine('https://reader.test/',moduleImport(imported));
    try {
      const cancelled=new AbortController();cancelled.abort();
      await expect(engine.prepare(parseQuery('rust'),cancelled.signal)).rejects.toThrow();
      await expect(engine.prepare(parseQuery('source:partial'))).rejects.toThrow(/Unknown source/);
      expect(imported).not.toHaveBeenCalled();
      await Promise.all([engine.prepare(parseQuery('rust')),engine.prepare(parseQuery('source:blog'))]);
      expect(imported).toHaveBeenCalledTimes(1);
      expect(search).not.toHaveBeenCalled();
      expect(preload).toHaveBeenCalledExactlyOnceWith(null,{filters:{source:{any:['blog']}}});
      await engine.search(parseQuery('rust'),1,25);
      expect(imported).toHaveBeenCalledTimes(1);
      expect(search).toHaveBeenCalledTimes(1);
    } finally {fetcher.mockRestore();}
  });
  it('loads completion vocabulary without importing Pagefind or downloading filter indexes', async () => {
    const fetcher = vi.spyOn(globalThis,'fetch').mockResolvedValue(new Response(JSON.stringify(manifest)));
    const filters = vi.fn(async () => ({}));
    const imported = vi.fn(async () => ({init:async()=>{},options:async()=>{},preload:async()=>{},filters,search:async()=>({results:[],filters:{source:{blog:0}}})}));
    const engine = new SearchEngine('https://reader.test/', moduleImport(imported));
    try {
      expect((await engine.load()).manifest.facets.source[0].label).toBe('Blog');
      expect(fetcher).toHaveBeenCalledWith(new URL('https://reader.test/search-catalog.json'),{cache:'no-store',signal:expect.any(AbortSignal)});
      expect(imported).not.toHaveBeenCalled();
      await engine.search(parseQuery('rust'),1,25);
      expect(imported).toHaveBeenCalledTimes(1);
      expect(filters).not.toHaveBeenCalled();
      await engine.facetCounts(parseQuery('category:rust'),'source');
      await engine.facetCounts(parseQuery('category:"World News"'),'source');
      expect(filters).toHaveBeenCalledTimes(1);
    } finally {fetcher.mockRestore();}
  });
  it('reports an unavailable catalogue without downloading the full offline manifest', async () => {
    const fetcher = vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('not found', { status: 404 }));
    const imported = vi.fn();
    try {
      const engine = new SearchEngine('https://reader.test/', imported);
      await expect(engine.load()).rejects.toThrow('Search index is unavailable');
      expect(fetcher.mock.calls.map(call => String(call[0]))).toEqual(['https://reader.test/search-catalog.json']);
      expect(imported).not.toHaveBeenCalled();
    } finally { fetcher.mockRestore(); }
  });
  it('retains the last usable index when catalogue refresh fails', async () => {
    const fetcher = vi.spyOn(globalThis, 'fetch').mockResolvedValueOnce(new Response(JSON.stringify(manifest)))
      .mockRejectedValueOnce(new Error('offline'));
    try {
      const engine = new SearchEngine('https://reader.test/', vi.fn());
      const initial = await engine.load();
      expect(await engine.load(true)).toBe(initial);
      expect(fetcher.mock.calls.map(call => String(call[0]))).toEqual(['https://reader.test/search-catalog.json', 'https://reader.test/search-catalog.json']);
    } finally { fetcher.mockRestore(); }
  });
  it('loads the worker committed index after invalidation instead of retaining the online index', async () => {
    let served = { ...manifest, version: 'new', base: 'pagefind/new/' };
    const fetcher = vi.spyOn(globalThis, 'fetch').mockImplementation(async () => new Response(JSON.stringify(served)));
    const imported: string[] = [];
    const engine = new SearchEngine('https://reader.test/reader/', moduleImport(async url => {
      imported.push(url);
      return { init: async () => {}, options: async () => {}, filters: async () => ({}), preload: async () => {}, search: async () => ({ results: [] }) };
    }));
    try {
      expect((await engine.load()).manifest.version).toBe('new');
      await engine.search(parseQuery('source:blog'), 1, 25);
      served = { ...manifest, version: 'old', base: 'pagefind/old/' };
      engine.invalidate();
      expect((await engine.load()).manifest.version).toBe('old');
      await engine.search(parseQuery('source:blog'), 1, 25);
      expect(imported).toEqual(['https://reader.test/reader/pagefind/new/pagefind.js', 'https://reader.test/reader/pagefind/old/pagefind.js']);
      expect(fetcher).toHaveBeenCalledTimes(2);
    } finally { fetcher.mockRestore(); }
  });
  it('does not allow a pending online manifest to replace the committed offline index', async () => {
    let finishOnline!: (response: Response) => void;
    const fetcher = vi.spyOn(globalThis, 'fetch')
      .mockImplementationOnce(() => new Promise<Response>(resolve => { finishOnline = resolve; }))
      .mockResolvedValue(new Response(JSON.stringify(manifest)));
    const engine = new SearchEngine('https://reader.test/', moduleImport(async () => ({ init: async () => {}, options: async () => {}, filters: async () => ({}), preload: async () => {}, search: async () => ({ results: [] }) })));
    try {
      const online = engine.load();
      engine.invalidate();
      expect((await engine.load()).manifest.version).toBe('v1');
      finishOnline(new Response(JSON.stringify({ ...manifest, version: 'stale-online' })));
      await online;
      expect((await engine.load()).manifest.version).toBe('v1');
    } finally { fetcher.mockRestore(); }
  });
});

describe('application search session', () => {
  afterEach(() => vi.restoreAllMocks());
  function session() {
    let served = manifest;
    const fetcher = vi.spyOn(globalThis, 'fetch').mockImplementation(async () => new Response(JSON.stringify(served)));
    const init = vi.fn(async () => {}), options = vi.fn(async () => {});
    const search = vi.fn(async () => ({results:[]}));
    const imported = vi.fn(async (_url:string) => ({init, options, search, filters:async()=>({}), preload:async()=>{}}));
    const engine = new SearchEngine('https://reader.test/', moduleImport(imported));
    return {engine,fetcher,imported,init,options,search,serve:(version:string)=>{served={...manifest,version,base:`pagefind/${version}/`};}};
  }
  it('shares catalogue and initialized queries between pages without coupling page cancellation', async () => {
    const {engine,fetcher,imported,init,search} = session();
    const page = new AbortController();
    await engine.prepare(parseQuery('rust'),page.signal);
    page.abort();
    const result = await engine.search(parseQuery('rust'),1,25);
    expect(await engine.search(parseQuery('rust'),1,25)).toEqual(result);
    expect(fetcher).toHaveBeenCalledTimes(1);
    expect(imported).toHaveBeenCalledTimes(1);
    expect(init).toHaveBeenCalledTimes(1);
    expect(search).toHaveBeenCalledTimes(1);
    engine.dispose();
    await expect(engine.load()).rejects.toThrow(/disposed/);
  });
  it('revalidates stale catalogues once and reuses initialization if the index is unchanged', async () => {
    const {engine,fetcher,imported,init,serve} = session();
    await engine.search(parseQuery('rust'),1,25);
    engine.markStale();
    await Promise.all([engine.search(parseQuery('rust'),1,25),engine.search(parseQuery('rust'),1,25)]);
    expect(fetcher).toHaveBeenCalledTimes(2);
    expect(init).toHaveBeenCalledTimes(1);
    serve('v2');
    engine.markStale();
    await engine.search(parseQuery('rust'),1,25);
    expect(imported.mock.calls.map(call=>call[0])).toEqual(['https://reader.test/pagefind/v1/pagefind.js','https://reader.test/pagefind/v2/pagefind.js']);
    engine.dispose();
  });
  it('notices network changes between pages and bounds retained initialized versions', async () => {
    const {engine,fetcher,imported,serve} = session();
    engine.setOnline(true);
    await engine.search(parseQuery('rust'),1,25);
    engine.setOnline(true);
    await engine.load();
    expect(fetcher).toHaveBeenCalledTimes(1);
    serve('v2');engine.setOnline(false);
    await engine.search(parseQuery('rust'),1,25);
    serve('v1');engine.setOnline(true);
    await engine.search(parseQuery('rust'),1,25);
    expect(imported).toHaveBeenCalledTimes(2);
    serve('v3');engine.markStale();
    await engine.search(parseQuery('rust'),1,25);
    serve('v2');engine.markStale();
    await engine.search(parseQuery('rust'),1,25);
    expect(imported).toHaveBeenCalledTimes(4);
    engine.dispose();
  });
  it('keeps the usable index if revalidation fails and retries on the next page', async () => {
    const {engine,fetcher,serve}=session();
    const initial=await engine.load();
    engine.markStale();
    fetcher.mockRejectedValueOnce(new Error('temporarily disconnected'));
    expect(await engine.load()).toBe(initial);
    serve('v2');
    expect((await engine.load()).manifest.version).toBe('v2');
    engine.dispose();
  });
  it('does not initialize a late catalogue after application disposal', async () => {
    let finish!: (response:Response)=>void;
    const fetcher=vi.spyOn(globalThis,'fetch').mockImplementation(()=>new Promise(resolve=>{finish=resolve;}));
    const imported=vi.fn();
    const engine=new SearchEngine('https://reader.test/',imported);
    const pending=engine.load();
    engine.dispose();
    finish(new Response(JSON.stringify(manifest)));
    await expect(pending).rejects.toThrow(/disposed/);
    expect(imported).not.toHaveBeenCalled();
    expect(fetcher).toHaveBeenCalledTimes(1);
  });
  it('does not create an index instance after disposal during module import', async () => {
    let finish!: (module:PagefindModule)=>void;
    vi.spyOn(globalThis,'fetch').mockResolvedValue(new Response(JSON.stringify(manifest)));
    const createInstance=vi.fn();
    const engine=new SearchEngine('https://reader.test/',()=>new Promise(resolve=>{finish=resolve;}));
    const initializing=engine.prepare(parseQuery('rust'));
    await vi.waitFor(()=>expect(finish).toBeTypeOf('function'));
    const disposal=engine.dispose();finish({createInstance});
    await expect(initializing).rejects.toThrow(/disposed/);
    await disposal;
    expect(createInstance).not.toHaveBeenCalled();
  });
});
