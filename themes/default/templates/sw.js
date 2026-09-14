// aggr service worker, rendered at build time. Revisioned precaching makes deployment updates
// transactional and reuses unchanged files. Separate bounded caches keep readable pages and the
// assets they reference available across builds without letting remote images evict them.
"use strict";

var VERSION = {{ version | json }};
var APP_VERSION = {{ app_version | json }};
var CONTENT_VERSION = {{ content_version | json }};
var SCOPE_KEY = encodeURIComponent(new URL(self.registration.scope).pathname);
var CACHE_NAMESPACE = "aggr:" + SCOPE_KEY + ":";
var PRECACHE_PREFIX = CACHE_NAMESPACE + "precache-";
var REVISIONS_PREFIX = CACHE_NAMESPACE + "revisions-";
var PRECACHE = PRECACHE_PREFIX + VERSION;
var REVISIONS = CACHE_NAMESPACE + "revisions-" + VERSION;
var PAGES = CACHE_NAMESPACE + "pages";
var ASSETS = CACHE_NAMESPACE + "assets";
var SEARCH_PREFIX = CACHE_NAMESPACE + "search-";
var SEARCH = SEARCH_PREFIX + VERSION;
var IMAGES = CACHE_NAMESPACE + "images";
var PAGE_MAX = 500;
var ASSET_MAX = 128;
var SEARCH_MAX = 512;
var IMAGE_MAX = 128;
var BASE = new URL("./", self.registration.scope).pathname;
var OFFLINE = BASE + "offline.html";
var NETWORK_TIMEOUT = 4000;
var PRECACHE_TIMEOUT = 10000;
var ENTRIES = {{ precache | json }}.map(function (entry) {
  return {
    url: new URL(entry.url, self.registration.scope).pathname,
    revision: entry.revision,
    required: entry.required
  };
});
var REQUIRED_URLS = ENTRIES.filter(function (entry) { return entry.required; });
var OPTIONAL_URLS = ENTRIES.filter(function (entry) { return !entry.required; });

var OFFLINE_PAGES = CACHE_NAMESPACE + "offline-articles";
var OFFLINE_REVISIONS = CACHE_NAMESPACE + "offline-revisions";
var OFFLINE_SETTINGS = CACHE_NAMESPACE + "offline-settings";
var OFFLINE_COUNT = {{ offline_count | json }};
var OFFLINE_CATALOG = {{ offline_catalog | json }};
var offlineQueue = Promise.resolve();
var offlineStatus = null;
var offlineGeneration = 0;
var offlineAbort = null;
var offlinePendingCount = null;
var SEARCH_MANIFEST = {{ search_manifest | json }};
var OFFLINE_SEARCH_PREFIX = CACHE_NAMESPACE + "offline-search-";
var activeSearch = null;
var searchEnabled = false;
var searchTask = null;
var searchAbort = null;
var searchGeneration = 0;
var searchStatus = { phase: "disabled", activeVersion: null, targetVersion: null, base: null, downloadedFiles: 0, totalFiles: 0, downloadedBytes: 0, totalBytes: 0, error: null };
var configurationCount = null;
var configurationTask = null;
var configurationGeneration = 0;
var settingsQueue = Promise.resolve();

function validSearchManifest(manifest) {
  if (!manifest || !/^[a-f0-9]{64}$/.test(manifest.version) || manifest.base !== "pagefind/" + manifest.version + "/" || !Array.isArray(manifest.files) || !manifest.files.length) return false;
  var urls = new Set(), total = 0;
  var valid = manifest.files.every(function (file) {
    if (!file || typeof file.url !== "string" || !file.url.startsWith(manifest.base) || !Number.isSafeInteger(file.size) || file.size < 0 || !/^[a-f0-9]{64}$/.test(file.digest) || urls.has(file.url)) return false;
    var url = new URL(file.url, self.registration.scope);
    if (url.href !== self.registration.scope + file.url || url.search || url.hash) return false;
    urls.add(file.url); total += file.size;
    return Number.isSafeInteger(total);
  });
  return valid && total === manifest.totalBytes && urls.has(manifest.base + "pagefind.js");
}

