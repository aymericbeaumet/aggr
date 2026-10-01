import { untrack } from 'svelte';
import { offline } from '../state/offline.svelte';
import { preferences } from '../state/preferences.svelte';
import { isOfflineStatus, validOfflineCount, type PageMessage } from '../sw/messages';

/**
 * The page side of the service worker: registration, the offline configuration it is told
 * (from the `offline-items` preference), and the status it reports back into state. A site
 * built without a worker unregisters any it left behind. Registration waits for a prerendered
 * page to be shown, like every other side effect of a visit.
 */

type Options = {
  /** Absolute URL of the site root: the worker's script sits beside it and its scope is its path. */
  root: string;
  pwa: boolean;
  /** Another worker took the page: the deployment may have changed. */
  onControllerChange?: () => void;
  doc?: Document;
  win?: Window;
};

let container: ServiceWorkerContainer | null = null;
let registration: ServiceWorkerRegistration | undefined;
let configuredWorker: ServiceWorker | null = null;
let configuredCount = -1;

const post = (worker: ServiceWorker, message: PageMessage): void => worker.postMessage(message);

/** Tell the controlling worker how many articles to keep, when that changed (or `force`). */
export function configureOffline(force = false): void {
  const worker = container?.controller ?? null;
  const count = preferences.offlineItems;
  if (!worker || !validOfflineCount(count)) return;
  if (!force && worker === configuredWorker && count === configuredCount) return;
  configuredWorker = worker;
  configuredCount = count;
  post(worker, { type: 'AGGR_OFFLINE_CONFIG', count });
  post(worker, { type: 'AGGR_OFFLINE_GET_STATUS' });
}

/** Ask the browser to look for a newer worker script. */
export function updateWorker(): void {
  void registration?.update().catch(() => {});
}

/** Relay one worker message into state; anything but a status from the controller is ignored. */
export function receive(event: { data: unknown; source: unknown }, controller: ServiceWorker | null): boolean {
  if (controller === null || event.source !== controller || !isOfflineStatus(event.data)) return false;
  offline.receive(event.data);
  return true;
}

/** A site without a worker: let go of the one an earlier build registered, and its caches. */
function unregister(root: string, serviceWorker: ServiceWorkerContainer, win: Window): void {
  void serviceWorker
    .getRegistration(root)
    .then((found) => (found?.scope === root ? found.unregister() : undefined))
    .catch(() => {});
  if (!('caches' in win)) return;
  const prefix = `aggr:${encodeURIComponent(new URL(root).pathname)}:`;
  void win.caches
    .keys()
    .then((names) => Promise.all(names.filter((name) => name.startsWith(prefix)).map((name) => win.caches.delete(name))))
    .catch(() => {});
}

export function startOfflineClient({ root, pwa, onControllerChange, doc = document, win = window }: Options): void {
  const serviceWorker = win.navigator.serviceWorker as ServiceWorkerContainer | undefined;
  offline.enabled = pwa;
  if (!serviceWorker) return;
  if (!pwa) {
    unregister(root, serviceWorker, win);
    return;
  }
  const start = () => {
    container = serviceWorker;
    serviceWorker.addEventListener('message', (event) => receive(event, serviceWorker.controller));
    serviceWorker.addEventListener('controllerchange', () => {
      configureOffline(true);
      onControllerChange?.();
    });
    // The preference is the configuration: every change reaches the worker.
    $effect.root(() => {
      $effect(() => {
        void preferences.offlineItems;
        untrack(() => configureOffline());
      });
    });
    serviceWorker
      .register(new URL('sw.js', root).href, { scope: new URL(root).pathname, updateViaCache: 'none' })
      .then(() => serviceWorker.ready)
      .then((ready) => {
        registration = ready;
        // A page whose load began while its worker was still activating is not one the worker
        // claimed then. Pages change in place, so it would stay uncontrolled, and uncached
        // offline, for the whole visit: ask to be claimed.
        if (!serviceWorker.controller) ready.active?.postMessage({ type: 'claim' } satisfies PageMessage);
        configureOffline();
      })
      .catch((error: unknown) => console.error('aggr: service worker', error));
    win.setInterval(() => {
      if (win.navigator.onLine && !doc.hidden) updateWorker();
    }, 60_000);
  };
  if (doc.prerendering) doc.addEventListener('prerenderingchange', start, { once: true });
  else start();
}
