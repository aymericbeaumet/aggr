import { flushSync } from 'svelte';
import type { ClientPage } from '../generated/ClientPage';
import { extract, parseDocument } from '../model/extract';
import { pageScope } from '../navigation/scope';
import { page } from '../state/page.svelte';
import { badges } from './badges';

/**
 * New items arrive without a reload: when the content version moves, the list on screen swaps
 * its rows for the current ones. The swap waits for the top of the list unless the reader has
 * just come back to the window, so it never moves the ground under them.
 */
export function shouldApply(scrollTop: number, justActivated: boolean): boolean {
  return scrollTop <= 0 || justActivated;
}

/**
 * The page on screen with the fetched page's list, versions and shortcuts; null when either
 * side is not a list. Everything else (the page's identity, the site) stays, so the list
 * component is updated rather than rebuilt and rows it already shows keep their nodes.
 */
export function withFreshList(current: ClientPage, freshPage: ClientPage): ClientPage | null {
  if (current.page.view !== 'list' || freshPage.page.view !== 'list') return null;
  return {
    ...current,
    build: freshPage.build,
    site: { ...current.site, entries: freshPage.site.entries },
    page: freshPage.page,
  };
}

const ROWS = '.rows:not(.search-results) .row[data-url]';

/** The first row on screen and where it sits, so the swap can put it back there. */
function anchor(doc: Document): { url: string; top: number } | null {
  for (const row of doc.querySelectorAll<HTMLElement>(ROWS)) {
    if (row.hidden || !row.getClientRects().length) continue;
    const box = row.getBoundingClientRect();
    if (box.bottom > 0) return { url: row.dataset.url || '', top: box.top };
  }
  return null;
}

/**
 * Put the fetched list on screen in place of the current one: a `page` state update, applied
 * synchronously, that keeps the reader's place by the row they were looking at. Returns the
 * model shown, or null when there was nothing to swap.
 */
export function applyFreshList(freshPage: ClientPage, doc: Document = document, win: Window = window): ClientPage | null {
  const current = page.model;
  const href = page.href;
  if (!current || href === null) return null;
  const next = withFreshList(current, freshPage);
  if (!next) return null;
  const held = win.scrollY > 0 ? anchor(doc) : null;
  badges.resetMarks();
  page.show({ model: next, content: page.content, href });
  flushSync();
  if (held) {
    const restored = [...doc.querySelectorAll<HTMLElement>(ROWS)].find((row) => row.dataset.url === held.url);
    if (restored) win.scrollBy(0, restored.getBoundingClientRect().top - held.top);
  }
  return next;
}

/** Fetch the page at `href` again and read its model; null for anything that is not one of ours. */
export async function loadPage(href: string, signal: AbortSignal): Promise<ClientPage | null> {
  const response = await fetch(href, { cache: 'no-store', signal, headers: { accept: 'text/html' } });
  if (!response.ok) return null;
  return extract(parseDocument(await response.text()))?.model ?? null;
}

export type FeedUpdaterOptions = {
  load?: (href: string, signal: AbortSignal) => Promise<ClientPage | null>;
  /** After rows were swapped: the versions the page now carries are on screen. */
  onApplied?: (model: ClientPage) => void;
};

/**
 * Keeps the list on screen at the content version the poller last saw. A version the page is
 * behind is wanted; it is applied as soon as the gate allows, retried when a fetch fails, and
 * re-evaluated for every page that arrives.
 */
export class FeedUpdater {
  private wanted: string | null = null;
  private swapping = false;
  /** The reader came back to the window and has not touched the page since. */
  private activated = false;
  private doc: Document | null = null;
  private win: Window | null = null;

  constructor(private readonly options: FeedUpdaterOptions = {}) {}

  install(doc: Document = document, win: Window = window): void {
    this.doc = doc;
    this.win = win;
    doc.addEventListener('visibilitychange', () => {
      if (doc.hidden) return;
      this.activated = true;
      this.attempt();
    });
    const touched = () => {
      this.activated = false;
    };
    for (const name of ['wheel', 'touchstart', 'keydown', 'pointerdown']) {
      doc.addEventListener(name, touched, { capture: true, passive: true });
    }
    win.addEventListener(
      'scroll',
      () => {
        if (win.scrollY <= 0) this.attempt();
      },
      { passive: true },
    );
  }

  /** `version` is live: a list on screen built from another version is due for its rows. */
  notice(version: string): void {
    const model = page.model;
    if (!model || model.page.view !== 'list') return;
    if (model.build.content === version) {
      if (this.wanted === version) this.wanted = null;
      return;
    }
    this.wanted = version;
    this.attempt();
  }

  private attempt(): void {
    if (!this.wanted || this.swapping || !this.win) return;
    if (!shouldApply(this.win.scrollY, this.activated)) return;
    void this.swap();
  }

  private async swap(): Promise<void> {
    const target = this.wanted;
    const href = page.href;
    const doc = this.doc;
    const win = this.win;
    if (!target || href === null || !doc || !win) return;
    this.swapping = true;
    let applied = false;
    const signal = pageScope.signal;
    try {
      // Only the version is pinned: the page comes back as whatever is live when it is fetched.
      const freshPage = await (this.options.load ?? loadPage)(href, signal);
      // The reader may have moved on to another page while this was in flight.
      if (signal.aborted || page.href !== href || !freshPage) return;
      const current = page.model;
      if (!current) return;
      // A page from another release needs that release's styles and scripts: the pill says so.
      if (freshPage.build.app !== current.build.app) {
        this.wanted = null;
        return;
      }
      const next = applyFreshList(freshPage, doc, win);
      if (!next) return;
      // Only now are the rows on screen this build's. Anything newer that arrived while this
      // was in flight is still owed a pass.
      if (this.wanted === target || this.wanted === next.build.content) this.wanted = null;
      this.activated = false;
      applied = true;
      this.options.onApplied?.(next);
    } catch {
      // Offline, aborted, or a broken response: the version stays wanted, so the next check
      // asks for it again.
    } finally {
      this.swapping = false;
      if (applied && this.wanted) this.attempt();
    }
  }
}
