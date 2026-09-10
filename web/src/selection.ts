import type { ListPosition } from './contracts';

interface SelectionOptions {
  rows(): HTMLElement[];
  base(): string;
  pageURL(): string;
  window: Window;
  renderSelection?(url: string): boolean;
}
export function selectedLink(row: Element | null | undefined) {
  return row?.querySelector<HTMLAnchorElement>('[data-row-open]') || null;
}

/** Persist by article URL so refreshed/reordered lists retain the same selection. */
export function createSelection(options: SelectionOptions) {
  const win = options.window;
  function key() {
    const url = new URL(options.pageURL());
    url.hash = '';
    return 'aggr:list-cursor:' + encodeURIComponent(new URL(options.base()).pathname) + ':' + encodeURIComponent(url.href);
  }
  function read(): ListPosition {
    try { return JSON.parse(win.sessionStorage.getItem(key()) || 'null') || {}; } catch { return {}; }
  }
  function write(state: ListPosition) {
    try { win.sessionStorage.setItem(key(), JSON.stringify(state)); } catch { /* private mode */ }
  }
  function select(row: HTMLElement | null, focus: boolean, scroll: boolean) {
    const link = selectedLink(row);
    if (!link) return false;
    if (!options.renderSelection?.(link.href)) for (const entry of options.rows()) entry.classList.toggle('is-selected', entry === row);
    write({ ...read(), url: link.href, y: win.scrollY });
    if (focus) link.focus({ preventScroll: true });
    if (scroll) row?.scrollIntoView({ block: 'nearest', inline: 'nearest', behavior: 'instant' });
    return true;
  }
  return {
    select,
    edge(edge: 'first' | 'last') {
      const rows = options.rows();
      return select((edge === 'first' ? rows[0] : rows.at(-1)) || null, true, true);
    },
    save() {
      const rows = options.rows();
      if (!rows.length) return;
      const state = read();
      const selected = selectedLink(rows.find(row => row.classList.contains('is-selected')));
      if (selected) state.url = selected.href;
      write({ ...state, y: win.scrollY });
    },
    restore(focus: boolean, preferred?: string | null) {
      const rows = options.rows();
      if (!rows.length) return false;
      const state = focus ? read() : {};
      const wanted = preferred || state.url;
      const retained = wanted && rows.find(row => selectedLink(row)?.href === wanted);
      const selected = rows.find(row => row.classList.contains('is-selected'));
      const row = retained || selected || rows[0];
      const url = selectedLink(row)?.href || '';
      if (!options.renderSelection?.(url)) for (const entry of rows) entry.classList.toggle('is-selected', entry === row);
      if (focus && wanted && !retained) return false;
      if (focus) selectedLink(row)?.focus({ preventScroll: true });
      if (focus && retained && !preferred && typeof state.y === 'number') win.scrollTo(0, state.y);
      return true;
    },
    move(direction: number) {
      const rows = options.rows();
      if (!rows.length) return false;
      const current = rows.findIndex(row => row.classList.contains('is-selected'));
      return select(rows[current === -1 ? 0 : Math.max(0, Math.min(rows.length - 1, current + direction))], true, true);
    }
  };
}
