/** A page's address without its fragment: two fragments of one page are one page. */
export function address(href: string, base?: string): string {
  const url = new URL(href, base);
  url.hash = '';
  return url.href;
}

/** Pages of this archive, as opposed to its files (feeds, Markdown, TOML) and other sites. */
export function routable(url: URL, root: URL): boolean {
  return (
    url.origin === root.origin &&
    url.pathname.startsWith(root.pathname) &&
    (url.pathname.endsWith('/') || url.pathname.endsWith('.html'))
  );
}

/**
 * Where a link leads when the reader may follow it in place; null when the browser should have
 * it: downloads, other frames or windows, links marked external, and anything not routable.
 */
export function linkDestination(link: HTMLAnchorElement, root: URL): URL | null {
  if (link.hasAttribute('download')) return null;
  if (link.target && link.target !== '_self') return null;
  if ((link.getAttribute('rel') || '').split(/\s+/).includes('external')) return null;
  let url: URL;
  try {
    url = new URL(link.href);
  } catch {
    return null;
  }
  return routable(url, root) ? url : null;
}

/** The routable link an event landed on, if any. */
export function linkIn(event: Event, root: URL): { link: HTMLAnchorElement; url: URL } | null {
  const target = event.target;
  const link = target instanceof Element ? target.closest('a[href]') : null;
  if (!(link instanceof HTMLAnchorElement)) return null;
  const url = linkDestination(link, root);
  return url ? { link, url } : null;
}
