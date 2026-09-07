const workerSource = arguments[0];
const finish = arguments[arguments.length - 1];

(async function () {
  const scope = "http://localhost/reader/";
  const handlers = {};
  const buckets = new Map();
  let rejectWrites = false;
  let skipWaitingCalls = 0;
  let network = async () => new Response("fresh page");
  const key = request => new URL(typeof request === "string" ? request : request.url, scope).href;
  const storage = {
    keys: async () => Array.from(buckets.keys()),
    delete: async name => buckets.delete(name),
    open: async name => {
      if (!buckets.has(name)) buckets.set(name, new Map());
      const entries = buckets.get(name);
      return {
        put: async (request, response) => {
          if (rejectWrites) throw new Error("quota exceeded");
          entries.set(key(request), response.clone());
        },
        delete: async request => entries.delete(key(request)),
        keys: async () => Array.from(entries.keys()).map(url => new Request(url)),
        match: async (request, options) => {
          const wanted = new URL(key(request));
          for (const [url, response] of entries) {
            const actual = new URL(url);
            if (url === wanted.href || (options && options.ignoreSearch && actual.pathname === wanted.pathname)) {
              return response.clone();
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
    .replace(/\{\{ version \| json \}\}/g, JSON.stringify("test"))
    .replace(/\{\{ precache \| json \}\}/g, JSON.stringify([{ url: "", revision: "new", required: true }]));
  const api = new Function("self", "caches", "fetch", source +
    "\nreturn {networkFirst, remember, fetchEntry, PRECACHE, PAGES, ASSETS, SEARCH, SEARCH_PREFIX, LEGACY_SEARCH, IMAGES, ENTRIES, REQUIRED_URLS, OPTIONAL_URLS, setPrecacheTimeout: ms => PRECACHE_TIMEOUT = ms};"
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
  api.ENTRIES.push({ url: new URL(scope + "library/").pathname, revision: "optional", required: false });
  await precache.put(request, new Response("new deployment"));
  await (await storage.open(api.PAGES)).put(new Request(scope + "?q=old"), new Response("old runtime"));
  await (await storage.open(api.PAGES)).put(new Request(scope + "library/"), new Response("saved directory"));
  const oldSearch = api.SEARCH_PREFIX + "previous";
  await (await storage.open(oldSearch)).put(new Request(scope + "pagefind/fragment.pf_index"), new Response("stale search"));
  await (await storage.open(api.LEGACY_SEARCH)).put(new Request(scope + "pagefind/pagefind-entry.json"), new Response("legacy search"));
  let activated;
  handlers.activate({ waitUntil: promise => { activated = promise; } });
  await activated;
  assert(await (await api.networkFirst(request, undefined, event)).text() === "new deployment", "a new deployment must replace older runtime entries including query variants");
  assert(await (await api.networkFirst(new Request(scope + "library/"), undefined, event)).text() === "saved directory", "a failed optional precache must preserve the previous readable runtime copy");
  assert(!buckets.has(oldSearch) && !buckets.has(api.LEGACY_SEARCH), "activation must discard stale search-index generations");

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
  network = (request, options) => new Promise((resolve, reject) => {
    options.signal.addEventListener("abort", () => reject(new Error("aborted")), { once: true });
  });
  api.setPrecacheTimeout(20);
  let timedOut = false;
  try { await api.fetchEntry({ url: scope + "never-finishes" }); } catch (_) { timedOut = true; }
  assert(timedOut, "stalled precache downloads must have a deadline");
  return { checks: 14 };
})().then(finish, error => finish({ error: error.message }));
