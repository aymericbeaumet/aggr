// Selected articles: complete families of a page and its retained renditions, stored apart from
// the evictable runtime caches, with the configured count persisted across worker updates.

import { fetchEntry } from './caches';
import { broadcastOfflineStatus, disabledSearchStatus, offlineUrl, type WorkerContext } from './context';
import { validOfflineCount, type OfflineStatus } from './messages';
import { commitSelection, completeItems, localOfflineItem, pruneSelection, readSelection, resourceKey, storedResource, validPageRevision } from './protected';
import { configureOfflineSearch, restoreSearch } from './search';
import type { OfflineItem, ResourceEntry } from './types';

/** How many articles download at once; each slot fetches one resource at a time. */
const DOWNLOAD_SLOTS = 6;

/** The published article catalog. A zero download count never asks for it. */
async function catalog(ctx: WorkerContext, signal?: AbortSignal): Promise<OfflineItem[]> {
  const cached = ctx.state.offlineCatalog;
  if (cached) return cached;
  let response: Response;
  const deadline = Date.now() + 5 * 60_000;
  for (;;) {
    response = await fetchEntry(
      ctx.fetch,
      offlineUrl(ctx, ctx.config.offline_catalog_url),
      ctx.timeouts.precache,
      signal,
      'reload',
    );
    if (response.status !== 202) break;
    if (Date.now() >= deadline) throw new Error('network');
    const retry = Number(response.headers.get('Retry-After'));
    const delay = Number.isFinite(retry) ? Math.min(5000, Math.max(100, retry * 1000)) : 1000;
    await new Promise<void>((resolve, reject) => {
      const cancelled = () => {
        clearTimeout(timer);
        reject(new DOMException('aborted', 'AbortError'));
      };
      const timer = setTimeout(() => {
        signal?.removeEventListener('abort', cancelled);
        resolve();
      }, delay);
      signal?.addEventListener('abort', cancelled, { once: true });
      if (signal?.aborted) cancelled();
    });
  }
  const body: unknown = await response.json();
  const items = Array.isArray(body) ? body.filter((item) => localOfflineItem(ctx, item)) : [];
  ctx.state.offlineCatalog = items;
  return items;
}

/** The current state from storage: only articles whose every resource is present at its revision count as saved. */
export async function readOfflineStatus(ctx: WorkerContext): Promise<OfflineStatus> {
  const { state, names, config } = ctx;
  const generation = state.configurationGeneration;
  const settings = await ctx.caches.open(names.offlineSettings);
  const configured = await settings.match(offlineUrl(ctx, '__offline_count'));
  const persisted = configured ? Number(await configured.text()) : config.offline_count;
  let count = state.configurationCount === null ? persisted : state.configurationCount;
  if (!validOfflineCount(count)) count = config.offline_count;
  const selection = await readSelection(ctx);
  const complete = (await completeItems(ctx, selection)).slice(0, count).map(({ url, title }) => ({ url, title }));
  await restoreSearch(ctx);
  if (generation !== state.configurationGeneration || (await readSelection(ctx)).version !== selection.version)
    return readOfflineStatus(ctx);
  const available = count > 0 ? await catalog(ctx).catch(() => []) : [];
  const search = count ? { ...state.searchStatus } : disabledSearchStatus();
  if (count && !state.searchTask) {
    const active = state.activeSearch;
    const target = config.search_manifest.version;
    const current = active !== null && active.version === target;
    search.phase = current
      ? 'ready'
      : search.error === 'quota'
        ? 'blocked'
        : search.error
          ? 'error'
          : active
            ? 'updating'
            : 'downloading';
    search.targetVersion = target;
    if (active && current) {
      search.downloadedFiles = search.totalFiles = active.files.length;
      search.downloadedBytes = search.totalBytes = active.totalBytes;
    }
  }
  return {
    type: 'AGGR_OFFLINE_STATUS',
    requested: count,
    total: Math.min(count, available.length),
    saved: complete,
    failed: 0,
    downloading: state.offlinePendingCount !== null,
    search,
  };
}

/**
 * Store the first `count` catalog articles. A family counts only once its page and every
 * rendition are written; older complete downloads within `count` survive a failed replacement;
 * everything outside the selection is dropped from the protected caches.
 */
export function saveOfflineArticles(
  ctx: WorkerContext,
  count: number,
  generation?: number,
  signal?: AbortSignal,
): Promise<OfflineStatus> {
  const run = () => saveSelection(ctx, count, generation, signal);
  const task = ctx.state.offlineStoreQueue.catch(() => {}).then(() =>
    ctx.locks ? ctx.locks.request(`${ctx.names.namespace}offline-selection`, run) : run(),
  );
  ctx.state.offlineStoreQueue = task;
  return task;
}

