/**
 * The service worker template, run against a fake Cache Storage and network. The scenario is sequential:
 * later tests build on the caches earlier ones populated, exactly as one worker's life would, so the file
 * shares a single harness and its tests run in order.
 */
import { readFileSync } from "node:fs";
import { beforeAll, describe, expect, it } from "vitest";

const scope = "http://localhost/reader/";
const SLOW = 20000;

interface OfflineStatus {
  requested: number; total: number; saved: Array<{ url: string; title: string }>; failed: number; downloading: boolean;
  search: SearchStatus; cancelled?: boolean;
}
interface SearchStatus {
  phase: string; activeVersion: string | null; targetVersion: string | null; base: string | null;
  downloadedFiles: number; totalFiles: number; downloadedBytes: number; totalBytes: number; error: string | null;
}
interface PrecacheEntry { url: string; revision: string; required?: boolean }
interface CatalogItem { url: string; title: string; resources: PrecacheEntry[] }
interface FetchEventLike { waitUntil(promise: Promise<unknown>): void }
interface SearchManifest { version: string; base: string; docs: number; totalBytes: number; files: Array<{ url: string; size: number; digest: string }>; facets: object }

interface FakeCache {
  put(request: Request | string, response: Response): Promise<void>;
  delete(request: Request | string): Promise<boolean>;
  keys(): Promise<Request[]>;
  match(request: Request | string, options?: { ignoreSearch?: boolean }): Promise<Response | undefined>;
}

interface WorkerApi {
  saveOfflineArticles(count: number): Promise<OfflineStatus>;
  configureOffline(count: number): Promise<OfflineStatus | undefined>;
  readOfflineStatus(): Promise<OfflineStatus>;
  broadcastOfflineStatus(status: object): Promise<void>;
  OFFLINE_CATALOG: CatalogItem[];
  OFFLINE_PAGES: string; OFFLINE_SETTINGS: string; OFFLINE_SEARCH_PREFIX: string;
  SEARCH_MANIFEST: { version: string; base: string };
  searchStatus(): SearchStatus;
  networkFirst(request: Request, preload: undefined, event: FetchEventLike): Promise<Response>;
  remember(name: string, maximum: number, request: Request, response: Response | object): Promise<Response>;
  fetchEntry(entry: { url: string }): Promise<Response>;
  populate(cache: FakeCache, revisions: FakeCache, revisionCaches: string[], entries: PrecacheEntry[], strict: boolean): Promise<void>;
  PRECACHE: string; PAGES: string; ASSETS: string; SEARCH: string; SEARCH_PREFIX: string; CACHE_NAMESPACE: string; PRECACHE_PREFIX: string; IMAGES: string;
  ENTRIES: PrecacheEntry[]; REQUIRED_URLS: PrecacheEntry[]; OPTIONAL_URLS: PrecacheEntry[];
  setPrecacheTimeout(ms: number): void;
}

type Network = (request: Request | string, options?: { priority?: string; signal: AbortSignal }) => Promise<Response>;
interface ClientLike { postMessage(status: OfflineStatus): void }

/** The worker's global: what `self` offers a service worker, as far as the template uses it. */
interface WorkerGlobal {
  location: URL;
  registration: { scope: string };
  clients: { claim(): Promise<void>; matchAll?(query?: unknown): Promise<ClientLike[]> };
  skipWaiting(): Promise<void>;
  addEventListener(name: string, handler: (event: any) => void): void;
}

function key(request: Request | string) {
  return new URL(typeof request === "string" ? request : request.url, scope).href;
}