async function completeSearch(manifest) {
  if (!validSearchManifest(manifest)) return false;
  var name = OFFLINE_SEARCH_PREFIX + manifest.version;
  if (!(await caches.keys()).includes(name)) return false;
  var cache = await caches.open(name);
  var urls = new Set((await cache.keys()).map(function (request) { return request.url; }));
  return manifest.files.every(function (file) { return urls.has(offlineUrl(file.url)); });
}

async function restoreSearch() {
  var generation = searchGeneration;
  if (configurationCount === 0) return null;
  var settings = await caches.open(OFFLINE_SETTINGS);
  var response = await settings.match(offlineUrl("__offline_search"));
  var manifest = response ? await response.json().catch(function () { return null; }) : null;
  var restored = await completeSearch(manifest) ? manifest : null;
  if (generation !== searchGeneration) return activeSearch;
  activeSearch = restored;
  searchStatus.activeVersion = activeSearch ? activeSearch.version : null;
  searchStatus.base = activeSearch ? activeSearch.base : null;
  if (manifest && !activeSearch) { searchStatus.phase = "error"; searchStatus.error = "evicted"; }
  return activeSearch;
}

function publishSearchStatus() {
  return broadcastOfflineStatus(offlineStatus || {type: "AGGR_OFFLINE_STATUS", requested: configurationCount || 0, total: 0, saved: [], failed: 0, downloading: false});
}

async function verifiedSearchResponse(response, file) {
  if (!response || !response.ok) throw new Error("network");
  var reader = response.body.getReader(), chunks = [], length = 0;
  try {
    while (true) {
      var next = await withTimeout(reader.read(), PRECACHE_TIMEOUT);
      if (next.done) break;
      length += next.value.byteLength;
      if (length > file.size) throw new Error("integrity");
      chunks.push(next.value);
    }
  } catch (error) { await reader.cancel().catch(function () {}); throw error; }
  if (length !== file.size) throw new Error("integrity");
  var bytes = new Uint8Array(length), offset = 0;
  chunks.forEach(function (chunk) { bytes.set(chunk, offset); offset += chunk.byteLength; });
  var digest = Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes))).map(function (byte) { return byte.toString(16).padStart(2, "0"); }).join("");
  if (digest !== file.digest) throw new Error("integrity");
  return new Response(bytes, {headers: response.headers});
}

