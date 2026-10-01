/**
 * Links leaving the site say so. A browser tab asks for a new tab; an installed app hands the
 * link to the platform instead, which opens its own browser, so the label says "external site"
 * and the `target` comes off. The label is remembered so the marking can be redone either way.
 */

/** Whether the reader runs as an installed app rather than in a browser tab. */
export function isInstalled(): boolean {
  if (Reflect.get(navigator, 'standalone') === true) return true;
  if (typeof matchMedia !== 'function') return false;
  return matchMedia('(display-mode: standalone), (display-mode: minimal-ui), (display-mode: fullscreen)').matches;
}

/** Mark every link under `root` that leaves `siteRoot` (an absolute URL with a trailing slash). */
export function markExternalLinks(root: ParentNode, siteRoot: string, installed: boolean): void {
  for (const link of root.querySelectorAll('a[href]')) {
    if (!(link instanceof HTMLAnchorElement)) continue;
    // A fragment stays in this document wherever the document is.
    if (link.getAttribute('href')?.startsWith('#')) continue;
    let url: URL;
    try {
      url = new URL(link.href);
    } catch {
      continue;
    }
    if (!/^https?:$/.test(url.protocol) || url.href.startsWith(siteRoot)) continue;
    const label =
      link.dataset.aggrExternalLabel ||
      link.getAttribute('aria-label')?.replace(/, (?:opens in a new tab|external site)$/, '') ||
      link.textContent?.trim();
    if (label) {
      link.dataset.aggrExternalLabel = label;
      link.setAttribute('aria-label', `${label}, ${installed ? 'external site' : 'opens in a new tab'}`);
    }
    if (installed) link.removeAttribute('target');
    else link.target = '_blank';
    link.relList.add('noopener', 'noreferrer');
  }
}
