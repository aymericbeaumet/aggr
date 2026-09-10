import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { get, type Readable } from 'svelte/store';
import type { Completion } from './completion';
import type { ViewState } from './state';
import type { Query, SearchHandle, SearchCatalog } from './types';
import type { SearchSession } from './engine';

const mocks = vi.hoisted(() => ({ mount: vi.fn(), unmount: vi.fn(), load: vi.fn(), search: vi.fn(), facetCounts: vi.fn(), prepare:vi.fn(), moveSelection: vi.fn(), openSelected: vi.fn() }));
vi.mock('svelte', () => ({ mount: mocks.mount, unmount: mocks.unmount, tick: async () => {} }));
vi.mock('./Controls.svelte', () => ({ default: 'controls' }));
vi.mock('./Results.svelte', () => ({ default: 'results' }));
import { buildFilters } from './engine';
import { mountSearch } from './index';

const manifest: SearchCatalog = {
  version: 'v1', base: 'pagefind/v1/', docs: 4,
  facets: { source: [
    { value: 'rust-blog', label: 'The Rust Blog', count: 3 },
    { value: 'openai', label: 'OpenAI', count: 1 }
  ], category: [], tag: [], 'published-day': [] }
};
interface Controls {
  model: Readable<ViewState>;
  move(direction: number): boolean; open(): boolean;
  change(query: string, cursor: number, composing: boolean): void;
  choose(completion: Completion): void;
  focusInput(): void;
  revealInput(): void;
}
let handle: SearchHandle | undefined;
function fakeSession(): SearchSession { return {load:mocks.load,search:mocks.search,facetCounts:mocks.facetCounts,prepare:mocks.prepare,setOnline:vi.fn(),markStale:vi.fn(),dispose:vi.fn()}; }
function setup(query = '', session = fakeSession()) {
  vi.stubGlobal('location', new URL('https://reader.test/' + (query ? '?q=' + encodeURIComponent(query) : '')));
  const input = { focus: vi.fn(), setSelectionRange: vi.fn(), getBoundingClientRect: vi.fn(() => ({ top: 66, bottom: 110, left: 12, right: 378 })) };
  const staticFeed = { hidden: false, querySelector: () => null };
  const root = { dataset: {}, childNodes: [], replaceChildren: vi.fn(), querySelector: () => input };
  handle = mountSearch({
    root: root as unknown as HTMLElement,
    resultsRoot: { contains: () => false, querySelector: () => null, querySelectorAll: () => [] } as unknown as HTMLElement,
    staticFeed: staticFeed as unknown as HTMLElement,
    base: 'https://reader.test/', preferences: { values: {} },
    session,
    navigate: vi.fn(), onRowsChanged: vi.fn(), getOfflineStatus: () => null,
    onQueryChanged: vi.fn(), replaceLocation: vi.fn(),
    moveSelection: mocks.moveSelection, openSelected: mocks.openSelected
  });
  const controls = mocks.mount.mock.calls.filter(call => call[0] === 'controls').at(-1)![1].props as Controls;
  return { controls, state: () => get(controls.model), staticFeed, root, input };
}
async function settle() { await vi.advanceTimersByTimeAsync(200); }

beforeEach(() => {
  vi.useFakeTimers();
  vi.resetAllMocks();
  vi.stubGlobal('navigator', { onLine: true });
  vi.stubGlobal('window', Object.assign(new EventTarget(), { innerWidth: 390, innerHeight: 844, scrollTo: vi.fn(), getComputedStyle: () => ({ visibility: 'visible', display: 'block' }) }));
  vi.stubGlobal('document', { activeElement: null, querySelector: (selector: string) => selector === '.top' ? { getBoundingClientRect: () => ({ bottom: 44 }) } : null });
  vi.stubGlobal('history', { state: null, replaceState: vi.fn() });
  mocks.load.mockResolvedValue({ manifest });
  mocks.facetCounts.mockResolvedValue([]);
  mocks.prepare.mockResolvedValue(undefined);
  mocks.search.mockImplementation(async (query: Query) => {
    buildFilters(query, manifest);
    return { results: [{ url: '/article/', meta: { title: 'An article' } }], total: 1, page: 1, pages: 1, size: 50 };
  });
});
afterEach(async () => { await handle?.destroy(); handle = undefined; vi.useRealTimers(); vi.unstubAllGlobals(); });

