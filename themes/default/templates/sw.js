// The reader's service worker: make the shell installable, keep visited pages readable offline,
// and preserve complete selected articles and their images independently of runtime caches.
//
// Rendered from this template by the Rust build, which supplies the version and precache list.
"use strict";

/** @typedef { {url: string, revision: string, required?: boolean} } ResourceEntry */
/** @typedef { {url: string, title: string, resources: ResourceEntry[]} } OfflineItem */
/** @typedef { {url: string, size: number, digest: string} } SearchFile */
/** @typedef { {version: string, base: string, files: SearchFile[], totalBytes: number} } SearchManifest */
/** @typedef { {phase: string, activeVersion: string | null, targetVersion: string | null, base: string | null, downloadedFiles: number, totalFiles: number, downloadedBytes: number, totalBytes: number, error: string | null} } SearchStatus */
/** @typedef { {type: string, requested: number, total: number, saved: Array<{url: string, title: string}>, failed: number, downloading: boolean, search?: SearchStatus, cancelled?: boolean} } OfflineStatus */

var VERSION = {{ version | json }};
var APP_VERSION = {{ build.app_version | json }};
var CONTENT_VERSION = {{ build.content_version | json }};
/** @type {ResourceEntry[]} */
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

var PRECACHE_TIMEOUT = 10000;

/** @template T @param {Promise<T>} promise @param {number} milliseconds @returns {Promise<T>} */
function withTimeout(promise, milliseconds) {
  /** @type {ReturnType<typeof setTimeout> | undefined} */
  var timer;
  /** @type {Promise<never>} */
  var deadline = new Promise(function (_, reject) {
    timer = setTimeout(function () { reject(new Error("network")); }, milliseconds);
  });
  return Promise.race([promise, deadline]).finally(function () { clearTimeout(timer); });
}

/** @param { {url: string} } entry @param {AbortSignal} [signal] */
async function fetchEntry(entry, signal) {
  var controller = new AbortController();
  var abort = function () { controller.abort(); };
  if (signal) {
    if (signal.aborted) controller.abort();
    else signal.addEventListener("abort", abort, {once: true});
  }
  var timer = setTimeout(abort, PRECACHE_TIMEOUT);
  try {
    var response = await fetch(entry.url, {signal: controller.signal, cache: "reload", priority: "low"});
    if (!response.ok || response.type === "opaque") throw new Error("network");
    // Keep the deadline active while reading a stalled response body, not only its headers.
    var bytes = await response.arrayBuffer();
    return new Response(bytes, {status: response.status, headers: response.headers});
  } finally {
    clearTimeout(timer);
    if (signal) signal.removeEventListener("abort", abort);
  }
}

var OFFLINE_PAGES = NAMESPACE + "offline-articles";
var OFFLINE_REVISIONS = NAMESPACE + "offline-revisions";
var OFFLINE_SETTINGS = NAMESPACE + "offline-settings";
var OFFLINE_COUNT = {{ offline_count | json }};
/** @type {OfflineItem[]} */
var OFFLINE_CATALOG = {{ offline_catalog | json }};
/** @type {Promise<OfflineStatus | void>} */
var offlineQueue = Promise.resolve();
/** @type {OfflineStatus | null} */
var offlineStatus = null;
var offlineGeneration = 0;
/** @type {AbortController | null} */
var offlineAbort = null;
/** @type {number | null} */
var offlinePendingCount = null;
var SEARCH_MANIFEST = {{ search_manifest | json }};
var OFFLINE_SEARCH_PREFIX = NAMESPACE + "offline-search-";
/** @type {SearchManifest | null} */
var activeSearch = null;
var searchEnabled = false;
/** @type {Promise<void> | null} */
var searchTask = null;
/** @type {AbortController | null} */
var searchAbort = null;
var searchGeneration = 0;
/** @type {SearchStatus} */
var searchStatus = { phase: "disabled", activeVersion: null, targetVersion: null, base: null, downloadedFiles: 0, totalFiles: 0, downloadedBytes: 0, totalBytes: 0, error: null };
/** @type {number | null} */
var configurationCount = null;
/** @type {Promise<OfflineStatus | void> | null} */
var configurationTask = null;
var configurationGeneration = 0;
var settingsQueue = Promise.resolve();