function harness() {
  const handlers: Record<string, (event: any) => void> = {};
  const buckets = new Map<string, Map<string, Response>>();
  const state = {
    rejectWrites: false as boolean | ((name: string, url: string) => boolean),
    skipWaitingCalls: 0,
    writes: 0,
    enumerations: 0,
    pauseRead: null as null | ((name: string, url: string) => Promise<void>),
    network: (async (_request, options) => {
      expect(!options || options.priority !== "low", "foreground navigation must retain normal fetch priority").toBe(true);
      return new Response("fresh page");
    }) as Network
  };
  const storage = {
    keys: async () => Array.from(buckets.keys()),
    delete: async (name: string) => buckets.delete(name),
    open: async (name: string): Promise<FakeCache> => {
      if (!buckets.has(name)) buckets.set(name, new Map());
      const entries = buckets.get(name)!;
      return {
        put: async (request, response) => {
          state.writes += 1;
          if (state.rejectWrites === true || (typeof state.rejectWrites === "function" && state.rejectWrites(name, key(request)))) throw new DOMException("quota exceeded", "QuotaExceededError");
          entries.set(key(request), response.clone());
        },
        delete: async request => entries.delete(key(request)),
        keys: async () => {
          state.enumerations += 1;
          return Array.from(entries.keys()).map(url => new Request(url));
        },
        match: async (request, options) => {
          const wanted = new URL(key(request));
          for (const [url, response] of entries) {
            const actual = new URL(url);
            if (url === wanted.href || (options && options.ignoreSearch && actual.pathname === wanted.pathname)) {
              const copy = response.clone();
              if (state.pauseRead) await state.pauseRead(name, wanted.href);
              return copy;
            }
          }
          return undefined;
        }
      };
    }
  };
  const self: WorkerGlobal = {
    location: new URL(scope + "sw.js"),
    registration: { scope },
    clients: { claim: async () => undefined },
    skipWaiting: async () => { state.skipWaitingCalls += 1; },
    addEventListener: (name, handler) => { handlers[name] = handler; }
  };
  const template = readFileSync(new URL("../../themes/default/templates/sw.js", import.meta.url), "utf8");
  const catalog: CatalogItem[] = [
    { url: "items/new/", title: "Newest", resources: [{ url: "items/new/", revision: "new" }, { url: "assets/images/shared.webp", revision: "image" }] },
    { url: "items/older/", title: "Older", resources: [{ url: "items/older/", revision: "older" }, { url: "assets/images/shared.webp", revision: "image" }] }
  ];
  const source = template
    .replace("{{ version | json }}", () => JSON.stringify("test"))
    .replace("{{ app_version | json }}", () => JSON.stringify("app-test"))
    .replace("{{ content_version | json }}", () => JSON.stringify("content-test"))
    .replace("{{ precache | json }}", () => JSON.stringify([{ url: "", revision: "new", required: true }]))
    .replace("{{ offline_count | json }}", () => "0")
    .replace("{{ offline_catalog | json }}", () => JSON.stringify(catalog))
    .replace("{{ search_manifest | json }}", () => JSON.stringify({ version: "a".repeat(64), base: "pagefind/" + "a".repeat(64) + "/" }));
  expect(source, "every template placeholder must be stubbed").not.toMatch(/\{\{|\{%/);
  const api = new Function("self", "caches", "fetch", source +
    "\nreturn {saveOfflineArticles, configureOffline, readOfflineStatus, broadcastOfflineStatus, OFFLINE_CATALOG, OFFLINE_PAGES, OFFLINE_SETTINGS, OFFLINE_SEARCH_PREFIX, SEARCH_MANIFEST, searchStatus: () => Object.assign({}, searchStatus), networkFirst, remember, fetchEntry, populate, PRECACHE, PAGES, ASSETS, SEARCH, SEARCH_PREFIX, CACHE_NAMESPACE, PRECACHE_PREFIX, IMAGES, ENTRIES, REQUIRED_URLS, OPTIONAL_URLS, setPrecacheTimeout: ms => PRECACHE_TIMEOUT = ms};"
  )(self, storage, (request: Request | string, options?: { priority?: string; signal: AbortSignal }) => state.network(request, options)) as WorkerApi;

  async function dispatch(request: Request) {
    const lifetime: Promise<unknown>[] = [];
    let response: Promise<Response> | undefined;
    handlers.fetch({
      request,
      preloadResponse: undefined,
      respondWith: (value: Promise<Response>) => { response = value; },
      waitUntil: (value: Promise<unknown>) => { lifetime.push(value); }
    });
    expect(response, "the worker must handle article representations").toBeDefined();
    const resolved = await response!;
    await Promise.all(lifetime);
    return resolved;
  }

  return { handlers, buckets, state, storage, self, api, dispatch };
}

const text = async (response: Response | undefined) => (await response!).text();
const sleep = (ms: number) => new Promise<void>(resolve => setTimeout(resolve, ms));

describe("service worker", () => {
  let w: ReturnType<typeof harness>;
  const request = new Request(scope);
  const pending: Promise<unknown>[] = [];
  const event: FetchEventLike = { waitUntil: promise => { pending.push(promise); } };
  let manifest: SearchManifest;
  let previousVersion: string;
  let completeVersion: string;
  const indexContent: Record<string, string> = { "pagefind.js": "loader", "index.pf_index": "all searchable words", "filter.pf_filter": "all filter values", "fragment.pf_fragment": "all result text" };

  beforeAll(() => { w = harness(); });

  it("serves the network first and keeps the latest runtime page for offline", async () => {
    const precache = await w.storage.open(w.api.PRECACHE);
    await precache.put(request, new Response("old install"));
    expect(await text(await w.api.networkFirst(request, undefined, event))).toBe("fresh page");
    await Promise.all(pending);
    w.state.network = async () => { throw new Error("offline"); };
    expect(await text(await w.api.networkFirst(request, undefined, event)), "offline must prefer the latest runtime page").toBe("fresh page");
    expect(pending.length, "background cache writes must extend the fetch event lifetime").toBeGreaterThan(0);
  });

  it("activation replaces older runtime entries, discards stale generations and keeps other applications' caches", async () => {
    const { api, storage, buckets, handlers } = w;
    api.ENTRIES.splice(0, api.ENTRIES.length, { url: new URL(scope).pathname, revision: "new", required: true });
    api.ENTRIES.push({ url: new URL(scope + "browse/").pathname, revision: "optional", required: false });
    const precache = await storage.open(api.PRECACHE);
    await precache.put(request, new Response("new deployment"));
    await (await storage.open(api.PAGES)).put(new Request(scope + "?q=old"), new Response("old runtime"));
    await (await storage.open(api.PAGES)).put(new Request(scope + "browse/"), new Response("saved directory"));
    const oldSearch = api.SEARCH_PREFIX + "previous";
    await (await storage.open(oldSearch)).put(new Request(scope + "pagefind/fragment.pf_index"), new Response("stale search"));
    const obsolete = api.CACHE_NAMESPACE + "obsolete-layout";
    const foreign = "another-app:pages";
    const savedSearch = api.OFFLINE_SEARCH_PREFIX + "retained";
    await (await storage.open(obsolete)).put(request, new Response("obsolete"));
    await (await storage.open(foreign)).put(request, new Response("unrelated application"));
    await (await storage.open(savedSearch)).put(request, new Response("saved index"));
    const oldPrecache = api.PRECACHE_PREFIX + "previous";
    const archivedScript = new Request(scope + "assets/app-012345abcdef.js");
    await (await storage.open(oldPrecache)).put(archivedScript, new Response("archived script"));
    let activated: Promise<unknown> | undefined;
    handlers.activate({ waitUntil: (promise: Promise<unknown>) => { activated = promise; } });
    await activated;
    expect(await text(await api.networkFirst(request, undefined, event)), "a new deployment must replace older runtime entries including query variants").toBe("new deployment");
    expect(await text(await api.networkFirst(new Request(scope + "browse/"), undefined, event)), "a failed optional precache must preserve the previous readable runtime copy").toBe("saved directory");
    expect(buckets.has(oldSearch), "activation must discard stale runtime search-index generations").toBe(false);
    expect(buckets.has(obsolete), "activation removes unknown own caches").toBe(false);
    expect(buckets.has(foreign), "activation must not touch other applications").toBe(true);
    expect(buckets.has(savedSearch) && buckets.has(api.PAGES), "activation preserves retained offline search and readable pages").toBe(true);
    expect(buckets.has(oldPrecache), "old precaches are removed").toBe(false);
    expect(await text(await (await storage.open(api.ASSETS)).match(archivedScript)), "split-cache upgrades preserve immutable assets before removing old precaches").toBe("archived script");
  });

  it("renders cached HTML without waiting for the network, refreshes it in the background, and honours explicit reloads", async () => {
    const { api, storage, handlers, dispatch } = w;
    const html = new Request(scope, { headers: { accept: "text/html" } });
    let refresh!: (response: Response) => void;
    w.state.network = () => new Promise(resolve => { refresh = resolve; });
    const refreshLifetime: Promise<unknown>[] = [];
    let htmlResponse: Promise<Response> | undefined;
    handlers.fetch({
      request: html,
      respondWith: (value: Promise<Response>) => { htmlResponse = value; },
      waitUntil: (value: Promise<unknown>) => { refreshLifetime.push(value); }
    });
    const immediate = await Promise.race([htmlResponse, sleep(100).then(() => null)]);
    expect(immediate, "cached HTML must render without waiting for a slow network").not.toBeNull();
    expect(await immediate!.text()).toBe("new deployment");
    refresh(new Response("background update"));
    await Promise.all(refreshLifetime);
    expect(await text(await (await storage.open(api.PAGES)).match(html)), "cached navigation must refresh in the background").toBe("background update");
    for (const cache of ["reload", "no-cache"] as const) {
      w.state.network = async () => new Response("explicit reload " + cache);
      const reload = new Request(scope, { headers: { accept: "text/html" }, cache });
      expect(await text(await dispatch(reload)), "an explicit app update or pull refresh must fetch current HTML instead of a saved article").toBe("explicit reload " + cache);
    }
  });

  it("bypasses stale article representations online and keeps the fresh copy offline", async () => {
    const { api, storage, dispatch } = w;
    for (const extension of ["md", "txt", "rst", "json"]) {
      const representation = new Request(scope + "items/example/story." + extension);
      await (await storage.open(api.PAGES)).put(representation, new Response("stale " + extension));
      w.state.network = async () => new Response("fresh " + extension);
      expect(await text(await dispatch(representation)), "online " + extension + " must bypass a stale runtime copy").toBe("fresh " + extension);
      w.state.network = async () => { throw new Error("offline"); };
      expect(await text(await dispatch(representation)), "updated " + extension + " must remain available offline").toBe("fresh " + extension);
    }
  });

  it("keeps article images in the bounded image cache and serves hits without rewriting or enumerating", async () => {
    const { api, storage, dispatch, state } = w;
    const localImage = new Request(scope + "assets/images/0123456789abcdef.webp");
    state.network = async () => new Response("local article image", { headers: { "content-type": "image/webp" } });
    expect(await text(await dispatch(localImage)), "article images must remain readable while entering the runtime cache").toBe("local article image");
    const imageKeys = await (await storage.open(api.IMAGES)).keys();
    const assetKeys = await (await storage.open(api.ASSETS)).keys();
    expect(imageKeys.some(request => request.url === localImage.url), "article images use the bounded image cache").toBe(true);
    expect(assetKeys.some(request => request.url === localImage.url), "article images must not enter the app-asset cache").toBe(false);
    const writesBeforeHit = state.writes;
    const enumerationsBeforeHit = state.enumerations;
    state.network = async () => { throw new Error("cache hit must not fetch"); };
    expect(await text(await dispatch(localImage)), "cached images must remain available").toBe("local article image");
    expect([state.writes, state.enumerations], "asset cache hits must not rewrite responses or enumerate the entire cache").toEqual([writesBeforeHit, enumerationsBeforeHit]);
  });

  it("never persists opaque remote images and keeps the old readable copy when a write hits quota", async () => {
    const { api, storage, state } = w;
    const opaque = { ok: false, type: "opaque", clone: () => new Response("unverifiable remote response") };
    const imageCountBeforeOpaque = (await (await storage.open(api.IMAGES)).keys()).length;
    await api.remember(api.IMAGES, 128, new Request("https://publisher.example/image.jpg"), opaque);
    expect((await (await storage.open(api.IMAGES)).keys()).length, "opaque remote images must not be persisted").toBe(imageCountBeforeOpaque);
    const pages = await storage.open(api.PAGES);
    await pages.put(request, new Response("saved before quota"));
    state.rejectWrites = true;
    const latest = await api.remember(api.PAGES, 500, request, new Response("fresh despite quota"));
    expect(await latest.text(), "quota must not block the online response").toBe("fresh despite quota");
    expect(await text(await pages.match(request)), "a failed cache write must preserve the old readable copy").toBe("saved before quota");
    state.rejectWrites = false;
  });

  it("rejects an installation missing a required resource and tolerates optional failures", async () => {
    const { api, storage, buckets, handlers, state } = w;
    const previousName = api.PRECACHE + "-previous";
    const previous = await storage.open(previousName);
    await previous.put(request, new Response("previous working installation"));
    api.REQUIRED_URLS.splice(0, api.REQUIRED_URLS.length, { url: scope, revision: "required", required: true });
    api.OPTIONAL_URLS.splice(0, api.OPTIONAL_URLS.length, { url: scope + "optional", revision: "optional", required: false });
    state.network = async () => new Response("unavailable", { status: 503 });
    let installation: Promise<unknown> | undefined;
    handlers.install({ waitUntil: (promise: Promise<unknown>) => { installation = promise; } });
    let installFailed = false;
    try { await installation; } catch (_) { installFailed = true; }
    expect(installFailed && state.skipWaitingCalls === 0 && !buckets.has(api.PRECACHE), "a failed required resource must reject and discard the new installation").toBe(true);
    expect(await text(await previous.match(request)), "failed installation must leave the working generation intact").toBe("previous working installation");
    state.network = async request => new Response("resource", { status: key(request) === scope ? 200 : 404 });
    handlers.install({ waitUntil: (promise: Promise<unknown>) => { installation = promise; } });
    await installation;
    expect(state.skipWaitingCalls).toBe(1);
    expect(await (await storage.open(api.PRECACHE)).match(request), "optional failures must not prevent installing the readable app shell").toBeDefined();
  });

  it("downloads the precache in parallel slots with a deadline and low priority, without delaying activation", { timeout: SLOW }, async () => {
    const { api, storage, handlers, state } = w;
    let releaseSlowDownload!: (response: Response) => void;
    const downloads: string[] = [];
    state.network = async request => {
      downloads.push(key(request));
      return key(request).endsWith("/0")
        ? new Promise<Response>(resolve => { releaseSlowDownload = resolve; })
        : new Response("downloaded");
    };
    const entries = Array.from({ length: 25 }, (_, index) => ({ url: scope + index, revision: String(index) }));
    const population = api.populate(await storage.open("pool-pages"), await storage.open("pool-revisions"), [], entries, true);
    const downloadDeadline = Date.now() + 1000;
    while (!downloads.includes(scope + "24") && Date.now() < downloadDeadline) await sleep(10);
    expect(downloads.includes(scope + "24"), "one slow download must not stall the rest of the precache queue").toBe(true);
    releaseSlowDownload(new Response("slow download"));
    await population;
    let precachePriority: string | undefined;
    state.network = (_request, options) => new Promise((_resolve, reject) => {
      precachePriority = options!.priority;
      options!.signal.addEventListener("abort", () => reject(new Error("aborted")), { once: true });
    });
    api.setPrecacheTimeout(20);
    let timedOut = false;
    try { await api.fetchEntry({ url: scope + "never-finishes" }); } catch (_) { timedOut = true; }
    expect(timedOut, "stalled precache downloads must have a deadline").toBe(true);
    expect(precachePriority, "offline precache downloads must yield network priority to foreground navigation").toBe("low");
    state.network = () => new Promise(() => {});
    let shortActivation!: Promise<unknown>;
    handlers.activate({ waitUntil: (value: Promise<unknown>) => { shortActivation = value; } });
    expect(await Promise.race([shortActivation.then(() => true), sleep(100).then(() => false)]), "activation must not wait for bulk offline downloads").toBe(true);
    api.setPrecacheTimeout(10000);
  });

  it("saves complete articles for offline reading, shares their assets, honours N and reports quota failures", { timeout: SLOW }, async () => {
    const { api, storage, dispatch, state } = w;
    const offlineRequests: string[] = [];
    state.network = async request => { offlineRequests.push(key(request)); return new Response("saved " + key(request)); };
    await (await storage.open(api.PAGES)).put(scope + "items/new/?reading=1", new Response("stale visited article"));
    const status = await api.saveOfflineArticles(2);
    expect([status.saved.length, status.failed], "offline count includes only fully saved articles").toEqual([2, 0]);
    expect(await (await storage.open(api.PAGES)).match(scope + "items/new/?reading=1"), "new protected pages invalidate stale visited copies including query variants").toBeUndefined();
    expect(offlineRequests.filter(url => url.endsWith("shared.webp")).length, "shared offline assets download once").toBe(1);
    const beforeRepeat = offlineRequests.length;
    await api.saveOfflineArticles(2);
    expect(offlineRequests.length, "unchanged offline resources reuse verified revisions").toBe(beforeRepeat);
    state.network = async () => { throw new Error("offline"); };
    const protectedImage = new Request(scope + "assets/images/shared.webp");
    expect((await text(await dispatch(protectedImage))).startsWith("saved"), "selected article assets remain available outside bounded runtime caches").toBe(true);
    const smaller = await api.saveOfflineArticles(1);
    const protectedCache = await storage.open(api.OFFLINE_PAGES);
    expect(smaller.saved.length).toBe(1);
    expect(await protectedCache.match(scope + "items/older/"), "reducing N removes unselected articles").toBeUndefined();
    await protectedCache.delete(scope + "assets/images/shared.webp");
    const incomplete = await api.saveOfflineArticles(1);
    expect([incomplete.saved.length, incomplete.failed], "an article missing an asset must not be reported ready offline").toEqual([0, 1]);
    state.rejectWrites = true;
    const quota = await api.saveOfflineArticles(2);
    expect(quota.failed, "quota errors report incomplete downloads without rejecting the app").toBe(2);
    state.rejectWrites = false;
    const disabled = await api.saveOfflineArticles(0);
    expect(disabled.saved.length === 0 && (await protectedCache.keys()).length === 0, "zero disables automatic article storage").toBe(true);
    state.network = async request => new Response("fallback " + key(request));
    await api.saveOfflineArticles(1);
    const prior = api.OFFLINE_CATALOG[0];
    api.OFFLINE_CATALOG.unshift({ url: "items/future/", title: "Future", resources: [{ url: "items/future/", revision: "future" }] });
    state.network = async () => { throw new Error("temporary outage"); };
    const fallback = await api.saveOfflineArticles(1);
    expect([fallback.failed, fallback.saved.length, fallback.saved[0].url], "failed newer downloads retain an older complete article within N").toEqual([1, 1, prior.url]);
    await api.saveOfflineArticles(0);
    let aborted = false;
    let downloadsStarted = 0;
    state.network = (_request, options) => new Promise((_resolve, reject) => {
      downloadsStarted += 1;
      options!.signal.addEventListener("abort", () => { aborted = true; reject(new Error("cancelled")); }, { once: true });
    });
    const large = api.configureOffline(3);
    const repeated = api.configureOffline(3);
    expect(large, "identical concurrent offline requests share one job").toBe(repeated);
    const startDeadline = Date.now() + 1000;
    while (!downloadsStarted && Date.now() < startDeadline) await sleep(5);
    const stopped = api.configureOffline(0);
    await Promise.all([large, stopped]);
    expect(aborted && (await (await storage.open(api.OFFLINE_PAGES)).keys()).length === 0, "turning off downloads cancels active requests and clears the selection promptly").toBe(true);
  });

  async function searchManifest(version: string, content: Record<string, string>): Promise<SearchManifest> {
    const base = "pagefind/" + version + "/";
    const files = await Promise.all(Object.entries(content).map(async ([name, body]) => {
      const bytes = new TextEncoder().encode(body);
      const digest = Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes))).map(byte => byte.toString(16).padStart(2, "0")).join("");
      return { url: base + name, size: bytes.byteLength, digest };
    }));
    return { version, base, docs: 10000, totalBytes: files.reduce((total, file) => total + file.size, 0), files, facets: {} };
  }

  it("downloads the offline search index with verification, resumes partial downloads and commits atomically", { timeout: SLOW }, async () => {
    const { api, storage, buckets, handlers, dispatch, state } = w;
    manifest = await searchManifest("a".repeat(64), indexContent);
    const indexRequests: string[] = [];
    let missing: string | null = "fragment.pf_fragment", corrupt = false, activeDownloads = 0, peakDownloads = 0;
    const healthyNetwork: Network = async request => {
      const url = key(request);
      if (!url.includes("/pagefind/")) return new Response("saved article");
      indexRequests.push(url);
      if (url.endsWith("search-manifest.json")) return new Response(JSON.stringify(manifest), { headers: { "content-type": "application/json" } });
      activeDownloads++; peakDownloads = Math.max(peakDownloads, activeDownloads);
      await sleep(2); activeDownloads--;
      const name = url.split("/").pop()!;
      if (name === missing) return new Response("unavailable", { status: 503 });
      return new Response(corrupt && name === "index.pf_index" ? "corrupt" : indexContent[name]);
    };
    state.network = healthyNetwork;
    await api.configureOffline(1);
    expect([api.searchStatus().phase, api.searchStatus().activeVersion], "partial index download must never report ready").toEqual(["error", null]);
    expect(peakDownloads, "complete index downloads use two independent bounded slots").toBe(2);
    const beforeStatus = indexRequests.length;
    let statusLifetime!: Promise<unknown>, reported!: OfflineStatus;
    handlers.message({ data: { type: "AGGR_OFFLINE_GET_STATUS" }, source: { url: scope, postMessage: (value: OfflineStatus) => { reported = value; } }, waitUntil: (value: Promise<unknown>) => { statusLifetime = value; } });
    await statusLifetime;
    expect(indexRequests.length === beforeStatus && reported.saved.length === 1 && reported.search.phase === "error", "read-only status reports separate article and index readiness without fetching").toBe(true);
    const completedDownloads = indexRequests.filter(url => !url.endsWith(missing!) && !url.endsWith("search-manifest.json"));
    missing = null;
    await api.configureOffline(1);
    expect(api.searchStatus().phase === "ready" && api.searchStatus().downloadedFiles === manifest.files.length && api.searchStatus().downloadedBytes === manifest.totalBytes, "the entire verified index must finish before readiness is committed").toBe(true);
    expect(completedDownloads.every(url => indexRequests.filter(request => request === url).length === 1), "retry resumes already verified staged files without repeated downloads").toBe(true);
    const readyRequests = indexRequests.length;
    await api.configureOffline(2);
    expect(indexRequests.length, "changing positive N must not download the same search index again").toBe(readyRequests);
    expect((await (await storage.open(api.OFFLINE_PAGES)).keys()).filter(request => request.url.includes("/items/")).length, "full archive search must not save more article pages than N").toBe(2);
    previousVersion = manifest.version;
    manifest = await searchManifest("b".repeat(64), indexContent);
    Object.assign(api.SEARCH_MANIFEST, { version: manifest.version, base: manifest.base });
    state.rejectWrites = name => name === api.OFFLINE_SEARCH_PREFIX + manifest.version;
    await api.configureOffline(2);
    expect([api.searchStatus().phase, api.searchStatus().error, api.searchStatus().activeVersion], "quota failure must preserve the previous complete index").toEqual(["blocked", "quota", previousVersion]);
    expect(buckets.has(api.OFFLINE_SEARCH_PREFIX + manifest.version), "quota failure releases uncommitted staging data").toBe(false);
    state.rejectWrites = false;
    const beforeReplacement = indexRequests.length;
    await api.configureOffline(2);
    expect([api.searchStatus().activeVersion, api.searchStatus().phase], "a verified replacement becomes active atomically").toEqual([manifest.version, "ready"]);
    expect(indexRequests.slice(beforeReplacement).every(url => url.endsWith("search-manifest.json")), "unchanged resources reuse the verified previous index across version changes").toBe(true);
    state.network = async () => { throw new Error("offline"); };
    const offlineManifest = await (await dispatch(new Request(scope + "search-manifest.json"))).json();
    expect(offlineManifest.version, "offline manifest selects the last fully committed index").toBe(manifest.version);
    const offlineCatalog = await (await dispatch(new Request(scope + "search-catalog.json"))).json();
    expect(offlineCatalog.version === manifest.version && offlineCatalog.base === manifest.base && offlineCatalog.docs === manifest.docs && !offlineCatalog.files && !offlineCatalog.totalBytes, "offline vocabulary is projected from the same committed index without its large verification inventory").toBe(true);
    const newerCatalog = { version: "9".repeat(64), base: "pagefind/" + "9".repeat(64) + "/", docs: 10001, facets: {} };
    state.network = async () => new Response(JSON.stringify(newerCatalog), { headers: { "content-type": "application/json" } });
    const onlineCatalog = await (await dispatch(new Request(scope + "search-catalog.json"))).json();
    expect(onlineCatalog.version === newerCatalog.version && api.searchStatus().activeVersion === manifest.version, "online catalogue uses the latest deployment without promoting an unverified offline index").toBe(true);
    state.network = async () => { throw new Error("offline"); };
    const fallbackCatalog = await (await dispatch(new Request(scope + "search-catalog.json"))).json();
    expect(fallbackCatalog.version, "offline catalogue prefers the committed version over a newer browsing-cache catalogue").toBe(manifest.version);
    expect(await text(await dispatch(new Request(scope + "pagefind/" + previousVersion + "/pagefind.js"))), "a preceding complete index remains readable by already open tabs").toBe("loader");
    completeVersion = manifest.version;
    indexContent["index.pf_index"] = "changed searchable words";
    manifest = await searchManifest("c".repeat(64), indexContent);
    Object.assign(api.SEARCH_MANIFEST, { version: manifest.version, base: manifest.base });
    corrupt = true; state.network = healthyNetwork;
    await api.configureOffline(2);
    expect([api.searchStatus().phase, api.searchStatus().error, api.searchStatus().activeVersion], "a corrupt chunk must not replace the verified complete index").toEqual(["error", "integrity", completeVersion]);
    corrupt = false;
    await api.configureOffline(2);
    expect([api.searchStatus().phase, api.searchStatus().activeVersion], "corrupt resources can be retried without losing the previous index").toEqual(["ready", manifest.version]);
    await (await storage.open(api.OFFLINE_SEARCH_PREFIX + manifest.version)).delete(scope + manifest.base + "filter.pf_filter");
    const evicted = await api.readOfflineStatus();
    expect([evicted.search.activeVersion, evicted.search.error], "evicted index resources invalidate readiness").toEqual([null, "evicted"]);
    await api.configureOffline(0);
    expect(api.searchStatus().phase === "disabled" && !Array.from(buckets.keys()).some(name => name.startsWith(api.OFFLINE_SEARCH_PREFIX)), "zero clears protected full indexes without retaining staged generations").toBe(true);
  });

  it("never lets a delayed read or broadcast resurrect a disabled offline state", { timeout: SLOW }, async () => {
    const { api, self, state } = w;
    await api.configureOffline(2);
    let releaseRead!: () => void, readStarted!: () => void;
    const startedRead = new Promise<void>(resolve => { readStarted = resolve; });
    state.pauseRead = async (_name, url) => {
      if (!url.endsWith("/__offline_count")) return;
      state.pauseRead = null; readStarted();
      await new Promise<void>(resolve => { releaseRead = resolve; });
    };
    const staleRead = api.readOfflineStatus();
    await startedRead;
    await api.configureOffline(0);
    releaseRead();
    const afterDisable = await staleRead;
    expect(afterDisable.requested === 0 && afterDisable.search.phase === "disabled" && afterDisable.search.activeVersion === null && api.searchStatus().phase === "disabled", "a delayed read must not restore a previous offline preference or overwrite disabled search state").toBe(true);
    await api.configureOffline(2);
    const startedManifest = new Promise<void>(resolve => { readStarted = resolve; });
    state.pauseRead = async (_name, url) => {
      if (!url.endsWith("/__offline_search")) return;
      state.pauseRead = null; readStarted();
      await new Promise<void>(resolve => { releaseRead = resolve; });
    };
    const staleManifest = api.readOfflineStatus();
    await startedManifest;
    await api.configureOffline(0);
    releaseRead();
    const afterManifest = await staleManifest;
    expect(afterManifest.search.phase === "disabled" && afterManifest.search.activeVersion === null && api.searchStatus().phase === "disabled" && api.searchStatus().error === null, "a delayed committed-index read cannot restore readiness or report eviction after saving is disabled").toBe(true);
    await api.configureOffline(2);
    const delivered: OfflineStatus[] = [], clients: ClientLike[] = [{ postMessage: status => delivered.push(status) }];
    let releaseClients!: (clients: ClientLike[]) => void;
    self.clients.matchAll = () => new Promise(resolve => { releaseClients = resolve; });
    const oldBroadcast = api.broadcastOfflineStatus({ type: "AGGR_OFFLINE_STATUS", requested: 2, saved: [] });
    self.clients.matchAll = async () => clients;
    await api.configureOffline(0);
    releaseClients(clients);
    await oldBroadcast;
    expect(delivered.length > 0 && delivered.at(-1)!.requested === 0 && delivered.at(-1)!.search.phase === "disabled" && delivered.at(-1)!.search.activeVersion === null, "a delayed ready broadcast cannot arrive after and supersede the disabled status").toBe(true);
  });
});
