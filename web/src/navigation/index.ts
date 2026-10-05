import { flushSync } from 'svelte';
import { extract, parseDocument } from '../model/extract';
import { sitePath } from '../model/urls';
import { page } from '../state/page.svelte';
import { selection } from '../state/selection.svelte';
import { arrive, settle, trackScrolling } from './arrive';
import { PageCache, type Fetched } from './cache';
import { patchHead } from './head';
import { address, linkIn, routable } from './routable';
import { pageScope } from './scope';
import { ScrollMemory, newKey } from './scroll';
import { Speculation, frugal } from './speculation';

/**
 * Following a link inside the archive swaps the page in place instead of loading a new document:
 * the header, the tab bar and the scripts already running stay, and a page fetched while the
 * finger was still coming down is on screen in the frame after it lifts. Links stay ordinary
 * links, so modifier clicks, new tabs and readers without JavaScript get plain navigation, and
 * anything this cannot swap (another site, a file, an error page, another release, no network)
 * falls back to it.
 */

/** Where a navigation is: nothing pending, a page on its way, being put on screen, or done. */
export type Phase = 'idle' | 'fetching' | 'rendering' | 'settled';

export type GoOptions = {
  /** Stand in for the current history entry rather than adding one. */
  replace?: boolean;
  /** Back or Forward already moved the address bar; the page has to catch up with it. */
  traverse?: boolean;
  /** The link followed, so the row or card it sits in stays lit until its page replaces it. */
  from?: HTMLAnchorElement | null;
};

/** What every arriving page sets up for itself, under the signal that the next arrival aborts. */
export type PageMount = (signal: AbortSignal) => void;

/** A page fetched longer ago than this is fetched again rather than shown. */
const PAGE_LIFETIME = 5 * 60_000;

let rootUrl: URL | null = null;
let installed = false;
let phase: Phase = 'idle';
/** The address on screen, without its fragment. */
let current = '';
/** The key of the history entry on screen. */
let entry = '';
let token = 0;
/** Whether a page is on its way: until it lands, the one on screen is no longer current. */
let pending = false;
let mounts: PageMount[] = [];
const memory = new ScrollMemory();
const speculation = new Speculation();
let cache: PageCache | null = null;

const root = (): URL => rootUrl ?? new URL(page.root || document.baseURI);

/** The history state of the entry on screen, with `fields` added to its own record. */
const stateWith = (fields: Record<string, unknown>) => ({
  ...((history.state as Record<string, unknown> | null) || {}),
  aggr: { key: entry, ...fields },
});

/**
 * The page on screen took an address of its own (a search query, canonicalised): the entry it
 * is shown at follows, with no navigation, so the next link or Back still measures from here.
 */
function replaceAddress(href: string): void {
  const url = new URL(href, location.href);
  if (url.href === location.href) return;
  try {
    history.replaceState(installed ? stateWith({}) : history.state, '', url.href);
  } catch {
    return;
  }
  if (installed) current = address(location.href);
}

/** Addresses the reader has asked for. Their fetch outranks a guess still in flight. */
const intent = new Set<string>();

/** Parses waiting to happen, one per frame, so a burst of guesses cannot stall a tap. */
const parseQueue: Array<() => void> = [];
let draining = false;

function drainParses(): void {
  const next = parseQueue.shift();
  if (!next) {
    draining = false;
    return;
  }
  next();
  requestAnimationFrame(drainParses);
}

function enqueueParse(run: () => void): void {
  parseQueue.push(run);
  if (draining) return;
  draining = true;
  requestAnimationFrame(drainParses);
}

async function request(key: string): Promise<Fetched> {
  const response = await fetch(key, {
    headers: { accept: 'text/html' },
    credentials: 'same-origin',
    priority: intent.has(key) ? 'high' : 'low',
  });
  const type = response.headers.get('content-type') || '';
  if (!response.ok || !type.includes('text/html')) throw new Error('not a page');
  return { url: response.url || key, html: await response.text() };
}

function pages(): PageCache {
  cache ??= new PageCache(request, { lifetime: PAGE_LIFETIME });
  return cache;
}

/** Parse `record` now. A page that is already parsed is left alone. */
function parseNow(key: string, record: ReturnType<PageCache['peek']>): void {
  if (!record?.fetched || record.parsed || cache?.peek(key) !== record) return;
  record.parsed = parseDocument(record.fetched.html);
}

/**
 * Fetch a page the reader is about to open, and parse it before the tap. Guesses are counted
 * and bounded, and their parse waits a frame; intent is not, and is parsed the moment it arrives.
 */
