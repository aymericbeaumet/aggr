import type { ClientPage } from '@aggr-model/ClientPage';
import { readModel } from './page';

export type Extracted = { model: ClientPage; bodyHtml: string };
export function parseDocument(html: string): Document {
  return new DOMParser().parseFromString(html, 'text/html');
}
/** Article prose is an opaque payload; every UI element comes from the typed model. */
export function extract(doc: Document): Extracted | null {
  const model = readModel(doc.getElementById('aggr-page')?.textContent);
  if (!model) return null;
  return { model, bodyHtml: doc.querySelector('[data-article-body]')?.innerHTML ?? '' };
}
