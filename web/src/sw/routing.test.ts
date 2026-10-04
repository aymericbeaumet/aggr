import { describe, expect, it } from 'vitest';
import { PAGE_LIMIT, trim } from './caches';
import { harness, scope } from './harness';
import { assetResponse, classify, handleFetch, immutable, pageResponse, type RequestLike } from './routing';
import type { FetchEventLike } from './types';

const origin = new URL(scope).origin;
const scopePath = new URL(scope).pathname;
const sha1 = '0'.repeat(40);
const sha256 = 'b'.repeat(64);

const request = (url: string, over: Partial<RequestLike> = {}): RequestLike => ({
  method: 'GET',
  url: new URL(url, scope).href,
  mode: 'cors',
  headers: new Headers(),
  ...over,
});

describe('classify', () => {
  it('leaves other methods, origins and scopes to the browser', () => {
    expect(classify(request('items/a/', { method: 'POST' }), origin, scopePath)).toBeNull();
    expect(classify(request('https://elsewhere.invalid/archive/items/a/'), origin, scopePath)).toBeNull();
    expect(classify(request('https://reader.invalid/other/items/a/'), origin, scopePath)).toBeNull();
  });

  it('treats navigations and HTML requests as pages', () => {
    expect(classify(request('items/a/', { mode: 'navigate' }), origin, scopePath)).toBe('page');
    expect(classify(request('items/a/', { headers: new Headers({ accept: 'text/html,*/*' }) }), origin, scopePath)).toBe('page');
  });

  it('serves content-addressed files cache first and everything else network first', () => {
    expect(classify(request(`assets/images/${sha1}.webp`), origin, scopePath)).toBe('asset');
    expect(classify(request(`assets/previews/${sha1}.png`), origin, scopePath)).toBe('asset');
    expect(classify(request(`assets/documents/${sha1}.pdf`), origin, scopePath)).toBe('asset');
    expect(classify(request('assets/app/worker-0123456789ab.js'), origin, scopePath)).toBe('asset');
    expect(classify(request(`pagefind/${sha256}/pagefind.js`), origin, scopePath)).toBe('asset');
    expect(classify(request('feed.xml'), origin, scopePath)).toBe('mutable');
    expect(classify(request('updates.json'), origin, scopePath)).toBe('mutable');
    expect(classify(request('search-manifest.json'), origin, scopePath)).toBe('mutable');
    expect(classify(request('assets/images/not-hashed.webp'), origin, scopePath)).toBe('mutable');
    expect(classify(request('assets/style.css'), origin, scopePath)).toBe('mutable');
  });
});

describe('immutable', () => {
  it('recognizes archived asset hashes and generated browser chunk hashes', () => {
    expect(immutable(scopePath, `${scopePath}assets/images/${sha1}.webp`)).toBe(true);
    expect(immutable(scopePath, `${scopePath}assets/images/${sha1.slice(1)}.webp`)).toBe(false);
    expect(immutable(scopePath, `${scopePath}assets/images/${sha1}`)).toBe(true);
    expect(immutable(scopePath, `${scopePath}assets/icons/icon-0123456789ab.svg`)).toBe(true);
    expect(immutable(scopePath, `${scopePath}assets/icons/icon-0123456789.svg`)).toBe(false);
    expect(immutable(scopePath, `${scopePath}assets/app/page.svelte-Ck4TY_xv.js`)).toBe(true);
    expect(immutable(scopePath, `${scopePath}assets/app/main.js`)).toBe(false);
    expect(immutable(scopePath, `${scopePath}assets/icons/icon-Ck4TY_xv.svg`)).toBe(false);
    expect(immutable(scopePath, `${scopePath}pagefind/${sha256}/x.pf_fragment`)).toBe(true);
    expect(immutable(scopePath, `${scopePath}pagefind/${sha256.slice(1)}/x.pf_fragment`)).toBe(false);
    expect(immutable(scopePath, `${scopePath}items/${sha1}/`)).toBe(false);
  });
});

