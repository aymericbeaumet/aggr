import type { BuildManifest } from './contracts';

interface BuildWatcherOptions {
  window: Window;
  document: Document;
  base(): string;
  enabled(): boolean;
  apply(build: BuildManifest | null): Promise<void>;
  fetch: typeof fetch;
}

export function createBuildWatcher(options: BuildWatcherOptions) {
  const listeners = new AbortController();
  let pending: Promise<void> | undefined;
  let cancellation: AbortController | undefined;
  let queued = false;
  let started = false;
  let timer: ReturnType<typeof setInterval> | undefined;
  let timeout: ReturnType<typeof setTimeout> | undefined;
  function check(queue: boolean | Event = false): Promise<void> {
    if (listeners.signal.aborted || !options.enabled() || !options.window.navigator.onLine
      || options.document.visibilityState !== 'visible') return Promise.resolve();
    if (pending) { queued ||= queue === true; return pending; }
    queued = false;
    cancellation = new AbortController();
    timeout = setTimeout(() => cancellation?.abort(), 10000);
    pending = options.fetch(new URL('updates.json', options.base()).href, { cache: 'no-store', signal: cancellation.signal })
      .then(response => response.ok ? response.json() : null)
      .then(build => { if (!listeners.signal.aborted) return options.apply(build); })
      .catch(() => { /* Keep the last complete reader during offline or partial deployments. */ })
      .finally(() => {
        clearTimeout(timeout);
        pending = undefined;
        if (queued) void check();
      });
    return pending;
  }
  return {
    check,
    start() {
      if (started || listeners.signal.aborted || !options.enabled() || options.window.location.protocol === 'file:') return;
      started = true;
      const eventOptions = { signal: listeners.signal };
      options.window.addEventListener('aggr:build', event => { event.preventDefault(); void check(true); }, eventOptions);
      options.document.addEventListener('visibilitychange', () => { if (options.document.visibilityState === 'visible') void check(); }, eventOptions);
      options.window.addEventListener('pageshow', check, eventOptions);
      options.window.addEventListener('online', check, eventOptions);
      timer = setInterval(check, 15000);
      void check();
    },
    dispose() { listeners.abort(); cancellation?.abort(); clearInterval(timer); clearTimeout(timeout); queued = false; }
  };
}