describe('result cursor keys in the search controller', () => {
  it('moves and opens results only once a search has produced rows', async () => {
    mocks.moveSelection.mockReturnValue(true);
    mocks.openSelected.mockReturnValue(true);
    const { controls, state } = setup();
    expect(controls.move(1)).toBe(false);
    expect(controls.open()).toBe(false);
    expect(mocks.moveSelection).not.toHaveBeenCalled();
    expect(mocks.openSelected).not.toHaveBeenCalled();
    controls.change('rust', 4, false);
    expect(controls.open()).toBe(false);
    await settle();
    expect(state().ready).toBe(true);
    expect(controls.move(-1)).toBe(true);
    expect(mocks.moveSelection).toHaveBeenCalledWith(-1);
    expect(controls.open()).toBe(true);
    expect(mocks.openSelected).toHaveBeenCalledTimes(1);
  });
});

describe('facet completion in the search controller', () => {
  it('warms only completed query intent during debounce and cancels it on replacement', async () => {
    const {controls}=setup();
    controls.focusInput();
    await settle();
    controls.change('source:rust',11,false);
    expect(mocks.prepare).not.toHaveBeenCalled();
    controls.change('source:"The Rust Blog"',22,false);
    expect(mocks.prepare).toHaveBeenCalledTimes(1);
    expect(mocks.search).not.toHaveBeenCalled();
    const signal=mocks.prepare.mock.lastCall![1] as AbortSignal;
    controls.change('source:',7,false);
    expect(signal.aborted).toBe(true);
    expect(mocks.prepare).toHaveBeenCalledTimes(1);
  });
  it('loads source values for an incomplete initial URL without a validation error', async () => {
    const { controls, state, staticFeed } = setup('source:');
    await settle();
    expect(mocks.load).toHaveBeenCalled();
    expect(state().error).toBe('');
    expect(state().suggestions.map(item => item.label)).toEqual(['The Rust Blog', 'OpenAI']);
    controls.focusInput();
    expect(state().open).toBe(true);
    expect(staticFeed.hidden).toBe(true);
    expect(state().ready).toBe(false);
    expect(mocks.search).not.toHaveBeenCalled();
  });

  it('hides results while completing blank or partial sources and searches the selected readable alias', async () => {
    const { controls, state } = setup('rust');
    await settle();
    const previous = state().page;
    for (const query of ['source:', 'source:rust', 'source:"The Rust']) {
      controls.change(query, query.length, false);
      await settle();
      expect(state().error).toBe('');
      expect(state().busy).toBe(false);
      expect(state().page).toBe(previous);
      expect(state().ready).toBe(false);
      expect(state().open).toBe(true);
      expect(state().suggestions[0].label).toBe('The Rust Blog');
    }
    expect(mocks.search).toHaveBeenCalledTimes(1);
    controls.choose(state().suggestions[0]);
    await settle();
    expect(state().query).toBe('source:"The Rust Blog" ');
    expect(state().error).toBe('');
    expect(state().open).toBe(false);
    expect(state().ready).toBe(true);
    expect(mocks.search).toHaveBeenCalledTimes(2);
    expect(mocks.search.mock.lastCall![0].clauses[0]).toMatchObject({ kind: 'facet', field: 'source', value: 'The Rust Blog' });
  });

  it('hides the initial feed immediately and waits for the current search response', async () => {
    let resolve!: (value: Awaited<ReturnType<typeof mocks.search>>) => void;
    mocks.search.mockImplementationOnce(() => new Promise(done => { resolve = done; }));
    const { controls, state, staticFeed } = setup();
    controls.change('rust', 4, false);
    expect(staticFeed.hidden).toBe(true);
    expect(state().ready).toBe(false);
    await settle();
    expect(state().busy).toBe(true);
    expect(state().ready).toBe(false);
    resolve({ results: [{ url: '/rust/', meta: { title: 'Rust article' } }], total: 1, page: 1, pages: 1, size: 50 });
    await settle();
    expect(state().ready).toBe(true);
    expect(state().page.results[0].url).toBe('/rust/');
    controls.change('next', 4, false);
    expect(state().ready).toBe(false);
    expect(staticFeed.hidden).toBe(true);
    controls.change('', 0, false);
    expect(staticFeed.hidden).toBe(false);
    expect(state().active).toBe(false);
  });

  it('keeps current rows mounted during background index refresh but hides changed queries', async () => {
    const { controls, state } = setup('rust');
    await settle();
    const page = state().page;
    let resolve!: (value: unknown) => void;
    mocks.search.mockImplementationOnce(() => new Promise(done => { resolve = done; }));
    handle!.refresh();
    await settle();
    expect(state().ready).toBe(true);
    expect(state().busy).toBe(true);
    expect(state().page).toBe(page);
    resolve({ ...page, total: 2, results: [...page.results, { url: '/new/', meta: { title: 'New article' } }] });
    await settle();
    expect(state().page.total).toBe(2);
    controls.change('another query', 13, false);
    expect(state().ready).toBe(false);
  });

  it('keeps stale results hidden after a failed replacement search', async () => {
    const { controls, state, staticFeed } = setup('rust');
    await settle();
    expect(state().ready).toBe(true);
    mocks.search.mockRejectedValueOnce(new Error('Search failed'));
    controls.change('next', 4, false);
    await settle();
    expect(state().ready).toBe(false);
    expect(state().busy).toBe(false);
    expect(state().error).toBe('Search failed');
    expect(staticFeed.hidden).toBe(true);
  });

  it('does not reveal an older response while the replacement query is loading', async () => {
    const pending: ((value: unknown) => void)[] = [];
    mocks.search.mockImplementation(() => new Promise(resolve => pending.push(resolve)));
    const { controls, state } = setup('first');
    await settle();
    controls.change('second', 6, false);
    await settle();
    const page = (title: string) => ({ results: [{ url: `/${title}/`, meta: { title } }], total: 1, page: 1, pages: 1, size: 50 });
    pending[0](page('first'));
    await settle();
    expect(state().ready).toBe(false);
    pending[1](page('second'));
    await settle();
    expect(state().ready).toBe(true);
    expect(state().page.results[0].meta.title).toBe('second');
  });

  it.each(['source:rust-blog', 'source:"The Rust Blog"', 'source:"the rust blog"'])('executes the complete facet URL %s without trailing whitespace', async query => {
    const { state, staticFeed } = setup(query);
    await settle();
    expect(state().error).toBe('');
    expect(state().ready).toBe(true);
    expect(state().suggestions).toEqual([]);
    expect(staticFeed.hidden).toBe(true);
    expect(mocks.search).toHaveBeenCalledTimes(1);
    expect(mocks.search.mock.lastCall![0].raw).toBe(query === 'source:rust-blog' ? 'source:"The Rust Blog"' : query);
  });

  it('still reports unmatched values and invalid syntax outside the active completion', async () => {
    const { controls, state } = setup();
    for (const [query, message] of [
      ['source:missing', /Unknown source/],
      ['sort:invalid source:', /Sort by/],
      ['date:2026-99-01 source:', /valid UTC date/],
      ['source: tag:', /Add a value after source:/]
    ] as const) {
      controls.change(query, query.length, false);
      await settle();
      expect(state().error).toMatch(message);
    }
  });

  it('uses only contextual sources and rejects a slower response from the previous filters', async () => {
    const facets = {...manifest.facets, category:[{value:'programming',label:'Programming',count:3},{value:'news',label:'News',count:1}]};
    mocks.load.mockResolvedValue({manifest:{...manifest,facets}});
    const pending: {query: Query; resolve: (value: typeof facets.source) => void; signal: AbortSignal}[] = [];
    mocks.facetCounts.mockImplementation((query, _field, signal) => new Promise(resolve => pending.push({query,resolve,signal})));
    const {controls,state} = setup();
    controls.change('category:programming source:',28,false);
    await settle();
    expect(state().suggestions).toEqual([]);
    expect(pending[0].query.raw.trim()).toBe('category:programming');
    controls.change('category:news source:',21,false);
    await settle();
    expect(pending[0].signal.aborted).toBe(true);
    pending[1].resolve([{...facets.source[1],count:1}]);
    await settle();
    expect(state().suggestions.map(item=>[item.id,item.count])).toEqual([['source:openai',1]]);
    pending[0].resolve([{...facets.source[0],count:3}]);
    await settle();
    expect(state().suggestions.map(item=>item.id)).toEqual(['source:openai']);
    controls.choose(state().suggestions[0]);
    expect(state().query).toBe('category:news source:openai ');
    expect(state().open).toBe(false);
  });

  it('ignores delayed selection callbacks after the token changes', async () => {
    const {controls,state} = setup();
    controls.change('source:rust',11,false);
    await settle();
    const old = state().suggestions[0];
    controls.change('source:open',11,false);
    controls.choose(old);
    expect(state().query).toBe('source:open');
    expect(state().suggestions[0].id).toBe('source:openai');
    controls.choose(state().suggestions[0]);
    expect(state().query).toBe('source:openai ');
  });

  it('does not rewrite user typing when initial URL vocabulary arrives late', async () => {
    let resolve!: (value: {manifest: SearchCatalog}) => void;
    mocks.load.mockImplementation(() => new Promise(done => {resolve = done;}));
    const {controls,state} = setup('source:rust-blog');
    controls.change('source:rust',11,false);
    await settle();
    resolve({manifest});
    await settle();
    expect(state().query).toBe('source:rust');
    expect(state().suggestions[0].insert).toBe('source:"The Rust Blog"');
  });
});