describe('pageResponse', () => {
  it('caches successful pages and answers from the last copy when the network fails', async () => {
    const { ctx, state, storage } = harness();
    const url = `${scope}items/a/`;
    expect(await (await pageResponse(ctx, new Request(url))).text()).toBe(url);
    expect(await (await storage.open(ctx.names.pages)).match(url)).toBeTruthy();
    state.offline = true;
    expect(await (await pageResponse(ctx, new Request(url))).text()).toBe(url);
    expect(await (await pageResponse(ctx, new Request(`${url}?ref=feed`))).text()).toBe(url);
  });

  it('keeps the last copy over a server error but lets 404 stand', async () => {
    const { ctx, state, storage } = harness();
    const url = `${scope}items/a/`;
    await (await storage.open(ctx.names.pages)).put(url, new Response('earlier copy'));
    state.network = async () => new Response('down', { status: 503 });
    expect(await (await pageResponse(ctx, new Request(url))).text()).toBe('earlier copy');
    state.network = async () => new Response('gone', { status: 404 });
    expect((await pageResponse(ctx, new Request(url))).status).toBe(404);
    state.network = async () => new Response('down', { status: 500 });
    expect((await pageResponse(ctx, new Request(`${scope}items/unknown/`))).status).toBe(500);
  });

  it('falls back to the offline page, and otherwise fails like the network', async () => {
    const { ctx, state, storage } = harness();
    state.offline = true;
    await expect(pageResponse(ctx, new Request(`${scope}items/a/`))).rejects.toThrow('offline');
    await (await storage.open(ctx.names.shell)).put(`${scope}offline.html`, new Response('offline page'));
    expect(await (await pageResponse(ctx, new Request(`${scope}items/a/`))).text()).toBe('offline page');
  });

  it('prefers a protected article over a visited copy', async () => {
    const { ctx, state, storage } = harness();
    const url = `${scope}items/a/`;
    await (await storage.open(ctx.names.pages)).put(url, new Response('visited'));
    await (await storage.open(ctx.names.offlinePages)).put(url, new Response('protected'));
    state.offline = true;
    expect(await (await pageResponse(ctx, new Request(url))).text()).toBe('protected');
  });

  it('uses the navigation preload within the deadline and requests its own page past it', async () => {
    const { ctx, requests, storage } = harness({ timeouts: { network: 20 } });
    const url = `${scope}items/a/`;
    const preloaded = await pageResponse(ctx, new Request(url), Promise.resolve(new Response('preloaded')));
    expect(await preloaded.text()).toBe('preloaded');
    expect(requests).toHaveLength(0);
    // Unsupported preload resolves to undefined: the worker still owes a request of its own.
    expect(await (await pageResponse(ctx, new Request(url), Promise.resolve(undefined))).text()).toBe(url);
    expect(requests).toHaveLength(1);
    await (await storage.open(ctx.names.pages)).put(url, new Response('last known'));
    const late = await pageResponse(ctx, new Request(url), new Promise(() => {}));
    expect(await late.text()).toBe('last known');
    expect(requests).toHaveLength(1);
  });
});

describe('assetResponse and trim', () => {
  it('answers from any cache before the network and remembers new assets within the bound', async () => {
    const { ctx, requests, storage } = harness();
    const url = `${scope}assets/images/${sha1}.webp`;
    await (await storage.open(ctx.names.offlinePages)).put(url, new Response('protected image'));
    expect(await (await assetResponse(ctx, new Request(url), ctx.names.assets, 2)).text()).toBe('protected image');
    expect(requests).toHaveLength(0);
    for (const name of ['one', 'two', 'three']) {
      await assetResponse(ctx, new Request(`${scope}assets/app/${name}-0123456789ab.js`), ctx.names.assets, 2);
    }
    const assets = await storage.open(ctx.names.assets);
    expect((await assets.keys()).map((key) => key.url)).toEqual([
      `${scope}assets/app/two-0123456789ab.js`,
      `${scope}assets/app/three-0123456789ab.js`,
    ]);
    await trim(storage, ctx.names.assets, 1);
    expect(await assets.keys()).toHaveLength(1);
  });

  it('does not remember error responses', async () => {
    const { ctx, state, storage } = harness();
    state.network = async () => new Response('missing', { status: 404 });
    const response = await assetResponse(ctx, new Request(`${scope}assets/app/x-0123456789ab.js`), ctx.names.assets, 2);
    expect(response.status).toBe(404);
    expect(await (await storage.open(ctx.names.assets)).keys()).toHaveLength(0);
  });
});

describe('handleFetch', () => {
  const event = (url: string, init: RequestInit = {}) => {
    let responded: Promise<Response> | undefined;
    const fetchEvent: FetchEventLike = {
      request: new Request(new URL(url, scope).href, init),
      preloadResponse: Promise.resolve(undefined),
      respondWith: (response) => {
        responded = response;
      },
      waitUntil: () => {},
    };
    return { fetchEvent, responded: () => responded };
  };

  it('answers mutable files from any cached copy when the network fails, else with a network error', async () => {
    const { ctx, state, storage } = harness();
    const url = `${scope}feed.xml`;
    await (await storage.open(ctx.names.shell)).put(url, new Response('<feed/>'));
    state.offline = true;
    const cached = event(url);
    handleFetch(ctx, cached.fetchEvent);
    expect(await (await cached.responded())?.text()).toBe('<feed/>');
    const missing = event(`${scope}updates.json`);
    handleFetch(ctx, missing.fetchEvent);
    expect((await missing.responded())?.type).toBe('error');
  });

  it('ignores requests outside the scope and routes HTML through the page strategy', async () => {
    const { ctx, storage } = harness();
    const foreign = event('https://elsewhere.invalid/archive/items/a/');
    handleFetch(ctx, foreign.fetchEvent);
    expect(foreign.responded()).toBeUndefined();
    const page = event(`${scope}items/a/`, { headers: new Headers({ accept: 'text/html' }) });
    handleFetch(ctx, page.fetchEvent);
    expect(await (await page.responded())?.text()).toBe(`${scope}items/a/`);
    expect(await (await storage.open(ctx.names.pages)).keys()).toHaveLength(1);
    expect(PAGE_LIMIT).toBe(200);
  });
});