async function downloadSearch(generation, signal) {
  function superseded() { return generation !== searchGeneration || signal.aborted; }
  var manifest, cache;
  try {
    await restoreSearch();
    if (superseded()) return;
    searchStatus = {phase: activeSearch ? "updating" : "downloading", activeVersion: activeSearch ? activeSearch.version : null, targetVersion: SEARCH_MANIFEST.version, base: activeSearch ? activeSearch.base : null, downloadedFiles: 0, totalFiles: 0, downloadedBytes: 0, totalBytes: 0, error: null};
    if (activeSearch && activeSearch.version === SEARCH_MANIFEST.version) {
      searchStatus.phase = "ready";
      searchStatus.downloadedFiles = searchStatus.totalFiles = activeSearch.files.length;
      searchStatus.downloadedBytes = searchStatus.totalBytes = activeSearch.totalBytes;
      return;
    }
    // During an update retain only the usable index and its replacement, bounding peak storage.
    var keepVersions = new Set([OFFLINE_SEARCH_PREFIX + SEARCH_MANIFEST.version]);
    if (activeSearch) keepVersions.add(OFFLINE_SEARCH_PREFIX + activeSearch.version);
    await Promise.all((await caches.keys()).filter(function (name) { return name.startsWith(OFFLINE_SEARCH_PREFIX) && !keepVersions.has(name); }).map(function (name) { return caches.delete(name); }));
    cache = await caches.open(OFFLINE_SEARCH_PREFIX + SEARCH_MANIFEST.version);
    var manifestUrl = offlineUrl(SEARCH_MANIFEST.base + "search-manifest.json");
    var response = await cache.match(manifestUrl);
    if (!response) response = await fetchEntry({url: manifestUrl}, signal);
    manifest = await response.json();
    if (!validSearchManifest(manifest) || manifest.version !== SEARCH_MANIFEST.version) throw new Error("integrity");
    await cache.put(manifestUrl, new Response(JSON.stringify(manifest), {headers: {"content-type": "application/json"}}));
    searchStatus.totalFiles = manifest.files.length;
    searchStatus.totalBytes = manifest.totalBytes;
    var previousVersion = activeSearch && activeSearch.version;
    var previous = activeSearch && await caches.open(OFFLINE_SEARCH_PREFIX + activeSearch.version);
    var reusable = new Map(activeSearch ? activeSearch.files.map(function (file) { return [file.digest + ":" + file.size, file]; }) : []);
    var cursor = 0, failure;
    async function download() {
      while (cursor < manifest.files.length && !superseded() && !failure) {
        var file = manifest.files[cursor++], url = offlineUrl(file.url);
        try {
          var retained = await cache.match(url);
          if (retained) {
            try { retained = await verifiedSearchResponse(retained, file); }
            catch (_) { await cache.delete(url); retained = null; }
          }
          if (!retained) {
            var same = reusable.get(file.digest + ":" + file.size);
            var candidate = same && await previous.match(offlineUrl(same.url));
            if (candidate) {
              try { candidate = await verifiedSearchResponse(candidate, file); }
              catch (_) { candidate = null; }
            }
            retained = candidate || await verifiedSearchResponse(await fetchEntry({url: url}, signal), file);
            if (superseded()) return;
            await cache.put(url, retained);
          }
          searchStatus.downloadedFiles += 1;
          searchStatus.downloadedBytes += file.size;
          await publishSearchStatus();
        } catch (error) { failure = error; }
      }
    }
    await Promise.all([download(), download()]);
    if (superseded()) return;
    if (failure) throw failure;
    if (!await completeSearch(manifest)) throw new Error("evicted");
    var settings = await caches.open(OFFLINE_SETTINGS);
    await settings.put(offlineUrl("__offline_search"), new Response(JSON.stringify(manifest), {headers: {"content-type": "application/json"}}));
    activeSearch = manifest;
    searchStatus.activeVersion = manifest.version;
    searchStatus.base = manifest.base;
    searchStatus.phase = "ready";
    // Keep one complete predecessor for tabs whose Pagefind instance is still using it.
    var keep = new Set([OFFLINE_SEARCH_PREFIX + manifest.version]);
    if (previousVersion) keep.add(OFFLINE_SEARCH_PREFIX + previousVersion);
    var names = await caches.keys();
    await Promise.all(names.filter(function (name) { return name.startsWith(OFFLINE_SEARCH_PREFIX) && !keep.has(name); }).map(function (name) { return caches.delete(name); }));
  } catch (error) {
    if (superseded()) return;
    var quota = error && (error.name === "QuotaExceededError" || /quota/i.test(error.message));
    searchStatus.phase = quota ? "blocked" : "error";
    searchStatus.error = quota ? "quota" : error && /^(integrity|evicted)$/.test(error.message) ? error.message : "network";
    if (quota) await caches.delete(OFFLINE_SEARCH_PREFIX + SEARCH_MANIFEST.version).catch(function () {});
  } finally { if (!superseded()) await publishSearchStatus(); }
}

function configureOfflineSearch(enabled) {
  if (searchEnabled === enabled && searchTask) return searchTask;
  searchEnabled = enabled;
  var generation = ++searchGeneration;
  if (!enabled) { activeSearch = null; searchStatus = disabledSearchStatus(); }
  if (searchAbort) searchAbort.abort();
  var previous = searchTask;
  searchTask = Promise.resolve(previous).catch(function () {}).then(async function () {
    if (generation !== searchGeneration) return;
    if (enabled) {
      searchAbort = new AbortController();
      await downloadSearch(generation, searchAbort.signal);
    } else {
      var names = await caches.keys();
      await Promise.all(names.filter(function (name) { return name.startsWith(OFFLINE_SEARCH_PREFIX); }).map(function (name) { return caches.delete(name); }));
      await (await caches.open(OFFLINE_SETTINGS)).delete(offlineUrl("__offline_search"));
      activeSearch = null;
      searchStatus = disabledSearchStatus();
      await publishSearchStatus();
    }
  }).finally(function () { if (generation === searchGeneration) { searchTask = null; searchAbort = null; } });
  return searchTask;
}

function disabledSearchStatus() {
  return {phase: "disabled", activeVersion: null, targetVersion: null, base: null, downloadedFiles: 0, totalFiles: 0, downloadedBytes: 0, totalBytes: 0, error: null};
}

