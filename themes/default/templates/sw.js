// aggr service worker, rendered at build time. Revisioned precaching makes deployment updates
// transactional and reuses unchanged files. Separate bounded caches keep readable pages and the
// assets they reference available across builds without letting remote images evict them.
"use strict";

var VERSION = {{ version | json }};
var SCOPE_KEY = encodeURIComponent(new URL(self.registration.scope).pathname);
var CACHE_NAMESPACE = "aggr:" + SCOPE_KEY + ":";
var PRECACHE_PREFIX = CACHE_NAMESPACE + "precache-";
var REVISIONS_PREFIX = CACHE_NAMESPACE + "revisions-";
var PRECACHE = PRECACHE_PREFIX + VERSION;
var REVISIONS = CACHE_NAMESPACE + "revisions-" + VERSION;
var PAGES = CACHE_NAMESPACE + "pages";
var ASSETS = CACHE_NAMESPACE + "assets";
var SEARCH = CACHE_NAMESPACE + "search";
var IMAGES = CACHE_NAMESPACE + "images";
var LEGACY_RUNTIME = CACHE_NAMESPACE + "runtime";
var PAGE_MAX = 500;
var ASSET_MAX = 128;
var SEARCH_MAX = 512;
var IMAGE_MAX = 128;
var BASE = new URL("./", self.registration.scope).pathname;
var OFFLINE = BASE + "offline.html";
var NETWORK_TIMEOUT = 4000;
var ENTRIES = {{ precache | json }}.map(function (entry) {
  return {
    url: new URL(entry.url, self.registration.scope).pathname,
    revision: entry.revision,
    required: entry.required
  };
});
var REQUIRED_URLS = ENTRIES.filter(function (entry) { return entry.required; });
var OPTIONAL_URLS = ENTRIES.filter(function (entry) { return !entry.required; });

function priorResponse(entry, revisionCaches, index) {
  if (index >= revisionCaches.length) return Promise.resolve(null);
  var revisionsName = revisionCaches[index];
  return caches.open(revisionsName).then(function (cache) {
    return cache.match(entry.url);
  }).then(function (marker) {
    if (!marker) return priorResponse(entry, revisionCaches, index + 1);
    return marker.text().then(function (revision) {
      if (revision !== entry.revision) return priorResponse(entry, revisionCaches, index + 1);
      var suffix = revisionsName.slice(REVISIONS_PREFIX.length);
      return caches.open(PRECACHE_PREFIX + suffix).then(function (cache) {
        return cache.match(entry.url);
      }).then(function (response) {
        return response || priorResponse(entry, revisionCaches, index + 1);
      });
    });
  }).catch(function () { return priorResponse(entry, revisionCaches, index + 1); });
}

function fetchEntry(entry) {
  return fetch(new Request(entry.url, { cache: "reload" })).then(function (response) {
    if (!response || !response.ok) {
      throw new Error("could not precache " + entry.url);
    }
    return response;
  });
}

function storeEntry(cache, revisions, revisionCaches, entry) {
  return priorResponse(entry, revisionCaches, 0).then(function (response) {
    return response || fetchEntry(entry);
  }).then(function (response) {
    return Promise.all([
      cache.put(entry.url, response.clone()),
      revisions.put(entry.url, new Response(entry.revision, {
        headers: { "content-type": "text/plain; charset=utf-8" }
      }))
    ]);
  });
}

function populate(cache, revisions, revisionCaches, entries, strict) {
  var chunk = entries.slice(0, 12);
  if (!chunk.length) return Promise.resolve();
  return Promise.all(chunk.map(function (entry) {
    var stored = storeEntry(cache, revisions, revisionCaches, entry);
    return strict ? stored : stored.catch(function () { return null; });
  })).then(function () {
    return populate(cache, revisions, revisionCaches, entries.slice(12), strict);
  });
}