it('awaits both components before restoring fallback and ignores callbacks from a disposed search', async () => {
  let finishLoad!: (value: {manifest: SearchCatalog}) => void;
  mocks.load.mockImplementation(() => new Promise(resolve => { finishLoad = resolve; }));
  const releases: Array<() => void> = [];
  mocks.unmount.mockImplementation(() => new Promise<void>(resolve => releases.push(resolve)));
  const {controls, root} = setup('rust');
  const disposed = handle!.destroy();
  expect(handle!.destroy()).toBe(disposed);
  controls.change('new query', 9, false);
  finishLoad({manifest});
  await settle();
  expect(mocks.search).not.toHaveBeenCalled();
  expect(root.replaceChildren).toHaveBeenCalledTimes(1);
  releases[0]();
  await Promise.resolve();
  expect(root.replaceChildren).toHaveBeenCalledTimes(1);
  releases[1]();
  await disposed;
  expect(root.replaceChildren).toHaveBeenCalledTimes(2);
});

it('cancels a departing page without disposing the shared session or changing its replacement', async () => {
  const session=fakeSession();
  let finish!: (value:unknown)=>void;
  mocks.search.mockImplementationOnce(()=>new Promise(resolve=>{finish=resolve;}));
  const departing=setup('rust',session);
  await settle();
  await handle!.destroy();
  const replacement=setup('next',session);
  await settle();
  const current=replacement.state().page;
  finish({results:[{url:'/late/',meta:{title:'Late old page'}}],total:1,page:1,pages:1,size:50});
  await settle();
  expect(departing.state().ready).toBe(false);
  expect(replacement.state().page).toBe(current);
  expect(replacement.state().ready).toBe(true);
  expect(session.dispose).not.toHaveBeenCalled();
  expect(session.markStale).not.toHaveBeenCalled();
});

