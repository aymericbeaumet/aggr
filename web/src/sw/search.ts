// The offline search index: a manifest verified by size and SHA-256 digest, downloaded on two
// concurrent slots into a versioned cache, and made active only once every file is present.
// A previous complete index stays until the replacement has been committed.

import { fetchEntry, withTimeout } from './caches';
import { broadcastOfflineStatus, disabledSearchStatus, offlineUrl, type WorkerContext } from './context';
import { isRecord } from './messages';
import type { SearchFile, SearchManifest } from './types';

const SHA256 = /^[a-f0-9]{64}$/;

/** A manifest names only files under its own versioned base, with consistent sizes and digests. */
export function validSearchManifest(scope: string, manifest: unknown): manifest is SearchManifest {
  if (!isRecord(manifest) || typeof manifest.version !== 'string' || !SHA256.test(manifest.version)) return false;
  const base = manifest.base;
  if (
    typeof base !== 'string' ||
    base !== `pagefind/${manifest.version}/` ||
    !Array.isArray(manifest.files) ||
    !manifest.files.length
  )
    return false;
  const urls = new Set<string>();
  let total = 0;
  const valid = manifest.files.every((file: unknown) => {
    if (
      !isRecord(file) ||
      typeof file.url !== 'string' ||
      !file.url.startsWith(base) ||
      typeof file.size !== 'number' ||
      !Number.isSafeInteger(file.size) ||
      file.size < 0 ||
      typeof file.digest !== 'string' ||
      !SHA256.test(file.digest) ||
      urls.has(file.url)
    )
      return false;
    const url = new URL(file.url, scope);
    if (url.href !== scope + file.url || url.search || url.hash) return false;
    urls.add(file.url);
    total += file.size;
    return Number.isSafeInteger(total);
  });
  return valid && total === manifest.totalBytes && urls.has(`${base}pagefind.js`);
}

/** Whether every file of a manifest is stored in its versioned cache. */
export async function completeSearch(ctx: WorkerContext, manifest: unknown): Promise<boolean> {
  if (!validSearchManifest(ctx.scope, manifest)) return false;
  const name = ctx.names.offlineSearchPrefix + manifest.version;
  if (!(await ctx.caches.keys()).includes(name)) return false;
  const cache = await ctx.caches.open(name);
  const urls = new Set((await cache.keys()).map((request) => request.url));
  return manifest.files.every((file) => urls.has(offlineUrl(ctx, file.url)));
}

/** Reload the active index from the persisted manifest, if it is still complete. */
export async function restoreSearch(ctx: WorkerContext): Promise<SearchManifest | null> {
  const { state } = ctx;
  const generation = state.searchGeneration;
  if (state.configurationCount === 0) return null;
  const settings = await ctx.caches.open(ctx.names.offlineSettings);
  const response = await settings.match(offlineUrl(ctx, '__offline_search'));
  const manifest: unknown = response ? await response.json().catch(() => null) : null;
  const restored = validSearchManifest(ctx.scope, manifest) && (await completeSearch(ctx, manifest)) ? manifest : null;
  if (generation !== state.searchGeneration) return state.activeSearch;
  state.activeSearch = restored;
  state.searchStatus.activeVersion = restored ? restored.version : null;
  state.searchStatus.base = restored ? restored.base : null;
  if (manifest && !restored) {
    state.searchStatus.phase = 'error';
    state.searchStatus.error = 'evicted';
  }
  return state.activeSearch;
}

export function publishSearchStatus(ctx: WorkerContext): Promise<void> {
  const { state } = ctx;
  return broadcastOfflineStatus(
    ctx,
    state.offlineStatus || {
      type: 'AGGR_OFFLINE_STATUS',
      requested: state.configurationCount || 0,
      total: 0,
      saved: [],
      failed: 0,
      downloading: false,
    },
  );
}

/** The response's whole body, once its size and digest match the manifest. */
export async function verifiedSearchResponse(response: Response, file: SearchFile, timeout: number): Promise<Response> {
  if (!response.ok || !response.body) throw new Error('network');
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let length = 0;
  try {
    while (true) {
      const next = await withTimeout(reader.read(), timeout);
      if (next.done) break;
      length += next.value.byteLength;
      if (length > file.size) throw new Error('integrity');
      chunks.push(next.value);
    }
  } catch (error) {
    await reader.cancel().catch(() => {});
    throw error;
  }
  if (length !== file.size) throw new Error('integrity');
  const bytes = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  const digest = Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', bytes)), (byte) =>
    byte.toString(16).padStart(2, '0'),
  ).join('');
  if (digest !== file.digest) throw new Error('integrity');
  return new Response(bytes, { headers: response.headers });
}

async function deleteSearchCaches(ctx: WorkerContext, keep: ReadonlySet<string>): Promise<void> {
  const names = await ctx.caches.keys();
  await Promise.all(
    names
      .filter((name) => name.startsWith(ctx.names.offlineSearchPrefix) && !keep.has(name))
      .map((name) => ctx.caches.delete(name)),
  );
}

