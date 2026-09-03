// aggr service worker, rendered at build time. Two caches: the precache is rebuilt for every
// build (its name carries the build version), the runtime cache keeps pages read online so
// they stay readable offline across builds.
"use strict";

var VERSION = {{ version | json }};
var SCOPE_KEY = encodeURIComponent(new URL(self.registration.scope).pathname);
var CACHE_NAMESPACE = "aggr:" + SCOPE_KEY + ":";
var PRECACHE = CACHE_NAMESPACE + "precache-" + VERSION;
var RUNTIME = CACHE_NAMESPACE + "runtime";
var RUNTIME_MAX = 500;
var BASE = new URL("./", self.registration.scope).pathname;
var OFFLINE = BASE + "offline.html";
var NETWORK_TIMEOUT = 4000;
var URLS = {{ precache | json }}.map(function (path) { return new URL(path, self.registration.scope).pathname; });

// Fetch past the HTTP cache so a new build never precaches the previous build's pages. Every
// listed file was emitted by the same atomic build. One transient response must not strand an old
// worker forever, so entries retry naturally on navigation when an individual install fetch fails.
function precache(cache, urls) {
  var chunk = urls.slice(0, 16);
  if (!chunk.length) return Promise.resolve();
  return Promise.all(chunk.map(function (url) {
    return cache.add(new Request(url, { cache: "reload" })).catch(function () { return null; });
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
        return name.indexOf(CACHE_NAMESPACE + "precache-") === 0 && name !== PRECACHE;
      }).map(function (name) { return caches.delete(name); }));
    }).then(function () {
      if (self.registration.navigationPreload) return self.registration.navigationPreload.enable();
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
  if (!response || (!response.ok && response.type !== "opaque")) return Promise.resolve(response);
  var copy = response.clone();
  return caches.open(RUNTIME).then(function (cache) {
    return cache.put(request, copy).then(function () { return cache.keys(); }).then(function (keys) {
      return Promise.all(keys.slice(0, Math.max(0, keys.length - RUNTIME_MAX)).map(function (key) {
        return cache.delete(key);
      })).then(function () { return response; });
    });
  }).catch(function () { return response; });
}

// Pages: the network when it answers quickly, the cache otherwise, and
// the offline page for a navigation nobody has cached.
function networkFirst(request, preload) {
  var network = Promise.resolve(preload).then(function (response) { return response || fetch(request); });
  return Promise.race([network, timeout(NETWORK_TIMEOUT)])
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
  // Pagefind appends a cache tag to its metadata request. Match the immutable generated path so
  // the build-time precache works before the first online search as well as afterwards.
  return caches.match(request, { ignoreSearch: true }).then(function (cached) {
    if (cached) return cached;
    return fetch(request).then(function (response) { return remember(request, response); });
  });
}

self.addEventListener("fetch", function (event) {
  var request = event.request;
  if (request.method !== "GET") return;
  var url = new URL(request.url);
  if (url.origin !== self.location.origin) {
    if (request.destination === "image") event.respondWith(cacheFirst(request));
    return;
  }
  if (url.pathname.indexOf(BASE) !== 0) return;
  var acceptsHtml = (request.headers.get("accept") || "").indexOf("text/html") !== -1;
  var isSwup = (request.headers.get("x-requested-with") || "").toLowerCase() === "swup";
  var mutable = /\/(?:atom|rss|feed)\.xml$|\/feed\.json$|\/manifest\.webmanifest$|\/opensearch\.xml$|\/sitemap(?:-\d+)?\.xml$|\/robots\.txt$|\/aggr\.toml$/.test(url.pathname);
  if (request.mode === "navigate" || acceptsHtml || isSwup || mutable) {
    event.respondWith(networkFirst(request, event.preloadResponse));
  } else {
    event.respondWith(cacheFirst(request));
  }
});
