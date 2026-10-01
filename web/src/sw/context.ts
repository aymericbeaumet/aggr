// The environment and mutable state every worker module shares, built once per worker.

import { DEFAULT_TIMEOUTS, cacheNames, scopePath, type CacheNames } from './caches';
import type { OfflineStatus, SearchStatus } from './messages';
import type {
  CacheStorageLike,
  ClientsLike,
  Fetch,
  SearchManifest,
  SwConfig,
  Timeouts,
  WorkerEnv,
} from './types';

export interface WorkerState {
  /** Article downloads run one configuration at a time. */
  offlineQueue: Promise<OfflineStatus | void>;
  /** The latest download result, broadcast with every search progress update. */
  offlineStatus: OfflineStatus | null;
  offlineGeneration: number;
  offlineAbort: AbortController | null;
  /** The count being downloaded, `null` between downloads. */
  offlinePendingCount: number | null;
  /** The complete index answering offline queries. */
  activeSearch: SearchManifest | null;
  searchEnabled: boolean;
  searchTask: Promise<void> | null;
  searchAbort: AbortController | null;
  searchGeneration: number;
  searchStatus: SearchStatus;
  /** The count from the latest `AGGR_OFFLINE_CONFIG`, `null` until one arrives. */
  configurationCount: number | null;
  configurationTask: Promise<OfflineStatus | void> | null;
  configurationGeneration: number;
  settingsQueue: Promise<void>;
}

export interface WorkerContext {
  readonly scope: string;
  readonly origin: string;
  readonly caches: CacheStorageLike;
  readonly fetch: Fetch;
  readonly clients: ClientsLike;
  readonly skipWaiting: () => Promise<void>;
  readonly navigationPreload: { enable(): Promise<void> } | null;
  readonly config: SwConfig;
  readonly timeouts: Timeouts;
  readonly names: CacheNames;
  /** The site root's path, the prefix every handled request starts with. */
  readonly scopePath: string;
  /** The page explaining that a route is not available offline. */
  readonly offlinePage: string;
  readonly state: WorkerState;
}

export function disabledSearchStatus(): SearchStatus {
  return {
    phase: 'disabled',
    activeVersion: null,
    targetVersion: null,
    base: null,
    downloadedFiles: 0,
    totalFiles: 0,
    downloadedBytes: 0,
    totalBytes: 0,
    error: null,
  };
}

export function createContext(env: WorkerEnv): WorkerContext {
  const path = scopePath(env.scope);
  return {
    scope: env.scope,
    origin: env.origin,
    caches: env.caches,
    fetch: env.fetch,
    clients: env.clients,
    skipWaiting: () => env.skipWaiting(),
    navigationPreload: env.navigationPreload ?? null,
    config: env.config,
    timeouts: { ...DEFAULT_TIMEOUTS, ...env.timeouts },
    names: cacheNames(env.scope, env.config.version),
    scopePath: path,
    offlinePage: `${path}offline.html`,
    state: {
      offlineQueue: Promise.resolve(),
      offlineStatus: null,
      offlineGeneration: 0,
      offlineAbort: null,
      offlinePendingCount: null,
      activeSearch: null,
      searchEnabled: false,
      searchTask: null,
      searchAbort: null,
      searchGeneration: 0,
      searchStatus: disabledSearchStatus(),
      configurationCount: null,
      configurationTask: null,
      configurationGeneration: 0,
      settingsQueue: Promise.resolve(),
    },
  };
}

/** A site path or URL resolved against the registration scope. */
export function offlineUrl(ctx: WorkerContext, path: string): string {
  return new URL(path, ctx.scope).href;
}

/**
 * Tell every window about the offline state, unless the configuration moved on since the status
 * was produced: a stale download must not overwrite the newer answer.
 */
export function broadcastOfflineStatus(ctx: WorkerContext, status: OfflineStatus): Promise<void> {
  const { state } = ctx;
  const generation = state.configurationGeneration;
  return ctx.clients
    .matchAll({ type: 'window' })
    .then((clients) => {
      if (
        generation !== state.configurationGeneration ||
        (state.configurationCount !== null && status.requested !== state.configurationCount)
      )
        return;
      const snapshot: OfflineStatus = { ...status, search: { ...state.searchStatus } };
      for (const client of clients) client.postMessage(snapshot);
    })
    .catch(() => {});
}
