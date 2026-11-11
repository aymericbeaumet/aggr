import { flushSync } from 'svelte';
import { extract, parseDocument } from '../model/extract';
import { page } from '../state/page.svelte';
import { selection } from '../state/selection.svelte';
import { tabIntent } from '../state/tabIntent.svelte';
import { arrive, settle, trackScrolling } from './arrive';
import { PageCache, type Fetched, type Priority } from './cache';
import { address, linkIn, routable } from './routable';
import { ScrollMemory, newKey } from './scroll';
import { markCurrentTab, markTabForKind, tabUnder } from './tabs';

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
const memory = new ScrollMemory();
let cache: PageCache | null = null;
const pendingPrefetch = new Set<string>();

const root = (): URL => rootUrl ?? new URL(page.root || document.baseURI);

/** The history state of the entry on screen, with `fields` added to its own record. */
const stateWith = (fields: Record<string, unknown>) => ({
  ...((history.state as Record<string, unknown> | null) || {}),
  aggr: { key: entry, ...fields },
});

/**
 * The page on screen took an address of its own (a search query, canonicalized): the entry it
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

async function request(key: string, priority: Priority): Promise<Fetched> {
  const controller = new AbortController();
  // Keep the deadline through the body: receiving headers does not make a stalled page usable.
  const timer = setTimeout(() => controller.abort(), 15_000);
  try {
    const response = await fetch(key, {
      headers: { accept: 'text/html' },
      credentials: 'same-origin',
      priority,
      signal: controller.signal,
    });
    const type = response.headers.get('content-type') || '';
    if (!response.ok || !type.includes('text/html')) throw new Error('not a page');
    return { url: response.url || key, html: await response.text() };
  } finally {
    clearTimeout(timer);
  }
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

/** Fetch and parse a page when the reader points, focuses or touches its link. */
function prefetch(href: string): void {
  if (!installed) return;
  const connection = navigator.connection;
  if (connection?.saveData || ['slow-2g', '2g'].includes(connection?.effectiveType || '')) return;
  let url: URL;
  try {
    url = new URL(href, location.href);
  } catch {
    return;
  }
  const key = address(url.href);
  if (!routable(url, root()) || key === current) return;
  const store = pages();
  if (!store.has(key) && pendingPrefetch.size >= 2) return;
  const record = store.load(key, 'low');
  const parse = () => parseNow(key, record);
  if (record.fetched) {
    parse();
    return;
  }
  pendingPrefetch.add(key);
  record.page.then(parse).catch(() => {}).finally(() => pendingPrefetch.delete(key));
}

/** Leave the page on screen: keep its place and its cursor, and end what it set up. */
function leave(): void {
  memory.remember(entry, window.scrollY);
  if (page.kind !== 'item') memory.leaveList(current, window.scrollY);
  selection.save(current);
}

let slow: ReturnType<typeof setTimeout> | undefined;