async function downloadSearch(ctx: WorkerContext, generation: number, signal: AbortSignal): Promise<void> {
  const { state, names } = ctx;
  const target = ctx.config.search_manifest;
  const timeout = ctx.timeouts.precache;
  const superseded = () => generation !== state.searchGeneration || signal.aborted;
  try {
    await restoreSearch(ctx);
    if (superseded()) return;
    const active = state.activeSearch;
    state.searchStatus = {
      phase: active ? 'updating' : 'downloading',
      activeVersion: active ? active.version : null,
      targetVersion: target.version,
      base: active ? active.base : null,
      downloadedFiles: 0,
      totalFiles: 0,
      downloadedBytes: 0,
      totalBytes: 0,
      error: null,
    };
    if (active && active.version === target.version) {
      state.searchStatus.phase = 'ready';
      state.searchStatus.downloadedFiles = state.searchStatus.totalFiles = active.files.length;
      state.searchStatus.downloadedBytes = state.searchStatus.totalBytes = active.totalBytes;
      return;
    }
    // During an update retain only the usable index and its replacement, bounding peak storage.
    const keepVersions = new Set([names.offlineSearchPrefix + target.version]);
    if (active) keepVersions.add(names.offlineSearchPrefix + active.version);
    await deleteSearchCaches(ctx, keepVersions);
    const cache = await ctx.caches.open(names.offlineSearchPrefix + target.version);
    const manifestUrl = offlineUrl(ctx, `${target.base}search-manifest.json`);
    let response = await cache.match(manifestUrl);
    if (!response) response = await fetchEntry(ctx.fetch, manifestUrl, timeout, signal);
    const decoded: unknown = await response.json();
    if (!validSearchManifest(ctx.scope, decoded) || decoded.version !== target.version) throw new Error('integrity');
    const manifest = decoded;
    await cache.put(
      manifestUrl,
      new Response(JSON.stringify(manifest), { headers: { 'content-type': 'application/json' } }),
    );
    state.searchStatus.totalFiles = manifest.files.length;
    state.searchStatus.totalBytes = manifest.totalBytes;
    const previousVersion = active ? active.version : null;
    const previous = active ? await ctx.caches.open(names.offlineSearchPrefix + active.version) : null;
    const reusable = new Map(active ? active.files.map((file) => [`${file.digest}:${file.size}`, file] as const) : []);
    let cursor = 0;
    let failed = false;
    let failure: unknown;
    const download = async () => {
      while (cursor < manifest.files.length && !superseded() && !failed) {
        const file = manifest.files[cursor++];
        const url = offlineUrl(ctx, file.url);
        try {
          let retained = await cache.match(url);
          if (retained) {
            try {
              retained = await verifiedSearchResponse(retained, file, timeout);
            } catch {
              await cache.delete(url);
              retained = undefined;
            }
          }
          if (!retained) {
            // A file the previous index already holds byte for byte is copied, not downloaded.
            const same = reusable.get(`${file.digest}:${file.size}`);
            let candidate = same && previous ? await previous.match(offlineUrl(ctx, same.url)) : undefined;
            if (candidate) {
              try {
                candidate = await verifiedSearchResponse(candidate, file, timeout);
              } catch {
                candidate = undefined;
              }
            }
            retained = candidate || (await verifiedSearchResponse(await fetchEntry(ctx.fetch, url, timeout, signal), file, timeout));
            if (superseded()) return;
            await cache.put(url, retained);
          }
          state.searchStatus.downloadedFiles += 1;
          state.searchStatus.downloadedBytes += file.size;
          await publishSearchStatus(ctx);
        } catch (error) {
          failure = error;
          failed = true;
        }
      }
    };
    await Promise.all([download(), download()]);
    if (superseded()) return;
    if (failed) throw failure;
    if (!(await completeSearch(ctx, manifest))) throw new Error('evicted');
    const settings = await ctx.caches.open(names.offlineSettings);
    await settings.put(
      offlineUrl(ctx, '__offline_search'),
      new Response(JSON.stringify(manifest), { headers: { 'content-type': 'application/json' } }),
    );
    state.activeSearch = manifest;
    state.searchStatus.activeVersion = manifest.version;
    state.searchStatus.base = manifest.base;
    state.searchStatus.phase = 'ready';
    // Keep one complete predecessor for tabs whose Pagefind instance is still using it.
    const keep = new Set([names.offlineSearchPrefix + manifest.version]);
    if (previousVersion) keep.add(names.offlineSearchPrefix + previousVersion);
    await deleteSearchCaches(ctx, keep);
  } catch (error) {
    if (superseded()) return;
    const quota = error instanceof Error && (error.name === 'QuotaExceededError' || /quota/i.test(error.message));
    const reason = error instanceof Error ? error.message : '';
    state.searchStatus.phase = quota ? 'blocked' : 'error';
    state.searchStatus.error = quota ? 'quota' : reason === 'integrity' || reason === 'evicted' ? reason : 'network';
    if (quota) await ctx.caches.delete(names.offlineSearchPrefix + target.version).catch(() => {});
  } finally {
    if (!superseded()) await publishSearchStatus(ctx);
  }
}

/** Download the current build's index, or drop every stored index when disabled. */
export function configureOfflineSearch(ctx: WorkerContext, enabled: boolean): Promise<void> {
  const { state } = ctx;
  if (state.searchEnabled === enabled && state.searchTask) return state.searchTask;
  state.searchEnabled = enabled;
  const generation = ++state.searchGeneration;
  if (!enabled) {
    state.activeSearch = null;
    state.searchStatus = disabledSearchStatus();
  }
  if (state.searchAbort) state.searchAbort.abort();
  const previous = state.searchTask;
  const task = Promise.resolve(previous)
    .catch(() => {})
    .then(async () => {
      if (generation !== state.searchGeneration) return;
      if (enabled) {
        const abort = new AbortController();
        state.searchAbort = abort;
        await downloadSearch(ctx, generation, abort.signal);
      } else {
        await deleteSearchCaches(ctx, new Set());
        await (await ctx.caches.open(ctx.names.offlineSettings)).delete(offlineUrl(ctx, '__offline_search'));
        state.activeSearch = null;
        state.searchStatus = disabledSearchStatus();
        await publishSearchStatus(ctx);
      }
    })
    .finally(() => {
      if (generation === state.searchGeneration) {
        state.searchTask = null;
        state.searchAbort = null;
      }
    });
  state.searchTask = task;
  return task;
}
