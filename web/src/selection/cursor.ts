import type { ClientRow } from '../generated/ClientRow';
import { sitePath } from '../model/urls';

/** What the cursor needs to know about a row: where it opens and what its shortcuts reach. */
export type CursorRow = {
  path: string;
  /** Absolute URL of the item page; the identity the cursor is remembered by. */
  href: string;
  link: string;
  title: string;
  original: string | null;
  discussions: Array<{ name: string; href: string }>;
};

export function cursorRow(row: ClientRow, root: string): CursorRow {
  return {
    path: row.path,
    href: new URL(sitePath(row.url), root).href,
    link: row.link,
    title: row.title,
    original: row.metadata.original || null,
    discussions: row.metadata.discussions.map((discussion) => ({ name: discussion.name, href: discussion.url })),
  };
}

/**
 * The `sessionStorage` key of a list's cursor: per site (several readers on one origin never
 * share state) and per address without its fragment.
 */
export function cursorKey(rootPathname: string, href: string): string {
  const url = new URL(href);
  url.hash = '';
  return `aggr:list-cursor:${encodeURIComponent(rootPathname)}:${encodeURIComponent(url.href)}`;
}

type Store = Pick<Storage, 'getItem' | 'setItem'>;

export function readCursor(store: Store, key: string): string | null {
  try {
    const state: unknown = JSON.parse(store.getItem(key) || 'null');
    return typeof state === 'object' && state !== null && typeof (state as { url?: unknown }).url === 'string'
      ? (state as { url: string }).url
      : null;
  } catch {
    return null;
  }
}

export function writeCursor(store: Store, key: string, url: string): boolean {
  try {
    store.setItem(key, JSON.stringify({ url }));
    return true;
  } catch {
    return false;
  }
}

/** The index a step lands on: the first press selects the first row, the ends hold. */
export function stepIndex(current: number, direction: number, length: number): number {
  if (length <= 0) return -1;
  if (current === -1) return 0;
  return Math.min(length - 1, Math.max(0, current + direction));
}
