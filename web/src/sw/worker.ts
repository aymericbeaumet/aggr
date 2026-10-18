// The worker lifecycle: install the shell, activate by dropping stale caches and restoring the
// persisted offline configuration, route fetches, and answer the page protocol.

import { fetchEntry, SHELL_REVISION, stale } from './caches';
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
        (async () => {
          const cache = await ctx.caches.open(ctx.names.shell);
          const prefix = `${ctx.names.namespace}shell-`;
          const previous = (await ctx.caches.keys()).filter((name) => name.startsWith(prefix) && name !== ctx.names.shell);
          // Required shell failures keep the previous working worker installed.
          await Promise.all(
            ctx.config.precache.map(async (entry) => {
              const url = offlineUrl(ctx, entry.url);
              try {
                let response: Response | undefined;
                for (const name of previous) {
                  const hit = await (await ctx.caches.open(name)).match(url);
                  if (hit?.ok && hit.headers.get(SHELL_REVISION) === entry.revision) {
                    response = hit;
                    break;
                  }
                }
                if (!response) {
                  const cacheMode = immutable(ctx.scopePath, new URL(url).pathname) ? 'force-cache' : 'reload';
                  const fetched = await fetchEntry(ctx.fetch, url, ctx.timeouts.precache, undefined, cacheMode);
                  const headers = new Headers(fetched.headers);
                  headers.set(SHELL_REVISION, entry.revision);
                  response = new Response(fetched.body, {
                    status: fetched.status,
                    statusText: fetched.statusText,
                    headers,
                  });
                }
                await cache.put(url, response);
              } catch (error) {
                if (entry.required) throw error;
              }
            }),
          );
          await ctx.skipWaiting();
        })(),
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
