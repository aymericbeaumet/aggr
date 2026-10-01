// A worker environment over in-memory caches and a scripted network, for the sw tests.

import { createContext, type WorkerContext } from './context';
import type { CacheLike, CacheStorageLike, Fetch, OfflineItem, SwConfig, Timeouts, WorkerEnv } from './types';

export const scope = 'https://reader.invalid/archive/';
export const version = 'a'.repeat(64);
export const indexBase = `pagefind/${version}/`;

export const hex = (buffer: ArrayBuffer): string =>
  Array.from(new Uint8Array(buffer), (byte) => byte.toString(16).padStart(2, '0')).join('');

export const digest = async (body: string): Promise<string> =>
  hex(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(body)));

export const files = [
  { url: `${indexBase}pagefind.js`, body: 'export const index = 1;' },
  { url: `${indexBase}search-catalog.json`, body: JSON.stringify({ version, base: indexBase, docs: 2, facets: {} }) },
  { url: `${indexBase}fragment.pf_fragment`, body: 'complete searchable article' },
];

const size = (body: string) => new TextEncoder().encode(body).byteLength;

export const manifest = {
  version,
  base: indexBase,
  files: await Promise.all(files.map(async (file) => ({ url: file.url, size: size(file.body), digest: await digest(file.body) }))),
  totalBytes: files.reduce((total, file) => total + size(file.body), 0),
};

export const catalog: OfflineItem[] = [
  {
    url: 'items/new/',
    title: 'Newest',
    resources: [
      { url: 'items/new/', revision: 'new' },
      { url: 'assets/images/shared.webp', revision: 'image' },
      { url: 'assets/images/large.webp', revision: 'large' },
    ],
  },
  {
    url: 'items/older/',
    title: 'Older',
    resources: [
      { url: 'items/older/', revision: 'older' },
      { url: 'assets/images/shared.webp', revision: 'image' },
    ],
  },
];

export interface HarnessState {
  offline: boolean;
  quota: (name: string, url: string) => boolean;
  network?: (url: string, init?: RequestInit) => Promise<Response>;
}

export interface Harness {
  ctx: WorkerContext;
  env: WorkerEnv;
  state: HarnessState;
  storage: CacheStorageLike;
  buckets: Map<string, Map<string, Response>>;
  messages: unknown[];
  requests: string[];
  claims: () => number;
  preloadEnabled: () => boolean;
}

export const key = (request: RequestInfo | URL): string =>
  new URL(typeof request === 'string' ? request : request instanceof URL ? request.href : request.url, scope).href;

export function harness(over: { config?: Partial<SwConfig>; timeouts?: Partial<Timeouts> } = {}): Harness {
  const buckets = new Map<string, Map<string, Response>>();
  const messages: unknown[] = [];
  const requests: string[] = [];
  const state: HarnessState = { offline: false, quota: () => false };
  const storage: CacheStorageLike = {
    keys: async () => [...buckets.keys()],
    delete: async (name) => buckets.delete(name),
    open: async (name) => {
      let entries = buckets.get(name);
      if (!entries) {
        entries = new Map();
        buckets.set(name, entries);
      }
      const stored = entries;
      const cache: CacheLike = {
        put: async (request, response) => {
          const url = key(request);
          if (state.quota(name, url)) throw new DOMException('quota exceeded', 'QuotaExceededError');
          stored.set(url, response.clone());
        },
        delete: async (request) => stored.delete(key(request)),
        keys: async () => [...stored.keys()].map((url) => new Request(url)),
        match: async (request, options) => {
          const wanted = new URL(key(request));
          for (const [url, response] of stored) {
            if (url === wanted.href || (options?.ignoreSearch && new URL(url).pathname === wanted.pathname))
              return response.clone();
          }
          return undefined;
        },
      };
      return cache;
    },
    match: async (request, options) => {
      for (const name of buckets.keys()) {
        const found = await (await storage.open(name)).match(request, options);
        if (found) return found;
      }
      return undefined;
    },
  };
  let claimed = 0;
  let preload = false;
  const fetch: Fetch = async (input, init) => {
    const url = key(input);
    requests.push(url);
    if (state.offline) throw new Error('offline');
    if (state.network) return state.network(url, init);
    if (url.endsWith('search-manifest.json')) return Response.json(manifest);
    const file = files.find((candidate) => key(candidate.url) === url);
    return new Response(file?.body ?? url);
  };
  const env: WorkerEnv = {
    scope,
    origin: new URL(scope).origin,
    caches: storage,
    fetch,
    clients: {
      claim: async () => {
        claimed += 1;
      },
      matchAll: async () => [{ postMessage: (message: unknown) => messages.push(message) }],
    },
    skipWaiting: async () => {},
    navigationPreload: {
      enable: async () => {
        preload = true;
      },
    },
    config: {
      version: 'build',
      app_version: 'app',
      content_version: 'content',
      precache: [{ url: '', revision: 'root', required: true }],
      offline_count: 0,
      offline_catalog: catalog,
      search_manifest: { version, base: indexBase },
      ...over.config,
    },
    timeouts: over.timeouts,
  };
  const ctx = createContext(env);
  return {
    ctx,
    env,
    state,
    storage,
    buckets,
    messages,
    requests,
    claims: () => claimed,
    preloadEnabled: () => preload,
  };
}

/** Let queued macrotasks run, so in-flight downloads reach their network call. */
export const tick = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));
