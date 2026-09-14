const workerSource = arguments[0];
const finish = arguments[arguments.length - 1];

(async function () {
  const scope = "http://localhost/reader/";
  const handlers = {};
  const buckets = new Map();
  let rejectWrites = false;
  let skipWaitingCalls = 0;
  let writes = 0;
  let enumerations = 0;
  let pauseRead = null;
  let network = async (request, options) => {
    assert(!options || options.priority !== "low", "foreground navigation must retain normal fetch priority");
    return new Response("fresh page");
  };
  const key = request => new URL(typeof request === "string" ? request : request.url, scope).href;
  const storage = {
    keys: async () => Array.from(buckets.keys()),
    delete: async name => buckets.delete(name),
    open: async name => {
      if (!buckets.has(name)) buckets.set(name, new Map());
      const entries = buckets.get(name);
      return {
        put: async (request, response) => {
          writes += 1;
          if (rejectWrites === true || (typeof rejectWrites === "function" && rejectWrites(name, key(request)))) throw new DOMException("quota exceeded", "QuotaExceededError");
          entries.set(key(request), response.clone());
        },
        delete: async request => entries.delete(key(request)),
        keys: async () => {
          enumerations += 1;
          return Array.from(entries.keys()).map(url => new Request(url));
        },
        match: async (request, options) => {
          const wanted = new URL(key(request));
          for (const [url, response] of entries) {
            const actual = new URL(url);
            if (url === wanted.href || (options && options.ignoreSearch && actual.pathname === wanted.pathname)) {
              const copy = response.clone();
              if (pauseRead) await pauseRead(name, wanted.href);
              return copy;
            }
          }
          return undefined;
        }
      };
    }
  };
  const self = {
    location: new URL(scope + "sw.js"),
    registration: { scope },
    clients: { claim: async () => undefined },
    skipWaiting: async () => { skipWaitingCalls += 1; },
    addEventListener: (name, handler) => { handlers[name] = handler; }
  };
  const source = workerSource
    .replace(/(?<=var OFFLINE_CATALOG = ).*(?=;)/g, JSON.stringify([
      { url: "items/new/", title: "Newest", resources: [{url:"items/new/", revision:"new"}, {url:"assets/images/shared.webp",revision:"image"}] },
      { url: "items/older/", title: "Older", resources: [{url:"items/older/", revision:"older"}, {url:"assets/images/shared.webp",revision:"image"}] }
    ]))
    .replace(/(?<=var OFFLINE_COUNT = ).*(?=;)/g, "0")
    .replace(/(?<=var SEARCH_MANIFEST = ).*(?=;)/g, JSON.stringify({version:"a".repeat(64),base:"pagefind/" + "a".repeat(64) + "/"}))
    .replace(/\{\{ version \| json \}\}/g, JSON.stringify("test"))
    .replace(/\{\{ precache \| json \}\}/g, JSON.stringify([{ url: "", revision: "new", required: true }]));
  const api = new Function("self", "caches", "fetch", source +
    "\nreturn {saveOfflineArticles, configureOffline, readOfflineStatus, broadcastOfflineStatus, OFFLINE_CATALOG, OFFLINE_PAGES, OFFLINE_SETTINGS, OFFLINE_SEARCH_PREFIX, SEARCH_MANIFEST, searchStatus: () => Object.assign({},searchStatus), networkFirst, remember, fetchEntry, populate, PRECACHE, PAGES, ASSETS, SEARCH, SEARCH_PREFIX, CACHE_NAMESPACE, PRECACHE_PREFIX, IMAGES, ENTRIES, REQUIRED_URLS, OPTIONAL_URLS, setPrecacheTimeout: ms => PRECACHE_TIMEOUT = ms};"
  )(self, storage, (request, options) => network(request, options));
  function assert(value, message) { if (!value) throw new Error(message); }
  const pending = [];
  const event = { waitUntil: promise => { pending.push(promise); } };
  const request = new Request(scope);
  const precache = await storage.open(api.PRECACHE);
  await precache.put(request, new Response("old install"));
  assert(await (await api.networkFirst(request, undefined, event)).text() === "fresh page", "online response");
  await Promise.all(pending);
  network = async () => { throw new Error("offline"); };
  assert(await (await api.networkFirst(request, undefined, event)).text() === "fresh page", "offline must prefer the latest runtime page");
  assert(pending.length > 0, "background cache writes must extend the fetch event lifetime");

  api.ENTRIES.splice(0, api.ENTRIES.length, { url: new URL(scope).pathname, revision: "new", required: true });
  api.ENTRIES.push({ url: new URL(scope + "browse/").pathname, revision: "optional", required: false });
  await precache.put(request, new Response("new deployment"));
  await (await storage.open(api.PAGES)).put(new Request(scope + "?q=old"), new Response("old runtime"));
  await (await storage.open(api.PAGES)).put(new Request(scope + "browse/"), new Response("saved directory"));
  const oldSearch = api.SEARCH_PREFIX + "previous";
  await (await storage.open(oldSearch)).put(new Request(scope + "pagefind/fragment.pf_index"), new Response("stale search"));
  const obsolete = api.CACHE_NAMESPACE + "obsolete-layout";
  const foreign = "another-app:pages";
  const savedSearch = api.OFFLINE_SEARCH_PREFIX + "retained";
  await (await storage.open(obsolete)).put(request, new Response("obsolete"));
  await (await storage.open(foreign)).put(request, new Response("unrelated application"));
  await (await storage.open(savedSearch)).put(request, new Response("saved index"));
  const oldPrecache = api.PRECACHE_PREFIX + "previous";
  const archivedScript = new Request(scope + "assets/app-012345abcdef.js");
  await (await storage.open(oldPrecache)).put(archivedScript, new Response("archived script"));
  let activated;
  handlers.activate({ waitUntil: promise => { activated = promise; } });
  await activated;
  assert(await (await api.networkFirst(request, undefined, event)).text() === "new deployment", "a new deployment must replace older runtime entries including query variants");
  assert(await (await api.networkFirst(new Request(scope + "browse/"), undefined, event)).text() === "saved directory", "a failed optional precache must preserve the previous readable runtime copy");
  assert(!buckets.has(oldSearch), "activation must discard stale runtime search-index generations");
  assert(!buckets.has(obsolete) && buckets.has(foreign), "activation removes unknown own caches without touching other applications");
  assert(buckets.has(savedSearch) && buckets.has(api.PAGES), "activation preserves retained offline search and readable pages");
  assert(!buckets.has(oldPrecache) && await (await (await storage.open(api.ASSETS)).match(archivedScript)).text() === "archived script", "current split-cache upgrades preserve immutable assets before removing old precaches");

  async function dispatch(request) {
    const lifetime = [];
    let response;
    handlers.fetch({
      request,
      preloadResponse: undefined,
      respondWith: value => { response = value; },
      waitUntil: value => { lifetime.push(value); }
    });
    assert(response, "the worker must handle article representations");
    const resolved = await response;
    await Promise.all(lifetime);
    return resolved;
  }
  const html = new Request(scope, { headers: { accept: "text/html" } });
  let refresh;
  network = () => new Promise(resolve => { refresh = resolve; });
  const refreshLifetime = [];
  let htmlResponse;
  handlers.fetch({
    request: html,
    respondWith: value => { htmlResponse = value; },
    waitUntil: value => { refreshLifetime.push(value); }
  });
  const immediate = await Promise.race([
    htmlResponse,
    new Promise(resolve => setTimeout(() => resolve(null), 100))
  ]);
  assert(immediate && await immediate.text() === "new deployment", "cached HTML must render without waiting for a slow network");
  refresh(new Response("background update"));
  await Promise.all(refreshLifetime);
  assert(await (await (await storage.open(api.PAGES)).match(html)).text() === "background update", "cached navigation must refresh in the background");
  for (const cache of ["reload", "no-cache"]) {
    network = async () => new Response("explicit reload " + cache);
    const reload = new Request(scope, { headers: { accept: "text/html" }, cache });
    assert(await (await dispatch(reload)).text() === "explicit reload " + cache, "an explicit app update or pull refresh must fetch current HTML instead of a saved article");
  }

  for (const extension of ["md", "txt", "rst", "json"]) {
    const representation = new Request(scope + "items/example/story." + extension);
    await (await storage.open(api.PAGES)).put(representation, new Response("stale " + extension));
    network = async () => new Response("fresh " + extension);
    assert(await (await dispatch(representation)).text() === "fresh " + extension, "online " + extension + " must bypass a stale runtime copy");
    network = async () => { throw new Error("offline"); };
    assert(await (await dispatch(representation)).text() === "fresh " + extension, "updated " + extension + " must remain available offline");
  }

  const localImage = new Request(scope + "assets/images/0123456789abcdef.webp");
  network = async () => new Response("local article image", { headers: { "content-type": "image/webp" } });
  assert(await (await dispatch(localImage)).text() === "local article image", "article images must remain readable while entering the runtime cache");
  const imageKeys = await (await storage.open(api.IMAGES)).keys();
  const assetKeys = await (await storage.open(api.ASSETS)).keys();
  assert(imageKeys.some(request => request.url === localImage.url) && !assetKeys.some(request => request.url === localImage.url), "article images must use the bounded image cache, not the app-asset cache");
  const writesBeforeHit = writes;
  const enumerationsBeforeHit = enumerations;
  network = async () => { throw new Error("cache hit must not fetch"); };
  assert(await (await dispatch(localImage)).text() === "local article image", "cached images must remain available");
  assert(writes === writesBeforeHit && enumerations === enumerationsBeforeHit, "asset cache hits must not rewrite responses or enumerate the entire cache");

  const opaque = { ok: false, type: "opaque", clone: () => new Response("unverifiable remote response") };
  const imageCountBeforeOpaque = (await (await storage.open(api.IMAGES)).keys()).length;
  await api.remember(api.IMAGES, 128, new Request("https://publisher.example/image.jpg"), opaque);
  assert((await (await storage.open(api.IMAGES)).keys()).length === imageCountBeforeOpaque, "opaque remote images must not be persisted");
  const pages = await storage.open(api.PAGES);
  await pages.put(request, new Response("saved before quota"));
  rejectWrites = true;
  const latest = await api.remember(api.PAGES, 500, request, new Response("fresh despite quota"));
  assert(await latest.text() === "fresh despite quota", "quota must not block the online response");
  assert(await (await pages.match(request)).text() === "saved before quota", "a failed cache write must preserve the old readable copy");
  rejectWrites = false;
  const previousName = api.PRECACHE + "-previous";
  const previous = await storage.open(previousName);
  await previous.put(request, new Response("previous working installation"));
  api.REQUIRED_URLS.splice(0, api.REQUIRED_URLS.length, {url:scope, revision:"required", required:true});
  api.OPTIONAL_URLS.splice(0, api.OPTIONAL_URLS.length, {url:scope + "optional", revision:"optional", required:false});
  network = async () => new Response("unavailable", {status:503});
  let installation;
  handlers.install({waitUntil: promise => { installation = promise; }});
  let installFailed = false;
  try { await installation; } catch (_) { installFailed = true; }
  assert(installFailed && skipWaitingCalls === 0 && !buckets.has(api.PRECACHE), "a failed required resource must reject and discard the new installation");
  assert(await (await previous.match(request)).text() === "previous working installation", "failed installation must leave the working generation intact");
  network = async request => new Response("resource", {status:key(request) === scope ? 200 : 404});
  handlers.install({waitUntil: promise => { installation = promise; }});
  await installation;
  assert(skipWaitingCalls === 1 && await (await storage.open(api.PRECACHE)).match(request), "optional failures must not prevent installing the readable app shell");
  let releaseSlowDownload;
  const downloads = [];
  network = async request => {
    downloads.push(key(request));
    return key(request).endsWith("/0")
      ? new Promise(resolve => { releaseSlowDownload = resolve; })
      : new Response("downloaded");
  };
  const entries = Array.from({ length: 25 }, (_, index) => ({ url: scope + index, revision: String(index) }));
  const population = api.populate(await storage.open("pool-pages"), await storage.open("pool-revisions"), [], entries, true);
  const downloadDeadline = Date.now() + 1000;
  while (!downloads.includes(scope + "24") && Date.now() < downloadDeadline) {
    await new Promise(resolve => setTimeout(resolve, 10));
  }
  assert(downloads.includes(scope + "24"), "one slow download must not stall the rest of the precache queue");
  releaseSlowDownload(new Response("slow download"));
  await population;
  let precachePriority;
  network = (request, options) => new Promise((resolve, reject) => {
    precachePriority = options.priority;
    options.signal.addEventListener("abort", () => reject(new Error("aborted")), { once: true });
  });
  api.setPrecacheTimeout(20);
  let timedOut = false;
  try { await api.fetchEntry({ url: scope + "never-finishes" }); } catch (_) { timedOut = true; }
  assert(timedOut, "stalled precache downloads must have a deadline");
  assert(precachePriority === "low", "offline precache downloads must yield network priority to foreground navigation");
  network = () => new Promise(() => {});
  let shortActivation;
  handlers.activate({waitUntil: value => {shortActivation = value;}});
  assert(await Promise.race([shortActivation.then(() => true), new Promise(resolve => setTimeout(() => resolve(false), 100))]), "activation must not wait for bulk offline downloads");
  api.setPrecacheTimeout(10000);
  const offlineRequests = [];
  network = async request => { offlineRequests.push(key(request)); return new Response("saved " + key(request)); };
  await (await storage.open(api.PAGES)).put(scope + "items/new/?reading=1", new Response("stale visited article"));
  const status = await api.saveOfflineArticles(2);
  assert(status.saved.length === 2 && status.failed === 0, "offline count includes only fully saved articles");
  assert(!await (await storage.open(api.PAGES)).match(scope + "items/new/?reading=1"), "new protected pages invalidate stale visited copies including query variants");
  assert(offlineRequests.filter(url => url.endsWith("shared.webp")).length === 1, "shared offline assets download once");
  const beforeRepeat = offlineRequests.length;
  await api.saveOfflineArticles(2);
  assert(offlineRequests.length === beforeRepeat, "unchanged offline resources reuse verified revisions");
  network = async () => { throw new Error("offline"); };
  const protectedImage = new Request(scope + "assets/images/shared.webp");
  assert((await (await dispatch(protectedImage)).text()).startsWith("saved"), "selected article assets remain available outside bounded runtime caches");
  const smaller = await api.saveOfflineArticles(1);
  const protectedCache = await storage.open(api.OFFLINE_PAGES);
  assert(smaller.saved.length === 1 && !await protectedCache.match(scope + "items/older/"), "reducing N removes unselected articles");
  await protectedCache.delete(scope + "assets/images/shared.webp");
  const incomplete = await api.saveOfflineArticles(1);
  assert(incomplete.saved.length === 0 && incomplete.failed === 1, "an article missing an asset must not be reported ready offline");
  rejectWrites = true;
  const quota = await api.saveOfflineArticles(2);
  assert(quota.failed === 2, "quota errors report incomplete downloads without rejecting the app");
  rejectWrites = false;
  const disabled = await api.saveOfflineArticles(0);
  assert(disabled.saved.length === 0 && (await protectedCache.keys()).length === 0, "zero disables automatic article storage");
  network = async request => new Response("fallback " + key(request));
  await api.saveOfflineArticles(1);
  const prior = api.OFFLINE_CATALOG[0];
  api.OFFLINE_CATALOG.unshift({url:"items/future/",title:"Future",resources:[{url:"items/future/",revision:"future"}]});
  network = async () => { throw new Error("temporary outage"); };
  const fallback = await api.saveOfflineArticles(1);
  assert(fallback.failed === 1 && fallback.saved.length === 1 && fallback.saved[0].url === prior.url, "failed newer downloads retain an older complete article within N");
  await api.saveOfflineArticles(0);
  let aborted = false;
  let downloadsStarted = 0;
  network = (request, options) => new Promise((resolve, reject) => {
    downloadsStarted += 1;
    options.signal.addEventListener("abort", () => {aborted = true; reject(new Error("cancelled"));}, {once:true});
  });
  const large = api.configureOffline(3);
  const repeated = api.configureOffline(3);
  assert(large === repeated, "identical concurrent offline requests share one job");
  const startDeadline = Date.now() + 1000;
  while (!downloadsStarted && Date.now() < startDeadline) await new Promise(resolve => setTimeout(resolve, 5));
  const stopped = api.configureOffline(0);
  await Promise.all([large, stopped]);
  assert(aborted && (await (await storage.open(api.OFFLINE_PAGES)).keys()).length === 0, "turning off downloads cancels active requests and clears the selection promptly");

  async function searchManifest(version, content) {
    const base = "pagefind/" + version + "/";
    const files = await Promise.all(Object.entries(content).map(async ([name, text]) => {
      const bytes = new TextEncoder().encode(text);
      const digest = Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes))).map(byte => byte.toString(16).padStart(2,"0")).join("");
      return {url:base + name,size:bytes.byteLength,digest};
    }));
    return {version,base,docs:10000,totalBytes:files.reduce((total,file) => total + file.size,0),files,facets:{}};
  }
  const indexContent = {"pagefind.js":"loader", "index.pf_index":"all searchable words", "filter.pf_filter":"all filter values", "fragment.pf_fragment":"all result text"};
  let manifest = await searchManifest("a".repeat(64), indexContent);
  const indexRequests = [];
  let missing = "fragment.pf_fragment", corrupt = false, activeDownloads = 0, peakDownloads = 0;
  network = async request => {
    const url = key(request);
    if (!url.includes("/pagefind/")) return new Response("saved article");
    indexRequests.push(url);
    if (url.endsWith("search-manifest.json")) return new Response(JSON.stringify(manifest), {headers:{"content-type":"application/json"}});
    activeDownloads++; peakDownloads = Math.max(peakDownloads, activeDownloads);
    await new Promise(resolve => setTimeout(resolve, 2)); activeDownloads--;
    const name = url.split("/").pop();
    if (name === missing) return new Response("unavailable", {status:503});
    return new Response(corrupt && name === "index.pf_index" ? "corrupt" : indexContent[name]);
  };
  await api.configureOffline(1);
  assert(api.searchStatus().phase === "error" && api.searchStatus().activeVersion === null, "partial index download must never report ready");
  assert(peakDownloads === 2, "complete index downloads use two independent bounded slots");
  const beforeStatus = indexRequests.length;
  let statusLifetime, reported;
  handlers.message({data:{type:"AGGR_OFFLINE_GET_STATUS"},source:{url:scope,postMessage:value => {reported=value;}},waitUntil:value => {statusLifetime=value;}});
  await statusLifetime;
  assert(indexRequests.length === beforeStatus && reported.saved.length === 1 && reported.search.phase === "error", "read-only status reports separate article and index readiness without fetching");
  const completedDownloads = indexRequests.filter(url => !url.endsWith(missing) && !url.endsWith("search-manifest.json"));
  missing = null;
  await api.configureOffline(1);
  assert(api.searchStatus().phase === "ready" && api.searchStatus().downloadedFiles === manifest.files.length && api.searchStatus().downloadedBytes === manifest.totalBytes, "the entire verified index must finish before readiness is committed");
  assert(completedDownloads.every(url => indexRequests.filter(request => request === url).length === 1), "retry resumes already verified staged files without repeated downloads");
  const readyRequests = indexRequests.length;
  await api.configureOffline(2);
  assert(indexRequests.length === readyRequests, "changing positive N must not download the same search index again");
  assert((await (await storage.open(api.OFFLINE_PAGES)).keys()).filter(request => request.url.includes("/items/")).length === 2, "full archive search must not save more article pages than N");
  const previousVersion = manifest.version;
  const healthyNetwork = network;
  manifest = await searchManifest("b".repeat(64), indexContent);
  Object.assign(api.SEARCH_MANIFEST, {version:manifest.version,base:manifest.base});
  rejectWrites = name => name === api.OFFLINE_SEARCH_PREFIX + manifest.version;
  await api.configureOffline(2);
  assert(api.searchStatus().phase === "blocked" && api.searchStatus().error === "quota" && api.searchStatus().activeVersion === previousVersion, "quota failure must preserve the previous complete index");
  assert(!buckets.has(api.OFFLINE_SEARCH_PREFIX + manifest.version), "quota failure releases uncommitted staging data");
  rejectWrites = false;
  const beforeReplacement = indexRequests.length;
  await api.configureOffline(2);
  assert(api.searchStatus().activeVersion === manifest.version && api.searchStatus().phase === "ready", "a verified replacement becomes active atomically");
  assert(indexRequests.slice(beforeReplacement).every(url => url.endsWith("search-manifest.json")), "unchanged resources reuse the verified previous index across version changes");
  network = async () => { throw new Error("offline"); };
  const offlineManifest = await (await dispatch(new Request(scope + "search-manifest.json"))).json();
  assert(offlineManifest.version === manifest.version, "offline manifest selects the last fully committed index");
  const offlineCatalog = await (await dispatch(new Request(scope + "search-catalog.json"))).json();
  assert(offlineCatalog.version === manifest.version && offlineCatalog.base === manifest.base && offlineCatalog.docs === manifest.docs && !offlineCatalog.files && !offlineCatalog.totalBytes, "offline vocabulary is projected from the same committed index without its large verification inventory");
  const newerCatalog = {version:"9".repeat(64),base:"pagefind/" + "9".repeat(64) + "/",docs:10001,facets:{}};
  network = async () => new Response(JSON.stringify(newerCatalog), {headers:{"content-type":"application/json"}});
  const onlineCatalog = await (await dispatch(new Request(scope + "search-catalog.json"))).json();
  assert(onlineCatalog.version === newerCatalog.version && api.searchStatus().activeVersion === manifest.version, "online catalogue uses the latest deployment without promoting an unverified offline index");
  network = async () => {throw new Error("offline");};
  const fallbackCatalog = await (await dispatch(new Request(scope + "search-catalog.json"))).json();
  assert(fallbackCatalog.version === manifest.version, "offline catalogue prefers the committed version over a newer browsing-cache catalogue");
  assert(await (await dispatch(new Request(scope + "pagefind/" + previousVersion + "/pagefind.js"))).text() === "loader", "a preceding complete index remains readable by already open tabs");
  const completeVersion = manifest.version;
  indexContent["index.pf_index"] = "changed searchable words";
  manifest = await searchManifest("c".repeat(64), indexContent);
  Object.assign(api.SEARCH_MANIFEST, {version:manifest.version,base:manifest.base});
  corrupt = true; network = healthyNetwork;
  await api.configureOffline(2);
  assert(api.searchStatus().phase === "error" && api.searchStatus().error === "integrity" && api.searchStatus().activeVersion === completeVersion, "a corrupt chunk must not replace the verified complete index");
  corrupt = false;
  await api.configureOffline(2);
  assert(api.searchStatus().phase === "ready" && api.searchStatus().activeVersion === manifest.version, "corrupt resources can be retried without losing the previous index");
  await (await storage.open(api.OFFLINE_SEARCH_PREFIX + manifest.version)).delete(scope + manifest.base + "filter.pf_filter");
  const evicted = await api.readOfflineStatus();
  assert(evicted.search.activeVersion === null && evicted.search.error === "evicted", "evicted index resources invalidate readiness");
  await api.configureOffline(0);
  assert(api.searchStatus().phase === "disabled" && !Array.from(buckets.keys()).some(name => name.startsWith(api.OFFLINE_SEARCH_PREFIX)), "zero clears protected full indexes without retaining staged generations");
  await api.configureOffline(2);
  let releaseRead, readStarted;
  const startedRead = new Promise(resolve => { readStarted = resolve; });
  pauseRead = async (name, url) => {
    if (!url.endsWith("/__offline_count")) return;
    pauseRead = null; readStarted();
    await new Promise(resolve => { releaseRead = resolve; });
  };
  const staleRead = api.readOfflineStatus();
  await startedRead;
  await api.configureOffline(0);
  releaseRead();
  const afterDisable = await staleRead;
  assert(afterDisable.requested === 0 && afterDisable.search.phase === "disabled" && afterDisable.search.activeVersion === null && api.searchStatus().phase === "disabled", "a delayed read must not restore a previous offline preference or overwrite disabled search state");
  await api.configureOffline(2);
  const startedManifest = new Promise(resolve => { readStarted = resolve; });
  pauseRead = async (name, url) => {
    if (!url.endsWith("/__offline_search")) return;
    pauseRead = null; readStarted();
    await new Promise(resolve => { releaseRead = resolve; });
  };
  const staleManifest = api.readOfflineStatus();
  await startedManifest;
  await api.configureOffline(0);
  releaseRead();
  const afterManifest = await staleManifest;
  assert(afterManifest.search.phase === "disabled" && afterManifest.search.activeVersion === null && api.searchStatus().phase === "disabled" && api.searchStatus().error === null, "a delayed committed-index read cannot restore readiness or report eviction after saving is disabled");
  await api.configureOffline(2);
  const delivered = [], clients = [{postMessage: status => delivered.push(status)}];
  let releaseClients;
  self.clients.matchAll = () => new Promise(resolve => { releaseClients = resolve; });
  const oldBroadcast = api.broadcastOfflineStatus({type:"AGGR_OFFLINE_STATUS",requested:2,saved:[]});
  self.clients.matchAll = async () => clients;
  await api.configureOffline(0);
  releaseClients(clients);
  await oldBroadcast;
  assert(delivered.length && delivered.at(-1).requested === 0 && delivered.at(-1).search.phase === "disabled" && delivered.at(-1).search.activeVersion === null, "a delayed ready broadcast cannot arrive after and supersede the disabled status");
  return { checks: 55 };
})().then(finish, error => finish({ error: error.message }));