/** Say a page is on its way once it has taken longer than a glance, and stop saying so. */
function waiting(on: boolean): void {
  clearTimeout(slow);
  const content = document.getElementById('content');
  if (on) content?.setAttribute('aria-busy', 'true');
  else content?.removeAttribute('aria-busy');
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

/** Drop a tab chosen ahead of its page and show the one the page on screen actually owns. */
function restoreCurrentTab(): void {
  tabIntent.route = null;
  markTabForKind(document, page.kind);
}

/** The reader chose to stay: a page still on its way must not replace this one when it lands. */
function cancel(): void {
  restoreCurrentTab();
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
  const record = pages().load(address(url.href));
  // A page already in memory is on screen before the click handler returns. Waiting would
  // cost a frame, which is the whole of the delay a reader can feel.
  const show = (fetched: Fetched): void => {
    if (mine !== token) return;
    // The page now decides which tab is current. Clearing the press first keeps one flush
    // from painting the old choice over the page that just arrived.
    tabIntent.route = null;
    pending = false;
    waiting(false);
    const destination = new URL(fetched.url, url.href);
    if (!routable(destination, root())) {
      fallback(url.href, traverse);
      return;
    }
    destination.hash = url.hash;
    const doc = record.parsed ?? parseDocument(fetched.html);
    // The parsed page is taken apart by the swap; the next visit parses its own copy.
    record.parsed = null;
    const extracted = extract(doc);
    if (!extracted || extracted.model.build.app !== page.model?.build.app) {
      fallback(url.href, traverse);
      return;
    }
    phase = 'rendering';
    if (!traverse) {
      const nextEntry = newKey();
      try {
        if (replace) history.replaceState({ aggr: { key: nextEntry } }, '', destination.href);
        else history.pushState({ aggr: { key: nextEntry } }, '', destination.href);
      } catch {
        fallback(url.href, traverse);
        return;
      }
      leave();
      entry = nextEntry;
    }
    current = address(location.href);
    page.show({ model: extracted.model, bodyHtml: extracted.bodyHtml, href: location.href });
    flushSync();
    opening = null;
    const saved = traverse
      ? (memory.recall(entry) ?? (history.state?.aggr?.scroll as number | undefined))
      : extracted.model.kind !== 'item' && !url.hash
        ? memory.place(current)
        : undefined;
    arrive(saved, url.hash);
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

function tabLinks(bar: Element): HTMLAnchorElement[] {
  return [...bar.querySelectorAll(':scope > a[data-route]')].filter(
    (node): node is HTMLAnchorElement => node instanceof HTMLAnchorElement,
  );
}

function tabBarAt(x: number, y: number): Element | null {
  if (document.fullscreenElement || document.querySelector('dialog[open]')) return null;
  const bar = document.querySelector('.mobile-tabs');
  if (!bar) return null;
  const style = getComputedStyle(bar);
  if (style.display === 'none' || style.visibility === 'hidden' || style.visibility === 'collapse' || style.pointerEvents === 'none') return null;
  const box = bar.getBoundingClientRect();
  if (box.width <= 0 || box.height <= 0 || x < box.left || x >= box.right || y < box.top || y >= box.bottom) return null;
  const topmost = document.elementFromPoint?.(x, y);
  return topmost && !bar.contains(topmost) ? null : bar;
}

/** Light `link` now: the column under the finger, before its page has been fetched. */
function holdTab(link: HTMLAnchorElement): void {
  if (link.hasAttribute('data-pressed') && link.getAttribute('aria-current') === 'page') return;
  const bar = link.closest('.mobile-tabs');
  if (bar) {
    for (const pressed of bar.querySelectorAll('[data-pressed]')) {
      if (pressed !== link) pressed.removeAttribute('data-pressed');
    }
    link.setAttribute('data-pressed', '');
  }
  const route = link.getAttribute('data-route');
  if (route === null) return;
  markCurrentTab(document, route);
  tabIntent.route = route;
  prefetch(link.href);
}

/** The press left the bar. Put back the tab the page on screen owns. */
function releaseTab(): void {
  tabGesture = null;
  for (const pressed of document.querySelectorAll('.mobile-tabs [data-pressed]')) pressed.removeAttribute('data-pressed');
  restoreCurrentTab();
}

/**
 * A tab answers the touch that lands anywhere on the bar, including its padding and a touch
 * that lands while the page is still gliding: the platform spends that touch on stopping the
 * scroll and never sends the click. The release is the activation, on whichever column it
 * lifts from, as long as the finger is still on the bar. Leaving the bar is a scroll instead.
 */
function installTabActivation(): void {
  let swallowed = false;
  // During scrolling, a compatibility click can retain the row that was under the fixed bar.
  document.addEventListener('click', (event) => {
    const bar = tabBarAt(event.clientX, event.clientY);
    if (bar && event.target instanceof Node && !bar.contains(event.target)) {
      event.preventDefault();
      event.stopImmediatePropagation();
    }
  }, { capture: true });
  document.addEventListener(
    'touchstart',
    (event) => {
      swallowed = false;
      const touch = event.touches.length === 1 ? event.touches[0] : null;
      const tabs = touch ? tabBarAt(touch.clientX, touch.clientY) : null;
      if (!tabs || !touch) {
        if (tabGesture) releaseTab();
        return;
      }
      const rtl = getComputedStyle(tabs).direction === 'rtl';
      const link = tabUnder(tabLinks(tabs), tabs.getBoundingClientRect(), touch.clientX, touch.clientY, rtl);
      if (!link) {
        tabGesture = null;
        return;
      }
      tabGesture = { bar: tabs, rtl };
      holdTab(link);
    },
    { passive: true },
  );
  document.addEventListener(
    'touchmove',
    (event) => {
      const gesture = tabGesture;
      const touch = event.touches.length === 1 ? event.touches[0] : null;
      if (!gesture) return;
      if (!touch) {
        swallowed = true;
        releaseTab();
        return;
      }
      const link = tabUnder(tabLinks(gesture.bar), gesture.bar.getBoundingClientRect(), touch.clientX, touch.clientY, gesture.rtl);
      if (!link) {
        swallowed = true;
        releaseTab();
        return;
      }
      holdTab(link);
    },
    { passive: true },
  );
  document.addEventListener(
    'touchcancel',
    () => {
      if (!tabGesture) return;
      swallowed = true;
      releaseTab();
    },
    { passive: true },
  );
  document.addEventListener(
    'touchend',
    (event) => {
      const gesture = tabGesture;
      const cancel = swallowed;
      tabGesture = null;
      swallowed = false;
      if (!gesture && !cancel) return;
      // Canceling the release cancels the click the platform would have sent after it, so one
      // tap stays one navigation and a drag off the bar does not land on whatever is underneath.
      if (cancel || event.touches.length || !gesture) {
        if (event.touches.length) releaseTab();
        event.preventDefault();
        return;
      }
      const touch = event.changedTouches[0];
      const link = touch
        ? tabUnder(tabLinks(gesture.bar), gesture.bar.getBoundingClientRect(), touch.clientX, touch.clientY, gesture.rtl)
        : null;
      if (!link) {
        releaseTab();
        event.preventDefault();
        return;
      }
      event.preventDefault();
      // Scrolling may have canceled the pointer, so no pointerup remains to clear its feedback.
      for (const pressed of gesture.bar.querySelectorAll('[data-pressed]')) pressed.removeAttribute('data-pressed');
      link.click();
    },
    { passive: false },
  );
}

let tabGesture: { bar: Element; rtl: boolean } | null = null;

/** Install document-level navigation once the app owns the page. */
function install(): void {
  if (installed || !('pushState' in history) || typeof DOMParser !== 'function') return;
  installed = true;
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
        const route = hit.link.getAttribute('data-route');
        if (route !== null) {
          markCurrentTab(document, route);
          tabIntent.route = route;
        }
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
  document.addEventListener('focusin', (event) => {
    const hit = linkIn(event, root());
    if (hit) prefetch(hit.url.href);
  });
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
  phase = 'settled';
}

export const navigation = {
  install,
  go,
  prefetch,
  pushFragment,
  replaceAddress,
  /** Forget every fetched page, e.g. when the content version changed. */
  clear(): void {
    cache?.clear();
  },
  get phase(): Phase {
    return phase;
  },
  /** Whether the page on screen has been left for one still on its way. */
  get pending(): boolean {
    return pending;
  },
};
