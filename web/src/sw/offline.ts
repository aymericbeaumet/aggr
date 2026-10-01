// Selected articles: complete families of a page and its retained renditions, stored apart from
// the evictable runtime caches, with the configured count persisted across worker updates.

import { fetchEntry } from './caches';
import { broadcastOfflineStatus, disabledSearchStatus, offlineUrl, type WorkerContext } from './context';
import { isRecord, validOfflineCount, type OfflineStatus, type SavedArticle } from './messages';
import { configureOfflineSearch, restoreSearch } from './search';
import type { OfflineItem, ResourceEntry } from './types';

/** How many articles download at once; each slot fetches one resource at a time. */
const DOWNLOAD_SLOTS = 6;

export function validOfflineItem(value: unknown): value is OfflineItem {
  return (
    isRecord(value) &&
    typeof value.url === 'string' &&
    typeof value.title === 'string' &&
    Array.isArray(value.resources) &&
    value.resources.length > 0 &&
    value.resources.every(
      (entry: unknown) => isRecord(entry) && typeof entry.url === 'string' && typeof entry.revision === 'string',
    )
  );
}

/** The current state from storage: only articles whose every resource is present at its revision count as saved. */
export async function readOfflineStatus(ctx: WorkerContext): Promise<OfflineStatus> {
  const { state, names, config } = ctx;
  const generation = state.configurationGeneration;
  const settings = await ctx.caches.open(names.offlineSettings);
  const configured = await settings.match(offlineUrl(ctx, '__offline_count'));
  const persisted = configured ? Number(await configured.text()) : config.offline_count;
  let count = state.configurationCount === null ? persisted : state.configurationCount;
  if (!validOfflineCount(count)) count = 0;
  const response = await settings.match(offlineUrl(ctx, '__offline_articles'));
  const saved: unknown = response ? await response.json().catch(() => []) : [];
  const pages = await ctx.caches.open(names.offlinePages);
  const revisions = await ctx.caches.open(names.offlineRevisions);
  const complete: SavedArticle[] = [];
  if (Array.isArray(saved))
    for (const item of saved.slice(0, count)) {
      if (!validOfflineItem(item)) continue;
      const valid = await Promise.all(
        item.resources.map(async (entry) => {
          const url = offlineUrl(ctx, entry.url);
          if (!url.startsWith(ctx.scope)) return false;
          const page = await pages.match(url);
          const revision = await revisions.match(url);
          return !!page && !!revision && (await revision.text()) === entry.revision;
        }),
      );
      if (valid.every(Boolean)) complete.push({ url: item.url, title: item.title });
    }
  await restoreSearch(ctx);
  if (generation !== state.configurationGeneration) return readOfflineStatus(ctx);
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
    total: Math.min(count, config.offline_catalog.length),
    saved: complete,
    failed: 0,
    downloading: state.offlinePendingCount !== null,
    search,
  };
}

/**
 * Store the first `count` catalogue articles. A family counts only once its page and every
 * rendition are written; older complete downloads within `count` survive a failed replacement;
 * everything outside the selection is dropped from the protected caches.
 */
