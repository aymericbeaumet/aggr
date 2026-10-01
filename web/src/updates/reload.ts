/**
 * A reload that keeps the reader's place: the refresh pill and pull-to-refresh both go through
 * here. The place is written to `sessionStorage` just before reloading and read back, once,
 * by the page that comes up.
 */

type Store = Pick<Storage, 'getItem' | 'setItem'>;

export type Place = { url: string; x: number; y: number; focus: string; help: boolean };

export const reloadKey = (rootPathname: string): string => `aggr:reader-reload:${encodeURIComponent(rootPathname)}`;

/** The saved place, if it is this document's; anything else is ignored. */
export function parsePlace(text: string | null, href: string): Place | null {
  if (!text) return null;
  try {
    const saved: unknown = JSON.parse(text);
    if (typeof saved !== 'object' || saved === null) return null;
    const { url, x, y, focus, help } = saved as Record<string, unknown>;
    if (url !== href || typeof x !== 'number' || typeof y !== 'number') return null;
    return { url, x, y, focus: typeof focus === 'string' ? focus : '', help: help === true };
  } catch {
    return null;
  }
}

/** Remember where the reader is and reload. */
export function reloadKeepingPlace(rootPathname: string, doc: Document = document, win: Window = window): void {
  const place: Place = {
    url: win.location.href,
    x: win.scrollX,
    y: win.scrollY,
    focus: doc.activeElement?.id || '',
    help: doc.querySelector('dialog[open]') !== null,
  };
  try {
    win.sessionStorage.setItem(reloadKey(rootPathname), JSON.stringify(place));
  } catch {
    /* private browsing, quota */
  }
  win.location.reload();
}

/** Take the saved place, if any, and put the reader back there once the page has painted. */
export function restorePlace(rootPathname: string, doc: Document = document, win: Window = window): void {
  let store: Store | null = null;
  try {
    store = win.sessionStorage;
  } catch {
    return;
  }
  const key = reloadKey(rootPathname);
  let text: string | null = null;
  try {
    text = store.getItem(key);
    store.setItem(key, '');
  } catch {
    return;
  }
  const place = parsePlace(text, win.location.href);
  if (!place) return;
  win.requestAnimationFrame(() =>
    win.requestAnimationFrame(() => {
      if (place.help) doc.querySelector('dialog')?.showModal();
      if (place.focus) doc.getElementById(place.focus)?.focus({ preventScroll: true });
      win.scrollTo(place.x, place.y);
    }),
  );
}
