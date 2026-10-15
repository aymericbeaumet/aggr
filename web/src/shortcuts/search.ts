import { navigation } from '../navigation';
import { page } from '../state/page.svelte';
import { search } from '../state/search.svelte';

/**
 * Whether the search field sits entirely between the sticky header and the bottom bar. Moving
 * the page when the reader can already see the field would cost them their place for nothing.
 */
function searchFieldVisible(field: HTMLInputElement): boolean {
  const box = field.getBoundingClientRect();
  const header = document.querySelector('.top')?.getBoundingClientRect().bottom ?? 0;
  const tabs = document.querySelector('.mobile-tabs')?.getBoundingClientRect();
  const floor = tabs && tabs.height > 0 ? tabs.top : window.innerHeight;
  return box.top >= header && box.bottom <= floor;
}

/** Bring the search field into view, but only when it is not already there. */
export function revealSearchField(field: HTMLInputElement): void {
  if (!searchFieldVisible(field)) window.scrollTo({ top: 0, behavior: 'instant' });
}

export function searchField(): HTMLInputElement | null {
  const field = document.getElementById('q');
  return field instanceof HTMLInputElement ? field : null;
}

/** Put the keyboard in the search field and select what it already holds. */
function placeSearchFocus(field: HTMLInputElement): void {
  const current = field.isConnected ? field : searchField();
  if (!current) return;
  current.focus({ preventScroll: true });
  revealSearchField(current);
  if (document.activeElement === current) current.select();
}

/** Focus the shared search field, coming home first when the current page has none. */
export function focusSearch(): void {
  const field = searchField();
  if (!field) {
    void navigation.go(new URL('?focus-search=1', page.root).href);
    return;
  }
  // The field is about to be used: have the search module here before the first keystroke.
  void search.load().catch(() => {});
  // Focus first without moving, then decide: the field's own box is what we measure.
  placeSearchFocus(field);
  // Arrival claims the page on the next frame. When that claim wins, put the field back.
  requestAnimationFrame(() => {
    if (!field.isConnected) return;
    const active = document.activeElement;
    const main = document.getElementById('content');
    if (active === main || active === document.body || active === document.documentElement) placeSearchFocus(field);
  });
}

/** The field reveals itself on use, and a page reached with `?focus-search` starts in it. */
export function installSearchIntent(signal: AbortSignal): void {
  const field = searchField();
  if (field) {
    for (const name of ['click', 'focus']) field.addEventListener(name, () => revealSearchField(field), { signal });
    // A pointer heading for the field is intent enough to fetch the search module.
    field.addEventListener('pointerdown', () => void search.load().catch(() => {}), { signal, passive: true });
  }
  const url = new URL(location.href);
  if (url.searchParams.has('focus-search')) {
    url.searchParams.delete('focus-search');
    history.replaceState(history.state, '', url.href);
    focusSearch();
  }
}
