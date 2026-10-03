/**
 * What every page shares and the swap leaves alone: the title is set apart, the scripts and
 * styles are the running application, and the theme colour follows the preference.
 */
const SHARED =
  'title, script:not([type="application/ld+json"]), style, link[rel~="stylesheet"], link[rel="modulepreload"], meta[charset], meta[name="viewport"], #theme-color';

/**
 * Bring the document's `<head>` in line with the page arriving: the page's own metadata
 * (canonical, description and social cards, feed alternates, prev/next, article properties, the
 * JSON-LD script) is replaced, node for node, by what the fetched head carries. Nodes with the
 * same markup on both sides stay where they are.
 */
export function patchHead(current: Document, next: Document): void {
  current.title = next.title;
  const shared = (node: Element) => node.matches(SHARED);
  const old = [...current.head.children].filter((node) => !shared(node));
  const renewed = [...next.head.children].filter((node) => !shared(node));
  const kept = new Set(renewed.map((node) => node.outerHTML));
  const present = new Set(old.map((node) => node.outerHTML));
  for (const node of old) if (!kept.has(node.outerHTML)) node.remove();
  for (const node of renewed) if (!present.has(node.outerHTML)) current.head.appendChild(current.adoptNode(node));
  // The embedded model is data, not the running application: keep it describing the page shown
  // so anything reading `#aggr-page` after a swap sees the current page.
  const model = next.getElementById('aggr-page');
  const shown = current.getElementById('aggr-page');
  if (model && shown) shown.textContent = model.textContent;
}
