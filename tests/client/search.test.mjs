import assert from 'node:assert/strict';
import { test } from 'node:test';

globalThis.window = new EventTarget();
Object.defineProperty(globalThis.navigator, 'onLine', { configurable: true, value: true });
const { parseQuery, queryURL, complete, acceptCompletion, createSession } =
  await import('../../themes/default/static/search.js');

const base = 'https://reader.invalid/archive/';
const facet = (value, count = 1, label = value) => ({ value, label, count });
const catalog = version => ({ version, base: `search/${version}/`, docs: 2, facets: {
  source: [facet('publisher.invalid', 2)], category: [], tag: [], type: [],
  'published-day': [facet('2026-09-29'), facet('2026-09-28')],
} });
const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; };
const flush = async () => { await new Promise(resolve => setImmediate(resolve)); };

function runtime() {
  const instances = [];
  return { instances, importAPI: async () => ({ createInstance() {
    const instance = {
      initialized: 0, destroyed: 0, preloaded: 0,
      async init() { this.initialized++; },
      async destroy() { this.destroyed++; },
      async preload() { this.preloaded++; },
      async search() { return { results: [] }; },
    };
    instances.push(instance);
    return instance;
  } }) };
}

test('query grammar retains exclusions and quoted clauses and clears stale search pagination', () => {
  const query = parseQuery('"exact phrase" -"other phrase" source:publisher.invalid date:>=2026-09-28');
  assert.equal(query.clauses[0].quoted, true);
  assert.equal(query.clauses[1].exclude, true);
  assert.equal(query.clauses[3].from, '2026-09-28');
  const url = new URL(queryURL(`${base}?q=old&search-page=3`, 'new', 1));
  assert.equal(url.searchParams.get('q'), 'new');
  assert.equal(url.searchParams.has('search-page'), false);
  assert.equal(new URL(queryURL(url.href, '')).searchParams.has('q'), false);
});

test('completion replaces only the edited clause and keeps canonical publisher identifiers', () => {
  const facets = catalog('a').facets;
  const raw = 'article -source:pub type:video';
  const options = complete(raw, raw.indexOf(' type:'), facets);
  assert.equal(options[0].insert, '-source:publisher.invalid');
  assert.equal(acceptCompletion(raw, options[0]).query, 'article -source:publisher.invalid type:video');
  assert.deepEqual(complete('source:publisher.invalid', 24, facets), []);
});

test('date completion counts ranges against only the remaining query context', () => {
  const facets = { 'published-day': [facet('2026-09-29', 3), facet('2026-09-28', 2)] };
  const now = Date.parse('2026-09-29T12:00:00Z');
  const options = complete('date:', 5, facets, now, facets, true);
  assert.equal(options.find(option => option.id === 'date:today').count, 3);
  assert.equal(options.find(option => option.id === 'date:last7d').count, 5);
  const before = complete('before:', 7, facets, now, facets, true);
  assert.equal(before.find(option => option.id === 'before:2026-09-29').count, 2);
  assert.equal(before.some(option => option.id === 'before:2026-09-28'), false);
  assert.equal(complete('-date:', 6, facets, now, facets, true).find(option => option.id === 'date:today').count, 2);
  assert.deepEqual(complete('date:', 5, { 'published-day': [] }, now, facets, true), []);
});

test('catalogue-only startup reuses unchanged versions and retires evicted runtime instances', async () => {
  let version = 'a', requests = 0;
  globalThis.fetch = async () => { requests++; return { ok: true, json: async () => catalog(version) }; };
  const mock = runtime(), session = createSession(base, mock.importAPI);
  const first = await session.load();
  assert.equal(mock.instances.length, 0, 'completion vocabulary does not initialize Pagefind');
  await first.prepare(parseQuery('source:publisher.invalid'));
  assert.equal(mock.instances[0].preloaded, 1);
  session.stale();
  assert.equal(await session.load(), first);
  assert.equal(mock.instances.length, 1);
  version = 'b'; await (await session.load(true)).prepare(parseQuery('article'));
  version = 'c'; await (await session.load(true)).prepare(parseQuery('article'));
  await flush();
  assert.equal(mock.instances[0].destroyed, 1);
  assert.equal(requests, 4);
  session.dispose(); await flush();
  assert.deepEqual(mock.instances.map(instance => instance.destroyed), [1, 1, 1]);
});

test('eviction retains a runtime until pending fragment hydration releases it', async () => {
  let version = 'a';
  globalThis.fetch = async () => ({ ok: true, json: async () => catalog(version) });
  const mock = runtime(), session = createSession(base, mock.importAPI);
  const first = await session.load(); await first.prepare(parseQuery('article'));
  const fragment = deferred();
  mock.instances[0].search = async () => ({ results: [{ id: 'one', data: () => fragment.promise }] });
  const results = first.run(parseQuery('article'), 1, 10); await flush();
  version = 'b'; await session.load(true);
  version = 'c'; await session.load(true);
  assert.equal(mock.instances[0].destroyed, 0);
  fragment.resolve({ url: '/archive/item/', meta: { title: 'Article' } });
  assert.equal((await results).total, 1); await flush();
  assert.equal(mock.instances[0].destroyed, 1);
  session.dispose();
});

