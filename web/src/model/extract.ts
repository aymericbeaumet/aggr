import type { ClientPage } from '../generated/ClientPage';
import { readModel } from './page';

/** What a fetched page contributes to the one on screen. */
export type Extracted = {
  model: ClientPage;
  /** The region a view adopts rather than renders, already owned by the current document. */
  content: Element | null;
  title: string;
  head: HTMLHeadElement;
  body: HTMLElement;
};

export function parseDocument(html: string): Document {
  return new DOMParser().parseFromString(html, 'text/html');
}

/**
 * The element a view adopts rather than renders: an article's content region (the media figure
 * and the body), or a static page's whole `<main>`. Lists are rendered from the model alone.
 */
export function contentElement(root: ParentNode, model: ClientPage): Element | null {
  switch (model.page.view) {
    case 'article':
      return root.querySelector('[data-article-content]');
    case 'list':
      return null;
    default:
      return root.querySelector('main#content');
  }
}

/**
 * The model, the adopted region and the metadata of a parsed page; null when the page has no
 * usable model, in which case it is not one of ours to swap in. The region is adopted into
 * `into` so its nodes can be moved without a copy.
 */
export function extract(doc: Document, into: Document = document): Extracted | null {
  const model = readModel(doc.getElementById('aggr-page')?.textContent);
  if (!model) return null;
  const captured = contentElement(doc, model);
  const content = captured ? into.adoptNode(captured) : null;
  return { model, content, title: doc.title, head: doc.head, body: doc.body };
}
