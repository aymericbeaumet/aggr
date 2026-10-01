// Fetch strategies: pages fresh when the network answers, content-addressed assets from cache
// first, and everything else current with the cache as a last resort.

import { ASSET_LIMIT, PAGE_LIMIT, trim } from './caches';
import type { WorkerContext } from './context';
import type { FetchEventLike } from './types';

export type Route = 'page' | 'asset' | 'mutable';

export type RequestLike = Pick<Request, 'method' | 'url' | 'mode' | 'headers'>;

/** Which strategy answers a request, or `null` for one the worker leaves to the browser. */
export function classify(request: RequestLike, origin: string, scopePath: string): Route | null {
  if (request.method !== 'GET') return null;
  const url = new URL(request.url);
  if (url.origin !== origin || !url.pathname.startsWith(scopePath)) return null;
  if (request.mode === 'navigate' || (request.headers.get('accept') || '').includes('text/html')) return 'page';
  if (immutable(scopePath, url.pathname)) return 'asset';
  // Everything else (feeds, manifests, item representations) should stay current.
  return 'mutable';
}

/** Content-addressed URLs never change meaning, so they are safe to serve from cache first. */
export function immutable(scopePath: string, pathname: string): boolean {
  const rest = pathname.slice(scopePath.length);
  if (/^pagefind\/[0-9a-f]{64}\//.test(rest)) return true;
  if (!rest.startsWith('assets/')) return false;
  const name = rest.split('/').pop() || '';
  const stem = name.includes('.') ? name.slice(0, name.lastIndexOf('.')) : name;
  if (/^(images|previews|documents)\//.test(rest.slice('assets/'.length)) && /^[0-9a-f]{40}$/.test(stem)) return true;
  return /-[0-9a-f]{12}$/.test(stem);
}

async function fromNetwork(ctx: WorkerContext, request: Request, timeout: number): Promise<Response> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeout);
  try {
    return await ctx.fetch(request, { signal: controller.signal });
  } finally {
    clearTimeout(timer);
  }
}

/** The last good copy of this page, else the page that explains why there is none. */
async function lastKnown(ctx: WorkerContext, request: Request): Promise<Response | undefined> {
  const protectedPages = await ctx.caches.open(ctx.names.offlinePages);
  return (
    (await protectedPages.match(request, { ignoreSearch: true })) ||
    (await ctx.caches.match(request, { ignoreSearch: true })) ||
    (await ctx.caches.match(ctx.offlinePage))
  );
}

/** A promise held to the same deadline a fetch of our own would get; `null` when it passes. */
function beforeDeadline<T>(promise: Promise<T>, timeout: number): Promise<T | null> {
  const deadline = new Promise<null>((resolve) => {
    setTimeout(() => resolve(null), timeout);
  });
  return Promise.race([promise, deadline]);
}

/** Pages: fresh when the network answers, the last copy when it does not. */
export async function pageResponse(
  ctx: WorkerContext,
  request: Request,
  preload?: Promise<Response | undefined>,
): Promise<Response> {
  const timeout = ctx.timeouts.network;
  try {
    // A navigation is already in flight before this worker wakes: preload is that request, and
    // using it saves opening a second one. It still answers to the deadline that lets a slow
    // network fall back to the last good copy.
    let response = preload ? await beforeDeadline(preload, timeout) : undefined;
    // `event.preloadResponse` resolves to undefined wherever preload is unsupported or off, which
    // is not the same as missing the deadline: that still owes the reader a request of our own.
    if (response === undefined) response = await fromNetwork(ctx, request, timeout);
    else if (response === null) return (await lastKnown(ctx, request)) || fromNetwork(ctx, request, timeout);
    if (response.ok) {
      const cache = await ctx.caches.open(ctx.names.pages);
      await cache.put(request, response.clone()).catch(() => {});
      void trim(ctx.caches, ctx.names.pages, PAGE_LIMIT).catch(() => {});
      return response;
    }
    // A host in trouble is no better than a host that cannot be reached: an error body must not
    // replace a page that was read before. Its own 404 and 410 are answers, and stand.
    if (response.status >= 500) return (await lastKnown(ctx, request)) || response;
    return response;
  } catch (error) {
    const offered = await lastKnown(ctx, request);
    if (offered) return offered;
    throw error;
  }
}

/** Assets: whatever is cached, else the network, remembered for next time. */
export async function assetResponse(
  ctx: WorkerContext,
  request: Request,
  name: string,
  limit: number,
): Promise<Response> {
  const protectedPages = await ctx.caches.open(ctx.names.offlinePages);
  const cached = (await protectedPages.match(request)) || (await ctx.caches.match(request));
  if (cached) return cached;
  const response = await ctx.fetch(request);
  if (response.ok) {
    const cache = await ctx.caches.open(name);
    await cache.put(request, response.clone()).catch(() => {});
    void trim(ctx.caches, name, limit).catch(() => {});
  }
  return response;
}

/** Mutable files: current when possible, any cached copy otherwise, and a network error last. */
export function mutableResponse(ctx: WorkerContext, request: Request): Promise<Response> {
  return pageResponse(ctx, request).catch(async () => (await ctx.caches.match(request)) || Response.error());
}

export function handleFetch(ctx: WorkerContext, event: FetchEventLike): void {
  const { request } = event;
  switch (classify(request, ctx.origin, ctx.scopePath)) {
    case 'page':
      event.respondWith(pageResponse(ctx, request, event.preloadResponse));
      return;
    case 'asset':
      event.respondWith(assetResponse(ctx, request, ctx.names.assets, ASSET_LIMIT));
      return;
    case 'mutable':
      event.respondWith(mutableResponse(ctx, request));
      return;
    case null:
      return;
  }
}
