import { describe, expect, it } from 'vitest';
import { cacheNames, stale } from './caches';
import { harness, scope, tick } from './harness';
import { isBuildMessage, isOfflineStatus, parsePageMessage, validOfflineCount } from './messages';
import type { ClientLike, MessageEventLike } from './types';
import { createWorker, lifecycle } from './worker';

const message = (data: unknown, url: string | null = scope) => {
  const posted: unknown[] = [];
  const waited: Promise<unknown>[] = [];
  const source: ClientLike = { ...(url === null ? {} : { url }), postMessage: (value) => posted.push(value) };
  const event: MessageEventLike = { data, source, waitUntil: (promise) => waited.push(promise) };
  return { event, posted, settle: () => Promise.all(waited) };
};

describe('parsePageMessage', () => {
  it('accepts the four page messages and validates the count', () => {
    expect(parsePageMessage({ type: 'claim' })).toEqual({ type: 'claim' });
    expect(parsePageMessage({ type: 'AGGR_OFFLINE_GET_STATUS' })).toEqual({ type: 'AGGR_OFFLINE_GET_STATUS' });
    expect(parsePageMessage({ type: 'AGGR_GET_BUILD', extra: 1 })).toEqual({ type: 'AGGR_GET_BUILD' });
    expect(parsePageMessage({ type: 'AGGR_OFFLINE_CONFIG', count: 30 })).toEqual({ type: 'AGGR_OFFLINE_CONFIG', count: 30 });
    for (const count of [-1, 1.5, 1001, '30', undefined, Number.NaN]) {
      expect(parsePageMessage({ type: 'AGGR_OFFLINE_CONFIG', count })).toBeNull();
      expect(validOfflineCount(count)).toBe(false);
    }
    expect(parsePageMessage({ type: 'AGGR_OTHER' })).toBeNull();
    expect(parsePageMessage('claim')).toBeNull();
    expect(parsePageMessage(null)).toBeNull();
  });
});

describe('cache names', () => {
  it('namespace one installation and mark only its unused caches stale', () => {
    const names = cacheNames(scope, 'v1');
    expect(names.namespace).toBe('aggr:%2Farchive%2F:');
    expect(names.shell).toBe('aggr:%2Farchive%2F:shell-v1');
    expect(stale(names, 'aggr:%2Farchive%2F:shell-v0')).toBe(true);
    expect(stale(names, 'aggr:%2Farchive%2F:legacy')).toBe(true);
    expect(stale(names, 'aggr:%2Fother%2F:shell-v0')).toBe(false);
    for (const name of [names.shell, names.pages, names.assets, names.offlinePages, names.offlineRevisions, names.offlineSettings, `${names.offlineSearchPrefix}${'a'.repeat(64)}`]) {
      expect(stale(names, name)).toBe(false);
    }
  });
});

describe('messages', () => {
  it('answers the build versions and the offline status to a page in scope', async () => {
    const { ctx } = harness();
    const handlers = lifecycle(ctx);
    const build = message({ type: 'AGGR_GET_BUILD' });
    handlers.message(build.event);
    expect(build.posted).toEqual([{ type: 'AGGR_BUILD', app_version: 'app', content_version: 'content' }]);
    expect(isBuildMessage(build.posted[0])).toBe(true);
    const status = message({ type: 'AGGR_OFFLINE_GET_STATUS' });
    handlers.message(status.event);
    await status.settle();
    expect(status.posted).toHaveLength(1);
    expect(isOfflineStatus(status.posted[0])).toBe(true);
    expect(status.posted[0]).toMatchObject({ requested: 0, saved: [], downloading: false, search: { phase: 'disabled' } });
  });

  it('ignores pages outside the scope and unknown data, but claims for anyone', async () => {
    const { ctx, claims } = harness();
    const handlers = lifecycle(ctx);
    const foreign = message({ type: 'AGGR_GET_BUILD' }, 'https://reader.invalid/other/');
    handlers.message(foreign.event);
    const anonymous = message({ type: 'AGGR_GET_BUILD' }, null);
    handlers.message(anonymous.event);
    handlers.message({ data: { type: 'AGGR_GET_BUILD' }, source: null, waitUntil: () => {} });
    handlers.message(message('AGGR_GET_BUILD').event);
    expect(foreign.posted).toEqual([]);
    expect(anonymous.posted).toEqual([]);
    const claim = message({ type: 'claim' }, 'https://reader.invalid/other/');
    handlers.message(claim.event);
    await claim.settle();
    expect(claims()).toBe(1);
  });

  it('configures downloads from AGGR_OFFLINE_CONFIG and broadcasts the result', async () => {
    const { ctx, messages } = harness();
    const handlers = lifecycle(ctx);
    const invalid = message({ type: 'AGGR_OFFLINE_CONFIG', count: 5000 });
    handlers.message(invalid.event);
    await invalid.settle();
    expect(ctx.state.configurationCount).toBeNull();
    const config = message({ type: 'AGGR_OFFLINE_CONFIG', count: 1 });
    handlers.message(config.event);
    await config.settle();
    const last = messages.filter(isOfflineStatus).at(-1);
    expect(last).toMatchObject({ requested: 1, total: 1, downloading: false, search: { phase: 'ready' } });
    expect(last?.saved.map((item) => item.title)).toEqual(['Newest']);
  });
});

