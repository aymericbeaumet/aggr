/**
 * Which tab a point belongs to. The bar's padding, the pill ends and the home-indicator gap are
 * part of the control: a thumb rarely lands on the glyph itself. Columns share the bar's width,
 * so the side padding belongs to the outer tabs.
 */

export type TabBox = { left: number; top: number; width: number; height: number };

/** The tab whose column contains `(x, y)`, or null when the point is outside the bar. */
export function tabUnder(
  links: readonly HTMLAnchorElement[],
  box: TabBox,
  x: number,
  y: number,
  rtl = false,
): HTMLAnchorElement | null {
  if (links.length === 0 || box.width <= 0 || box.height <= 0) return null;
  if (x < box.left || y < box.top || x >= box.left + box.width || y >= box.top + box.height) return null;
  const raw = Math.min(links.length - 1, Math.floor(((x - box.left) / box.width) * links.length));
  return links[rtl ? links.length - 1 - raw : raw] ?? null;
}

/** The tab under a point, measured from the bar on screen. */
export function tabAt(bar: Element, x: number, y: number): HTMLAnchorElement | null {
  const links = [...bar.querySelectorAll(':scope > a[data-route]')].filter(
    (node): node is HTMLAnchorElement => node instanceof HTMLAnchorElement,
  );
  return tabUnder(links, bar.getBoundingClientRect(), x, y, getComputedStyle(bar).direction === 'rtl');
}

const TAB_LINK = '[data-site-navigation] a[data-route]:not(.brand)';

/** Mark `route` current on every site tab, desktop and mobile, and clear the others. */
export function markCurrentTab(root: ParentNode, route: string): void {
  for (const node of root.querySelectorAll(TAB_LINK)) {
    if (!(node instanceof HTMLAnchorElement)) continue;
    if (node.getAttribute('data-route') === route) node.setAttribute('aria-current', 'page');
    else node.removeAttribute('aria-current');
  }
}

/** Mark the tab whose kinds include `kind`. An unknown kind clears every tab. */
export function markTabForKind(root: ParentNode, kind: string): void {
  for (const node of root.querySelectorAll(TAB_LINK)) {
    if (!(node instanceof HTMLAnchorElement)) continue;
    const kinds = (node.getAttribute('data-kinds') ?? '').split(' ').filter(Boolean);
    if (kind !== '' && kinds.includes(kind)) node.setAttribute('aria-current', 'page');
    else node.removeAttribute('aria-current');
  }
}