function prefetch(href: string, guess = false): void {
  if (!installed) return;
  let url: URL;
  try {
    url = new URL(href, location.href);
  } catch {
    return;
  }
  const key = address(url.href);
  if (!routable(url, root()) || key === current) return;
  const store = pages();
  if (guess && (frugal(navigator.connection) || !speculation.admit(store.has(key)))) return;
  if (!guess) intent.add(key);
  const record = store.load(url.href);
  const parse = () => parseNow(key, record);
  if (record.fetched) {
    if (guess) enqueueParse(parse);
    else parse();
    return;
  }
  record.page.then(() => (guess ? enqueueParse(parse) : parse())).catch(() => {});
}

/** Leave the page on screen: keep its place and its cursor, and end what it set up. */
function leave(): void {
  if (pageScope.aborted) return;
  memory.remember(entry, window.scrollY);
  if (page.kind !== 'item') memory.leaveList(current, window.scrollY);
  selection.save(current);
  pageScope.abort();
}

let slow: ReturnType<typeof setTimeout> | undefined;

/** Say a page is on its way once it has taken longer than a glance, and stop saying so. */
function waiting(on: boolean): void {
  clearTimeout(slow);
  if (on) slow = setTimeout(() => document.documentElement.setAttribute('data-navigating', ''), 150);
  else document.documentElement.removeAttribute('data-navigating');
}

/** The row or card whose link is on its way, lit until its page replaces it. */
let opening: Element | null = null;

function acknowledge(link: HTMLAnchorElement | null): void {
  opening?.removeAttribute('data-opening');
  opening = link?.closest('.row, .article-more-card') || null;
  opening?.setAttribute('data-opening', '');
}

/** The reader chose to stay: a page still on its way must not replace this one when it lands. */
function cancel(): void {
  if (pending) {
    opening?.removeAttribute('data-opening');
    opening = null;
  }
  token += 1;
  pending = false;
  phase = 'idle';
  waiting(false);
}

const editing = (target: Element | null) =>
  target !== null &&
  (target.closest("input, textarea, select, [contenteditable]:not([contenteditable='false'])") !== null ||
    ['textbox', 'combobox', 'searchbox'].includes(target.getAttribute('role') || ''));

/** The page on screen, asked for again: its top, with nothing left mid-edit. */
function top(): void {
  cancel();
  settle();
  const active = document.activeElement;
  if (active instanceof HTMLElement && editing(active)) active.blur();
  window.scrollTo({ top: 0, behavior: 'instant' });
}

/** Whatever this cannot swap is still a link: let the browser follow it. */
function fallback(href: string, traverse: boolean): void {
  phase = 'idle';
  if (traverse) location.reload();
  else location.assign(href);
}

/** Run what the page on screen sets up for itself. */
function mountPage(): void {
  const signal = pageScope.renew();
  for (const mount of mounts) {
    try {
      mount(signal);
    } catch (error) {
      console.error('aggr: page', error);
    }
  }
}

/** Bring the body's attributes (`data-kind`) in line with the page arriving. */
function syncBody(next: HTMLElement): void {
  const body = document.body;
  for (const name of body.getAttributeNames()) if (!next.hasAttribute(name)) body.removeAttribute(name);
  for (const name of next.getAttributeNames()) body.setAttribute(name, next.getAttribute(name) || '');
}

/** Show the page at `href`. */
async function go(href: string, { replace = false, traverse = false, from = null }: GoOptions = {}): Promise<void> {
  const url = new URL(href, location.href);
  if (!installed || !routable(url, root())) {
    if (replace) location.replace(url.href);
    else location.assign(url.href);
    return;
  }
  if (!traverse && address(url.href) === current) {
    if (url.hash) {
      settle();
      location.hash = url.hash;
    } else top();
    return;
  }
  const mine = ++token;
  pending = true;
  phase = 'fetching';
  acknowledge(from);
  intent.add(address(url.href));
  const record = pages().load(url.href);
  // A page already in memory is on screen before the click handler returns. Waiting would
  // cost a frame, which is the whole of the delay a reader can feel.
  const show = (fetched: Fetched): void => {
    if (mine !== token) return;
    pending = false;
    waiting(false);
    const doc = record.parsed ?? parseDocument(fetched.html);
    // The parsed page is taken apart by the swap; the next visit parses its own copy.
    record.parsed = null;
    const extracted = extract(doc, document);
    if (!extracted || extracted.model.build.app !== page.model?.build.app) {
      fallback(url.href, traverse);
      return;
    }
    phase = 'rendering';
    if (!traverse) {
      leave();
      const destination = new URL(fetched.url);
      destination.hash = url.hash;
      entry = newKey();
      if (replace) history.replaceState({ aggr: { key: entry } }, '', destination.href);
      else history.pushState({ aggr: { key: entry } }, '', destination.href);
    }
    current = address(location.href);
    patchHead(document, doc);
    syncBody(extracted.body);
    page.show({ model: extracted.model, content: extracted.content, href: location.href });
    flushSync();
    opening = null;
    const saved = traverse
      ? (memory.recall(entry) ?? (history.state?.aggr?.scroll as number | undefined))
      : extracted.model.kind !== 'item' && !url.hash
        ? memory.place(current)
        : undefined;
    arrive(saved, url.hash);
    mountPage();
    phase = 'settled';
  };
  if (record.fetched) {
    show(record.fetched);
    return;
  }
  waiting(true);
  const fetched = await record.page.catch(() => null);
  // An older response never wins: the reader has moved on, or chosen to stay.
  if (mine !== token) return;
  if (!fetched) {
    pending = false;
    waiting(false);
    fallback(url.href, traverse);
    return;
  }
  show(fetched);
}

