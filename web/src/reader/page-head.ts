import { $$ } from "./dom";

/** The head metadata that belongs to one page rather than to the shell. */
export const PAGE_HEAD_SELECTOR = [
  'meta[name="description"]',
  'meta[name="robots"]',
  'meta[name="author"]',
  'meta[name^="aggr:"]',
  'meta[property^="og:"]',
  'meta[name^="twitter:"]',
  'meta[property^="article:"]',
  'link[rel="canonical"]',
  'link[rel="first"]',
  'link[rel="last"]',
  'link[rel="prev"]',
  'link[rel="next"]',
  'link[rel="alternate"]',
  'link[rel="search"]',
  'link[rel="service-meta"]',
  'link[rel="type"]',
  'link[rel="via"]',
  'link[rel="original"]',
  'script[type="application/ld+json"]'
].join(',');

/** Replace the current page's head metadata with the incoming document's, leaving the shell's own tags alone. */
export function syncPageHead(head: HTMLHeadElement, incoming?: Document | null) {
  if (!incoming || !incoming.head) return;
  $$(PAGE_HEAD_SELECTOR, head).forEach(function (node) { node.remove(); });
  $$(PAGE_HEAD_SELECTOR, incoming.head).forEach(function (node) {
    head.appendChild(head.ownerDocument.importNode(node, true));
  });
}