async function readOfflineStatus() {
  var generation = configurationGeneration;
  var settings = await caches.open(OFFLINE_SETTINGS);
  var configured = await settings.match(offlineUrl("__offline_count"));
  var persisted = configured ? Number(await configured.text()) : OFFLINE_COUNT;
  var count = configurationCount === null ? persisted : configurationCount;
  if (!Number.isInteger(count) || count < 0 || count > 1000) count = 0;
  var response = await settings.match(offlineUrl("__offline_articles"));
  var saved = response ? await response.json().catch(function () { return []; }) : [];
  var pages = await caches.open(OFFLINE_PAGES), revisions = await caches.open(OFFLINE_REVISIONS);
  var complete = [];
  if (Array.isArray(saved)) for (var item of saved.slice(0, count)) {
    if (!item || !Array.isArray(item.resources) || !item.resources.length) continue;
    var valid = await Promise.all(item.resources.map(async function (entry) {
      if (!entry || !offlineUrl(entry.url).startsWith(self.registration.scope)) return false;
      var page = await pages.match(offlineUrl(entry.url)), revision = await revisions.match(offlineUrl(entry.url));
      return !!page && !!revision && await revision.text() === entry.revision;
    }));
    if (valid.every(Boolean)) complete.push({url: item.url, title: item.title});
  }
  await restoreSearch();
  if (generation !== configurationGeneration) return readOfflineStatus();
  var search = count ? Object.assign({}, searchStatus) : disabledSearchStatus();
  if (count && !searchTask) {
    search.phase = activeSearch && activeSearch.version === SEARCH_MANIFEST.version ? "ready" : search.error === "quota" ? "blocked" : search.error ? "error" : activeSearch ? "updating" : "downloading";
    search.targetVersion = SEARCH_MANIFEST.version;
    if (activeSearch && activeSearch.version === SEARCH_MANIFEST.version) {
      search.downloadedFiles = search.totalFiles = activeSearch.files.length;
      search.downloadedBytes = search.totalBytes = activeSearch.totalBytes;
    }
  }
  return {type: "AGGR_OFFLINE_STATUS", requested: count, total: Math.min(count, OFFLINE_CATALOG.length), saved: complete, failed: 0, downloading: offlinePendingCount !== null, search: search};
}

function offlineUrl(path) {
  return new URL(path, self.registration.scope).href;
}