async function saveSelection(
  ctx: WorkerContext,
  count: number,
  generation?: number,
  signal?: AbortSignal,
): Promise<OfflineStatus> {
  const { state, names } = ctx;
  const superseded = () => signal?.aborted || (generation !== undefined && generation !== state.offlineGeneration);
  let selected: OfflineItem[] = [];
  let order = new Map<string, number>();
  const result: OfflineStatus = {
    type: 'AGGR_OFFLINE_STATUS', requested: count, total: 0, saved: [], failed: 0, downloading: true,
  };
  const resources = new Map<string, Promise<void>>();
  let next = 0;
  const complete: OfflineItem[] = [];
  let previous = await readSelection(ctx);
  let previousComplete = await completeItems(ctx, previous);
  result.saved = previousComplete.slice(0, count).map(({ url, title }) => ({ url, title }));
  try {
    if (superseded()) return { ...result, cancelled: true };
    if (count > 0) {
      const items = await catalog(ctx, signal);
      selected = items.slice(0, count);
      order = new Map(items.map((item, index) => [item.url, index] as const));
      result.total = selected.length;
    }
    const pages = await ctx.caches.open(names.offlinePages);
    // Shared images share their in-flight promise, keyed by revision as well as URL.
    const saveResource = (entry: ResourceEntry): Promise<void> => {
      const key = resourceKey(ctx, entry);
      const found = resources.get(key);
      if (found) return found;
      const pending = (async () => {
        if (await pages.match(key)) return;
        const legacy = await storedResource(ctx, entry, pages);
        const response = legacy ?? await fetchEntry(ctx.fetch, offlineUrl(ctx, entry.url), ctx.timeouts.precache, signal);
        if (!await validPageRevision(ctx, entry, response)) throw new Error('article revision changed');
        if (superseded()) throw new DOMException('aborted', 'AbortError');
        await pages.put(key, response);
      })();
      resources.set(key, pending);
      return pending;
    };
    const saveNext = async (): Promise<void> => {
      while (next < selected.length && !superseded()) {
        const item = selected[next++];
        const page = offlineUrl(ctx, item.url);
        // Renditions first; the committed selection remains unchanged throughout staging.
        const ordered = [
          ...item.resources.filter((entry) => offlineUrl(ctx, entry.url) !== page),
          ...item.resources.filter((entry) => offlineUrl(ctx, entry.url) === page),
        ];
        try {
          for (const entry of ordered) await saveResource(entry);
          complete.push(item);
        } catch {
          result.failed += 1;
        }
        if (!superseded()) {
          state.offlineStatus = result;
          void broadcastOfflineStatus(ctx, result);
        }
      }
    };
    await Promise.all(Array.from({ length: Math.min(DOWNLOAD_SLOTS, selected.length) }, () => saveNext()));
    if (!superseded()) {
      if (result.failed) {
        // Legacy bodies are copied into the new cache before committing, so an overlapping
        // previous worker cannot prune resources the new selection depends on.
        for (const item of previousComplete) {
          if (complete.length >= count || complete.some((entry) => entry.url === item.url)) continue;
          for (const entry of item.resources) await saveResource(entry);
          complete.push(item);
        }
      }
      if (!superseded()) {
        complete.sort((a, b) => (order.get(a.url) ?? Infinity) - (order.get(b.url) ?? Infinity));
        await commitSelection(ctx, complete);
        result.saved = complete.map(({ url, title }) => ({ url, title }));
      }
    }
  } catch {
    // A failed metadata commit keeps the previous pointer and all of its resource revisions.
    previous = await readSelection(ctx);
    previousComplete = await completeItems(ctx, previous);
    result.saved = previousComplete.slice(0, count).map(({ url, title }) => ({ url, title }));
    result.failed = Math.max(result.failed, selected.length || count || 1);
  } finally {
    // All download promises have settled; the next queued transaction cannot stage files yet.
    await pruneSelection(ctx).catch(() => {});
  }
  if (superseded()) return { ...result, cancelled: true };
  // A refreshed protected page must not be shadowed by a visited copy from an older build.
  const saved = new Set(result.saved.map((item) => offlineUrl(ctx, item.url)));
  try {
    const cache = await ctx.caches.open(names.pages);
    for (const key of await cache.keys()) {
      const url = new URL(key.url);
      url.search = '';
      if (saved.has(url.href)) await cache.delete(key);
    }
  } catch {
    // The protected one still answers first.
  }
  result.total = Math.max(result.total, result.saved.length);
  result.downloading = false;
  state.offlineStatus = result;
  void broadcastOfflineStatus(ctx, result);
  return result;
}

/** Queue a download of `count` articles, canceling the one in flight. */
export function configureOfflineArticles(ctx: WorkerContext, count: number): Promise<OfflineStatus | void> {
  const { state } = ctx;
  if (state.offlinePendingCount === count) return state.offlineQueue;
  const generation = ++state.offlineGeneration;
  state.offlinePendingCount = count;
  if (state.offlineAbort) state.offlineAbort.abort();
  state.offlineQueue = state.offlineQueue
    .catch(() => {})
    .then(() => {
      if (generation !== state.offlineGeneration) return;
      const abort = new AbortController();
      state.offlineAbort = abort;
      return saveOfflineArticles(ctx, count, generation, abort.signal).finally(() => {
        if (generation === state.offlineGeneration) {
          state.offlinePendingCount = null;
          state.offlineAbort = null;
        }
      });
    });
  return state.offlineQueue;
}

/** Apply a download limit: articles, the search index when positive, and the persisted count. */
export function configureOffline(ctx: WorkerContext, count: number): Promise<OfflineStatus | void> {
  const { state } = ctx;
  if (state.configurationCount === count && state.configurationTask) return state.configurationTask;
  state.configurationCount = count;
  const generation = ++state.configurationGeneration;
  const articles = configureOfflineArticles(ctx, count);
  const search = configureOfflineSearch(ctx, count > 0);
  state.settingsQueue = state.settingsQueue
    .catch(() => {})
    .then(async () => {
      const settings = await ctx.caches.open(ctx.names.offlineSettings);
      await settings.put(offlineUrl(ctx, '__offline_count'), new Response(String(count)));
    })
    .catch(() => {});
  const task = Promise.all([articles, search, state.settingsQueue])
    .then(([status]) => status)
    .finally(() => {
      if (generation === state.configurationGeneration) state.configurationTask = null;
    });
  state.configurationTask = task;
  return task;
}
