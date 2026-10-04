// The worker lifecycle: install the shell, activate by dropping stale caches and restoring the
// persisted offline configuration, route fetches, and answer the page protocol.

import { fetchEntry, stale } from './caches';
import { createContext, offlineUrl, type WorkerContext } from './context';
import { parsePageMessage, validOfflineCount, type BuildMessage } from './messages';
import { configureOffline, readOfflineStatus } from './offline';
import { handleFetch, immutable } from './routing';
import type { ExtendableEventLike, FetchEventLike, MessageEventLike, WorkerEnv } from './types';

export interface WorkerHandlers {
  install(event: ExtendableEventLike): void;
  activate(event: ExtendableEventLike): void;
  fetch(event: FetchEventLike): void;
  message(event: MessageEventLike): void;
}

export function lifecycle(ctx: WorkerContext): WorkerHandlers {
  return {
    install(event) {
      event.waitUntil(
        ctx.caches
          .open(ctx.names.shell)
          .then((cache) =>
            // Required shell failures keep the previous working worker installed.
            Promise.all(
              ctx.config.precache.map((entry) => {
                const url = offlineUrl(ctx, entry.url);
                const cacheMode = immutable(ctx.scopePath, new URL(url).pathname) ? 'force-cache' : 'reload';
                return fetchEntry(ctx.fetch, url, ctx.timeouts.precache, undefined, cacheMode)
                  .then((response) => cache.put(url, response))
                  .catch((error: unknown) => {
                    if (entry.required) throw error;
                  });
              }),
            ),
          )
          .then(() => ctx.skipWaiting()),
      );
    },

    activate(event) {
      event.waitUntil(
        (async () => {
          if (ctx.navigationPreload) await ctx.navigationPreload.enable();
          const names = await ctx.caches.keys();
          await Promise.all(names.filter((name) => stale(ctx.names, name)).map((name) => ctx.caches.delete(name)));
          await ctx.clients.claim();
          const settings = await ctx.caches.open(ctx.names.offlineSettings);
          const saved = await settings.match(offlineUrl(ctx, '__offline_count'));
          const count = saved ? Number(await saved.text()) : ctx.config.offline_count;
          if (validOfflineCount(count)) await configureOffline(ctx, count);
        })(),
      );
    },

    fetch(event) {
      handleFetch(ctx, event);
    },

    message(event) {
      const message = parsePageMessage(event.data);
      if (!message) return;
      // A page that loaded while this worker was activating asks to be claimed: activation only
      // claims the pages that existed at that moment.
      if (message.type === 'claim') {
        event.waitUntil(ctx.clients.claim());
        return;
      }
      const source = event.source;
      if (!source || typeof source.url !== 'string' || !source.url.startsWith(ctx.scope)) return;
      switch (message.type) {
        case 'AGGR_OFFLINE_GET_STATUS':
          event.waitUntil(readOfflineStatus(ctx).then((status) => source.postMessage(status)));
          return;
        case 'AGGR_GET_BUILD': {
          const build: BuildMessage = {
            type: 'AGGR_BUILD',
            app_version: ctx.config.app_version,
            content_version: ctx.config.content_version,
          };
          source.postMessage(build);
          return;
        }
        case 'AGGR_OFFLINE_CONFIG':
          event.waitUntil(configureOffline(ctx, message.count));
          return;
      }
    },
  };
}

export function createWorker(env: WorkerEnv): WorkerHandlers & { context: WorkerContext } {
  const context = createContext(env);
  return { ...lifecycle(context), context };
}
