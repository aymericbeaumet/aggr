import { describe, expect, it } from 'vitest';
import { harness, indexBase, manifest, scope, tick, version } from './harness';
import { isOfflineStatus } from './messages';
import { configureOffline, readOfflineStatus, saveOfflineArticles } from './offline';
import { assetResponse, pageResponse } from './routing';
import { completeSearch, configureOfflineSearch, verifiedSearchResponse } from './search';
import { lifecycle } from './worker';

describe('offline articles', () => {
  it('does not download the catalogue or search index until offline reading is enabled', async () => {
    const { ctx, requests } = harness();
    await configureOffline(ctx, 0);
    await readOfflineStatus(ctx);
    expect(requests).toEqual([]);
    await configureOffline(ctx, 1);
    expect(requests.filter((url) => url.includes('/offline/'))).toHaveLength(1);
    expect((await readOfflineStatus(ctx)).saved).toHaveLength(1);
  });

  it('rejects a mismatched catalogue without pruning previously saved articles', async () => {
    const { ctx, storage } = harness();
    await configureOffline(ctx, 1);
    await (await storage.open(ctx.names.shell)).delete(new URL(ctx.config.offline_catalog.url, scope).href);
    ctx.config.offline_catalog.digest = '0'.repeat(64);
    const result = await saveOfflineArticles(ctx, 1);
    expect(result.failed).toBe(1);
    expect(result.saved.map((item) => item.title)).toEqual(['Newest']);
    expect((await readOfflineStatus(ctx)).saved).toHaveLength(1);
  });

  it('require the page and every retained rendition, sharing resources across items', async () => {
    const { ctx, requests, storage, state } = harness();
    const result = await saveOfflineArticles(ctx, 2);
    expect(result.saved).toHaveLength(2);
    expect(result.failed).toBe(0);
    expect(requests.filter((url) => url.endsWith('shared.webp'))).toHaveLength(1);
    const protectedPages = await storage.open(ctx.names.offlinePages);
    expect(await protectedPages.match(`${scope}assets/images/large.webp`)).toBeTruthy();
    await configureOffline(ctx, 2);
    await protectedPages.delete(`${scope}assets/images/large.webp`);
    const partial = await readOfflineStatus(ctx);
    // One missing image makes its article incomplete.
    expect(partial.saved.map((item) => item.title)).toEqual(['Older']);
    state.offline = true;
    const page = await pageResponse(ctx, new Request(`${scope}items/older/`));
    expect(await page.text()).toBe(`${scope}items/older/`);
  });

  it('quota failures preserve previous complete downloads and never advertise an incomplete replacement', async () => {
    const { ctx, state, storage } = harness();
    await saveOfflineArticles(ctx, 2);
    await (await storage.open(ctx.names.offlinePages)).delete(`${scope}assets/images/large.webp`);
    state.quota = (name, url) => name === ctx.names.offlinePages && url.endsWith('large.webp');
    const result = await saveOfflineArticles(ctx, 2);
    expect(result.failed).toBe(1);
    expect(result.saved.map((item) => item.title)).toEqual(['Older']);
    expect(await (await storage.open(ctx.names.offlinePages)).match(`${scope}items/older/`)).toBeTruthy();
  });

  it('persists saved articles in catalogue order when downloads finish out of order', async () => {
    const { ctx, state } = harness();
    state.network = async (url) => {
      if (url.endsWith('large.webp')) await new Promise((resolve) => setTimeout(resolve, 20));
      return new Response(url);
    };
    await saveOfflineArticles(ctx, 2);
    ctx.state.configurationCount = 1;
    expect((await readOfflineStatus(ctx)).saved.map((item) => item.title)).toEqual(['Newest']);
  });

  it('offline search is committed only after the complete manifest passes size and digest verification', async () => {
    const { ctx, state, storage } = harness();
    state.network = async (url) => (url.endsWith('search-manifest.json') ? Response.json(manifest) : new Response('corrupt'));
    await configureOfflineSearch(ctx, true);
    expect(ctx.state.searchStatus.phase).toBe('error');
    expect(ctx.state.searchStatus.activeVersion).toBeNull();
    expect(await completeSearch(ctx, manifest)).toBe(false);
    state.network = undefined;
    await configureOfflineSearch(ctx, true);
    expect(ctx.state.searchStatus.phase).toBe('ready');
    expect(ctx.state.searchStatus.activeVersion).toBe(version);
    expect(await completeSearch(ctx, manifest)).toBe(true);
    const index = await storage.open(ctx.names.offlineSearchPrefix + version);
    expect(await index.match(`${scope}${indexBase}search-catalog.json`)).toBeTruthy();
    await configureOffline(ctx, 0);
    expect(ctx.state.searchStatus.phase).toBe('disabled');
    expect((await readOfflineStatus(ctx)).saved).toHaveLength(0);
  });

  it('disabling while downloads are in flight cannot resurrect saved articles or search readiness', async () => {
    const { ctx, state } = harness();
    state.network = (_url, init) =>
      new Promise((_, reject) => {
        init?.signal?.addEventListener('abort', () => reject(new DOMException('aborted', 'AbortError')), { once: true });
      });
    const pending = configureOffline(ctx, 2);
    await tick();
    await configureOffline(ctx, 0);
    await pending;
    const status = await readOfflineStatus(ctx);
    expect(status.requested).toBe(0);
    expect(status.saved).toHaveLength(0);
    expect(status.search?.phase).toBe('disabled');
  });

  it('required shell failure rejects installation, and quota never hides a valid network response', async () => {
    const { ctx, state } = harness();
    const handlers = lifecycle(ctx);
    state.offline = true;
    const installs: Promise<unknown>[] = [];
    handlers.install({ waitUntil: (promise) => installs.push(promise) });
    await expect(Promise.all(installs)).rejects.toThrow();
    state.offline = false;
    state.quota = () => true;
    expect((await pageResponse(ctx, new Request(`${scope}items/fresh/`))).status).toBe(200);
    expect((await assetResponse(ctx, new Request(`${scope}assets/example.js`), ctx.names.assets, 256)).status).toBe(200);
  });

  it('corrupted persisted metadata and bodyless index responses cannot establish readiness', async () => {
    const { ctx, storage } = harness();
    const settings = await storage.open(ctx.names.offlineSettings);
    await settings.put(`${scope}__offline_count`, new Response('2'));
    await settings.put(
      `${scope}__offline_articles`,
      Response.json([null, { url: 'items/new/', title: 'New', resources: [null] }]),
    );
    await settings.put(`${scope}__offline_search`, Response.json({ version, files: null }));
    const status = await readOfflineStatus(ctx);
    expect(status.saved).toHaveLength(0);
    expect(status.search?.activeVersion).toBeNull();
    await expect(verifiedSearchResponse(new Response(null), manifest.files[0], 1000)).rejects.toThrow(/network/);
  });

  it('broadcasts progress to every window and drops the protected copy from the visited pages', async () => {
    const { ctx, messages, storage } = harness();
    const pages = await storage.open(ctx.names.pages);
    await pages.put(`${scope}items/new/?from=feed`, new Response('visited earlier'));
    await configureOffline(ctx, 1);
    const statuses = messages.filter(isOfflineStatus);
    expect(statuses.length).toBeGreaterThan(0);
    const last = statuses[statuses.length - 1];
    expect(last.requested).toBe(1);
    expect(last.saved.map((item) => item.title)).toEqual(['Newest']);
    expect(last.downloading).toBe(false);
    expect(last.search?.phase).toBe('ready');
    expect(await pages.match(`${scope}items/new/?from=feed`)).toBeUndefined();
    // Resources outside the selection leave the protected caches.
    expect(await (await storage.open(ctx.names.offlinePages)).match(`${scope}items/older/`)).toBeUndefined();
  });

  it('repeating the same configuration reuses the pending task and persists the count', async () => {
    const { ctx, storage } = harness();
    const first = configureOffline(ctx, 2);
    expect(configureOffline(ctx, 2)).toBe(first);
    await first;
    const settings = await storage.open(ctx.names.offlineSettings);
    expect(await (await settings.match(`${scope}__offline_count`))?.text()).toBe('2');
    const stored: unknown = await (await settings.match(`${scope}__offline_articles`))?.json();
    expect(Array.isArray(stored) && stored.length).toBe(2);
  });
});