test('failed refresh keeps a usable catalogue and failed runtime initialization can retry', async () => {
  let failing = false;
  globalThis.fetch = async () => { if (failing) throw Error('offline'); return { ok: true, json: async () => catalog('a') }; };
  const mock = runtime();
  const session = createSession(base, async () => {
    const module = await mock.importAPI();
    const create = module.createInstance;
    module.createInstance = () => {
      const instance = create();
      if (mock.instances.length === 1) instance.init = async () => { throw Error('fragment unavailable'); };
      return instance;
    };
    return module;
  });
  const index = await session.load();
  await assert.rejects(index.prepare(parseQuery('article')));
  assert.equal(mock.instances[0].destroyed, 1);
  await index.prepare(parseQuery('article'));
  failing = true; session.stale();
  assert.equal(await session.load(), index);
  session.dispose();
});

test('query intersections and exclusions apply before count and pagination', async () => {
  globalThis.fetch = async () => ({ ok: true, json: async () => catalog('a') });
  const mock = runtime(), session = createSession(base, mock.importAPI);
  const index = await session.load(); await index.prepare(parseQuery('article'));
  const hydrated = [];
  const ref = id => ({ id, async data() { hydrated.push(id); return { url: `${base}${id}/`, meta: { title: id } }; } });
  mock.instances[0].search = async term => ({ results: ({
    article: ['a', 'b', 'c', 'd'], '"exact phrase"': ['b', 'c', 'd'], excluded: ['b'],
  })[term].map(ref) });
  const result = await index.run(parseQuery('article "exact phrase" -excluded'), 2, 1);
  assert.equal(result.total, 2); assert.equal(result.page, 2);
  assert.deepEqual(hydrated, ['d']);
  session.dispose();
});

test('compound queries keep at most six index requests in flight', async () => {
  globalThis.fetch = async () => ({ ok: true, json: async () => catalog('a') });
  const mock = runtime(), session = createSession(base, mock.importAPI);
  const index = await session.load(); await index.prepare(parseQuery('article'));
  const gate = deferred(); let active = 0, peak = 0, calls = 0;
  mock.instances[0].search = async () => {
    calls++; peak = Math.max(peak, ++active);
    await gate.promise; active--;
    return { results: [] };
  };
  const result = index.run(parseQuery(Array.from({ length: 10 }, (_, at) => `"phrase ${at}"`).join(' ')), 1, 10);
  await flush(); assert.equal(calls, 6); assert.equal(peak, 6);
  gate.resolve(); await result;
  assert.equal(calls, 10); assert.equal(peak, 6);
  session.dispose();
});

test('application disposal prevents late imports from creating a worker', async () => {
  globalThis.fetch = async () => ({ ok: true, json: async () => catalog('a') });
  const imported = deferred(), mock = runtime();
  const session = createSession(base, () => imported.promise);
  const index = await session.load();
  const pending = index.prepare(parseQuery('article'));
  session.dispose();
  imported.resolve(await mock.importAPI());
  await assert.rejects(pending);
  assert.equal(mock.instances.length, 0);
});

test('offline transitions choose the complete committed catalogue and reject partial indexes', async () => {
  const requests = [];
  globalThis.fetch = async url => {
    requests.push(String(url));
    return { ok: true, json: async () => catalog(navigator.onLine ? 'online' : 'saved') };
  };
  const mock = runtime(), session = createSession(base, mock.importAPI);
  await session.load();
  Object.defineProperty(navigator, 'onLine', { configurable: true, value: false });
  session.network();
  await assert.rejects(session.load(), /complete offline/);
  window.AGGROffline = { saved: [], search: { activeVersion: 'saved', base: 'search/saved/' } };
  const index = await session.load();
  assert.equal(index.manifest.version, 'saved');
  assert.equal(requests.at(-1), `${base}search/saved/search-catalog.json`);
  Object.defineProperty(navigator, 'onLine', { configurable: true, value: true });
  delete window.AGGROffline;
  session.dispose();
});

test('a late online catalogue cannot replace the committed offline index', async () => {
  const delayed = deferred();
  globalThis.fetch = async () => navigator.onLine ? delayed.promise : { ok: true, json: async () => catalog('saved') };
  const mock = runtime(), session = createSession(base, mock.importAPI);
  const online = session.load(); await flush();
  Object.defineProperty(navigator, 'onLine', { configurable: true, value: false });
  window.AGGROffline = { saved: [], search: { activeVersion: 'saved', base: 'search/saved/' } };
  session.network();
  const offline = await session.load();
  delayed.resolve({ ok: true, json: async () => catalog('online') });
  assert.equal(await online, offline);
  assert.equal((await session.load()).manifest.version, 'saved');
  Object.defineProperty(navigator, 'onLine', { configurable: true, value: true });
  delete window.AGGROffline;
  session.dispose();
});
