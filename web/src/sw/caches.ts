// Cache names, bounds and the fetch every stored response goes through.

import type { CacheStorageLike, Fetch, Timeouts } from './types';

/** Visited pages kept for offline reading. */
export const PAGE_LIMIT = 200;
/** Content-addressed assets kept beside them. */
export const ASSET_LIMIT = 256;
export const NETWORK_TIMEOUT = 4000;
export const PRECACHE_TIMEOUT = 10000;

export const DEFAULT_TIMEOUTS: Timeouts = { network: NETWORK_TIMEOUT, precache: PRECACHE_TIMEOUT };

export interface CacheNames {
  namespace: string;
  /** The precached shell of one worker version. */
  shell: string;
  /** Visited pages, bounded by `PAGE_LIMIT`. */
  pages: string;
  /** Content-addressed assets, bounded by `ASSET_LIMIT`. */
  assets: string;
  /** Selected articles and their renditions: never evicted by the runtime caches. */
  offlinePages: string;
  /** The revision each protected resource was stored at. */
  offlineRevisions: string;
  /** The configured count, the saved article list and the active search manifest. */
  offlineSettings: string;
  /** One complete search index per version, after this prefix. */
  offlineSearchPrefix: string;
}

/** The site root's path, from the registration scope. */
export function scopePath(scope: string): string {
  return new URL('./', scope).pathname;
}

/** One namespace per installation, so several readers on an origin never evict each other. */
export function cacheNames(scope: string, version: string): CacheNames {
  const namespace = `aggr:${encodeURIComponent(scopePath(scope))}:`;
  return {
    namespace,
    shell: `${namespace}shell-${version}`,
    pages: `${namespace}pages`,
    assets: `${namespace}assets`,
    offlinePages: `${namespace}offline-articles`,
    offlineRevisions: `${namespace}offline-revisions`,
    offlineSettings: `${namespace}offline-settings`,
    offlineSearchPrefix: `${namespace}offline-search-`,
  };
}

/** Whether activation deletes `name`: this namespace's caches the worker does not use, older shells included. */
export function stale(names: CacheNames, name: string): boolean {
  return (
    name.startsWith(names.namespace) &&
    name !== names.shell &&
    name !== names.pages &&
    name !== names.assets &&
    name !== names.offlinePages &&
    name !== names.offlineRevisions &&
    name !== names.offlineSettings &&
    !name.startsWith(names.offlineSearchPrefix)
  );
}

export function withTimeout<T>(promise: Promise<T>, milliseconds: number): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const deadline = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error('network')), milliseconds);
  });
  return Promise.race([promise, deadline]).finally(() => clearTimeout(timer));
}

/** Drop the oldest entries once a runtime cache passes its bound. */
export async function trim(caches: CacheStorageLike, name: string, limit: number): Promise<void> {
  const cache = await caches.open(name);
  const keys = await cache.keys();
  for (let i = 0; i < keys.length - limit; i++) await cache.delete(keys[i]);
}

/**
 * Fetch one complete resource within `timeout` for the whole body. Mutable files require a
 * fresh response; content-addressed files can reuse a response already in the HTTP cache.
 */
export async function fetchEntry(fetch: Fetch, url: string, timeout: number, signal?: AbortSignal, cache: RequestCache = 'reload'): Promise<Response> {
  const controller = new AbortController();
  const abort = () => controller.abort();
  if (signal) {
    if (signal.aborted) controller.abort();
    else signal.addEventListener('abort', abort, { once: true });
  }
  const timer = setTimeout(abort, timeout);
  try {
    const response = await fetch(url, { signal: controller.signal, cache, priority: 'low' });
    if (!response.ok || response.type === 'opaque') throw new Error('network');
    // Keep the deadline active while reading a stalled response body, not only its headers.
    const bytes = await response.arrayBuffer();
    return new Response(bytes, { status: response.status, headers: response.headers });
  } finally {
    clearTimeout(timer);
    if (signal) signal.removeEventListener('abort', abort);
  }
}
