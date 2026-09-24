// The reader's service worker: make the shell installable, keep visited pages readable offline,
// and serve content-addressed assets from the cache. It never downloads an archive ahead of time;
// what you have read is what you keep.
//
// Rendered from this template by the Rust build, which supplies the version and precache list.
"use strict";

var VERSION = {{ version | json }};
var PRECACHE = {{ precache | json }};

// One namespace per installation, so several readers on an origin never evict each other.
var SCOPE = new URL("./", self.registration.scope).pathname;
var NAMESPACE = "aggr:" + encodeURIComponent(SCOPE) + ":";
var SHELL = NAMESPACE + "shell-" + VERSION;
var PAGES = NAMESPACE + "pages";
var ASSETS = NAMESPACE + "assets";
var OFFLINE = SCOPE + "offline.html";

var PAGE_LIMIT = 200;
var ASSET_LIMIT = 256;
var NETWORK_TIMEOUT = 4000;

/** Content-addressed URLs never change meaning, so they are safe to serve from cache first. */
function immutable(pathname) {
  var rest = pathname.slice(SCOPE.length);
  if (/^pagefind\/[0-9a-f]{64}\//.test(rest)) return true;
  if (rest.indexOf("assets/") !== 0) return false;
  var name = rest.split("/").pop() || "";
  var stem = name.indexOf(".") === -1 ? name : name.slice(0, name.lastIndexOf("."));
  if (/^(images|previews|documents)\//.test(rest.slice("assets/".length)) && /^[0-9a-f]{40}$/.test(stem)) return true;
  return /-[0-9a-f]{12}$/.test(stem);
}

/** Drop the oldest entries once a runtime cache passes its bound. */
async function trim(name, limit) {
  var cache = await caches.open(name);
  var keys = await cache.keys();
  for (var i = 0; i < keys.length - limit; i++) await cache.delete(keys[i]);
}

async function fromNetwork(request, timeout) {
  var controller = new AbortController();
  var timer = setTimeout(function () { controller.abort(); }, timeout);
  try {
    return await fetch(request, { signal: controller.signal });
  } finally {
    clearTimeout(timer);
  }
}

/** The last good copy of this page, else the page that explains why there is none. */
async function lastKnown(request) {
  return (await caches.match(request, { ignoreSearch: true })) || (await caches.match(OFFLINE));
}

/** A promise held to the same deadline a fetch of our own would get; `null` when it passes. */
function beforeDeadline(promise, timeout) {
  return Promise.race([
    promise,
    new Promise(function (resolve) {
      setTimeout(function () { resolve(null); }, timeout);
    }),
  ]);
}

/** Pages: fresh when the network answers, the last copy when it does not. */
async function pageResponse(request, preload) {
  try {
    // A navigation is already in flight before this worker wakes: preload is that request, and
    // using it saves opening a second one. It still answers to the deadline that lets a slow
    // network fall back to the last good copy.
    var response = preload ? await beforeDeadline(preload, NETWORK_TIMEOUT) : undefined;
    // `event.preloadResponse` resolves to undefined wherever preload is unsupported or off, which
    // is not the same as missing the deadline: that still owes the reader a request of our own.
    if (response === undefined) response = await fromNetwork(request, NETWORK_TIMEOUT);
    else if (response === null) return (await lastKnown(request)) || fromNetwork(request, NETWORK_TIMEOUT);
    if (response && response.ok) {
      var cache = await caches.open(PAGES);
      await cache.put(request, response.clone());
      void trim(PAGES, PAGE_LIMIT);
      return response;
    }
    // A host in trouble is no better than a host that cannot be reached: an error body must not
    // replace a page that was read before. Its own 404 and 410 are answers, and stand.
    if (response && response.status >= 500) return (await lastKnown(request)) || response;
    return response;
  } catch (error) {
    var offered = await lastKnown(request);
    if (offered) return offered;
    throw error;
  }
}

/** Assets: whatever is cached, else the network, remembered for next time. */
async function assetResponse(request, name, limit) {
  var cached = await caches.match(request);
  if (cached) return cached;
  var response = await fetch(request);
  if (response && response.ok) {
    var cache = await caches.open(name);
    await cache.put(request, response.clone());
    void trim(name, limit);
  }
  return response;
}

self.addEventListener("install", function (event) {
  event.waitUntil(
    caches
      .open(SHELL)
      .then(function (cache) {
        // One failed shell resource must not abandon the whole installation.
        return Promise.all(
          PRECACHE.map(function (entry) {
            return cache.add(new Request(entry.url, { cache: "reload" })).catch(function () {});
          }),
        );
      })
      .then(function () { return self.skipWaiting(); }),
  );
});

self.addEventListener("activate", function (event) {
  event.waitUntil(
    (async function () {
      if (self.registration.navigationPreload) await self.registration.navigationPreload.enable();
      var names = await caches.keys();
      await Promise.all(
        names
          .filter(function (name) {
            return name.indexOf(NAMESPACE) === 0 && name !== SHELL && name !== PAGES && name !== ASSETS;
          })
          .map(function (name) { return caches.delete(name); }),
      );
      await self.clients.claim();
    })(),
  );
});

self.addEventListener("fetch", function (event) {
  var request = event.request;
  if (request.method !== "GET") return;
  var url = new URL(request.url);
  if (url.origin !== self.location.origin || url.pathname.indexOf(SCOPE) !== 0) return;

  if (request.mode === "navigate" || (request.headers.get("accept") || "").indexOf("text/html") !== -1) {
    event.respondWith(pageResponse(request, event.preloadResponse));
    return;
  }
  if (immutable(url.pathname)) {
    event.respondWith(assetResponse(request, ASSETS, ASSET_LIMIT));
    return;
  }
  // Everything else (feeds, manifests, item representations) should stay current.
  event.respondWith(
    pageResponse(request).catch(function () {
      return caches.match(request);
    }),
  );
});
