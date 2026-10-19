import type { ClientSearchRow } from '../generated/ClientSearchRow';

/** Rust's escaped row fragment uses site-root-relative URLs, even on a nested reader page. */
export function rowHTML(row: ClientSearchRow, root: string): string {
  const template = document.createElement('template');
  template.innerHTML = row.html;
  const item = template.content.firstElementChild;
  if (!item?.matches('li.row') || template.content.childElementCount !== 1) return '';
  for (const node of item.querySelectorAll('[href], [src], [poster]')) {
    for (const attribute of ['href', 'src', 'poster']) {
      const value = node.getAttribute(attribute);
      if (value === null || /^(?:[a-z][a-z\d+.-]*:|\/\/|#)/i.test(value)) continue;
      node.setAttribute(attribute, new URL(value, root).href);
    }
  }
  return item.outerHTML;
}