/** Record a new entry for a place in this page, as following a fragment link would. */
function pushFragment(hash: string): void {
  memory.remember(entry, window.scrollY);
  entry = newKey();
  history.pushState({ aggr: { key: entry } }, '', hash);
}

/** The tabs and the neighbouring articles: where a reader is most likely to go from here. */
function nearest(): string[] {
  const base = root();
  const links = ['', 'browse/', 'preferences/'].map((route) => new URL(route, base).href);
  const view = page.model?.page;
  if (view?.view === 'article') {
    for (const neighbour of [view.data.next, view.data.previous]) {
      if (neighbour) links.push(new URL(sitePath(neighbour.url), base).href);
    }
  }
  return links;
}

/** Warm the tabs and a few rows already on screen. The rest wait until the reader moves. */
const FIRST_SCREEN = 3;

function speculate(signal: AbortSignal): void {
  speculation.reset();
  const refresh = () => {
    if (document.hidden || frugal(navigator.connection)) return;
    for (const href of nearest()) prefetch(href);
  };
  const timer = setInterval(refresh, PAGE_LIFETIME / 2);
  document.addEventListener('visibilitychange', refresh, { signal });
  signal.addEventListener('abort', () => clearInterval(timer));
  requestAnimationFrame(() => {
    if (signal.aborted || frugal(navigator.connection)) return;
    for (const href of nearest()) prefetch(href, true);
    const height = window.innerHeight || document.documentElement.clientHeight;
    let warmed = 0;
    for (const link of document.querySelectorAll('.rows .row [data-row-open]')) {
      if (warmed >= FIRST_SCREEN) return;
      if (!(link instanceof HTMLAnchorElement)) continue;
      const rect = link.getBoundingClientRect();
      if (rect.bottom <= 0 || rect.top >= height) continue;
      prefetch(link.href, true);
      warmed += 1;
    }
  });
}

/**
 * A tab answers the touch that lands on it, including one that lands while the page is still
 * gliding from a fling: the platform spends that touch on stopping the scroll and never sends
 * the click, so the release is the activation, on whichever tab it lifts from.
 */
function installTabActivation(): void {
  document.addEventListener(
    'touchstart',
    (event) => {
      const target = event.target;
      const tabs = target instanceof Element ? target.closest('.mobile-tabs') : null;
      if (!tabs) return;
      const touch = event.touches.length === 1 ? event.touches[0] : null;
      pressedTab = touch ? { y: touch.clientY } : null;
      const link = target instanceof Element ? target.closest('a[href]') : null;
      if (link instanceof HTMLAnchorElement) prefetch(link.href);
    },
    { passive: true },
  );
  // A finger sliding along the bar still means the tab it lifts from, as on a native tab bar;
  // one travelling up or down is scrolling the page instead.
  document.addEventListener(
    'touchmove',
    (event) => {
      const touch = event.touches[0];
      if (pressedTab && touch && Math.abs(touch.clientY - pressedTab.y) > 24) pressedTab = null;
    },
    { passive: true },
  );
  document.addEventListener('touchcancel', () => (pressedTab = null), { passive: true });
  document.addEventListener(
    'touchend',
    (event) => {
      const touched = pressedTab;
      pressedTab = null;
      if (!touched || event.touches.length) return;
      const touch = event.changedTouches[0];
      const under = touch ? document.elementFromPoint(touch.clientX, touch.clientY) : null;
      const link = under?.closest('a[href]');
      if (!(link instanceof HTMLAnchorElement) || !link.closest('.mobile-tabs')) return;
      // Cancelling the release cancels the click the platform would have sent after it, so one
      // tap stays one navigation.
      event.preventDefault();
      link.click();
    },
    { passive: false },
  );
}