self.addEventListener("install", function (event) {
  event.waitUntil(
    Promise.all([caches.delete(PRECACHE), caches.delete(REVISIONS)])
      .then(function () { return caches.keys(); })
      .then(function (names) {
        var revisionCaches = names.filter(function (name) {
          return name.indexOf(REVISIONS_PREFIX) === 0 && name !== REVISIONS;
        });
        return Promise.all([caches.open(PRECACHE), caches.open(REVISIONS)]).then(function (opened) {
          return populate(opened[0], opened[1], revisionCaches, REQUIRED_URLS, true)
            .then(function () {
              return populate(opened[0], opened[1], revisionCaches, OPTIONAL_URLS, false);
            });
        });
      })
      .then(function () { return self.skipWaiting(); })
      .catch(function (error) {
        return Promise.all([caches.delete(PRECACHE), caches.delete(REVISIONS)]).then(function () {
          throw error;
        });
      })
  );
});

function isAppAsset(request) {
  var url = new URL(request.url);
  return url.origin === self.location.origin && url.pathname.indexOf(BASE + "assets/") === 0;
}

function trimCache(name, maximum) {
  return caches.open(name).then(function (cache) {
    return cache.keys().then(function (keys) {
      return Promise.all(keys.slice(0, Math.max(0, keys.length - maximum)).map(function (key) {
        return cache.delete(key);
      }));
    });
  });
}

// Runtime-cached HTML can reference an older content-hashed stylesheet or script. Copy those
// small app assets out of retiring precaches before deleting them.
function migratePrecacheAssets(names) {
  var oldPrecaches = names.filter(function (name) {
    return name.indexOf(PRECACHE_PREFIX) === 0 && name !== PRECACHE;
  });
  return caches.open(ASSETS).then(function (destination) {
    return Promise.all(oldPrecaches.map(function (name) {
      return caches.open(name).then(function (source) {
        return source.keys().then(function (keys) {
          return Promise.all(keys.filter(isAppAsset).map(function (request) {
            return source.match(request).then(function (response) {
              return response ? destination.put(request, response).catch(function () { return null; }) : null;
            }).catch(function () { return null; });
          }));
        });
      }).catch(function () { return null; });
    }));
  }).then(function () { return trimCache(ASSETS, ASSET_MAX); })
    .catch(function () { return null; });
}

function runtimeCacheName(request) {
  var url = new URL(request.url);
  if (url.origin !== self.location.origin) return IMAGES;
  if (url.pathname.indexOf(BASE + "assets/") === 0) return ASSETS;
  if (url.pathname.indexOf(BASE + "pagefind/") === 0) return SEARCH;
  return PAGES;
}

// Preserve the useful entries written by workers released before the split-cache layout, then
// remove the legacy cache so it cannot shadow a newer precache or live forever.
function migrateLegacyRuntime(names) {
  if (names.indexOf(LEGACY_RUNTIME) === -1) return Promise.resolve();
  return caches.open(LEGACY_RUNTIME).then(function (source) {
    return source.keys().then(function (keys) {
      return Promise.all(keys.map(function (request) {
        return source.match(request).then(function (response) {
          if (!response) return null;
          return caches.open(runtimeCacheName(request)).then(function (destination) {
            return destination.put(request, response);
          });
        }).catch(function () { return null; });
      }));
    });
  }).then(function () {
    return Promise.all([
      trimCache(PAGES, PAGE_MAX),
      trimCache(ASSETS, ASSET_MAX),
      trimCache(SEARCH, SEARCH_MAX),
      trimCache(IMAGES, IMAGE_MAX)
    ]);
  }).catch(function () { return null; });
}

self.addEventListener("activate", function (event) {
  event.waitUntil(
    caches.keys().then(function (names) {
      return migratePrecacheAssets(names).then(function () {
        return migrateLegacyRuntime(names);
      }).then(function () {
        return Promise.all(names.filter(function (name) {
          var oldPrecache = name.indexOf(PRECACHE_PREFIX) === 0 && name !== PRECACHE;
          var oldRevisions = name.indexOf(REVISIONS_PREFIX) === 0 && name !== REVISIONS;
          return oldPrecache || oldRevisions || name === LEGACY_RUNTIME;
        }).map(function (name) {
          return caches.delete(name).catch(function () { return false; });
        }));
      });
    }).then(function () {
      if (self.registration.navigationPreload) {
        return Promise.resolve().then(function () {
          return self.registration.navigationPreload.enable();
        }).catch(function () { return null; });
      }
    }).then(function () { return self.clients.claim(); })
  );
});

