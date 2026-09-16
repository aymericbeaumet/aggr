// externalLinks() rewrites an outbound link's aria-label to "<label>, opens in a new tab" and keeps
// the original in data-aggr-external-label; anything deriving its own text from that link (such as
// a player's iframe title) wants the original.
export function originalLabel(element: { dataset: DOMStringMap; getAttribute(name: string): string | null }): string {
  return element.dataset.aggrExternalLabel || element.getAttribute("aria-label") || "";
}
