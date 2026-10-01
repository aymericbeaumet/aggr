import { cursorKey, readCursor, stepIndex, writeCursor, type CursorRow } from '../selection/cursor';

type Store = Pick<Storage, 'getItem' | 'setItem'>;

/**
 * The keyboard cursor over the list on screen, remembered by article URL so a reordered list
 * keeps the same row, and per list in `sessionStorage` so Back finds it again. The list feeds
 * it the rows that are on screen; one cursor serves the feed and, later, the search results.
 */
class Selection {
  rows = $state.raw<CursorRow[]>([]);
  url = $state<string | null>(null);
  private rootPathname = '/';
  private store: Store | null = null;

  configure(rootPathname: string, store: Store | null): void {
    this.rootPathname = rootPathname;
    this.store = store;
  }

  get current(): CursorRow | null {
    const url = this.url;
    return url === null ? null : (this.rows.find((row) => row.href === url) ?? null);
  }

  get index(): number {
    const url = this.url;
    return url === null ? -1 : this.rows.findIndex((row) => row.href === url);
  }

  /** The rows on screen changed: reselect the remembered row, else the first one. */
  sync(rows: CursorRow[], href: string): void {
    this.rows = rows;
    if (!rows.length) {
      this.url = null;
      return;
    }
    const wanted = this.store ? readCursor(this.store, cursorKey(this.rootPathname, href)) : null;
    this.url = wanted !== null && rows.some((row) => row.href === wanted) ? wanted : rows[0].href;
  }

  /** Put the cursor on `row` and remember it for the page at `href`. */
  select(row: CursorRow, href: string): void {
    this.url = row.href;
    this.write(row.href, href);
  }

  /** Step the cursor; the first press selects the first row. Returns the row now selected. */
  move(direction: number, href: string): CursorRow | null {
    const next = stepIndex(this.index, direction, this.rows.length);
    if (next === -1) return null;
    this.select(this.rows[next], href);
    return this.rows[next];
  }

  edge(which: 'first' | 'last', href: string): CursorRow | null {
    const row = which === 'first' ? this.rows[0] : this.rows.at(-1);
    if (!row) return null;
    this.select(row, href);
    return row;
  }

  /** Remember the cursor for the page at `href`, which is the one being left. */
  save(href: string): void {
    const current = this.current;
    if (current) this.write(current.href, href);
  }

  private write(url: string, href: string): void {
    if (this.store) writeCursor(this.store, cursorKey(this.rootPathname, href), url);
  }
}

export const selection = new Selection();
