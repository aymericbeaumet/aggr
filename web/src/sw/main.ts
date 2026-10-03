// The reader's service worker: make the shell installable, keep visited pages readable offline,
// and preserve complete selected articles and their images independently of runtime caches.
//
// `sw.js`, written by the Rust build at the site root, defines `self.AGGR_SW` and then imports
// this bundle, so the registration scope stays the site root whatever this file's hashed name.

import type { SwConfig } from './types';
import { createWorker } from './worker';

declare const self: ServiceWorkerGlobalScope & { AGGR_SW?: SwConfig };

const config = self.AGGR_SW;
if (!config) throw new Error('aggr: sw.js must define self.AGGR_SW before importing the worker');

const worker = createWorker({
  scope: self.registration.scope,
  origin: self.location.origin,
  caches,
  // The globals must keep the global scope as `this`; the context calls them as properties.
  fetch: (input, init) => fetch(input, init),
  clients: self.clients,
  skipWaiting: () => self.skipWaiting(),
  navigationPreload: self.registration.navigationPreload,
  config,
});

self.addEventListener('install', (event) => worker.install(event));
self.addEventListener('activate', (event) => worker.activate(event));
self.addEventListener('fetch', (event) => worker.fetch(event));
self.addEventListener('message', (event) => worker.message(event));
