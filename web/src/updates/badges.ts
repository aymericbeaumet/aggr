import { announcer } from '../state/announcer.svelte';
import { fresh } from '../state/fresh.svelte';

/**
 * What is new since this browsing session last saw the feed. The head of the entries is
 * remembered per site in `sessionStorage`; on the next look, everything above it is new. Rows on
 * screen light up and are announced; while the tab is hidden the title and the favicon carry a
 * dot instead. Without a remembered head nothing is new, so a first visit never lights up the
 * whole page.
 */

type Store = Pick<Storage, 'getItem' | 'setItem'>;

export const lastSeenKey = (rootPathname: string): string => `aggr:last-seen-entry:${encodeURIComponent(rootPathname)}`;
export const newEntriesKey = (rootPathname: string): string => `aggr:new-entries:${encodeURIComponent(rootPathname)}`;

/** Absolute URLs of the entries under the site root, in order; anything else is dropped. */
export function siteEntries(entries: readonly string[], root: string): string[] {
  const urls: string[] = [];
  for (const entry of entries) {
    try {
      const url = new URL(entry, root);
      if (url.href.startsWith(root)) urls.push(url.href);
    } catch {
      /* not a URL */
    }
  }
  return urls;
}

/** The pending entries once `urls` (newest first) are seen against the remembered head `previous`. */
export function detect(urls: readonly string[], previous: string | null, pending: readonly string[]): string[] {
  if (!previous || urls[0] === previous) return [...pending];
  const boundary = urls.indexOf(previous);
  return [...new Set([...pending, ...urls.slice(0, boundary < 0 ? urls.length : boundary)])];
}

/** Split the pending entries into the ones on screen, which are marked, and the rest. */
export function acknowledge(pending: readonly string[], onScreen: readonly string[]): { marked: string[]; pending: string[] } {
  const shown = new Set(onScreen);
  return {
    marked: pending.filter((url) => shown.has(url)),
    pending: pending.filter((url) => !shown.has(url)),
  };
}

/** The document title with or without the unread dot. */
export function badgedTitle(title: string, badged: boolean): string {
  const plain = title.replace(/^● /, '');
  return badged ? `● ${plain}` : plain;
}

export function readPending(store: Store | null, key: string): string[] {
  try {
    const saved: unknown = JSON.parse(store?.getItem(key) || '[]');
    return Array.isArray(saved) ? saved.filter((value): value is string => typeof value === 'string') : [];
  } catch {
    return [];
  }
}

const read = (store: Store | null, key: string): string | null => {
  try {
    return store?.getItem(key) ?? null;
  } catch {
    return null;
  }
};

const write = (store: Store | null, key: string, value: string): void => {
  try {
    store?.setItem(key, value);
  } catch {
    /* private browsing, quota */
  }
};

export class Badges {
  private root = '';
  private store: Store | null = null;
  private doc: Document | null = null;
  private originalIcon = '';
  private badgeIcon = '';

  /** Bind to the site and the session; the remembered pending entries come back. */
  configure(root: string, store: Store | null, doc: Document): void {
    this.root = root;
    this.store = store;
    this.doc = doc;
    fresh.visible = !doc.hidden;
    fresh.pending = readPending(store, newEntriesKey(new URL(root).pathname));
    doc.addEventListener('visibilitychange', () => {
      fresh.visible = !doc.hidden;
      this.show();
    });
  }

  /** A page arrived: its rows are the ones on screen now, so nothing they carried stays lit. */
  arrive(entries: readonly string[]): void {
    fresh.marked = [];
    this.observe(entries);
  }

  /** The entries are known (from the page or from `updates.json`): what is above the remembered head is new. */
  observe(entries: readonly string[]): void {
    if (!this.root) return;
    const urls = siteEntries(entries, this.root);
    const pathname = new URL(this.root).pathname;
    const saved = read(this.store, lastSeenKey(pathname));
    let previous: string | null = null;
    try {
      previous = saved ? new URL(saved, this.root).href : null;
    } catch {
      previous = null;
    }
    fresh.pending = detect(urls, previous, fresh.pending);
    if (urls[0]) write(this.store, lastSeenKey(pathname), urls[0]);
    this.persist();
    this.show();
  }

  /** Rows are in front of the reader: the pending ones among them light up and are announced. */
  reveal(onScreen: readonly string[]): void {
    if (!this.doc || this.doc.hidden) return;
    const { marked, pending } = acknowledge(fresh.pending, onScreen);
    if (!marked.length) return;
    fresh.marked = [...new Set([...fresh.marked, ...marked])];
    fresh.pending = pending;
    announcer.say(`${marked.length} new item${marked.length === 1 ? '' : 's'}`);
    this.persist();
    this.show();
  }

  /** The rows on screen were replaced: what they showed as new has been seen. */
  resetMarks(): void {
    fresh.marked = [];
  }

  /** The title and the favicon carry a dot while the tab is hidden with unseen entries. */
  show(): void {
    const doc = this.doc;
    if (!doc) return;
    const badged = doc.hidden && fresh.pending.length > 0;
    doc.title = badgedTitle(doc.title, badged);
    const icon = doc.querySelector('link[rel~="icon"]');
    if (!(icon instanceof HTMLLinkElement)) return;
    this.originalIcon ||= icon.href;
    if (!badged) {
      icon.href = this.originalIcon;
      return;
    }
    if (this.badgeIcon) {
      icon.href = this.badgeIcon;
      return;
    }
    const image = new Image();
    image.src = this.originalIcon;
    image.onload = () => {
      const canvas = doc.createElement('canvas');
      canvas.width = canvas.height = 32;
      const context = canvas.getContext('2d');
      if (!context) return;
      context.drawImage(image, 0, 0, 32, 32);
      context.beginPath();
      context.arc(25, 7, 6, 0, Math.PI * 2);
      context.fillStyle = '#e53935';
      context.fill();
      this.badgeIcon = canvas.toDataURL();
      if (doc.hidden && fresh.pending.length) icon.href = this.badgeIcon;
    };
  }

  private persist(): void {
    write(this.store, newEntriesKey(new URL(this.root).pathname), JSON.stringify(fresh.pending));
  }
}

export const badges = new Badges();