function timeout(ms) {
  return new Promise(function (_, reject) {
    setTimeout(function () { reject(new Error("timeout")); }, ms);
  });
}

function remember(name, maximum, request, response) {
  if (!response || (!response.ok && response.type !== "opaque")) return Promise.resolve(response);
  var copy = response.clone();
  return caches.open(name).then(function (cache) {
    return cache.delete(request).then(function () { return cache.put(request, copy); })
      .then(function () { return cache.keys(); })
      .then(function (keys) {
        return Promise.all(keys.slice(0, Math.max(0, keys.length - maximum)).map(function (key) {
          return cache.delete(key);
        }));
      }).then(function () { return response; });
  }).catch(function () { return response; });
}

function firstCached(request, choices, index) {
  if (index >= choices.length) return Promise.resolve(null);
  var choice = choices[index];
  return caches.open(choice.name).then(function (cache) {
    return cache.match(request, { ignoreSearch: !!choice.ignoreSearch });
  }).then(function (response) {
    return response || firstCached(request, choices, index + 1);
  }).catch(function () { return firstCached(request, choices, index + 1); });
}

// A late successful response still refreshes the page cache after the timeout has returned a
// saved copy to the user. Navigation preload rejection falls back to a normal network request.
function networkFirst(request, preload) {
  var network = Promise.resolve(preload).catch(function () { return null; })
    .then(function (response) { return response || fetch(request); })
    .then(function (response) {
      if (response && response.status >= 500) throw new Error("server error");
      return remember(PAGES, PAGE_MAX, request, response);
    });
  return Promise.race([network, timeout(NETWORK_TIMEOUT)]).catch(function () {
    return firstCached(request, [
      { name: PRECACHE, ignoreSearch: true },
      { name: PAGES, ignoreSearch: true }
    ], 0).then(function (cached) {
      if (cached) return cached;
      if (request.mode === "navigate") {
        return firstCached(OFFLINE, [{ name: PRECACHE }, { name: PAGES }], 0);
      }
      return Response.error();
    });
  });
}

function cacheFirst(request, name, maximum, ignorePrecacheSearch) {
  return firstCached(request, [
    { name: PRECACHE, ignoreSearch: !!ignorePrecacheSearch },
    { name: name }
  ], 0).then(function (cached) {
    if (cached) return remember(name, maximum, request, cached);
    return fetch(request).then(function (response) {
      return remember(name, maximum, request, response);
    });
  });
}

self.addEventListener("fetch", function (event) {
  var request = event.request;
  if (request.method !== "GET") return;
  var url = new URL(request.url);
  if (url.origin !== self.location.origin) {
    if (request.destination === "image") event.respondWith(cacheFirst(request, IMAGES, IMAGE_MAX, false));
    return;
  }
  if (url.pathname.indexOf(BASE) !== 0) return;
  var acceptsHtml = (request.headers.get("accept") || "").indexOf("text/html") !== -1;
  var isSwup = (request.headers.get("x-requested-with") || "").toLowerCase() === "swup";
  var mutable = /\/(?:atom|rss|feed)\.xml$|\/(?:feed|aggr|linkset)\.json$|\/manifest\.webmanifest$|\/opensearch\.xml$|\/sitemap(?:-\d+)?\.xml$|\/robots\.txt$|\/(?:aggr\.toml|llms\.txt)$/.test(url.pathname);
  if (request.mode === "navigate" || acceptsHtml || isSwup || mutable) {
    event.respondWith(networkFirst(request, event.preloadResponse));
  } else if (url.pathname.indexOf(BASE + "assets/") === 0) {
    event.respondWith(cacheFirst(request, ASSETS, ASSET_MAX, false));
  } else if (url.pathname.indexOf(BASE + "pagefind/") === 0) {
    event.respondWith(cacheFirst(request, SEARCH, SEARCH_MAX, true));
  } else {
    event.respondWith(cacheFirst(request, PAGES, PAGE_MAX, false));
  }
});
