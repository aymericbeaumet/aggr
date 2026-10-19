// Immutable resource revisions, selected by one atomic cache entry. Legacy downloads stay
// readable until a complete replacement has been committed in the separate v2 cache.

import { offlineUrl, type WorkerContext } from './context';
import { isRecord, OFFLINE_COUNT_LIMIT } from './messages';
import type { CacheLike, OfflineItem, ResourceEntry } from './types';

const SELECTION = '__offline_articles_v2';
const LEGACY_SELECTION = '__offline_articles';
const REVISION = '__aggr_revision';
const SELECTION_REVISION = 'x-aggr-selection';

interface Selection {
  version: string;
  items: OfflineItem[];
  legacy: boolean;
  resources: Map<string, ResourceEntry>;
}

const selections = new WeakMap<WorkerContext, Selection>();

export function validOfflineItem(value: unknown): value is OfflineItem {
  return isRecord(value) && typeof value.url === 'string' && typeof value.title === 'string' &&
    Array.isArray(value.resources) && value.resources.length > 0 && value.resources.every(
      (entry: unknown) => isRecord(entry) && typeof entry.url === 'string' && typeof entry.revision === 'string',
    );
}

export function localOfflineItem(ctx: WorkerContext, item: unknown): item is OfflineItem {
  if (!validOfflineItem(item)) return false;
  try {
    const page = offlineUrl(ctx, item.url);
    return page.startsWith(ctx.scope) && item.resources.some((entry) => offlineUrl(ctx, entry.url) === page) &&
      item.resources.every((entry) => {
        const url = new URL(entry.url, ctx.scope);
        return url.href.startsWith(ctx.scope) && !url.searchParams.has(REVISION);
      });
  } catch {
    return false;
  }
}

export function resourceKey(ctx: WorkerContext, entry: ResourceEntry): string {
  const url = new URL(entry.url, ctx.scope);
  url.hash = '';
  url.searchParams.set(REVISION, entry.revision);
  return url.href;
}

/** Rendered page revisions hash their bytes; a deployment may change between catalogue and page fetch. */
export async function validPageRevision(ctx: WorkerContext, entry: ResourceEntry, response: Response): Promise<boolean> {
  if (!new URL(entry.url, ctx.scope).pathname.endsWith('/') || !/^[0-9a-f]{40}$/.test(entry.revision)) return true;
  const digest = await crypto.subtle.digest('SHA-1', await response.clone().arrayBuffer());
  const hex = Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, '0')).join('');
  return hex === entry.revision;
}

export async function readSelection(ctx: WorkerContext): Promise<Selection> {
  const settings = await ctx.caches.open(ctx.names.offlineSettings);
  let response = await settings.match(offlineUrl(ctx, SELECTION));
  const legacy = !response;
  if (!response) response = await settings.match(offlineUrl(ctx, LEGACY_SELECTION));
  const stamped = response?.headers.get(SELECTION_REVISION);
  const body = !stamped && response ? await response.text() : null;
  const version = `${legacy ? 'legacy:' : 'v2:'}${stamped ?? body ?? 'absent'}`;
  const cached = selections.get(ctx);
  if (cached?.version === version) return cached;
  const value: unknown = body ? (() => { try { return JSON.parse(body); } catch { return []; } })() :
    response ? await response.json().catch(() => []) : [];
  const items = Array.isArray(value) ? value.slice(0, OFFLINE_COUNT_LIMIT).filter((item) => localOfflineItem(ctx, item)) : [];
  const selection = { version, items, legacy, resources: new Map<string, ResourceEntry>() };
  for (const item of items) for (const entry of item.resources) selection.resources.set(offlineUrl(ctx, entry.url), entry);
  selections.set(ctx, selection);
  return selection;
}

/** A revision-keyed body, or an unchanged body left by the previous worker. */
export async function storedResource(
  ctx: WorkerContext, entry: ResourceEntry, pages?: CacheLike, legacy = true,
): Promise<Response | undefined> {
  const current = pages ?? await ctx.caches.open(ctx.names.offlinePages);
  const response = await current.match(resourceKey(ctx, entry));
  if (response || !legacy) return response;
  const url = offlineUrl(ctx, entry.url);
  const revisions = await ctx.caches.open(ctx.names.offlineRevisions);
  const revision = await revisions.match(url);
  if (!revision || await revision.text() !== entry.revision) return undefined;
  const old = await (await ctx.caches.open(ctx.names.legacyOfflinePages)).match(url);
  return old && await validPageRevision(ctx, entry, old) ? old : undefined;
}

export async function completeItems(ctx: WorkerContext, selection: Selection): Promise<OfflineItem[]> {
  const pages = await ctx.caches.open(ctx.names.offlinePages);
  const complete: OfflineItem[] = [];
  for (const item of selection.items) {
    const resources = await Promise.all(item.resources.map((entry) => storedResource(ctx, entry, pages, selection.legacy)));
    if (resources.every(Boolean)) complete.push(item);
  }
  return complete;
}

export async function commitSelection(ctx: WorkerContext, items: OfflineItem[]): Promise<void> {
  const settings = await ctx.caches.open(ctx.names.offlineSettings);
  await settings.put(offlineUrl(ctx, SELECTION), Response.json(items, {
    headers: { [SELECTION_REVISION]: crypto.randomUUID() },
  }));
}

/** Prune only after all writes have settled, against the selection that actually committed. */
export async function pruneSelection(ctx: WorkerContext): Promise<void> {
  const selection = await readSelection(ctx);
  const pages = await ctx.caches.open(ctx.names.offlinePages);
  const wanted = new Set(selection.legacy ? [] :
    selection.items.flatMap((item) => item.resources.map((entry) => resourceKey(ctx, entry))));
  for (const key of await pages.keys()) if (!wanted.has(key.url)) await pages.delete(key);
  if (!selection.legacy) {
    await ctx.caches.delete(ctx.names.legacyOfflinePages);
    await ctx.caches.delete(ctx.names.offlineRevisions);
    await (await ctx.caches.open(ctx.names.offlineSettings)).delete(offlineUrl(ctx, LEGACY_SELECTION));
  }
}

/** Staged revisions are never visible through a public URL before their selection commits. */
export async function protectedResponse(ctx: WorkerContext, request: Request, ignoreSearch = false): Promise<Response | undefined> {
  const url = new URL(request.url);
  if (ignoreSearch) url.search = '';
  url.hash = '';
  for (let attempt = 0; attempt < 2; attempt++) {
    const selection = await readSelection(ctx);
    const entry = selection.resources.get(url.href);
    if (!entry) return undefined;
    const response = await storedResource(ctx, entry, undefined, selection.legacy);
    if (response) return response;
    if ((await readSelection(ctx)).version === selection.version) return undefined;
  }
  return undefined;
}
