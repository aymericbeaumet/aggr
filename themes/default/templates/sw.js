// aggr service worker, rendered at build time. Two caches: the precache is rebuilt for every
// build (its name carries the build version), the runtime cache keeps pages read online so
// they stay readable offline across builds.
"use strict";

var VERSION = {{ version | json }};
var PRECACHE = "aggr-precache-" + VERSION;
var RUNTIME = "aggr-runtime";
var RUNTIME_MAX = 500;
var BASE = {{ site.base_path | json }};
var OFFLINE = BASE + "offline.html";
var SEARCH = BASE + "search.json";
var NETWORK_TIMEOUT = 4000;
var URLS = {{ precache | json }};

// Fetch past the HTTP cache so a new build never precaches the previous build's pages. One
// missing file must not fail the install, hence one request at a time per chunk with a catch.
function precache(cache, urls) {
  var chunk = urls.slice(0, 16);
  if (!chunk.length) return Promise.resolve();
  return Promise.all(chunk.map(function (url) {
    return cache.add(new Request(url, { cache: "reload" })).catch(function () {});
  })).then(function () { return precache(cache, urls.slice(16)); });
}

self.addEventListener("install", function (event) {
  event.waitUntil(
    caches.open(PRECACHE)
      .then(function (cache) { return precache(cache, URLS); })
      .then(function () { return self.skipWaiting(); })
  );
});

self.addEventListener("activate", function (event) {
  event.waitUntil(
    caches.keys().then(function (names) {
      return Promise.all(names.filter(function (name) {
        return name.indexOf("aggr-precache-") === 0 && name !== PRECACHE;
      }).map(function (name) { return caches.delete(name); }));
    }).then(function () { return self.clients.claim(); })
  );
});

function timeout(ms) {
  return new Promise(function (_, reject) {
    setTimeout(function () { reject(new Error("timeout")); }, ms);
  });
}

// Keep the runtime cache bounded; Cache.keys() lists entries oldest first.
function remember(request, response) {
  if (!response || !response.ok) return response;
  var copy = response.clone();
  caches.open(RUNTIME).then(function (cache) {
    return cache.put(request, copy).then(function () { return cache.keys(); }).then(function (keys) {
      return Promise.all(keys.slice(0, Math.max(0, keys.length - RUNTIME_MAX)).map(function (key) {
        return cache.delete(key);
      }));
    });
  }).catch(function () {});
  return response;
}

// Pages and the search index: the network when it answers quickly, the cache otherwise, and
// the offline page for a navigation nobody has cached.
function networkFirst(request) {
  return Promise.race([fetch(request), timeout(NETWORK_TIMEOUT)])
    .then(function (response) { return remember(request, response); })
    .catch(function () {
      return caches.match(request, { ignoreSearch: true }).then(function (cached) {
        if (cached) return cached;
        if (request.mode === "navigate") return caches.match(OFFLINE);
        return Response.error();
      });
    });
}

// Assets, icons, raw views: whatever is cached, else the network (and remember it).
function cacheFirst(request) {
  return caches.match(request).then(function (cached) {
    if (cached) return cached;
    return fetch(request).then(function (response) { return remember(request, response); });
  });
}

self.addEventListener("fetch", function (event) {
  var request = event.request;
  if (request.method !== "GET") return;
  var url = new URL(request.url);
  if (url.origin !== self.location.origin || url.pathname.indexOf(BASE) !== 0) return;
  if (request.mode === "navigate" || url.pathname === SEARCH) {
    event.respondWith(networkFirst(request));
  } else {
    event.respondWith(cacheFirst(request));
  }
});