function saveOfflineArticles(count, generation, signal) {
  function superseded() { return generation !== undefined && generation !== offlineGeneration; }
  var selected = OFFLINE_CATALOG.slice(0, count);
  var result = { type: "AGGR_OFFLINE_STATUS", requested: count, total: selected.length, saved: [], failed: 0, downloading: true };
  var wanted = new Set();
  var resources = new Map();
  var next = 0;
  var complete = [];
  selected.forEach(function (item) {
    item.resources.forEach(function (entry) { wanted.add(offlineUrl(entry.url)); });
  });
  return Promise.all([caches.open(OFFLINE_PAGES), caches.open(OFFLINE_REVISIONS)]).then(function (opened) {
    var pages = opened[0], revisions = opened[1];
    function saveResource(entry) {
      var url = offlineUrl(entry.url);
      if (resources.has(url)) return resources.get(url);
      var pending = Promise.all([pages.match(url), revisions.match(url)]).then(function (cached) {
        return cached[0] && cached[1] ? cached[1].text().then(function (revision) {
          return revision === entry.revision ? cached[0] : null;
        }) : null;
      }).then(function (cached) {
        if (cached) return;
        return fetchEntry({ url: url }, signal).then(function (response) {
          // Record a revision only after its body is safely written.
          return pages.put(url, response).then(function () {
            return revisions.put(url, new Response(entry.revision));
          });
        });
      });
      resources.set(url, pending);
      return pending;
    }
    function saveNext() {
      if (next >= selected.length || superseded()) return Promise.resolve();
      var item = selected[next++];
      // Each slot downloads one resource at a time; shared images share their in-flight promise.
      var ordered = item.resources.filter(function (entry) { return entry.url !== item.url; })
        .concat(item.resources.filter(function (entry) { return entry.url === item.url; }));
      return ordered.reduce(function (chain, entry) {
        return chain.then(function () { return saveResource(entry); });
      }, Promise.resolve()).then(function () {
        complete.push(item);
        result.saved.push({url: item.url, title: item.title});
      }).catch(function () { result.failed += 1; }).then(function () {
        if (!superseded()) {
          offlineStatus = result;
          broadcastOfflineStatus(result);
        }
        return saveNext();
      });
    }
    return Promise.all(Array.from({length: Math.min(6, selected.length)}, saveNext)).then(function () {
      if (superseded()) return;
      return caches.open(OFFLINE_SETTINGS).then(function (settings) {
        return settings.match(offlineUrl("__offline_articles")).then(function (response) {
          return response ? response.json() : [];
        }).catch(function () { return []; }).then(function (previous) {
          if (!result.failed || !Array.isArray(previous)) return;
          // Keep older complete downloads within N until failed newer replacements are available.
          return previous.slice(0, 1000).reduce(function (chain, item) {
            return chain.then(function () {
              if (complete.length >= count || complete.some(function (entry) { return entry.url === item.url; }) || !Array.isArray(item.resources)) return;
              return Promise.all(item.resources.map(function (entry) {
                var url = offlineUrl(entry.url);
                if (!url.startsWith(self.registration.scope)) return false;
                return Promise.all([pages.match(url), revisions.match(url)]).then(function (cached) {
                  return cached[0] && cached[1] ? cached[1].text().then(function (revision) { return revision === entry.revision; }) : false;
                });
              })).then(function (valid) {
                if (!valid.length || !valid.every(Boolean)) return;
                complete.push(item);
                result.saved.push({url: item.url, title: item.title});
                item.resources.forEach(function (entry) { wanted.add(offlineUrl(entry.url)); });
              });
            });
          }, Promise.resolve());
        }).then(function () {
          if (superseded()) return;
          return settings.put(offlineUrl("__offline_articles"), new Response(JSON.stringify(complete))).catch(function () {});
        });
      });
    }).then(function () {
      if (superseded()) return;
      return Promise.all(opened.map(function (cache) {
        return cache.keys().then(function (keys) {
          return Promise.all(keys.filter(function (key) { return !wanted.has(key.url); }).map(function (key) { return cache.delete(key); }));
        });
      }));
    });
  }).catch(function () { result.failed = Math.max(0, selected.length - result.saved.length); }).then(function () {
    if (superseded()) return;
    // A refreshed protected page must not be shadowed by a visited copy from an older build.
    var saved = new Set(result.saved.map(function (item) { return offlineUrl(item.url); }));
    return caches.open(PAGES).then(function (cache) {
      return cache.keys().then(function (keys) {
        return Promise.all(keys.filter(function (key) {
          var url = new URL(key.url); url.search = "";
          return saved.has(url.href);
        }).map(function (key) { return cache.delete(key); }));
      });
    }).catch(function () {});
  }).then(function () {
    if (superseded()) { result.cancelled = true; return result; }
    var order = new Map(OFFLINE_CATALOG.map(function (item, index) { return [item.url, index]; }));
    result.saved.sort(function (a, b) { return order.get(a.url) - order.get(b.url); });
    result.total = Math.max(result.total, result.saved.length);
    result.downloading = false;
    offlineStatus = result;
    broadcastOfflineStatus(result);
    return result;
  });
}

function broadcastOfflineStatus(status) {
  var generation = configurationGeneration;
  if (!self.clients.matchAll) return Promise.resolve();
  return self.clients.matchAll({type: "window"}).then(function (clients) {
    if (generation !== configurationGeneration || (configurationCount !== null && status.requested !== configurationCount)) return;
    var snapshot = Object.assign({}, status, {search: Object.assign({}, searchStatus)});
    clients.forEach(function (client) { client.postMessage(snapshot); });
  }).catch(function () {});
}

function configureOfflineArticles(count) {
  if (offlinePendingCount === count) return offlineQueue;
  var generation = ++offlineGeneration;
  offlinePendingCount = count;
  if (offlineAbort) offlineAbort.abort();
  offlineQueue = offlineQueue.catch(function () {}).then(function () {
    if (generation !== offlineGeneration) return;
    offlineAbort = new AbortController();
    return saveOfflineArticles(count, generation, offlineAbort.signal).finally(function () {
      if (generation === offlineGeneration) { offlinePendingCount = null; offlineAbort = null; }
    });
  });
  return offlineQueue;
}