export async function saveOfflineArticles(
  ctx: WorkerContext,
  count: number,
  generation?: number,
  signal?: AbortSignal,
): Promise<OfflineStatus> {
  const { state, names, config } = ctx;
  const superseded = () => generation !== undefined && generation !== state.offlineGeneration;
  const selected = config.offline_catalog.slice(0, count);
  const result: OfflineStatus = {
    type: 'AGGR_OFFLINE_STATUS',
    requested: count,
    total: selected.length,
    saved: [],
    failed: 0,
    downloading: true,
  };
  const wanted = new Set<string>();
  const resources = new Map<string, Promise<void>>();
  let next = 0;
  const complete: OfflineItem[] = [];
  for (const item of selected) for (const entry of item.resources) wanted.add(offlineUrl(ctx, entry.url));
  try {
    const [pages, revisions] = await Promise.all([
      ctx.caches.open(names.offlinePages),
      ctx.caches.open(names.offlineRevisions),
    ]);
    // Shared images share their in-flight promise, so a resource is fetched once per run.
    const saveResource = (entry: ResourceEntry): Promise<void> => {
      const url = offlineUrl(ctx, entry.url);
      const found = resources.get(url);
      if (found) return found;
      const pending = (async () => {
        const [page, revision] = await Promise.all([pages.match(url), revisions.match(url)]);
        if (page && revision && (await revision.text()) === entry.revision) return;
        const response = await fetchEntry(ctx.fetch, url, ctx.timeouts.precache, signal);
        // Record a revision only after its body is safely written.
        await pages.put(url, response);
        await revisions.put(url, new Response(entry.revision));
      })();
      resources.set(url, pending);
      return pending;
    };
    const saveNext = async (): Promise<void> => {
      while (next < selected.length && !superseded()) {
        const item = selected[next++];
        // Renditions first, the page last: a stored page implies its images are already there.
        const ordered = [
          ...item.resources.filter((entry) => entry.url !== item.url),
          ...item.resources.filter((entry) => entry.url === item.url),
        ];
        try {
          for (const entry of ordered) await saveResource(entry);
          complete.push(item);
          result.saved.push({ url: item.url, title: item.title });
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
      const settings = await ctx.caches.open(names.offlineSettings);
      let previous: unknown = [];
      try {
        const stored = await settings.match(offlineUrl(ctx, '__offline_articles'));
        previous = stored ? await stored.json() : [];
      } catch {
        previous = [];
      }
      if (result.failed && Array.isArray(previous)) {
        // Keep older complete downloads within N until failed newer replacements are available.
        for (const item of previous.slice(0, 1000)) {
          if (!validOfflineItem(item) || complete.length >= count || complete.some((entry) => entry.url === item.url))
            continue;
          const valid = await Promise.all(
            item.resources.map(async (entry) => {
              const url = offlineUrl(ctx, entry.url);
              if (!url.startsWith(ctx.scope)) return false;
              const [page, revision] = await Promise.all([pages.match(url), revisions.match(url)]);
              return page && revision ? (await revision.text()) === entry.revision : false;
            }),
          );
          if (!valid.length || !valid.every(Boolean)) continue;
          complete.push(item);
          result.saved.push({ url: item.url, title: item.title });
          for (const entry of item.resources) wanted.add(offlineUrl(ctx, entry.url));
        }
      }
      if (!superseded())
        await settings.put(offlineUrl(ctx, '__offline_articles'), new Response(JSON.stringify(complete))).catch(() => {});
    }
    if (!superseded())
      await Promise.all(
        [pages, revisions].map(async (cache) => {
          const keys = await cache.keys();
          await Promise.all(keys.filter((key) => !wanted.has(key.url)).map((key) => cache.delete(key)));
        }),
      );
  } catch {
    result.failed = Math.max(0, selected.length - result.saved.length);
  }
  if (!superseded()) {
    // A refreshed protected page must not be shadowed by a visited copy from an older build.
    const saved = new Set(result.saved.map((item) => offlineUrl(ctx, item.url)));
    try {
      const cache = await ctx.caches.open(names.pages);
      const keys = await cache.keys();
      await Promise.all(
        keys
          .filter((key) => {
            const url = new URL(key.url);
            url.search = '';
            return saved.has(url.href);
          })
          .map((key) => cache.delete(key)),
      );
    } catch {
      // The visited copy stays; the protected one still answers first.
    }
  }
  if (superseded()) {
    result.cancelled = true;
    return result;
  }
  const order = new Map(config.offline_catalog.map((item, index) => [item.url, index] as const));
  result.saved.sort((a, b) => (order.get(a.url) ?? Infinity) - (order.get(b.url) ?? Infinity));
  result.total = Math.max(result.total, result.saved.length);
  result.downloading = false;
  state.offlineStatus = result;
  void broadcastOfflineStatus(ctx, result);
  return result;
}

/** Queue a download of `count` articles, cancelling the one in flight. */
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
