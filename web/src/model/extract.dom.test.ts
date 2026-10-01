import { describe, expect, it } from 'vitest';
import { contentElement, extract, parseDocument } from './extract';

const model = (view: object, kind = 'river') =>
  JSON.stringify({
    base: './',
    path: '',
    kind,
    title: 'Reader',
    site: { title: 'Reader', language: 'en', pwa: false, excerpts: true, discussions: [], entries: [], config_url: null, assets: { app: null, css: null, imports: [] } },
    build: { version: '1.0.0', app: 'a', content: 'c' },
    page: view,
  });

const html = (script: string, main: string) =>
  `<!doctype html><html><head><title>T</title><link rel="canonical" href="https://x/">${script}</head><body><div id="app"><main id="content">${main}</main></div></body></html>`;

describe('extract', () => {
  it('returns the model, the head and no content for a list', () => {
    const doc = parseDocument(html(`<script id="aggr-page" type="application/json">${model({ view: 'list', data: {} })}</script>`, '<ol></ol>'));
    const extracted = extract(doc, document);
    expect(extracted?.model.kind).toBe('river');
    expect(extracted?.content).toBeNull();
    expect(extracted?.title).toBe('T');
    expect(extracted?.head.querySelector('link[rel=canonical]')).not.toBeNull();
  });

  it('adopts the article content region into the current document', () => {
    const doc = parseDocument(html(`<script id="aggr-page" type="application/json">${model({ view: 'article', data: {} }, 'item')}</script>`, '<article><div data-article-content><p>Body</p></div></article>'));
    const extracted = extract(doc, document);
    expect(extracted?.content?.ownerDocument).toBe(document);
    expect(extracted?.content?.querySelector('p')?.textContent).toBe('Body');
    expect(doc.querySelector('[data-article-content]')).toBeNull();
  });

  it('adopts a static page as its main element', () => {
    const doc = parseDocument(html(`<script id="aggr-page" type="application/json">${model({ view: 'static' }, 'browse')}</script>`, '<section>Browse</section>'));
    const extracted = extract(doc, document);
    expect(extracted?.content?.localName).toBe('main');
    expect(extracted?.content?.textContent).toBe('Browse');
  });

  it('is null without a usable model', () => {
    expect(extract(parseDocument(html('', '')), document)).toBeNull();
    expect(extract(parseDocument(html('<script id="aggr-page" type="application/json">{</script>', '')), document)).toBeNull();
    expect(extract(parseDocument(html('<script id="aggr-page" type="application/json">{"kind":"x"}</script>', '')), document)).toBeNull();
  });

  it('finds the region by view', () => {
    const doc = parseDocument(html('', '<div data-article-content></div>'));
    const page = JSON.parse(model({ view: 'article' }, 'item'));
    expect(contentElement(doc, page)?.hasAttribute('data-article-content')).toBe(true);
    expect(contentElement(doc, { ...page, page: { view: 'list' } })).toBeNull();
    expect(contentElement(doc, { ...page, page: { view: 'offline' } })?.localName).toBe('main');
  });
});
