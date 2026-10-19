import { describe, expect, it } from 'vitest';
import { catalog, harness, indexBase, manifest, scope, tick, version } from './harness';
import { isOfflineStatus } from './messages';
import { configureOffline, readOfflineStatus, saveOfflineArticles } from './offline';
import { readSelection, resourceKey, storedResource } from './protected';
import { assetResponse, pageResponse } from './routing';
import { completeSearch, configureOfflineSearch, verifiedSearchResponse } from './search';
import { lifecycle } from './worker';

async function seedLegacy(env: ReturnType<typeof harness>, items = catalog): Promise<void> {
  const { ctx, storage } = env;
  await (await storage.open(ctx.names.offlineSettings)).put(`${scope}__offline_articles`, Response.json(items));
  const pages = await storage.open(ctx.names.legacyOfflinePages);
  const revisions = await storage.open(ctx.names.offlineRevisions);
  for (const item of items) for (const entry of item.resources) {
    const url = new URL(entry.url, scope).href;
    await pages.put(url, new Response(url));
    await revisions.put(url, new Response(entry.revision));
  }
}

describe('offline articles', () => {
  it('fetches the article catalogue only for a positive download count, and only once', async () => {
    const { ctx, requests } = harness();
    await saveOfflineArticles(ctx, 0);
    expect(requests.some((url) => url.endsWith('offline-catalog.json'))).toBe(false);
    await saveOfflineArticles(ctx, 1);
    expect(requests.filter((url) => url.endsWith('offline-catalog.json'))).toHaveLength(1);
    const before = requests.length;
    await saveOfflineArticles(ctx, 1);
    expect(requests.slice(before).some((url) => url.endsWith('offline-catalog.json'))).toBe(false);
  });

  it('require the page and every retained rendition, sharing resources across items', async () => {
    const { ctx, requests, storage, state } = harness();
    const result = await saveOfflineArticles(ctx, 2);
    expect(result.saved).toHaveLength(2);
    expect(result.failed).toBe(0);
    expect(requests.filter((url) => url.endsWith('shared.webp'))).toHaveLength(1);
    const protectedPages = await storage.open(ctx.names.offlinePages);
    expect(await storedResource(ctx, catalog[0].resources[2])).toBeTruthy();
    await configureOffline(ctx, 2);
    await protectedPages.delete(resourceKey(ctx, catalog[0].resources[2]));
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
    await (await storage.open(ctx.names.offlinePages)).delete(resourceKey(ctx, catalog[0].resources[2]));
    state.quota = (name, url) => name === ctx.names.offlinePages && new URL(url).pathname.endsWith('large.webp');
    const result = await saveOfflineArticles(ctx, 2);
    expect(result.failed).toBe(1);
    expect(result.saved.map((item) => item.title)).toEqual(['Older']);
    expect(await storedResource(ctx, catalog[1].resources[0])).toBeTruthy();
  });

  it('keeps the committed article and removes staged revisions when selection metadata hits quota', async () => {
    const { ctx, state, storage } = harness();
    await configureOffline(ctx, 1);
    const pages = await storage.open(ctx.names.offlinePages);
    const originalKeys = (await pages.keys()).map((key) => key.url).sort();
    ctx.state.offlineCatalog = [{
      ...catalog[0],
      title: 'Updated title',
      resources: [
        { url: 'items/new/', revision: 'updated' },
        { url: 'assets/images/replacement.webp', revision: 'replacement' },
      ],
    }];
    state.network = async (url) => new Response(url.endsWith('items/new/') ? 'updated article body' : url);
    state.quota = (name, url) => name === ctx.names.offlineSettings && url.endsWith('__offline_articles_v2');
    const result = await saveOfflineArticles(ctx, 1);
    expect(result.failed).toBe(1);
    expect(result.saved.map((item) => item.title)).toEqual(['Newest']);
    expect((await readOfflineStatus(ctx)).saved.map((item) => item.title)).toEqual(['Newest']);
    state.offline = true;
    expect(await (await pageResponse(ctx, new Request(`${scope}items/new/`))).text()).toBe(`${scope}items/new/`);
    expect((await pages.keys()).map((key) => key.url).sort()).toEqual(originalKeys);
  });

  it('hides staged pages and images until the complete replacement commits', async () => {
    const { ctx, state } = harness();
    await configureOffline(ctx, 1);
    ctx.state.offlineCatalog = [{ ...catalog[0], title: 'Updated', resources: [
      { url: 'items/new/', revision: 'updated' },
      { url: 'assets/images/replacement.webp', revision: 'replacement' },
    ] }];
    let release!: (response: Response) => void;
    const staged = new Promise<Response>((resolve) => { release = resolve; });
    state.network = async (url) => url.endsWith('items/new/') ? staged : new Response(url);
    const update = saveOfflineArticles(ctx, 1);
    await tick();
    state.offline = true;
    expect((await readOfflineStatus(ctx)).saved.map((item) => item.title)).toEqual(['Newest']);
    expect(await (await pageResponse(ctx, new Request(`${scope}items/new/?from=feed`))).text()).toBe(`${scope}items/new/`);
    await expect(assetResponse(ctx, new Request(`${scope}assets/images/replacement.webp`), ctx.names.assets, 256)).rejects.toThrow('offline');
    state.offline = false;
    release(new Response('updated article'));
    expect((await update).saved.map((item) => item.title)).toEqual(['Updated']);
    state.offline = true;
    expect(await (await pageResponse(ctx, new Request(`${scope}items/new/`))).text()).toBe('updated article');
  });

  it('serializes a pending direct save and clearing, leaving an authoritative empty selection', async () => {
    const env = harness();
    const { ctx, state, storage } = env;
    let release!: (response: Response) => void;
    const pendingPage = new Promise<Response>((resolve) => { release = resolve; });
    state.network = async (url) => url.endsWith('items/new/') ? pendingPage : new Response(url);
    const save = saveOfflineArticles(ctx, 1);
    await tick();
    const clear = saveOfflineArticles(ctx, 0);
    release(new Response('saved page'));
    await Promise.all([save, clear]);
    expect((await readSelection(ctx)).items).toEqual([]);
    expect(await (await storage.open(ctx.names.offlinePages)).keys()).toHaveLength(0);
    // A previous worker writing its old pointer cannot resurrect a cleared v2 selection.
    await seedLegacy(env);
    expect((await readSelection(ctx)).items).toEqual([]);
    state.offline = true;
    await expect(pageResponse(ctx, new Request(`${scope}items/new/`))).rejects.toThrow('offline');
  });

  it('reads legacy downloads and migrates unchanged bodies locally before pruning the old cache', async () => {
    const env = harness();
    const { ctx, state, requests, storage } = env;
    await seedLegacy(env);
    ctx.state.configurationCount = 2;
    ctx.state.offlineCatalog = catalog;
    state.offline = true;
    expect((await readOfflineStatus(ctx)).saved.map((item) => item.title)).toEqual(['Newest', 'Older']);
    expect(await (await pageResponse(ctx, new Request(`${scope}items/new/`))).text()).toBe(`${scope}items/new/`);
    requests.length = 0;
    expect((await saveOfflineArticles(ctx, 2)).saved).toHaveLength(2);
    expect(requests).toEqual([]);
    expect((await readSelection(ctx)).legacy).toBe(false);
    expect(await storage.keys()).not.toContain(ctx.names.legacyOfflinePages);
    expect(await storage.keys()).not.toContain(ctx.names.offlineRevisions);
    // Legacy pruning and metadata writes affect only the retired namespace.
    await seedLegacy(env, []);
    expect((await readOfflineStatus(ctx)).saved).toHaveLength(2);
  });

  it('preserves the legacy pointer and complete bodies when migrating hits quota', async () => {
    const env = harness();
    const { ctx, state, storage } = env;
    await seedLegacy(env);
    ctx.state.configurationCount = 2;
    ctx.state.offlineCatalog = catalog;
    state.quota = (name) => name === ctx.names.offlinePages;
    const result = await saveOfflineArticles(ctx, 2);
    expect(result.failed).toBeGreaterThan(0);
    expect(result.saved.map((item) => item.title)).toEqual(['Newest', 'Older']);
    expect((await readSelection(ctx)).legacy).toBe(true);
    expect(await storage.keys()).toContain(ctx.names.legacyOfflinePages);
    state.offline = true;
    expect(await (await pageResponse(ctx, new Request(`${scope}items/new/`))).text()).toBe(`${scope}items/new/`);
  });

  it('removes locally staged migration copies when the v2 selection cannot commit', async () => {
    const env = harness();
    const { ctx, state, storage } = env;
    await seedLegacy(env);
    ctx.state.offlineCatalog = catalog;
    state.quota = (name, url) => name === ctx.names.offlineSettings && url.endsWith('__offline_articles_v2');
    expect((await saveOfflineArticles(ctx, 2)).failed).toBe(2);
    expect((await readSelection(ctx)).legacy).toBe(true);
    expect(await (await storage.open(ctx.names.offlinePages)).keys()).toHaveLength(0);
    state.offline = true;
    expect(await (await pageResponse(ctx, new Request(`${scope}items/new/`))).text()).toBe(`${scope}items/new/`);
  });

  it('rejects mutable page bytes that disagree with the published revision, including legacy receipts', async () => {
    const env = harness();
    const { ctx, state, storage } = env;
    const body = 'the published article';
    const bytes = await crypto.subtle.digest('SHA-1', new TextEncoder().encode(body));
    const revision = Array.from(new Uint8Array(bytes), (byte) => byte.toString(16).padStart(2, '0')).join('');
    const item = { url: 'items/new/', title: 'Published', resources: [{ url: 'items/new/', revision }] };
    await seedLegacy(env, [item]);
    ctx.state.configurationCount = 1;
    ctx.state.offlineCatalog = [item];
    expect((await readOfflineStatus(ctx)).saved).toEqual([]);
    state.network = async () => new Response('changed during deploy');
    expect((await saveOfflineArticles(ctx, 1)).saved).toEqual([]);
    expect(await (await storage.open(ctx.names.offlinePages)).keys()).toHaveLength(0);
    state.network = async () => new Response(body);
    expect((await saveOfflineArticles(ctx, 1)).saved.map((item) => item.title)).toEqual(['Published']);
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
    expect((await readSelection(ctx)).items).toEqual([]);
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
    expect(await storedResource(ctx, catalog[1].resources[0], undefined, false)).toBeUndefined();
  });

  it('repeating the same configuration reuses the pending task and persists the count', async () => {
    const { ctx, storage } = harness();
    const first = configureOffline(ctx, 2);
    expect(configureOffline(ctx, 2)).toBe(first);
    await first;
    const settings = await storage.open(ctx.names.offlineSettings);
    expect(await (await settings.match(`${scope}__offline_count`))?.text()).toBe('2');
    const stored: unknown = await (await settings.match(`${scope}__offline_articles_v2`))?.json();
    expect(Array.isArray(stored) && stored.length).toBe(2);
  });
});