function configureOffline(count) {
  if (configurationCount === count && configurationTask) return configurationTask;
  configurationCount = count;
  var generation = ++configurationGeneration;
  var articles = configureOfflineArticles(count);
  var search = configureOfflineSearch(count > 0);
  settingsQueue = settingsQueue.catch(function () {}).then(async function () {
    await (await caches.open(OFFLINE_SETTINGS)).put(offlineUrl("__offline_count"), new Response(String(count)));
  }).catch(function () {});
  configurationTask = Promise.all([articles, search, settingsQueue]).then(function (results) { return results[0]; })
    .finally(function () { if (generation === configurationGeneration) configurationTask = null; });
  return configurationTask;
}

self.addEventListener("message", function (event) {
  var data = event.data;
  if (data && data.type === "AGGR_OFFLINE_GET_STATUS" && event.source && event.source.url && event.source.url.startsWith(self.registration.scope)) {
    event.waitUntil(readOfflineStatus().then(function (status) { event.source.postMessage(status); }));
    return;
  }
  if (data && data.type === "AGGR_GET_BUILD" && event.source && event.source.url && event.source.url.startsWith(self.registration.scope)) {
    event.source.postMessage({type: "AGGR_BUILD", app_version: APP_VERSION, content_version: CONTENT_VERSION});
    return;
  }
  if (!data || data.type !== "AGGR_OFFLINE_CONFIG" || !Number.isInteger(data.count) || data.count < 0 || data.count > 1000) return;
  if (!event.source || !event.source.url || !event.source.url.startsWith(self.registration.scope)) return;
  event.waitUntil(configureOffline(data.count));
});

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