it('reveals clipped search on direct or repeated explicit focus without changing query or selection', async () => {
  const { controls, input, state } = setup('rust');
  await settle();
  expect(window.scrollTo).not.toHaveBeenCalled();
  input.getBoundingClientRect.mockReturnValue({ top: -30, bottom: 14, left: 12, right: 378 });
  controls.focusInput();
  expect(window.scrollTo).toHaveBeenLastCalledWith({ top: 0, left: 0, behavior: 'instant' });
  handle!.focus(); handle!.focus(); controls.revealInput();
  expect(window.scrollTo).toHaveBeenCalledTimes(4);
  expect(input.focus).toHaveBeenLastCalledWith({ preventScroll: true });
  expect(input.setSelectionRange).not.toHaveBeenCalled();
  expect(state().query).toBe('rust');
  input.getBoundingClientRect.mockReturnValue({ top: 66, bottom: 110, left: 12, right: 378 });
  handle!.focus(); controls.focusInput(); controls.revealInput();
  expect(window.scrollTo).toHaveBeenCalledTimes(4);
});

it('preserves scroll during focus restoration and ignores focus requests after disposal', async () => {
  const { controls, input } = setup();
  input.getBoundingClientRect.mockReturnValue({ top: -30, bottom: 14, left: 12, right: 378 });
  input.focus.mockImplementation(() => controls.focusInput());
  handle!.focus({ restore: true });
  expect(window.scrollTo).not.toHaveBeenCalled();
  await handle!.destroy();
  handle!.focus(); controls.revealInput();
  expect(window.scrollTo).not.toHaveBeenCalled();
  expect(input.focus).toHaveBeenCalledOnce();
});