let pressedTab: { y: number } | null = null;

/** Install once the app owns the page. `mount` runs for the page on screen and every next one. */
function install(mount: PageMount[]): void {
  if (installed || !('pushState' in history) || typeof DOMParser !== 'function') return;
  installed = true;
  mounts = mount;
  rootUrl = new URL(page.root || document.baseURI);
  current = address(location.href);
  history.scrollRestoration = 'manual';
  trackScrolling();
  entry = (history.state?.aggr?.key as string | undefined) || newKey();
  const saved = history.state?.aggr?.scroll as number | undefined;
  history.replaceState(stateWith({}), '');
  // A reload or a return to a document the browser let go of: its own place, as it was left.
  const arrival = performance.getEntriesByType?.('navigation')?.[0] as PerformanceNavigationTiming | undefined;
  if (typeof saved === 'number' && (arrival?.type === 'reload' || arrival?.type === 'back_forward')) {
    requestAnimationFrame(() => window.scrollTo({ top: saved, behavior: 'instant' }));
  }

  /** A mouse press on the site's own navigation is the choice itself; its click is spent. */
  let pressed: HTMLAnchorElement | null = null;
  document.addEventListener('click', (event) => {
    const target = event.target;
    if (pressed && target instanceof Element && target.closest('a[href]') === pressed) {
      pressed = null;
      event.preventDefault();
      return;
    }
    pressed = null;
    if (event.defaultPrevented || event.button !== 0) return;
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    const hit = linkIn(event, root());
    if (!hit) return;
    if (address(hit.url.href) === current) {
      // A fragment of this page is the browser's own jump; the entry it leaves keeps its place.
      if (hit.url.hash) {
        cancel();
        settle();
        memory.remember(entry, window.scrollY);
        return;
      }
      event.preventDefault();
      top();
      return;
    }
    event.preventDefault();
    void go(hit.url.href, { from: hit.link });
  });
  // Intent: a press is a promise of a click, and a pointer resting on a link is a likely one.
  document.addEventListener(
    'pointerdown',
    (event) => {
      const hit = linkIn(event, root());
      if (!hit) return;
      prefetch(hit.url.href);
      const menu = hit.link.closest('[data-site-navigation]') && hit.link.hasAttribute('data-route');
      if (
        menu &&
        event.pointerType === 'mouse' &&
        event.button === 0 &&
        !event.metaKey &&
        !event.ctrlKey &&
        !event.shiftKey &&
        !event.altKey &&
        address(hit.url.href) !== current
      ) {
        pressed = hit.link;
        void go(hit.url.href, { from: hit.link });
      }
    },
    { capture: true, passive: true },
  );
  document.addEventListener(
    'pointerover',
    (event) => {
      if (event.pointerType !== 'mouse') return;
      const hit = linkIn(event, root());
      if (hit) prefetch(hit.url.href);
    },
    { passive: true },
  );
  // Keyboard activation of a link is a click as well; Enter on a focused link fires one.

  window.addEventListener('popstate', (event) => {
    const key = event.state?.aggr?.key as string | undefined;
    if (address(location.href) === current && !pending) {
      // Another place in this same page. Nothing was fetched, so nothing needs to arrive.
      settle();
      memory.remember(entry, window.scrollY);
      entry = key || newKey();
      const place = memory.recall(entry);
      if (place !== undefined) window.scrollTo({ top: place, behavior: 'instant' });
      return;
    }
    leave();
    entry = key || newKey();
    if (!key) history.replaceState(stateWith({}), '');
    void go(location.href, { traverse: true });
  });
  // A fragment the browser followed by itself made an entry with no key: give it one.
  window.addEventListener('hashchange', () => {
    if (history.state?.aggr?.key) return;
    entry = newKey();
    history.replaceState(stateWith({}), '');
  });
  // The place this entry was left at, for a reload or a Back that has to rebuild the document.
  window.addEventListener('pagehide', () => {
    selection.save(location.href);
    try {
      history.replaceState(stateWith({ scroll: window.scrollY }), '');
    } catch {
      /* a browser rationing history writes may refuse this one */
    }
  });
  installTabActivation();
  mountPage();
  phase = 'settled';
}

export const navigation = {
  install,
  go,
  prefetch,
  speculate,
  pushFragment,
  replaceAddress,
  /** Forget every fetched page, e.g. when the content version changed. */
  clear(): void {
    cache?.clear();
    speculation.reset();
  },
  get phase(): Phase {
    return phase;
  },
  /** Whether the page on screen has been left for one still on its way. */
  get pending(): boolean {
    return pending;
  },
};