function fetchEntry(entry, cancellation) {
  var controller = new AbortController();
  var timer = setTimeout(function () { controller.abort(); }, PRECACHE_TIMEOUT);
  var cancel = function () { controller.abort(); };
  if (cancellation) {
    if (cancellation.aborted) cancel();
    else cancellation.addEventListener("abort", cancel, {once: true});
  }
  return fetch(new Request(entry.url, { cache: "reload" }), { signal: controller.signal, priority: "low" }).then(function (response) {
    if (!response || !response.ok) throw new Error("could not precache " + entry.url);
    return response;
  }).finally(function () {
    clearTimeout(timer);
    if (cancellation) cancellation.removeEventListener("abort", cancel);
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
  var next = 0;
  var failure = null;
  function download() {
    if (next >= entries.length || failure) return Promise.resolve();
    return storeEntry(cache, revisions, revisionCaches, entries[next++])
      .catch(function (error) { if (strict) failure = error; })
      .then(download);
  }
  // Keep each slot busy independently; a slow image should not stall eleven completed downloads.
  // Settle in-flight writes before rejecting so failed-install cleanup cannot race those writes.
  return Promise.all(Array.from({ length: Math.min(entries.length, 12) }, download)).then(function () {
    if (failure) throw failure;
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

function isImageAsset(url) {
  return url.origin === self.location.origin &&
    (url.pathname.indexOf(BASE + "assets/previews/") === 0 ||
      url.pathname.indexOf(BASE + "assets/images/") === 0);
}

function isAppAsset(request) {
  var url = new URL(request.url);
  return url.origin === self.location.origin && url.pathname.indexOf(BASE + "assets/") === 0 &&
    !isImageAsset(url);
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

function invalidateOlderPages() {
  var replaced = new Set(ENTRIES.map(function (entry) { return entry.url; }));
  return Promise.all([caches.open(PAGES), caches.open(PRECACHE)]).then(function (opened) {
    var cache = opened[0];
    var installed = opened[1];
    return cache.keys().then(function (keys) {
      return Promise.all(keys.filter(function (request) {
        return replaced.has(new URL(request.url).pathname);
      }).map(function (request) {
        return installed.match(request, { ignoreSearch: true }).then(function (replacement) {
          return replacement && replacement.ok ? cache.delete(request) : false;
        });
      }));
    });
  }).catch(function () { return null; });
}

function removeUnverifiableImages() {
  return caches.open(IMAGES).then(function (cache) {
    return cache.keys().then(function (keys) {
      return Promise.all(keys.map(function (request) {
        return cache.match(request).then(function (response) {
          if (!response || !response.ok || new URL(request.url).origin !== self.location.origin) {
            return cache.delete(request);
          }
        });
      }));
    });
  }).catch(function () { return null; });
}

self.addEventListener("activate", function (event) {
  event.waitUntil(
    caches.keys().then(function (names) {
      return migratePrecacheAssets(names).then(function () {
        return invalidateOlderPages();
      }).then(function () {
        return removeUnverifiableImages();
      }).then(function () {
        var retained = new Set([PRECACHE, REVISIONS, PAGES, ASSETS, SEARCH, IMAGES,
          OFFLINE_PAGES, OFFLINE_REVISIONS, OFFLINE_SETTINGS]);
        return Promise.all(names.filter(function (name) {
          return name.startsWith(CACHE_NAMESPACE) && !retained.has(name) && !name.startsWith(OFFLINE_SEARCH_PREFIX);
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

function withTimeout(promise, ms) {
  return new Promise(function (resolve, reject) {
    var timer = setTimeout(function () { reject(new Error("timeout")); }, ms);
    promise.then(resolve, reject).finally(function () { clearTimeout(timer); });
  });
}

function remember(name, maximum, request, response) {
  if (!response || !response.ok) return Promise.resolve(response);
  var copy = response.clone();
  return caches.open(name).then(function (cache) {
    return cache.put(request, copy)
      .then(function () { return cache.keys(); })
      .then(function (keys) {
        return Promise.all(keys.slice(0, Math.max(0, keys.length - maximum)).map(function (key) {
          return cache.delete(key);
        }));
      }).then(function () { return response; });
  }).catch(function () { return response; });
}

async function searchManifestResponse(request, event, compact) {
  try {
    var response = await withTimeout(fetch(request), NETWORK_TIMEOUT);
    if (!response || !response.ok) throw new Error("search manifest unavailable");
    event.waitUntil(remember(PAGES, PAGE_MAX, request, response));
    return response;
  } catch (_) {
    await restoreSearch();
    if (activeSearch) {
      var selected = compact ? {version:activeSearch.version,base:activeSearch.base,docs:activeSearch.docs,facets:activeSearch.facets} : activeSearch;
      return new Response(JSON.stringify(selected), {headers: {"content-type": "application/json"}});
    }
    return await firstCached(request, [{name: PAGES}, {name: PRECACHE}], 0) || new Response("Offline search is not downloaded", {status: 503});
  }
}

async function protectedSearchResponse(request, event) {
  var path = new URL(request.url).pathname.slice(BASE.length);
  var match = /^pagefind\/([a-f0-9]{64})\//.exec(path);
  if (match) {
    var name = OFFLINE_SEARCH_PREFIX + match[1];
    if ((await caches.keys()).includes(name)) {
      var cache = await caches.open(name);
      var response = await cache.match(request, {ignoreSearch: true});
      if (response) return response;
      if (activeSearch && activeSearch.version === match[1]) {
        activeSearch = null;
        searchStatus.activeVersion = searchStatus.base = null;
        searchStatus.phase = "error"; searchStatus.error = "evicted";
        event.waitUntil(publishSearchStatus());
      }
    }
  }
  return cacheFirst(request, SEARCH, SEARCH_MAX, true, event);
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
function refreshPage(request, preload, event) {
  var network = Promise.resolve(preload).catch(function () { return null; })
    .then(function (response) { return response || fetch(request); })
    .then(function (response) {
      if (response && response.status >= 500) throw new Error("server error");
      return response;
    });
  var saved = network.then(function (response) {
    return remember(PAGES, PAGE_MAX, request, response);
  }).catch(function () { return null; });
  event.waitUntil(saved);
  return network;
}

function pageFallback(request, network) {
  return withTimeout(network, NETWORK_TIMEOUT).catch(function () {
    return firstCached(request, [
      { name: PAGES, ignoreSearch: true },
      { name: PRECACHE, ignoreSearch: true },
      { name: OFFLINE_PAGES, ignoreSearch: true }
    ], 0).then(function (cached) {
      if (cached) return cachedPageResponse(cached, request);
      if (request.mode === "navigate") {
        return firstCached(OFFLINE, [{ name: PRECACHE }, { name: PAGES }], 0);
      }
      return Response.error();
    });
  });
}

function networkFirst(request, preload, event) {
  return pageFallback(request, refreshPage(request, preload, event));
}

function cachedPageResponse(response, request) {
  if (!response.url || response.url === request.url) return response;
  var cached = new URL(response.url), requested = new URL(request.url);
  if (cached.origin !== requested.origin || cached.pathname !== requested.pathname) return response;
  // An ignoreSearch cache hit must not look like a redirect to Swup and drop the query.
  return new Response(response.body, {status: response.status, statusText: response.statusText, headers: response.headers});
}

// Cached HTML is already readable: refresh it without putting a mobile round trip on the
// navigation path. Activation removes runtime copies replaced by the new deployment.
function pageFirst(request, preload, event) {
  var network = refreshPage(request, preload, event);
  return firstCached(request, [
    { name: PAGES, ignoreSearch: true },
    { name: PRECACHE, ignoreSearch: true },
      { name: OFFLINE_PAGES, ignoreSearch: true }
  ], 0).then(function (cached) {
    return cached ? cachedPageResponse(cached, request) : pageFallback(request, network);
  });
}

function cacheFirst(request, name, maximum, ignorePrecacheSearch, event) {
  var fetched = false;
  var response = firstCached(request, [
    { name: PRECACHE, ignoreSearch: !!ignorePrecacheSearch },
    { name: OFFLINE_PAGES, ignoreSearch: !!ignorePrecacheSearch },
    { name: name }
  ], 0).then(function (cached) {
    if (cached) return cached;
    fetched = true;
    return fetch(request);
  });
  var saved = response.then(function (value) {
    return fetched ? remember(name, maximum, request, value) : null;
  }).catch(function () { return null; });
  event.waitUntil(saved);
  return response;
}

self.addEventListener("fetch", function (event) {
  var request = event.request;
  if (request.method !== "GET") return;
  var url = new URL(request.url);
  if (url.origin !== self.location.origin) return;
  if (url.pathname.indexOf(BASE) !== 0) return;
  var acceptsHtml = (request.headers.get("accept") || "").indexOf("text/html") !== -1;
  var isSwup = (request.headers.get("x-requested-with") || "").toLowerCase() === "swup";
  var mutable = /\/(?:atom|rss|feed)\.xml$|\/(?:feed|aggr|linkset|updates|pagefind-entry)\.json$|\/manifest\.webmanifest$|\/opensearch\.xml$|\/sitemap(?:-\d+)?\.xml$|\/robots\.txt$|\/(?:aggr\.toml|llms\.txt)$/.test(url.pathname);
  var articleRepresentation = url.pathname.indexOf(BASE + "items/") === 0 && /\.(?:md|txt|rst|json)$/.test(url.pathname);
  if (url.pathname === BASE + "search-manifest.json" || url.pathname === BASE + "search-catalog.json") {
    event.respondWith(searchManifestResponse(request, event, url.pathname === BASE + "search-catalog.json"));
  } else if (/^pagefind\/[a-f0-9]{64}\//.test(url.pathname.slice(BASE.length))) {
    event.respondWith(protectedSearchResponse(request, event));
  } else if (mutable || articleRepresentation) {
    event.respondWith(networkFirst(request, event.preloadResponse, event));
  } else if (request.mode === "navigate" || acceptsHtml || isSwup) {
    var reload = request.cache === "reload" || request.cache === "no-cache" || request.cache === "no-store";
    event.respondWith(reload
      ? networkFirst(request, event.preloadResponse, event)
      : pageFirst(request, event.preloadResponse, event));
  } else if (isImageAsset(url)) {
    event.respondWith(cacheFirst(request, IMAGES, IMAGE_MAX, true, event));
  } else if (url.pathname.indexOf(BASE + "assets/") === 0) {
    event.respondWith(cacheFirst(request, ASSETS, ASSET_MAX, true, event));
  } else if (url.pathname.indexOf(BASE + "pagefind/") === 0) {
    event.respondWith(cacheFirst(request, SEARCH, SEARCH_MAX, true, event));
  } else {
    event.respondWith(cacheFirst(request, PAGES, PAGE_MAX, false, event));
  }
});