/** @param {unknown} value @returns {value is Record<string, unknown>} */
function record(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** @param {unknown} value @returns {value is OfflineItem} */
function validOfflineItem(value) {
  return record(value) && typeof value.url === "string" && typeof value.title === "string"
    && Array.isArray(value.resources) && value.resources.length > 0
    && value.resources.every(function (entry) {
      return record(entry) && typeof entry.url === "string" && typeof entry.revision === "string";
    });
}

/** @param {unknown} manifest @returns {manifest is SearchManifest} */
function validSearchManifest(manifest) {
  if (!record(manifest) || typeof manifest.version !== "string" || !/^[a-f0-9]{64}$/.test(manifest.version) || manifest.base !== "pagefind/" + manifest.version + "/" || !Array.isArray(manifest.files) || !manifest.files.length) return false;
  var base = manifest.base;
  /** @type {Set<string>} */
  var urls = new Set(), total = 0;
  var valid = manifest.files.every(function (file) {
    if (!record(file) || typeof file.url !== "string" || !file.url.startsWith(base) || typeof file.size !== "number" || !Number.isSafeInteger(file.size) || file.size < 0 || typeof file.digest !== "string" || !/^[a-f0-9]{64}$/.test(file.digest) || urls.has(file.url)) return false;
    var url = new URL(file.url, self.registration.scope);
    if (url.href !== self.registration.scope + file.url || url.search || url.hash) return false;
    urls.add(file.url); total += file.size;
    return Number.isSafeInteger(total);
  });
  return valid && total === manifest.totalBytes && urls.has(manifest.base + "pagefind.js");
}

/** @param {unknown} manifest */
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
  /** @type {unknown} */
  var manifest = response ? await response.json().catch(function () { return null; }) : null;
  var restored = validSearchManifest(manifest) && await completeSearch(manifest) ? manifest : null;
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

/** @param {Response} response @param {SearchFile} file */
async function verifiedSearchResponse(response, file) {
  if (!response.ok || !response.body) throw new Error("network");
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

/** @param {number} generation @param {AbortSignal} signal */
async function downloadSearch(generation, signal) {
  function superseded() { return generation !== searchGeneration || signal.aborted; }
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
    var cache = await caches.open(OFFLINE_SEARCH_PREFIX + SEARCH_MANIFEST.version);
    var manifestUrl = offlineUrl(SEARCH_MANIFEST.base + "search-manifest.json");
    var response = await cache.match(manifestUrl);
    if (!response) response = await fetchEntry({url: manifestUrl}, signal);
    /** @type {unknown} */
    var decoded = await response.json();
    if (!validSearchManifest(decoded) || decoded.version !== SEARCH_MANIFEST.version) throw new Error("integrity");
    var manifest = decoded;
    await cache.put(manifestUrl, new Response(JSON.stringify(manifest), {headers: {"content-type": "application/json"}}));
    searchStatus.totalFiles = manifest.files.length;
    searchStatus.totalBytes = manifest.totalBytes;
    var previousVersion = activeSearch && activeSearch.version;
    var previous = activeSearch && await caches.open(OFFLINE_SEARCH_PREFIX + activeSearch.version);
    var reusable = new Map(activeSearch ? activeSearch.files.map(function (file) { return [file.digest + ":" + file.size, file]; }) : []);
    var cursor = 0;
    /** @type {unknown} */
    var failure;
    async function download() {
      while (cursor < manifest.files.length && !superseded() && !failure) {
        var file = manifest.files[cursor++], url = offlineUrl(file.url);
        try {
          var retained = await cache.match(url);
          if (retained) {
            try { retained = await verifiedSearchResponse(retained, file); }
            catch (_) { await cache.delete(url); retained = undefined; }
          }
          if (!retained) {
            var same = reusable.get(file.digest + ":" + file.size);
            var candidate = same && previous ? await previous.match(offlineUrl(same.url)) : undefined;
            if (candidate) {
              try { candidate = await verifiedSearchResponse(candidate, file); }
              catch (_) { candidate = undefined; }
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
    var quota = error instanceof Error && (error.name === "QuotaExceededError" || /quota/i.test(error.message));
    searchStatus.phase = quota ? "blocked" : "error";
    searchStatus.error = quota ? "quota" : error instanceof Error && /^(integrity|evicted)$/.test(error.message) ? error.message : "network";
    if (quota) await caches.delete(OFFLINE_SEARCH_PREFIX + SEARCH_MANIFEST.version).catch(function () {});
  } finally { if (!superseded()) await publishSearchStatus(); }
}

/** @param {boolean} enabled */
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

/** @returns {SearchStatus} */
function disabledSearchStatus() {
  return {phase: "disabled", activeVersion: null, targetVersion: null, base: null, downloadedFiles: 0, totalFiles: 0, downloadedBytes: 0, totalBytes: 0, error: null};
}

/** @returns {Promise<OfflineStatus>} */
async function readOfflineStatus() {
  var generation = configurationGeneration;
  var settings = await caches.open(OFFLINE_SETTINGS);
  var configured = await settings.match(offlineUrl("__offline_count"));
  var persisted = configured ? Number(await configured.text()) : OFFLINE_COUNT;
  var count = configurationCount === null ? persisted : configurationCount;
  if (!Number.isInteger(count) || count < 0 || count > 1000) count = 0;
  var response = await settings.match(offlineUrl("__offline_articles"));
  /** @type {unknown} */
  var saved = response ? await response.json().catch(function () { return []; }) : [];
  var pages = await caches.open(OFFLINE_PAGES), revisions = await caches.open(OFFLINE_REVISIONS);
  var complete = [];
  if (Array.isArray(saved)) for (var item of saved.slice(0, count)) {
    if (!validOfflineItem(item)) continue;
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

/** @param {string} path */
function offlineUrl(path) {
  return new URL(path, self.registration.scope).href;
}

/** @param {number} count @param {number} [generation] @param {AbortSignal} [signal] */
function saveOfflineArticles(count, generation, signal) {
  function superseded() { return generation !== undefined && generation !== offlineGeneration; }
  var selected = OFFLINE_CATALOG.slice(0, count);
  /** @type {OfflineStatus} */
  var result = { type: "AGGR_OFFLINE_STATUS", requested: count, total: selected.length, saved: [], failed: 0, downloading: true };
  /** @type {Set<string>} */
  var wanted = new Set();
  /** @type {Map<string, Promise<void>>} */
  var resources = new Map();
  var next = 0;
  /** @type {OfflineItem[]} */
  var complete = [];
  selected.forEach(function (item) {
    item.resources.forEach(function (entry) { wanted.add(offlineUrl(entry.url)); });
  });
  return Promise.all([caches.open(OFFLINE_PAGES), caches.open(OFFLINE_REVISIONS)]).then(function (opened) {
    var pages = opened[0], revisions = opened[1];
    /** @param {ResourceEntry} entry @returns {Promise<void>} */
    function saveResource(entry) {
      var url = offlineUrl(entry.url);
      var found = resources.get(url);
      if (found) return found;
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
    /** @returns {Promise<void>} */
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
              if (!validOfflineItem(item) || complete.length >= count || complete.some(function (entry) { return entry.url === item.url; })) return;
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
    result.saved.sort(function (a, b) { return (order.get(a.url) ?? Infinity) - (order.get(b.url) ?? Infinity); });
    result.total = Math.max(result.total, result.saved.length);
    result.downloading = false;
    offlineStatus = result;
    broadcastOfflineStatus(result);
    return result;
  });
}

/** @param {OfflineStatus} status */
function broadcastOfflineStatus(status) {
  var generation = configurationGeneration;
  if (!self.clients.matchAll) return Promise.resolve();
  return self.clients.matchAll({type: "window"}).then(function (clients) {
    if (generation !== configurationGeneration || (configurationCount !== null && status.requested !== configurationCount)) return;
    var snapshot = Object.assign({}, status, {search: Object.assign({}, searchStatus)});
    clients.forEach(function (client) { client.postMessage(snapshot); });
  }).catch(function () {});
}

/** @param {number} count */
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

/** @param {number} count */
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
  const source = event.source;
  if (!source || !("url" in source) || !source.url.startsWith(self.registration.scope)) return;
  /** @type {unknown} */
  var data = event.data;
  if (!record(data)) return;
  if (data.type === "AGGR_OFFLINE_GET_STATUS") {
    event.waitUntil(readOfflineStatus().then(function (status) { source.postMessage(status); }));
    return;
  }
  if (data.type === "AGGR_GET_BUILD") {
    source.postMessage({type: "AGGR_BUILD", app_version: APP_VERSION, content_version: CONTENT_VERSION});
    return;
  }
  if (data.type !== "AGGR_OFFLINE_CONFIG" || typeof data.count !== "number" || !Number.isInteger(data.count) || data.count < 0 || data.count > 1000) return;
  event.waitUntil(configureOffline(data.count));
});


/** Content-addressed URLs never change meaning, so they are safe to serve from cache first.
 * @param {string} pathname
 */
function immutable(pathname) {
  var rest = pathname.slice(SCOPE.length);
  if (/^pagefind\/[0-9a-f]{64}\//.test(rest)) return true;
  if (rest.indexOf("assets/") !== 0) return false;
  var name = rest.split("/").pop() || "";
  var stem = name.indexOf(".") === -1 ? name : name.slice(0, name.lastIndexOf("."));
  if (/^(images|previews|documents)\//.test(rest.slice("assets/".length)) && /^[0-9a-f]{40}$/.test(stem)) return true;
  return /-[0-9a-f]{12}$/.test(stem);
}

/** Drop the oldest entries once a runtime cache passes its bound. @param {string} name @param {number} limit */
async function trim(name, limit) {
  var cache = await caches.open(name);
  var keys = await cache.keys();
  for (var i = 0; i < keys.length - limit; i++) await cache.delete(keys[i]);
}

/** @param {Request} request @param {number} timeout */
async function fromNetwork(request, timeout) {
  var controller = new AbortController();
  var timer = setTimeout(function () { controller.abort(); }, timeout);
  try {
    return await fetch(request, { signal: controller.signal });
  } finally {
    clearTimeout(timer);
  }
}

/** The last good copy of this page, else the page that explains why there is none. @param {Request} request */
async function lastKnown(request) {
  var protectedPages = await caches.open(OFFLINE_PAGES);
  return (await protectedPages.match(request, {ignoreSearch: true})) || (await caches.match(request, { ignoreSearch: true })) || (await caches.match(OFFLINE));
}

/** A promise held to the same deadline a fetch of our own would get; `null` when it passes.
 * @template T @param {Promise<T>} promise @param {number} timeout @returns {Promise<T | null>}
 */
function beforeDeadline(promise, timeout) {
  /** @type {Promise<null>} */
  var deadline = new Promise(function (resolve) {
    setTimeout(function () { resolve(null); }, timeout);
  });
  return Promise.race([
    promise,
    deadline,
  ]);
}

/** Pages: fresh when the network answers, the last copy when it does not.
 * @param {Request} request @param {Promise<Response | undefined>} [preload]
 */
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
      await cache.put(request, response.clone()).catch(function () {});
      void trim(PAGES, PAGE_LIMIT).catch(function () {});
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

/** Assets: whatever is cached, else the network, remembered for next time.
 * @param {Request} request @param {string} name @param {number} limit
 */
async function assetResponse(request, name, limit) {
  var protectedPages = await caches.open(OFFLINE_PAGES);
  var cached = (await protectedPages.match(request)) || (await caches.match(request));
  if (cached) return cached;
  var response = await fetch(request);
  if (response && response.ok) {
    var cache = await caches.open(name);
    await cache.put(request, response.clone()).catch(function () {});
    void trim(name, limit).catch(function () {});
  }
  return response;
}

self.addEventListener("install", function (event) {
  event.waitUntil(
    caches
      .open(SHELL)
      .then(function (cache) {
        // Required shell failures keep the previous working worker installed.
        return Promise.all(
          PRECACHE.map(function (entry) {
            return fetchEntry({url: offlineUrl(entry.url)}).then(function (response) {
              return cache.put(offlineUrl(entry.url), response);
            }).catch(function (error) { if (entry.required) throw error; });
          }),
        );
      })
      .then(function () { return self.skipWaiting(); }),
  );
});

// A page that loaded while this worker was activating asks to be claimed: activation only
// claims the pages that existed at that moment.
self.addEventListener("message", function (event) {
  if (event.data && event.data.type === "claim") event.waitUntil(self.clients.claim());
});

self.addEventListener("activate", function (event) {
  event.waitUntil(
    (async function () {
      if (self.registration.navigationPreload) await self.registration.navigationPreload.enable();
      var names = await caches.keys();
      await Promise.all(
        names
          .filter(function (name) {
            return name.indexOf(NAMESPACE) === 0 && name !== SHELL && name !== PAGES && name !== ASSETS
              && name !== OFFLINE_PAGES && name !== OFFLINE_REVISIONS && name !== OFFLINE_SETTINGS
              && !name.startsWith(OFFLINE_SEARCH_PREFIX);
          })
          .map(function (name) { return caches.delete(name); }),
      );
      await self.clients.claim();
      var settings = await caches.open(OFFLINE_SETTINGS);
      var saved = await settings.match(offlineUrl("__offline_count"));
      var count = saved ? Number(await saved.text()) : OFFLINE_COUNT;
      if (Number.isInteger(count) && count >= 0 && count <= 1000) {
        await configureOffline(count);
      }
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
    pageResponse(request).catch(async function () {
      return (await caches.match(request)) || Response.error();
    }),
  );
});
