/** The few DOM helpers the reader's enhancements share. */

export function $<T extends Element = HTMLElement>(selector: string, root: ParentNode | null = document): T | null {
  return (root || document).querySelector<T>(selector);
}

export function $$<T extends Element = HTMLElement>(selector: string, root: ParentNode | null = document): T[] {
  return Array.from((root || document).querySelectorAll<T>(selector));
}

/** Build an element; `text` and `html` set the content, every other key is an attribute. */
export function el<K extends keyof HTMLElementTagNameMap>(tag: K, attrs: Record<string, string | number | boolean | null> = {}, children: Array<Node | null | false> = []): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  Object.keys(attrs || {}).forEach(function (key) {
    if (key === "text") node.textContent = String(attrs[key] ?? "");
    else if (key === "html") node.innerHTML = String(attrs[key] ?? "");
    else node.setAttribute(key, String(attrs[key]));
  });
  (children || []).forEach(function (child) { if (child) node.appendChild(child); });
  return node;
}

/** Write an inline style only when it changes, so a scroll handler never invalidates layout for nothing. */
export function setStyle(node: HTMLElement, property: string, value: string | number) {
  if (node.style.getPropertyValue(property) !== String(value)) node.style.setProperty(property, String(value));
}
