import { sitePath } from '../model/urls';
import { navigation } from '../navigation';
import { configureOffline, startOfflineClient, updateWorker } from '../offline/client.svelte';
import { installPullRefresh, installedApp } from '../offline/pull-refresh.svelte';
import { announcer } from '../state/announcer.svelte';
import { connection } from '../state/connection.svelte';
import { page } from '../state/page.svelte';
import { search } from '../state/search.svelte';
import { versions, type Build } from '../state/versions.svelte';
import { badges } from '../updates/badges';
import { FeedUpdater } from '../updates/feed';
import { Poller } from '../updates/poll';
import { reloadKeepingPlace, restorePlace } from '../updates/reload';

/**
 * Deployment updates and offline: the `updates.json` poll, the live feed, the new-entry
 * badges, pull-to-refresh, the connection banner's actions and the service worker. Started
 * once the app owns the page; nothing here touches the document at import, so the status bar
 * can be rendered on the server.
 */

let poller: Poller | null = null;
let feed: FeedUpdater | null = null;
let rootPathname = '/';
let started = false;

function onBuild(build: Build): void {
  versions.availableApp = build.app_version;
  const model = page.model;
  // A list still showing an older build is due for its rows on every check, not only the first.
  const stale = model?.page.view === 'list' && build.content_version !== model.build.content;
  if (build.content_version === versions.latest && !stale) return;
  if (build.content_version !== versions.latest) {
    versions.latest = build.content_version;
    if (build.entries) badges.observe(build.entries);
    document.dispatchEvent(new CustomEvent('aggr:content-update', { detail: build }));
    // Every page fetched so far is the old build's; the results catch up with the new one.
    navigation.clear();
    search.driver?.refresh();
  }
  feed?.notice(build.content_version);
}

/** Start polling, the gestures and the worker for the page on screen. */
export function startReader(doc: Document = document, win: Window = window): void {
  const model = page.model;
  if (started || !model) return;
  started = true;
  const root = page.root;
  rootPathname = new URL(root).pathname;
  connection.start(win);
  let store: Storage | null = null;
  try {
    store = win.sessionStorage;
  } catch {
    store = null;
  }
  badges.configure(root, store, doc);
  feed = new FeedUpdater();
  feed.install(doc, win);
  poller = new Poller({
    load: async (signal) => {
      const response = await fetch(new URL('updates.json', root).href, { cache: 'no-store', signal });
      return response.ok ? response.json() : null;
    },
    onBuild,
    active: () => win.navigator.onLine && !doc.hidden && versions.app !== '',
  });
  win.addEventListener('online', () => {
    configureOffline(true);
    void poller?.check();
    updateWorker();
  });
  doc.addEventListener('visibilitychange', () => {
    if (doc.hidden) return;
    void poller?.check();
    updateWorker();
  });
  win.addEventListener('pageshow', () => void poller?.check());
  // The development server's rebuild: a check that insists, even over one in flight.
  win.addEventListener('aggr:build', (event) => {
    event.preventDefault();
    void poller?.check(true);
  });
  poller.start();
  void poller.check();
  const pwa = model.site.pwa;
  installPullRefresh({ enabled: () => pwa && installedApp(win), refresh: refreshToUpdate, doc, win });
  restorePlace(rootPathname, doc, win);
  startOfflineClient({ root, pwa, onControllerChange: () => void poller?.check(), doc, win });
  // The readiness signal the browser contract suite waits for before it edits `updates.json`.
  doc.documentElement.dataset.readerReady = 'true';
}

/** A page is on screen: its entries are seen, its list is checked against the live version, and it is announced. */
export function pageArrived(doc: Document = document): void {
  const model = page.model;
  if (!started || !model) return;
  const entries = [...model.site.entries];
  if (model.page.view === 'list') for (const row of model.page.data.rows) entries.push(sitePath(row.url));
  badges.arrive(entries);
  if (versions.latest) feed?.notice(versions.latest);
  announcer.say(`Navigated to ${doc.title}`);
}

/** The banner's Retry: tell the worker again and check for a deployment. */
export function retryConnection(): void {
  configureOffline(true);
  void poller?.check();
}

/** The pill and pull-to-refresh: reload, keeping the reader's place. */
export function refreshToUpdate(): void {
  reloadKeepingPlace(rootPathname);
}