describe('activation', () => {
  it('enables preload, drops stale caches, claims clients and restores the persisted count', async () => {
    const { ctx, storage, claims, preloadEnabled, messages } = harness();
    const handlers = lifecycle(ctx);
    await storage.open(`${ctx.names.namespace}shell-old`);
    await storage.open('aggr:%2Fother%2F:shell-old');
    await storage.open(ctx.names.pages);
    await (await storage.open(ctx.names.offlineSettings)).put(`${scope}__offline_count`, new Response('2'));
    const waited: Promise<unknown>[] = [];
    handlers.activate({ waitUntil: (promise) => waited.push(promise) });
    await Promise.all(waited);
    expect(preloadEnabled()).toBe(true);
    expect(claims()).toBe(1);
    const names = await storage.keys();
    expect(names).not.toContain(`${ctx.names.namespace}shell-old`);
    expect(names).toContain('aggr:%2Fother%2F:shell-old');
    expect(names).toContain(ctx.names.pages);
    expect(ctx.state.configurationCount).toBe(2);
    expect(messages.filter(isOfflineStatus).at(-1)?.saved).toHaveLength(2);
  });

  it('falls back to the build default when nothing is persisted, and installs the shell', async () => {
    const { ctx, storage, requests } = harness({ config: { offline_count: 1 } });
    const handlers = lifecycle(ctx);
    const installs: Promise<unknown>[] = [];
    handlers.install({ waitUntil: (promise) => installs.push(promise) });
    await Promise.all(installs);
    expect(await (await storage.open(ctx.names.shell)).match(scope)).toBeTruthy();
    expect(requests).toEqual([scope]);
    const waited: Promise<unknown>[] = [];
    handlers.activate({ waitUntil: (promise) => waited.push(promise) });
    await Promise.all(waited);
    expect(ctx.state.configurationCount).toBe(1);
    await expect((await storage.open(ctx.names.offlineSettings)).match(`${scope}__offline_count`)).resolves.toBeTruthy();
  });

  it('tolerates optional shell resources that fail', async () => {
    const { ctx, state } = harness({
      config: { precache: [{ url: 'offline.html', revision: 'r', required: true }, { url: 'optional.css', revision: 'r', required: false }] },
    });
    state.network = async (url) => (url.endsWith('optional.css') ? new Response('nope', { status: 500 }) : new Response(url));
    const installs: Promise<unknown>[] = [];
    lifecycle(ctx).install({ waitUntil: (promise) => installs.push(promise) });
    await expect(Promise.all(installs)).resolves.toBeDefined();
  });
});

describe('createWorker', () => {
  it('wires the handlers over a fresh context', async () => {
    const { env } = harness();
    const worker = createWorker(env);
    expect(worker.context.names.shell).toBe('aggr:%2Farchive%2F:shell-build');
    const build = message({ type: 'AGGR_GET_BUILD' });
    worker.message(build.event);
    expect(build.posted).toHaveLength(1);
    await tick();
  });
});
