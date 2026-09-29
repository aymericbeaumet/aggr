// @ts-check
/** Passage sharing, deployment updates, and complete offline downloads. */
/** @typedef {import('../../../types/aggr').OfflineStatus} OfflineStatus */
/** @typedef {{app_version:string,content_version:string,entries?:string[]}} Build */
/** @param {string} selector @param {ParentNode} [root] */
const $ = (selector, root = document) => root.querySelector(/** @type {'div'} */ (selector));
/** @param {string} name @param {string} base */
const key = (name, base) => 'aggr:' + name + ':' + encodeURIComponent(new URL(base).pathname);
/** @param {string} name */
function read(name) { try { return sessionStorage.getItem(name); } catch { return null; } }
/** @param {string} name @param {string} value */
function write(name, value) { try { sessionStorage.setItem(name, value); } catch { /* Private browsing. */ } }
/** @param {string} message */
function announce(message) { const node = $('#reader-announcement'); if (!node) return; node.textContent = ''; requestAnimationFrame(() => { if (node.isConnected) node.textContent = message; }); }
const installed = () => matchMedia('(display-mode: standalone), (display-mode: minimal-ui), (display-mode: fullscreen)').matches || Reflect.get(navigator, 'standalone') === true;

/** @param {number} start @param {number} end */
export function encodeSelection(start, end) { return btoa(start + ',' + end).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, ''); }
/** @param {string} token @returns {[number,number]|null} */
export function decodeSelection(token) {
  try {
    const plain = atob(token.replace(/-/g, '+').replace(/_/g, '/'));
    if (!/^\d+,\d+$/.test(plain)) return null;
    const [start, end] = plain.split(',').map(Number);
    return Number.isSafeInteger(start) && Number.isSafeInteger(end) && end > start ? [start, end] : null;
  } catch { return null; }
}
/** @param {HTMLElement} body @param {AbortSignal} signal */
function shareSelection(body, signal) {
  const toolbar = document.createElement('div');
  toolbar.className = 'selection-share';
  toolbar.hidden = true;
  const button = document.createElement('button');
  button.type = 'button'; button.textContent = 'Share'; toolbar.append(button); document.body.append(toolbar);
  /** @type {{text:string,nodes:Array<{node:Text,start:number}>,words:Array<{start:number,end:number}>}|undefined} */
  let indexed;
  function index() {
    if (indexed) return indexed;
    const walker = document.createTreeWalker(body, NodeFilter.SHOW_TEXT);
    let text = '';
    /** @type {Array<{node:Text,start:number}>} */
    const nodes = [];
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      if (!(node instanceof Text)) continue;
      nodes.push({node, start:text.length}); text += node.data;
    }
    const words = Array.from(text.matchAll(/\S+/g), match => ({start:match.index, end:match.index + match[0].length}));
    return indexed = {text, nodes, words};
  }
  /** @param {Node} node @param {number} offset */
  function position(node, offset) {
    if (!body.contains(node)) return null;
    const probe = document.createRange();
    try { probe.setStart(body, 0); probe.setEnd(node, offset); return probe.toString().length; } catch { return null; }
  }
  let token = '', gestured = false, frame = 0, reposition = false, restoreFrame = 0;
  /** @type {ReturnType<typeof setTimeout>|undefined} */
  let labelTimer;
  function place() {
    frame = 0;
    if (signal.aborted) return;
    const selection = getSelection();
    if (!selection?.rangeCount || selection.isCollapsed) { toolbar.hidden = true; return; }
    const range = selection.getRangeAt(0);
    if (!reposition) {
    const from = position(range.startContainer, range.startOffset), to = position(range.endContainer, range.endOffset);
    if (from === null || to === null || to <= from) { toolbar.hidden = true; return; }
    const {words} = index();
    const start = words.findIndex(word => word.end > from);
    let end = words.length;
    while (end > 0 && words[end - 1].start >= to) end--;
    if (start < 0 || end <= start) { toolbar.hidden = true; return; }
    token = encodeSelection(start, end);
    }
    reposition = false;
    if (!gestured) return;
    const box = range.getBoundingClientRect();
    if (!box.width && !box.height) return;
    toolbar.hidden = false;
    toolbar.style.left = Math.max(8, Math.min(innerWidth - toolbar.offsetWidth - 8, box.left + box.width / 2 - toolbar.offsetWidth / 2)) + scrollX + 'px';
    toolbar.style.top = Math.max(0, box.top + scrollY - toolbar.offsetHeight - 10) + 'px';
  }
  function schedule() { reposition = false; if (!frame) frame = requestAnimationFrame(place); }
  function follow() { if (toolbar.hidden || frame) return; reposition = true; frame = requestAnimationFrame(place); }
  document.addEventListener('selectionchange', schedule, {signal});
  document.addEventListener('pointerdown', () => { gestured = true; }, {signal});
  document.addEventListener('keydown', event => { if (event.shiftKey || ((event.metaKey || event.ctrlKey) && event.key === 'a')) gestured = true; }, {signal});
  window.addEventListener('scroll', follow, {passive:true, signal});
  window.addEventListener('resize', follow, {signal});
  button.addEventListener('pointerdown', event => event.preventDefault(), {signal});
  button.addEventListener('click', async () => {
    const url = new URL(location.href); url.hash = 'selection=' + token;
    try {
      const native = typeof navigator.share === 'function';
      if (native) await navigator.share({title:document.title, url:url.href});
      else await navigator.clipboard.writeText(url.href);
      if (signal.aborted) return;
      button.textContent = native ? 'Shared' : 'Copied';
    } catch (error) {
      if (signal.aborted || (error instanceof DOMException && error.name === 'AbortError')) return;
      button.textContent = 'Copy failed';
    }
    clearTimeout(labelTimer); labelTimer = setTimeout(() => { button.textContent = 'Share'; }, 1600);
  }, {signal});
  signal.addEventListener('abort', () => { toolbar.remove(); cancelAnimationFrame(frame); cancelAnimationFrame(restoreFrame); clearTimeout(labelTimer); }, {once:true});
  function restore() {
    const shared = /(?:^|[#&])selection=([A-Za-z0-9_-]+)/.exec(location.hash)?.[1];
    const range = shared && decodeSelection(shared);
    if (!range || signal.aborted) return;
    const {words, nodes} = index(), first = words[range[0]], last = words[range[1] - 1];
    if (!first || !last) return;
    const from = [...nodes].reverse().find(entry => entry.start <= first.start), to = [...nodes].reverse().find(entry => entry.start < last.end);
    if (!from || !to) return;
    const selected = document.createRange();
    selected.setStart(from.node, first.start - from.start); selected.setEnd(to.node, last.end - to.start);
    const selection = getSelection(); selection?.removeAllRanges(); selection?.addRange(selected);
    from.node.parentElement?.scrollIntoView({block:'center', behavior:'instant'});
  }
  restoreFrame = requestAnimationFrame(() => { if (!signal.aborted) restoreFrame = requestAnimationFrame(restore); });
  window.addEventListener('hashchange', restore, {signal});
}

/** @param {OfflineStatus|undefined} state @param {boolean} enabled */
export function offlineSummary(state, enabled) {
  if (!enabled) return 'Offline storage is disabled for this site.';
  if (!state) return 'Preparing offline storage…';
  if (!state.requested) return 'Automatic downloads are off. Previously visited pages may still be cached.';
  let text = `${state.downloading ? 'Downloading' : 'Available offline'}: ${state.saved.length} of ${state.total} articles, with their retained images.`;
  if (state.failed) text += ' Some downloads are incomplete. Reconnect, lower the count, or free browser storage to retry.';
  const search = state.search;
  if (search?.phase === 'ready') text += ' Full archive search is available offline; only saved articles include pages and images.';
  else if (search?.phase === 'downloading' || search?.phase === 'updating') text += ` Downloading ${search.downloadedFiles} of ${search.totalFiles} search files.`;
  else if (search?.phase === 'blocked' || search?.phase === 'error') text += ' Search download incomplete. Reconnect or free browser storage to retry.';
  if (search?.activeVersion && search.phase !== 'ready') text += ' The previous search index remains available.';
  return text;
}
function renderOffline() {
  const status = $('#offline-download-status');
  if (status) status.textContent = offlineSummary(window.AGGROffline, window.AGGR?.pwa !== false);
  const list = $('#offline-articles');
  if (!list) return;
  list.replaceChildren();
  for (const saved of window.AGGROffline?.saved || []) {
    const url = new URL(saved.url, siteBase);
    if (url.origin !== location.origin || !url.href.startsWith(siteBase)) continue;
    const item = document.createElement('li'), link = document.createElement('a');
    link.href = url.href; link.textContent = saved.title; item.append(link); list.append(item);
  }
  const empty = $('#offline-empty'); if (empty) empty.hidden = list.childElementCount > 0;
}

let siteBase = '', initialized = false, appVersion = '', contentVersion = '', availableApp = '';
let configuredCount = -1;
/** @type {ServiceWorker|null} */
let configuredWorker = null;
/** @type {ServiceWorkerRegistration|undefined} */
let registration;
/** @type {Promise<void>|undefined} */
let checking;
let queuedCheck = false;
/** @type {string[]} */
let pendingEntries = [];
let originalIcon = '', badgeIcon = '';
function connection() {
  const root = $('#connection-status'), message = $('#connection-status-message'), retry = $('#connection-retry'), refresh = $('#pwa-refresh');
  const update = Boolean(availableApp && availableApp !== appVersion);
  if (root) root.hidden = navigator.onLine && !update;
  if (message) { message.hidden = update; message.textContent = navigator.onLine ? '' : 'Offline — downloaded articles remain available'; }
  if (retry) retry.hidden = navigator.onLine || update;
  if (refresh) refresh.hidden = !update;
  document.documentElement.dataset.updateState = update ? 'ready' : 'current';
}
function configureOffline(force = false) {
  if (window.AGGR?.pwa === false || !('serviceWorker' in navigator)) return;
  const worker = navigator.serviceWorker.controller;
  const count = Number(window.AGGRPreferences?.values['offline-items'] ?? 30);
  if (!worker || !Number.isInteger(count) || count < 0 || count > 1000) return;
  if (!force && worker === configuredWorker && count === configuredCount) return;
  configuredWorker = worker; configuredCount = count;
  worker.postMessage({type:'AGGR_OFFLINE_CONFIG', count});
  worker.postMessage({type:'AGGR_OFFLINE_GET_STATUS'});
}
/** @param {string[]} entries */
function detectEntries(entries) {
  const urls = entries.flatMap(value => { try { const url = new URL(value, siteBase); return url.href.startsWith(siteBase) ? [url.href] : []; } catch { return []; } });
  const saved = read(key('last-seen-entry', siteBase));
  let previous; try { previous = saved ? new URL(saved, siteBase).href : null; } catch { previous = null; }
  if (previous && urls[0] !== previous) {
    const boundary = urls.indexOf(previous);
    pendingEntries = [...new Set([...pendingEntries, ...urls.slice(0, boundary < 0 ? urls.length : boundary)])];
  }
  if (urls[0]) write(key('last-seen-entry', siteBase), urls[0]);
  write(key('new-entries', siteBase), JSON.stringify(pendingEntries));
  showEntries();
}
function showEntries() {
  if (!document.hidden) {
    let seen = 0;
    for (const row of document.querySelectorAll('.rows:not(.search-results) .row[data-url]')) {
      if (!(row instanceof HTMLElement) || row.hidden || !row.getClientRects().length) continue;
      const url = new URL(row.dataset.url || '', siteBase).href;
      if (!pendingEntries.includes(url)) continue;
      row.classList.add('is-new'); pendingEntries = pendingEntries.filter(entry => entry !== url); seen++;
    }
    if (seen) { announce(seen + (seen === 1 ? ' new item' : ' new items')); write(key('new-entries', siteBase), JSON.stringify(pendingEntries)); }
  }
  const badged = document.hidden && pendingEntries.length > 0;
  document.title = document.title.replace(/^● /, '');
  if (badged) document.title = '● ' + document.title;
  const icon = document.querySelector('link[rel~="icon"]');
  if (!(icon instanceof HTMLLinkElement)) return;
  originalIcon ||= icon.href;
  if (!badged) { icon.href = originalIcon; return; }
  if (badgeIcon) { icon.href = badgeIcon; return; }
  const image = new Image(); image.src = originalIcon;
  image.onload = () => {
    const canvas = document.createElement('canvas'); canvas.width = canvas.height = 32;
    const context = canvas.getContext('2d'); if (!context) return;
    context.drawImage(image, 0, 0, 32, 32); context.beginPath(); context.arc(25, 7, 6, 0, Math.PI * 2); context.fillStyle = '#e53935'; context.fill();
    badgeIcon = canvas.toDataURL(); if (document.hidden && pendingEntries.length) icon.href = badgeIcon;
  };
}
function refresh() {
  write(key('reader-reload', siteBase), JSON.stringify({url:location.href,x:scrollX,y:scrollY,focus:document.activeElement?.id || '',help:Boolean($('dialog[open]'))}));
  location.reload();
}
/** @param {boolean} [queue] */
function checkBuild(queue = false) {
  if (!navigator.onLine || document.hidden || !appVersion) return Promise.resolve();
  if (checking) { queuedCheck ||= queue; return checking; }
  queuedCheck = false;
  const controller = new AbortController(), timeout = setTimeout(() => controller.abort(), 10000);
  checking = fetch(new URL('updates.json', siteBase), {cache:'no-store', signal:controller.signal})
    .then(response => response.ok ? response.json() : null)
    .then(/** @param {Build|null} build */ build => {
      if (!build || typeof build.app_version !== 'string' || typeof build.content_version !== 'string') return;
      availableApp = build.app_version; connection();
      if (build.content_version !== contentVersion || (document.querySelector('.rows:not(.search-results)') && build.content_version !== window.AGGR?.content)) {
        contentVersion = build.content_version;
        if (Array.isArray(build.entries)) detectEntries(build.entries);
        document.dispatchEvent(new CustomEvent('aggr:content-update', {detail:build}));
      }
    }).catch(() => {}).finally(() => { clearTimeout(timeout); checking = undefined; if (queuedCheck) void checkBuild(); });
  return checking;
}
/** @param {string} phase @param {number} dx @param {number} dy */
export function pullState(phase, dx, dy) {
  if (!['tracking','pulling','armed'].includes(phase)) return {phase, distance:0};
  if (dy <= 0 || Math.abs(dx) > dy) return {phase:'idle', distance:0};
  if (dy < 6) return {phase, distance:0};
  return {phase:dy >= 84 ? 'armed' : 'pulling', distance:Math.min(72, Math.round(dy * .55))};
}
function installPull() {
  let phase = 'idle', x = 0, y = 0;
  /** @param {string} next @param {number} distance */
  const render = (next, distance) => {
    phase = next; document.documentElement.dataset.pullState = next;
    document.documentElement.style.setProperty('--pull-distance', distance + 'px');
    const indicator = $('#pull-refresh'); if (indicator) { indicator.hidden = next === 'idle' || next === 'tracking'; indicator.textContent = next === 'armed' ? 'Release to refresh' : next === 'refreshing' ? 'Refreshing…' : 'Pull to refresh'; }
  };
  document.addEventListener('touchstart', event => {
    if (!installed() || window.AGGR?.pwa === false || scrollY > 0 || event.touches.length !== 1 || $('dialog[open]') || event.target instanceof Element && event.target.closest('input,textarea,select,[contenteditable]')) return;
    x = event.touches[0].clientX; y = event.touches[0].clientY; render('tracking', 0);
  }, {passive:true});
  document.addEventListener('touchmove', event => {
    if (!['tracking','pulling','armed'].includes(phase)) return;
    if (event.touches.length !== 1 || scrollY > 0) { render('idle',0); return; }
    const next = pullState(phase,event.touches[0].clientX-x,event.touches[0].clientY-y);
    if ((next.phase === 'pulling' || next.phase === 'armed') && event.cancelable) event.preventDefault();
    render(next.phase,next.distance);
  }, {passive:false});
  document.addEventListener('touchend', () => { if (phase === 'armed') { render('refreshing',48); refresh(); } else render('idle',0); }, {passive:true});
  document.addEventListener('touchcancel', () => render('idle',0), {passive:true});
}
/** @param {{base:string}} options */
export function install(options) {
  if (initialized) return; initialized = true; siteBase = options.base;
  appVersion = window.AGGR?.app || ''; contentVersion = window.AGGR?.content || '';
  try { const saved = JSON.parse(read(key('new-entries', siteBase)) || '[]'); if (Array.isArray(saved)) pendingEntries = saved.filter(value => typeof value === 'string'); } catch { /* Ignore invalid state. */ }
  const position = read(key('reader-reload',siteBase)); write(key('reader-reload',siteBase),'');
  if (position) { try { const saved = JSON.parse(position); if (saved.url === location.href) requestAnimationFrame(() => requestAnimationFrame(() => { if (saved.help) document.querySelector('dialog')?.showModal(); document.getElementById(saved.focus)?.focus({preventScroll:true}); scrollTo(saved.x,saved.y); })); } catch { /* Ignore invalid state. */ } }
  $('#pwa-refresh')?.addEventListener('click', refresh);
  $('#connection-retry')?.addEventListener('click', () => { configureOffline(true); void checkBuild(); });
  window.addEventListener('online', () => { connection(); configureOffline(true); void checkBuild(); void registration?.update().catch(() => {}); });
  window.addEventListener('offline', connection);
  document.addEventListener('visibilitychange', () => { connection(); showEntries(); if (!document.hidden) { void checkBuild(); void registration?.update().catch(() => {}); } });
  window.addEventListener('pageshow', () => { void checkBuild(); });
  window.addEventListener('aggr:build', event => { event.preventDefault(); void checkBuild(true); });
  document.addEventListener('aggr:preferences', () => configureOffline());
  document.addEventListener('aggr:feed-refreshed', showEntries);
  document.addEventListener('aggr:search-results', event => { if (event instanceof CustomEvent && event.detail?.root instanceof Element) externalLinks(event.detail.root); });
  window.addEventListener('appinstalled', () => externalLinks(document));
  setInterval(() => { void checkBuild(); },15000);
  setInterval(() => { if (navigator.onLine && !document.hidden) void registration?.update().catch(() => {}); },60000);
  connection(); installPull(); void checkBuild();
  if (!('serviceWorker' in navigator)) return;
  if (window.AGGR?.pwa === false) {
    void navigator.serviceWorker.getRegistration(siteBase).then(worker => { if (worker?.scope === siteBase) return worker.unregister(); }).catch(() => {});
    if ('caches' in window) { const prefix = 'aggr:' + encodeURIComponent(new URL(siteBase).pathname) + ':'; void caches.keys().then(names => Promise.all(names.filter(name => name.startsWith(prefix)).map(name => caches.delete(name)))).catch(() => {}); }
    return;
  }
  navigator.serviceWorker.addEventListener('message', event => {
    if (event.source !== navigator.serviceWorker.controller || !event.data || typeof event.data !== 'object') return;
    if (event.data.type === 'AGGR_OFFLINE_STATUS' && Array.isArray(event.data.saved)) {
      window.AGGROffline = event.data; renderOffline(); document.dispatchEvent(new CustomEvent('aggr:offline-status', {detail:event.data}));
    }
  });
  navigator.serviceWorker.addEventListener('controllerchange', () => { configureOffline(true); void checkBuild(); });
  void navigator.serviceWorker.ready.then(worker => { registration = worker; configureOffline(); }).catch(() => {});
}
/** @param {ParentNode} root */
function externalLinks(root) {
  for (const link of root.querySelectorAll('a[href]')) {
    if (!(link instanceof HTMLAnchorElement)) continue;
    let url; try { url = new URL(link.href); } catch { continue; }
    if (!/^https?:$/.test(url.protocol) || url.href.startsWith(siteBase)) continue;
    const label = link.dataset.aggrExternalLabel || link.getAttribute('aria-label')?.replace(/, (?:opens in a new tab|external site)$/, '') || link.textContent?.trim();
    if (label) { link.dataset.aggrExternalLabel = label; link.setAttribute('aria-label', label + (installed() ? ', external site' : ', opens in a new tab')); }
    if (installed()) link.removeAttribute('target'); else link.target = '_blank';
    link.relList.add('noopener','noreferrer');
  }
}
/** @param {{base:string,signal:AbortSignal}} options */
export function mount(options) {
  if (options.signal.aborted) return;
  install(options); renderOffline(); configureOffline();
  const body = $('article.item .body'); if (body) shareSelection(body, options.signal);
  externalLinks(document);
  detectEntries([...(window.AGGR?.entries || []), ...Array.from(document.querySelectorAll('.rows:not(.search-results) .row[data-url]'), row => row instanceof HTMLElement ? row.dataset.url || '' : '')]);
  document.documentElement.dataset.readerReady = 'true';
  announce('Navigated to ' + document.title);
}
